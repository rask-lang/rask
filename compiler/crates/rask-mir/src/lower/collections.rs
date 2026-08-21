// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Collection construction and cloning lowering:
//! Vec.from, Map.from, JSON encode/decode, enum clone.

use crate::FieldAccess;
use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, types::{EnumLayoutId, StructLayoutId}, FunctionRef, MirOperand, MirRValue,
    MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{Expr, ExprKind};
use rask_mono::StructLayout;

impl<'a> MirLowerer<'a> {
    /// Vec.from([a, b, c]) → store elements into stack array, call rask_vec_from_static
    pub(super) fn lower_vec_from_array(
        &mut self,
        elems: &[Expr],
    ) -> Result<TypedOperand, LoweringError> {
        self.lower_vec_from_array_with(elems, None)
    }

    /// `lower_vec_from_array`, told what the elements are.
    ///
    /// The first element's own lowered type is a guess: it can't see that the
    /// slot wants a `T?`, so `[1, none, 3]` into a `Vec<i64?>` built 8-byte slots
    /// and dropped every tag. The checker knows, so it says.
    pub(super) fn lower_vec_from_array_with(
        &mut self,
        elems: &[Expr],
        elem_hint: Option<MirType>,
    ) -> Result<TypedOperand, LoweringError> {
        let mut elem_ty = MirType::I64;
        let mut lowered = Vec::new();
        for (i, elem) in elems.iter().enumerate() {
            let (op, ty) = self.lower_expr(elem)?;
            if i == 0 {
                elem_ty = ty.clone();
            }
            lowered.push((op, ty));
        }
        if let Some(hint) = elem_hint {
            elem_ty = hint;
        }
        // A Vec keeps scalars in 8-byte slots — `Vec.new()` declares elem_size 8
        // and readers load a whole word per element. An untyped integer literal
        // lowers as i32, so building the array at its natural 4-byte stride left
        // storage and readers disagreeing: `Vec.from([1, 2, 3])` summed to
        // 21474836486 because each 8-byte load straddled two elements (#461).
        // Widen narrow scalars to the slot width; genuinely wide elements
        // (string, trait object, aggregates) keep their real size.
        if elem_ty.size() < 8 && !matches!(elem_ty, MirType::Struct(_) | MirType::Enum(_)) {
            elem_ty = MirType::I64;
        }
        // A bare `T` filling a `T?` slot gets its layers here, same as an array
        // literal's elements and a struct field's value.
        let lowered: Vec<MirOperand> = lowered
            .into_iter()
            .map(|(op, val_ty)| self.wrap_collection_element(&elem_ty, &val_ty, op))
            .collect();
        let elem_size = elem_ty.size();
        let array_ty = MirType::Array {
            elem: Box::new(elem_ty.clone()),
            len: elems.len() as u32,
        };
        let arr_local = self.builder.alloc_temp(array_ty);
        // Elements wider than a word are values, not pointers: a string
        // constant lowers to the address of its 16-byte blob, so without a
        // store size codegen drops the address into the slot and the reader
        // sees the pointer's bytes as a string. `Vec.from(["ab", "cde"])`
        // reported len 15 for every element that way (#508).
        let store_size = (elem_size > 8).then_some(elem_size);
        for (i, op) in lowered.into_iter().enumerate() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: arr_local,
                offset: i as u32 * elem_size,
                value: op,
                store_size,
            }));
        }

        let vec_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(vec_local),
            func: FunctionRef::internal("rask_vec_from_static".to_string()),
            args: vec![
                MirOperand::Local(arr_local),
                MirOperand::Constant(MirConst::Int(elems.len() as i64)),
                MirOperand::Constant(MirConst::Int(elem_size as i64)),
            ],
        }));
        Ok((MirOperand::Local(vec_local), MirType::I64))
    }

    /// Map.from([(k, v), ...]) → Map.new() + Map.insert() per pair.
    pub(super) fn lower_map_from_pairs(
        &mut self,
        elems: &[Expr],
    ) -> Result<TypedOperand, LoweringError> {
        let has_string_keys = elems.first()
            .and_then(|e| match &e.kind {
                ExprKind::Tuple(parts) if parts.len() == 2 => {
                    self.ctx.lookup_raw_type(parts[0].id)
                        .map(|ty| matches!(ty, rask_types::Type::String))
                },
                _ => None,
            })
            .unwrap_or(false);

        let ctor = if has_string_keys { "Map_new_string_keys" } else { "Map_new" };
        let map_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(map_local),
            func: FunctionRef::internal(ctor.to_string()),
            args: vec![],
        }));

        for elem in elems {
            let (key_op, val_op) = match &elem.kind {
                ExprKind::Tuple(parts) if parts.len() == 2 => {
                    let (k, _) = self.lower_expr(&parts[0])?;
                    let (v, _) = self.lower_expr(&parts[1])?;
                    (k, v)
                }
                _ => {
                    let _ = self.lower_expr(elem)?;
                    continue;
                }
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Map_insert".to_string()),
                args: vec![MirOperand::Local(map_local), key_op, val_op],
            }));
        }

        Ok((MirOperand::Local(map_local), MirType::I64))
    }

    /// Expand `json.encode(struct_val)` into a sequence of json_buf_* calls.
    /// Re-indent an encoded JSON string when the call was `encode_pretty`.
    ///
    /// The struct and Vec encoders build text directly, so there's no value
    /// tree to hand a pretty printer — the indentation goes on afterwards, to
    /// the same shape `JsonValue.to_string_pretty` writes (#847).
    pub(super) fn maybe_json_pretty(
        &mut self,
        encoded: TypedOperand,
        pretty: bool,
    ) -> TypedOperand {
        if !pretty {
            return encoded;
        }
        let (op, _) = encoded;
        let dst = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(dst),
            func: FunctionRef::internal("json_pretty".to_string()),
            args: vec![op],
        }));
        (MirOperand::Local(dst), MirType::String)
    }

    pub(super) fn lower_json_encode_struct(
        &mut self,
        struct_op: MirOperand,
        layout: StructLayout,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_types::Type;

        let buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(buf),
            func: FunctionRef::internal("json_buf_new".to_string()),
            args: vec![],
        }));

        for (idx, field) in layout.fields.iter().enumerate() {
            // `@skip` keeps a field out of the serialized form (std.encoding/E19).
            if rask_ast::decl::field_attrs::is_skipped(&field.attrs) {
                continue;
            }
            // `@rename` overrides the key; otherwise it's the field's own name (E18).
            let key = rask_ast::decl::field_attrs::serial_name(&field.attrs, &field.name);
            // Hold the field in a local of its own type. An I64 temp made the
            // f64 load convert *numerically* on the way in, so 0.25 encoded as
            // 0 and 8.0 as 8 (#478).
            let field_val = self.builder.alloc_temp(self.ctx.type_to_mir(&field.ty));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: field_val,
                rvalue: MirRValue::Field {
                    base: struct_op.clone(),
                    field_index: idx as u32,
                    byte_offset: None,
                    access: FieldAccess::Word,
                },
            }));

            self.encode_field_into_buf(buf, &key, field_val, &field.ty)?;
        }

        // The StringOutParam adapter writes a 16-byte RaskStr and hands back its
        // address, so this is a string — typing it I64 made `let j =
        // json.encode(v)` print a pointer (#478).
        let result = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal("json_buf_finish".to_string()),
            args: vec![MirOperand::Local(buf)],
        }));

        Ok((MirOperand::Local(result), MirType::String))
    }

    /// Encode one already-loaded value into a json buffer under `key`.
    ///
    /// Shared between struct fields (`lower_json_encode_struct`) and enum
    /// variant payload fields (`lower_json_encode_enum`) — both are "a typed
    /// slot with a name," and the encoding rules (Vec, Map, `T?`, nested
    /// struct, nested enum, scalar) don't care which one it came from.
    fn encode_field_into_buf(
        &mut self,
        buf: crate::LocalId,
        key: &str,
        field_val: crate::LocalId,
        field_ty: &rask_types::Type,
    ) -> Result<(), LoweringError> {
        use rask_types::Type;

        // A `Vec<T>` field is an array, not a number. Without this the field
        // went through json_buf_add_i64 and `{"items": [...]}` came out as
        // `{"items":{}}`.
        let vec_args = match field_ty {
            Type::UnresolvedGeneric { name, args } if name == "Vec" => Some(args),
            Type::Generic { base, args }
                if self.ctx.type_names.get(base).map(|n| n == "Vec").unwrap_or(false) =>
            {
                Some(args)
            }
            _ => None,
        };
        let vec_elem = vec_args.and_then(|args| {
            args.first().and_then(|a| match a {
                rask_types::GenericArg::Type(t) => Some(t.as_ref().clone()),
                _ => None,
            })
        });
        if let Some(elem) = vec_elem {
            let elem_mir = self.ctx.type_to_mir(&elem);
            let (arr_json, _) = self.lower_json_encode_vec(
                MirOperand::Local(field_val),
                Some(elem),
                Some(elem_mir),
            )?;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("json_buf_add_raw".to_string()),
                args: vec![
                    MirOperand::Local(buf),
                    MirOperand::Constant(MirConst::String(key.to_string())),
                    arr_json,
                ],
            }));
            return Ok(());
        }

        // Two shapes can't be unrolled here and go to the runtime encoder
        // instead: a `Map<string, V>`, whose keys aren't known until it's
        // walked, and a `T?` around a collection, whose payload is a Vec or
        // Map pointer. The map used to match the nested-struct branch below
        // (find_struct finds the stdlib Map layout) and encode as `{}`; the
        // optional collection went through json_buf_add_i64 and printed the
        // pointer as a number.
        if self.is_map_type(field_ty) || self.is_optional_collection(field_ty) {
            if let Some(json) = self.lower_json_encode_shaped(field_val, field_ty) {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("json_buf_add_raw".to_string()),
                    args: vec![
                        MirOperand::Local(buf),
                        MirOperand::Constant(MirConst::String(key.to_string())),
                        MirOperand::Local(json),
                    ],
                }));
                return Ok(());
            }
        }

        // A `T?` field is the value or `null`. It used to go through
        // json_buf_add_i64, which wrote the address of the option's storage —
        // `"assignee":140204924960672` where the answer is `null`.
        if matches!(field_ty, Type::Result { err, .. } if **err == Type::None) {
            self.emit_json_optional_field(buf, key, field_val)?;
            return Ok(());
        }

        let nested_struct = match field_ty {
            Type::UnresolvedNamed(name) => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
            Type::UnresolvedGeneric { name, .. } => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
            _ => None,
        };

        if let Some(nested_layout) = nested_struct {
            let (nested_json, _) = self.lower_json_encode_struct(
                MirOperand::Local(field_val),
                nested_layout,
            )?;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("json_buf_add_raw".to_string()),
                args: vec![
                    MirOperand::Local(buf),
                    MirOperand::Constant(MirConst::String(key.to_string())),
                    nested_json,
                ],
            }));
            return Ok(());
        }

        // std.encoding/E22: a unit variant serializes as its own name.
        // Without this an enum field fell through to the integer encoder
        // and wrote the address of its slot as a number — valid JSON with a
        // stack address in it, and no warning (#854).
        let enum_layout = match field_ty {
            Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
                self.ctx.find_enum(name).map(|(_, l)| l.clone())
            }
            _ => None,
        };
        if let Some(layout) = enum_layout {
            let (nested_json, _) = self.lower_json_encode_enum(MirOperand::Local(field_val), &layout)?;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("json_buf_add_raw".to_string()),
                args: vec![
                    MirOperand::Local(buf),
                    MirOperand::Constant(MirConst::String(key.to_string())),
                    nested_json,
                ],
            }));
            return Ok(());
        }

        let helper = match field_ty {
            Type::String => "json_buf_add_string",
            Type::Bool => "json_buf_add_bool",
            Type::F32 | Type::F64 => "json_buf_add_f64",
            _ => "json_buf_add_i64",
        };

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(helper.to_string()),
            args: vec![
                MirOperand::Local(buf),
                MirOperand::Constant(MirConst::String(key.to_string())),
                MirOperand::Local(field_val),
            ],
        }));
        Ok(())
    }

    /// Encode an enum value as JSON (std.encoding/E22-E25).
    ///
    /// A chain of tag comparisons rather than a jump table: an enum small
    /// enough to be a struct field, or to be the whole argument to
    /// `json.encode()`, has a handful of variants, and this keeps the emitted
    /// MIR to shapes every backend already handles.
    ///
    /// Default (no `@tag` on the enum) is external tagging: a unit variant is
    /// its own name as a bare JSON string (`"Point"`); a variant with one
    /// unnamed field (`Circle(f64)`) puts the payload directly under the
    /// variant name (`{"Circle": 1.0}`, E23); anything else (named fields, or
    /// several unnamed ones) nests an object under the variant name
    /// (`{"Circle": {"radius": 1.0}}`, E22). `@tag("field")` switches to
    /// internal tagging: the tag goes in `field` inside the same object the
    /// payload's fields flatten into (`{"type": "Click", "x": 10}`, E24) —
    /// there's no field to flatten an unnamed payload into, so that
    /// combination panics naming what's missing rather than inventing a key.
    /// `@rename` on a variant overrides its serialized name either way (E25).
    pub(super) fn lower_json_encode_enum(
        &mut self,
        enum_op: MirOperand,
        layout: &rask_mono::EnumLayout,
    ) -> Result<TypedOperand, LoweringError> {
        let tag_field = rask_ast::decl::field_attrs::tag_field(&layout.attrs);

        let tag = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: enum_op.clone() },
        }));

        let result = self.builder.alloc_temp(MirType::String);
        let done = self.builder.create_block();
        for variant in &layout.variants {
            let hit = self.builder.create_block();
            let miss = self.builder.create_block();
            let eq = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: eq,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: MirOperand::Local(tag),
                    right: MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                },
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(eq),
                then_block: hit,
                else_block: miss,
            }));

            self.builder.switch_to_block(hit);
            let variant_name = rask_ast::decl::field_attrs::serial_name(&variant.attrs, &variant.name);
            let is_single_unnamed = variant.fields.len() == 1 && variant.fields[0].name == "_0";

            if variant.fields.is_empty() {
                match &tag_field {
                    None => {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result),
                            func: FunctionRef::internal("json_encode_string".to_string()),
                            args: vec![MirOperand::Constant(MirConst::String(variant_name))],
                        }));
                    }
                    Some(tf) => {
                        let buf = self.new_json_buf();
                        self.push_json_add_string(buf, tf, MirOperand::Constant(MirConst::String(variant_name)));
                        self.push_json_finish(buf, result);
                    }
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
            } else if is_single_unnamed && tag_field.is_none() {
                // E23: the payload goes directly under the variant name.
                let field = &variant.fields[0];
                let field_val = self.load_variant_field(enum_op.clone(), variant, field, 0);
                let buf = self.new_json_buf();
                self.encode_field_into_buf(buf, &variant_name, field_val, &field.ty)?;
                self.push_json_finish(buf, result);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
            } else if is_single_unnamed {
                // Internal tagging has no field name to flatten this payload
                // into — the object already has one job (the tag) and an
                // unnamed value can't share it without a made-up key.
                let msg = self.builder.alloc_temp(MirType::I64);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(msg),
                    func: FunctionRef::internal("panic".to_string()),
                    args: vec![MirOperand::Constant(MirConst::String(format!(
                        "json.encode can't write `{}.{}` yet: @tag needs a named payload to flatten into the tagged object, and this variant's payload is unnamed (std.encoding/E24)",
                        layout.name, variant.name,
                    )))],
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
            } else {
                // E22 (external) or E24 (internal): a struct-shaped payload.
                let buf = self.new_json_buf();
                if let Some(tf) = &tag_field {
                    self.push_json_add_string(buf, tf, MirOperand::Constant(MirConst::String(variant_name.clone())));
                }
                for (i, field) in variant.fields.iter().enumerate() {
                    let field_val = self.load_variant_field(enum_op.clone(), variant, field, i as u32);
                    self.encode_field_into_buf(buf, &field.name, field_val, &field.ty)?;
                }
                if tag_field.is_none() {
                    let inner = self.builder.alloc_temp(MirType::String);
                    self.push_json_finish(buf, inner);
                    let outer = self.new_json_buf();
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("json_buf_add_raw".to_string()),
                        args: vec![
                            MirOperand::Local(outer),
                            MirOperand::Constant(MirConst::String(variant_name)),
                            MirOperand::Local(inner),
                        ],
                    }));
                    self.push_json_finish(outer, result);
                } else {
                    self.push_json_finish(buf, result);
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
            }

            self.builder.switch_to_block(miss);
        }
        // No variant matched, which a well-formed value can't do. Fall through
        // rather than writing something invented.
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done }));
        self.builder.switch_to_block(done);
        Ok((MirOperand::Local(result), MirType::String))
    }

    fn new_json_buf(&mut self) -> crate::LocalId {
        let buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(buf),
            func: FunctionRef::internal("json_buf_new".to_string()),
            args: vec![],
        }));
        buf
    }

    fn push_json_add_string(&mut self, buf: crate::LocalId, key: &str, value: MirOperand) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("json_buf_add_string".to_string()),
            args: vec![MirOperand::Local(buf), MirOperand::Constant(MirConst::String(key.to_string())), value],
        }));
    }

    fn push_json_finish(&mut self, buf: crate::LocalId, dst: crate::LocalId) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(dst),
            func: FunctionRef::internal("json_buf_finish".to_string()),
            args: vec![MirOperand::Local(buf)],
        }));
    }

    /// Load one field out of an enum's matched-variant payload, at its exact
    /// offset — same mechanism `match_lower.rs` uses to bind a payload
    /// pattern, since this is the same operation: read a named slot out of
    /// whichever variant the tag already picked.
    fn load_variant_field(
        &mut self,
        enum_op: MirOperand,
        variant: &rask_mono::VariantLayout,
        field: &rask_mono::FieldLayout,
        field_index: u32,
    ) -> crate::LocalId {
        let field_mir_ty = self.ctx.type_to_mir(&field.ty);
        let field_val = self.builder.alloc_temp(field_mir_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: field_val,
            rvalue: MirRValue::Field {
                base: enum_op,
                field_index,
                byte_offset: Some(variant.payload_offset + field.offset),
                access: FieldAccess::for_field(&field_mir_ty, field.size),
            },
        }));
        field_val
    }

    /// Add a `T?` field to a json buffer: the payload when present, `null` when
    /// not. Both arms call an existing `json_buf_add_*`, so every payload kind
    /// (including bool, which has no raw literal to build) comes out right.
    fn emit_json_optional_field(
        &mut self,
        buf: crate::LocalId,
        field_name: &str,
        opt_local: crate::LocalId,
    ) -> Result<(), LoweringError> {
        let opt_ty = self.builder.local_type(opt_local).unwrap_or_else(|| crate::fallback::i64_fallback("lower/collections:285"));
        let payload_ty = Self::payload_of_mir(&opt_ty).unwrap_or_else(|| crate::fallback::i64_fallback("lower/collections:286"));

        let tag = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag,
            rvalue: MirRValue::EnumTag { value: MirOperand::Local(opt_local) },
        }));
        let is_some = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: is_some,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Eq,
                left: MirOperand::Local(tag),
                right: MirOperand::Constant(MirConst::Int(0)),
            },
        }));

        let some_block = self.builder.create_block();
        let none_block = self.builder.create_block();
        let done_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(is_some),
            then_block: some_block,
            else_block: none_block,
        }));

        self.builder.switch_to_block(some_block);
        let payload = self.emit_option_payload(
            MirOperand::Local(opt_local),
            payload_ty.clone(),
            false,
        );
        match &payload_ty {
            MirType::Struct(crate::types::StructLayoutId { id, .. }) => {
                let layout = self.ctx.struct_layouts.get(*id as usize).cloned();
                match layout {
                    Some(l) => {
                        let (nested, _) =
                            self.lower_json_encode_struct(MirOperand::Local(payload), l)?;
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: None,
                            func: FunctionRef::internal("json_buf_add_raw".to_string()),
                            args: vec![
                                MirOperand::Local(buf),
                                MirOperand::Constant(MirConst::String(field_name.to_string())),
                                nested,
                            ],
                        }));
                    }
                    None => self.emit_json_add_scalar(buf, field_name, payload, &payload_ty),
                }
            }
            _ => self.emit_json_add_scalar(buf, field_name, payload, &payload_ty),
        }
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(none_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("json_buf_add_raw".to_string()),
            args: vec![
                MirOperand::Local(buf),
                MirOperand::Constant(MirConst::String(field_name.to_string())),
                MirOperand::Constant(MirConst::String("null".to_string())),
            ],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));

        self.builder.switch_to_block(done_block);
        Ok(())
    }

    fn emit_json_add_scalar(
        &mut self,
        buf: crate::LocalId,
        field_name: &str,
        value: crate::LocalId,
        ty: &MirType,
    ) {
        let helper = match ty {
            MirType::String => "json_buf_add_string",
            MirType::Bool => "json_buf_add_bool",
            MirType::F32 | MirType::F64 => "json_buf_add_f64",
            _ => "json_buf_add_i64",
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(helper.to_string()),
            args: vec![
                MirOperand::Local(buf),
                MirOperand::Constant(MirConst::String(field_name.to_string())),
                MirOperand::Local(value),
            ],
        }));
    }

    /// Expand `json.encode(vec)` into a loop that encodes each element.
    /// `elem_mir` is the element's lowered type when the checker's type isn't
    /// available — a `mut v = Vec.new()` filled by `push` leaves the checker with
    /// an inference variable, and without the element type every element encoded
    /// as `json_buf_array_add_i64`: a Vec of structs came out `[1,2]`.
    pub(super) fn lower_json_encode_vec(
        &mut self,
        vec_op: MirOperand,
        elem_ty: Option<rask_types::Type>,
        elem_mir: Option<MirType>,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_types::Type;

        let arr_buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(arr_buf),
            func: FunctionRef::internal("json_buf_new_array".to_string()),
            args: vec![],
        }));

        let collection = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: collection,
            rvalue: MirRValue::Use(vec_op),
        }));

        let len_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(len_local),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(collection)],
        }));

        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        self.builder.switch_to_block(body_block);

        let elem_ref = &elem_ty;
        let nested_struct = match elem_ref {
            Some(Type::UnresolvedNamed(name)) => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
            Some(Type::UnresolvedGeneric { name, .. }) => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
            Some(Type::Named(type_id)) => {
                self.ctx.type_names.get(type_id)
                    .and_then(|name| self.ctx.find_struct(name).map(|(_, l)| l.clone()))
            }
            _ => None,
        }.or_else(|| match &elem_mir {
            Some(MirType::Struct(crate::types::StructLayoutId { id, .. })) => {
                self.ctx.struct_layouts.get(*id as usize).cloned()
            }
            _ => None,
        });

        // The element's own type, so a struct element comes back as a pointer to
        // its storage and a string keeps all 16 bytes.
        let elem_local_ty = match (&nested_struct, elem_ref, &elem_mir) {
            (Some(l), _, _) => {
                let idx = self.ctx.struct_layouts.iter().position(|s| s.name == l.name).unwrap_or(0);
                MirType::Struct(crate::types::StructLayoutId::new(idx as u32, l.size, l.align))
            }
            (None, Some(Type::String), _) => MirType::String,
            (None, _, Some(m)) if matches!(m, MirType::String) => MirType::String,
            _ => MirType::I64,
        };
        let elem = self.builder.alloc_temp(elem_local_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(elem),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
        }));

        if let Some(layout) = nested_struct {
            let (json_str, _) = self.lower_json_encode_struct(
                MirOperand::Local(elem),
                layout,
            )?;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("json_buf_array_add_raw".to_string()),
                args: vec![MirOperand::Local(arr_buf), json_str],
            }));
        } else {
            let helper = match elem_ref {
                Some(Type::String) => "json_buf_array_add_string",
                Some(Type::Bool) => "json_buf_array_add_bool",
                Some(Type::F32) | Some(Type::F64) => "json_buf_array_add_f64",
                Some(_) => "json_buf_array_add_i64",
                None => match &elem_mir {
                    Some(MirType::String) => "json_buf_array_add_string",
                    Some(MirType::Bool) => "json_buf_array_add_bool",
                    Some(MirType::F32) | Some(MirType::F64) => "json_buf_array_add_f64",
                    _ => "json_buf_array_add_i64",
                },
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(helper.to_string()),
                args: vec![MirOperand::Local(arr_buf), MirOperand::Local(elem)],
            }));
        }

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.builder.switch_to_block(exit_block);
        // The StringOutParam adapter writes a 16-byte RaskStr and hands back its
        // address, so this is a string — typing it I64 made `let j =
        // json.encode(v)` print a pointer (#478).
        let result = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal("json_buf_finish_array".to_string()),
            args: vec![MirOperand::Local(arr_buf)],
        }));

        Ok((MirOperand::Local(result), MirType::String))
    }

    /// Size in bytes for a MIR type (used for runtime allocation).
    pub(super) fn elem_size_for_type(&self, ty: &MirType) -> i64 {
        match ty {
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr
            | MirType::FuncPtr(_) | MirType::Handle => 8,
            MirType::I128 | MirType::U128 => 16,
            MirType::String => 16,
            MirType::Struct(sid) => sid.byte_size as i64,
            MirType::Enum(eid) => eid.byte_size as i64,
            MirType::Array { elem, len } => self.elem_size_for_type(elem) * (*len as i64),
            MirType::Tuple(_) | MirType::Slice(_) | MirType::Option(_)
            | MirType::Result { .. } | MirType::Union(_)
            | MirType::SimdVector { .. } | MirType::TraitObject { .. } => ty.size() as i64,
            MirType::Void => 0,
        }
    }

    /// The single slot-size authority for a value stored in a collection
    /// (Vec/Map element or key/value, Channel/Shared/Mutex/Pool element).
    ///
    /// Keyed on the resolved `Type` — no type-name string parsing. Delegates to
    /// the mono layout tables (via `type_to_mir`): scalars occupy an 8-byte slot
    /// (storing a scalar in a narrower slot truncated it on the i64 load-back),
    /// string is 16, `T?` is `[tag:8][payload]` with the payload floored to a
    /// word, and aggregates use their computed layout size. `None` when the type
    /// is still an unresolved variable.
    pub(super) fn slot_size_for_type(&self, ty: &rask_types::Type) -> Option<i64> {
        use rask_types::Type;
        match ty {
            Type::Var(_) => None,
            // `T?` (`T or none`) lays out as [tag:8][payload]; floor the payload
            // to a full word so scalar inners aren't truncated.
            Type::Result { ok, err } if **err == Type::None => {
                Some(8 + self.slot_size_for_type(ok)?.max(8))
            }
            _ => Some(Self::mir_slot_size(&self.ctx.type_to_mir(ty))),
        }
    }

    /// Slot size for an already-lowered `MirType`: scalars and pointers occupy
    /// one 8-byte slot, string is 16, aggregates use their layout size.
    pub(super) fn mir_slot_size(ty: &MirType) -> i64 {
        match ty {
            MirType::String => 16,
            MirType::Void => 0,
            MirType::Bool | MirType::I8 | MirType::U8
            | MirType::I16 | MirType::U16
            | MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char
            | MirType::I64 | MirType::U64 | MirType::F64
            | MirType::Ptr | MirType::FuncPtr(_) | MirType::Handle => 8,
            // Struct/Enum/Tuple/Slice/Option/Result/Union/Array/... — layout size.
            _ => ty.size() as i64,
        }
    }

    /// Slot size of the Nth generic argument of the collection/box type inferred
    /// at `node_id` (e.g. the `T` of `Vec<T>` / `Channel<T>`, or the key/value of
    /// `Map<K, V>`). Routes through `slot_size_for_type`; falls back to an 8-byte
    /// scalar slot when the argument is missing or still an unresolved variable.
    pub(super) fn generic_arg_slot_size(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> i64 {
        use rask_types::{GenericArg, Type};
        fn generic_args(ty: &Type) -> Option<&[GenericArg]> {
            match ty {
                Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => Some(args),
                _ => None,
            }
        }
        // The enclosing struct field's declared type, when the checker has nothing
        // for this node. `Headers { entries: Map<string, string> }` initialised by
        // `Map.new()` inside a stdlib body is exactly that case, and falling back
        // to an 8-byte slot silently halved every `string` value stored in it.
        if self.ctx.lookup_raw_type(node_id).is_none() {
            if let Some(hint) = self.field_type_hint.clone() {
                if let Some(inner) = super::generic_args_of_str(&hint) {
                    if let Some(arg) = inner.get(index) {
                        return Self::mir_slot_size(&self.ctx.resolve_type_str(arg));
                    }
                }
            }
        }
        let size = self.ctx.lookup_raw_type(node_id).and_then(|ty| {
            // `Channel<T>.buffered()` resolves to `(Sender<T>, Receiver<T>)` — the
            // element type lives in the tuple's first component. Everything else
            // (Vec/Map/Shared/Mutex/Pool) resolves to the wrapper directly.
            let container = match ty {
                Type::Tuple(elems) => elems.first()?,
                other => other,
            };
            let GenericArg::Type(inner) = generic_args(container)?.get(index)? else {
                return None;
            };
            self.slot_size_for_type(inner)
        });
        size.unwrap_or(8)
    }

    /// The MIR type of a container's Nth type argument — `Vec<i32?>` at index 0
    /// is `i32?`.
    ///
    /// Element *size* has a helper already (`generic_arg_slot_size`); the full
    /// type is what a value going into the slot has to be coerced to, and a size
    /// can't say whether the slot wants a wrapper layer added.
    pub(super) fn container_elem_mir_type(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> Option<MirType> {
        use rask_types::{GenericArg, Type};
        let ty = self.ctx.lookup_raw_type(node_id)?;
        let container = match ty {
            Type::Tuple(elems) => elems.first()?,
            other => other,
        };
        let args = match container {
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
            _ => return None,
        };
        let GenericArg::Type(inner) = args.get(index)? else {
            return None;
        };
        match inner.as_ref() {
            rask_types::Type::Var(_) => None,
            resolved => Some(self.ctx.type_to_mir(resolved)),
        }
    }

    /// How wide one channel element is, for the receive buffer.
    ///
    /// The tracked size comes from the `Channel.buffered()` call site, which
    /// only reaches a variable bound directly to it. A receiver pulled out of
    /// the returned pair (`let rx = ch.1`) has no such record, so fall back
    /// to its own `Receiver<T>` type — otherwise a 24-byte struct was received
    /// into an 8-byte buffer and smashed the stack (#360).
    pub(super) fn channel_elem_size(&self, object: &rask_ast::expr::Expr) -> i64 {
        if let rask_ast::expr::ExprKind::Ident(var_name) = &object.kind {
            if let Some(size) = self.meta(var_name).and_then(|m| m.channel_elem_size) {
                return size;
            }
        }
        self.generic_arg_slot_size(object.id, 0)
    }

    /// Clone function name for a type, or None if the type is Copy.
    pub(super) fn clone_fn_for_type(ty: &rask_types::Type) -> Option<&'static str> {
        match ty {
            rask_types::Type::String => Some("string_clone"),
            rask_types::Type::UnresolvedNamed(n) if n == "string" => Some("string_clone"),
            rask_types::Type::UnresolvedGeneric { name, .. } if name == "Vec" => Some("Vec_clone"),
            rask_types::Type::UnresolvedGeneric { name, .. } if name == "Map" => Some("Map_clone"),
            _ => None,
        }
    }

    /// Emit inline clone for an enum value: shallow copy the full block,
    /// then switch on the tag to deep-clone heap fields per variant.
    pub(super) fn lower_enum_clone(
        &mut self,
        layout: &rask_mono::EnumLayout,
        src: &MirOperand,
        obj_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(obj_ty.clone());

        // Shallow copy: copy each 8-byte word
        let num_words = (layout.size as u32 + 7) / 8;
        for i in 0..num_words {
            let offset = i * 8;
            let word = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: word,
                rvalue: MirRValue::Field {
                    base: src.clone(),
                    field_index: offset,
                    // These are byte offsets, so say so. Left as `None`,
                    // codegen read `field_index` as a *field* index, found no
                    // variant with that many fields, and fell back to the first
                    // variant's payload offset — so every word of the "copy"
                    // came from the same place.
                    byte_offset: Some(offset),
                    access: FieldAccess::Word,
                },
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: result,
                offset,
                value: MirOperand::Local(word),
                store_size: None,
            }));
        }

        let needs_switch = layout.variants.iter().any(|v| {
            v.fields.iter().any(|f| Self::clone_fn_for_type(&f.ty).is_some())
        });

        if needs_switch {
            let tag = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: tag,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(result),
                    field_index: layout.tag_offset,
                    byte_offset: None,
                    access: FieldAccess::Word,
                },
            }));

            // The switch belongs on the block that read the tag. Building the
            // variant blocks below moves the builder's cursor onto the last of
            // them, so remember where to come back to — terminating from here
            // put the switch on a variant block and left the tag-reading block
            // with its default `unreachable`, which is what a `.clone()` on an
            // enum carrying a string trapped on.
            let dispatch_block = self.builder.current_block();
            let exit_block = self.builder.create_block();
            let mut cases = Vec::new();

            for variant in &layout.variants {
                let has_heap = variant.fields.iter().any(|f| Self::clone_fn_for_type(&f.ty).is_some());
                if !has_heap {
                    continue;
                }
                let vblock = self.builder.create_block();
                cases.push((variant.tag as u64, vblock));

                self.builder.switch_to_block(vblock);
                for field in &variant.fields {
                    if let Some(cfn) = Self::clone_fn_for_type(&field.ty) {
                        let abs_offset = variant.payload_offset + field.offset;
                        if cfn == "string_clone" {
                            // A string is 16 bytes and the shallow copy above
                            // already moved all of them; the only thing left is
                            // the refcount. Take the field's address and bump
                            // it in place.
                            //
                            // Reading it as one word, "cloning" that, and
                            // storing the word back wrote the call's return
                            // register over the string's first 8 bytes — which
                            // is how `.clone()` on an enum carrying a string
                            // produced a value whose tag then trapped.
                            let field_addr = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: field_addr,
                                rvalue: MirRValue::Field {
                                    base: MirOperand::Local(result),
                                    field_index: abs_offset,
                                    byte_offset: Some(abs_offset),
                                    access: FieldAccess::InPlace(16),
                                },
                            }));
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: None,
                                func: FunctionRef::internal(cfn.to_string()),
                                args: vec![MirOperand::Local(field_addr)],
                            }));
                            continue;
                        }
                        // Vec/Map payloads are an 8-byte pointer, and their
                        // clone really does hand back a new one.
                        let field_val = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: field_val,
                            rvalue: MirRValue::Field {
                                base: MirOperand::Local(result),
                                field_index: abs_offset,
                                byte_offset: None,
                                access: FieldAccess::Word,
                            },
                        }));
                        let cloned = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(cloned),
                            func: FunctionRef::internal(cfn.to_string()),
                            args: vec![MirOperand::Local(field_val)],
                        }));
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result,
                            offset: abs_offset,
                            value: MirOperand::Local(cloned),
                            store_size: None,
                        }));
                    }
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: exit_block }));
            }

            self.builder.switch_to_block(dispatch_block);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: MirOperand::Local(tag),
                cases,
                default: exit_block,
            }));

            self.builder.switch_to_block(exit_block);
        }

        Ok((MirOperand::Local(result), obj_ty))
    }
}

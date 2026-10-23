// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Collection construction and cloning lowering:
//! Vec.from, Map.from, JSON encode/decode, enum clone.

use crate::FieldAccess;
use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, FunctionRef, MirOperand, MirRValue,
    MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::ty::TypeExpr;
use rask_types::layout::StructLayout;

impl<'a> MirLowerer<'a> {
    /// Vec.from([a, b, c]) → store elements into stack array, call rask_vec_from_static
    pub(super) fn lower_vec_from_array(
        &mut self,
        elems: &[Expr],
    ) -> Result<TypedOperand, LoweringError> {
        self.lower_vec_from_array_with(elems, None, None)
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
        literal_id: Option<rask_ast::NodeId>,
    ) -> Result<TypedOperand, LoweringError> {
        let mut lowered = Vec::new();
        for elem in elems {
            lowered.push(self.lower_expr(elem)?);
        }
        // The element type is the checker's: the first element's own lowered
        // type is a guess. It can't see that the slot wants a `T?`, so
        // `[1, none, 3]` into a `Vec<i64?>` built 8-byte slots and dropped
        // every tag; and an unsuffixed literal lowers as `i32` where the vector
        // holds `i64`s, which put four-byte elements where eight were read
        // (#461).
        let elem_ty = elem_hint
            .or_else(|| literal_id.and_then(|id| self.container_elem_mir_type(id, 0)))
            .or_else(|| {
                let first = elems.first()?;
                let raw = self.ctx.lookup_raw_type(first.id)?.clone();
                match raw {
                    rask_types::Type::Var(_) => None,
                    resolved => Some(self.ctx.type_to_mir(&resolved)),
                }
            })
            .or_else(|| lowered.first().map(|(_, ty)| ty.clone()))
            .unwrap_or(MirType::I64);
        // A bare `T` filling a `T?` slot gets its layers here, same as an array
        // literal's elements and a struct field's value; a scalar is converted
        // to the element's own type.
        let lowered: Vec<MirOperand> = lowered
            .into_iter()
            .map(|(op, val_ty)| {
                let op = self.wrap_collection_element(&elem_ty, &val_ty, op);
                self.to_slot_width(op, &elem_ty)
            })
            .collect();
        let elem_size = elem_ty.size();
        let array_ty = MirType::Array {
            elem: Box::new(elem_ty.clone()),
            len: elems.len() as u32,
        };
        let arr_local = self.builder.alloc_temp(array_ty);
        // Every element at its own size. Without one, codegen stored a string
        // constant's address in the slot where its 16 bytes belonged, and
        // `Vec.from(["ab", "cde"])` reported len 15 for every element (#508).
        for (i, op) in lowered.into_iter().enumerate() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: arr_local,
                offset: i as u32 * elem_size,
                value: op,
                store_size: elem_size,
                field: None,
            }));
        }

        // What the elements are, for the vector to give them back — and
        // anything pushed onto it later too. The checker's type for the
        // literal says it; `[]` has nothing else to read it off, so
        // `mut fns: Vec<func(i64) -> i64> = []` relies on it. Failing that,
        // the first element's own checked type: an element that is a
        // container is a `Ptr` in MIR like every other pointer, so `[a, b]`
        // being a `Vec<Vec<i32>>` is only known there.
        let elem_desc = literal_id
            .and_then(|id| self.container_elem_type(id, 0))
            .or_else(|| {
                let first = elems.first()?;
                let raw = self.ctx.lookup_raw_type(first.id)?.clone();
                Some(self.elem_type_of(&raw))
            })
            .unwrap_or_else(|| elem_ty.clone());

        // The handle is an address, and the binding that takes it over says
        // so. Typed `i64` here, an unannotated `let v = Vec.from([1, 2])` was
        // an integer local and `{v:debug}` printed the buffer's address.
        let vec_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(vec_local),
            func: FunctionRef::internal("rask_vec_from_static".to_string()),
            args: vec![
                MirOperand::Local(arr_local),
                MirOperand::Constant(MirConst::Int(elems.len() as i64)),
                MirOperand::Constant(MirConst::Int(elem_size as i64)),
                crate::elem_strs::elem(elem_desc),
            ],
        }));
        Ok((MirOperand::Local(vec_local), MirType::Ptr))
    }

    /// Map.from([(k, v), ...]) → Map.new() + Map.insert() per pair.
    ///
    /// `call` is the `Map.from(...)` expression and `written` the receiver's
    /// type arguments, because between them they say what the key and value
    /// types are — and the constructor needs to be told.
    ///
    /// It used to be told nothing: the call went out with no arguments and
    /// codegen filled in its generic defaults, one machine word per slot. A
    /// `string` key is sixteen bytes, so `Map.from([("k", 42)])` stored half a
    /// string in the key slot and hashed it as a word. Every later lookup
    /// missed — the map had the entry and could not find it (#812 is the same
    /// bug on `Map.new`, fixed there and not here).
    ///
    /// `arg` is a literal of pairs or an array held somewhere. A held array
    /// keeps its own entries, so the map takes a copy of each
    /// (`rask_map_insert_pairs`).
    pub(super) fn lower_map_from_pairs(
        &mut self,
        call: &Expr,
        written: &[TypeExpr],
        arg: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let elems: &[Expr] = match &arg.kind {
            ExprKind::Array(elems) => elems,
            _ => &[],
        };
        // The checker knows the map's own type; the spelling answers when it
        // doesn't (`Map<string, i64>.from(…)` inside a stdlib body has no
        // recorded type). The first pair is the last resort.
        let arg_ty = |i: usize| -> Option<MirType> {
            self.container_elem_mir_type(call.id, i)
        };
        let spelled_ty = |i: usize| -> Option<MirType> {
            written.get(i).map(|arg| self.ctx.resolve_type_expr(arg))
        };
        let pair_ty = |i: usize| -> Option<MirType> {
            elems.first().and_then(|e| match &e.kind {
                ExprKind::Tuple(parts) if parts.len() == 2 => {
                    self.ctx.lookup_node_type(parts[i].id)
                }
                _ => None,
            })
        };

        let key_ty = arg_ty(0).or_else(|| spelled_ty(0)).or_else(|| pair_ty(0))
            .unwrap_or(MirType::I64);
        let val_ty = arg_ty(1).or_else(|| spelled_ty(1)).or_else(|| pair_ty(1))
            .unwrap_or(MirType::I64);

        let mut args = vec![
            MirOperand::Constant(MirConst::Int(key_ty.size() as i64)),
            MirOperand::Constant(MirConst::Int(val_ty.size() as i64)),
        ];
        let ctor = match self.map_key_fn_addrs(call.id, &key_ty) {
            Some((hash, eq)) => {
                args.extend([hash, eq]);
                "Map_new_keyed"
            }
            None => crate::elem_strs::map_ctor_for(&key_ty),
        };
        // What keys and values are, with their containers named — the same
        // sources in the same order, through the conversions that keep a
        // `Vec` a `Vec`.
        let desc = |i: usize, fallback: &MirType| -> MirType {
            self.container_elem_type(call.id, i)
                .or_else(|| written.get(i).map(|arg| self.ctx.payload_of_expr(arg)))
                .unwrap_or_else(|| fallback.clone())
        };
        args.push(crate::elem_strs::elem(desc(0, &key_ty)));
        args.push(crate::elem_strs::elem(desc(1, &val_ty)));
        let map_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(map_local),
            func: FunctionRef::internal(ctor.to_string()),
            args,
        }));

        if !matches!(arg.kind, ExprKind::Array(_)) {
            let (arr_op, arr_ty) = self.lower_expr(arg)?;
            let MirType::Array { elem, len } = &arr_ty else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "Map.from takes a fixed array of pairs, and this argument lowered as {arr_ty:?}"
                )));
            };
            let MirType::Tuple(fields) = elem.as_ref() else {
                return Err(LoweringError::InvalidConstruct(format!(
                    "Map.from takes `(key, value)` pairs, and these lowered as {elem:?}"
                )));
            };
            let mut args = self.map_entries_args(MirOperand::Local(map_local), &fields[0], &fields[1]);
            args.insert(1, arr_op);
            args.insert(2, MirOperand::Constant(MirConst::Int(*len as i64)));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("rask_map_insert_pairs".to_string()),
                args,
            }));
            return Ok((MirOperand::Local(map_local), MirType::Ptr));
        }

        for elem in elems {
            let (key_op, val_op) = match &elem.kind {
                ExprKind::Tuple(parts) if parts.len() == 2 => {
                    let (k, k_ty) = self.lower_expr(&parts[0])?;
                    let (v, v_ty) = self.lower_expr(&parts[1])?;
                    // The map holds each at its own width, so a value goes in
                    // as the key or value type it is stored as.
                    let k = self.wrap_collection_element(&key_ty, &k_ty, k);
                    let v = self.wrap_collection_element(&val_ty, &v_ty, v);
                    (self.as_slot_value(k, &key_ty), self.as_slot_value(v, &val_ty))
                }
                _ => {
                    let _ = self.lower_expr(elem)?;
                    continue;
                }
            };
            // `Map_set`, not `Map_insert`: nothing takes the value a repeated
            // key displaces, so the map has to release it.
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Map_set".to_string()),
                args: vec![MirOperand::Local(map_local), key_op, val_op],
            }));
        }

        Ok((MirOperand::Local(map_local), MirType::Ptr))
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
        //
        // `field_ty` comes from the struct's own declaration (via
        // `StructLayout`), not from the checker's inferred node types — it
        // stays in the pre-resolve `UnresolvedGeneric` spelling, so there's no
        // resolved `Generic { base, .. }` form to match here.
        let vec_args = match field_ty {
            Type::UnresolvedGeneric { name, args } if name == "Vec" => Some(args),
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
        layout: &rask_types::layout::EnumLayout,
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
            let is_single_unnamed = variant.fields.len() == 1 && rask_ast::decl::is_positional_field_name(&variant.fields[0].name);

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
                // Both @tag shapes below are rejected at the declaration
                // (E0841/E0842), so reaching them means the check was skipped,
                // not that a user wrote something new. Fail the compile rather
                // than emit a panic into the program (ctrl.panic/S7).
                return Err(LoweringError::InvalidConstruct(format!(
                    "`@tag` on `{}.{}`, whose payload is unnamed — should have been rejected as E0841",
                    layout.name, variant.name,
                )));
            } else if tag_field.as_deref().is_some_and(|tf| variant.fields.iter().any(|f| f.name == tf)) {
                return Err(LoweringError::InvalidConstruct(format!(
                    "`@tag(\"{}\")` on `{}.{}` collides with a payload field — should have been rejected as E0842",
                    tag_field.as_deref().unwrap(), layout.name, variant.name,
                )));
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
        variant: &rask_types::layout::VariantLayout,
        field: &rask_types::layout::FieldLayout,
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
        let opt_ty = self.builder.local_type(opt_local).unwrap_or_else(|| crate::fallback::unknown_type("lower/collections:285"));
        let payload_ty = Self::payload_of_mir(&opt_ty).unwrap_or_else(|| crate::fallback::unknown_type("lower/collections:286"));

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
        // The index travels with the layout. Re-deriving it from the layout's
        // *name* afterwards is what let this land on the wrong entry: a name
        // shared by a user struct and a stdlib one resolves differently
        // depending on which side asks, and a name that isn't in the table at
        // all used to fall back to index 0 — whatever type happens to sit
        // there (#1062).
        let nested_struct: Option<(u32, StructLayout)> = match elem_ref {
            Some(Type::UnresolvedNamed(name)) => self.ctx.find_struct(name),
            Some(Type::UnresolvedGeneric { name, .. }) => self.ctx.find_struct(name),
            Some(Type::Named(type_id)) => {
                self.ctx.type_names.get(type_id).and_then(|name| self.ctx.find_struct(name))
            }
            _ => None,
        }
        .map(|(i, l)| (i, l.clone()))
        .or_else(|| match &elem_mir {
            Some(MirType::Struct(crate::types::StructLayoutId { id, .. })) => {
                self.ctx.struct_layouts.get(*id as usize).map(|l| (*id, l.clone()))
            }
            _ => None,
        });

        // The element's own type, so a struct element comes back as a pointer to
        // its storage, a string keeps all 16 bytes and a scalar is read at its
        // own width.
        let elem_local_ty = match (&nested_struct, elem_ref, &elem_mir) {
            (Some((idx, l)), _, _) => {
                MirType::Struct(crate::types::StructLayoutId::new(*idx, l.size, l.align))
            }
            (None, Some(ty), _) if !matches!(ty, Type::Var(_)) => self.ctx.type_to_mir(ty),
            (None, _, Some(m)) => m.clone(),
            _ => MirType::I64,
        };
        let elem = self.builder.alloc_temp(elem_local_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(elem),
            func: FunctionRef::internal("Vec_index".to_string()),
            args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
        }));

        if let Some((_, layout)) = nested_struct {
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
            // The helpers take an i64, an f64 or a bool; a narrower element
            // is converted by its own signedness.
            let helper_ty = match helper {
                "json_buf_array_add_f64" => Some(MirType::F64),
                "json_buf_array_add_i64" => Some(MirType::I64),
                _ => None,
            };
            let elem_op = match helper_ty {
                Some(ty) => self.to_slot_width(MirOperand::Local(elem), &ty),
                None => MirOperand::Local(elem),
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(helper.to_string()),
                args: vec![MirOperand::Local(arr_buf), elem_op],
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

    /// The slot a value takes in a collection — a Vec or Map element, a key,
    /// a channel element, a box payload: the type's own size. Every value sits
    /// at its own width wherever it is, so a `Vec<u8>` is a byte per element
    /// and a `Vec<f32>` holds f32s. `None` while the type is still a variable.
    pub(super) fn slot_size_for_type(&self, ty: &rask_types::Type) -> Option<i64> {
        match ty {
            rask_types::Type::Var(_) => None,
            _ => Some(self.ctx.type_to_mir(ty).size() as i64),
        }
    }

    /// Size of the Nth generic argument of the collection/box type inferred at
    /// `node_id` (e.g. the `T` of `Vec<T>` / `Channel<T>`, or the key/value of
    /// `Map<K, V>`).
    ///
    /// The checker rejects a container whose element type it couldn't work out
    /// (E0361), so a missing one here is a lowering bug. It used to be an
    /// 8-byte guess, which halved every string stored in the container.
    pub(super) fn generic_arg_slot_size(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> Result<i64, LoweringError> {
        use rask_types::{GenericArg, Type};
        fn generic_args(ty: &Type) -> Option<&[GenericArg]> {
            match ty {
                Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => Some(args),
                _ => None,
            }
        }
        // The enclosing struct field's declared type, when the checker has nothing
        // for this node. `Headers(entries: Map<string, string>)` initialised by
        // `Map.new()` inside a stdlib body is exactly that case, and falling back
        // to an 8-byte slot silently halved every `string` value stored in it.
        if self.ctx.lookup_raw_type(node_id).is_none() {
            if let Some(hint) = self.field_type_hint.clone() {
                if let Some(GenericArg::Type(arg)) = generic_args(&hint).and_then(|a| a.get(index)) {
                    return Ok(self.ctx.type_to_mir(arg).size() as i64);
                }
            }
        }
        let size = self.ctx.lookup_raw_type(node_id).and_then(|ty| {
            // `Channel<T>.buffered()` resolves to `(Sender<T>, Receiver<T>)` — the
            // element type lives in the tuple's first component. Everything else
            // (Vec/Map/Shared/Mutex) resolves to the wrapper directly.
            let container = match ty {
                Type::Tuple(elems) => elems.first()?,
                other => other,
            };
            let GenericArg::Type(inner) = generic_args(container)?.get(index)? else {
                return None;
            };
            self.slot_size_for_type(inner)
        });
        size.ok_or_else(|| {
            LoweringError::InvalidConstruct(format!(
                "no type for type argument {index} of the container built at {node_id:?}"
            ))
        })
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
        match self.container_elem_rask_type(node_id, index)? {
            rask_types::Type::Var(_) => None,
            resolved => Some(self.ctx.type_to_mir(&resolved)),
        }
    }

    /// The checker's own type for a container's `index`-th type argument.
    fn container_elem_rask_type(
        &self,
        node_id: rask_ast::NodeId,
        index: usize,
    ) -> Option<rask_types::Type> {
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
        Some(inner.as_ref().clone())
    }

    /// What a container's `index`-th type argument is, for its constructor to
    /// describe (`elem_strs`), or `None` when the checker has no type here.
    pub(super) fn container_elem_type(&self, node_id: rask_ast::NodeId, index: usize) -> Option<MirType> {
        match self.container_elem_rask_type(node_id, index)? {
            rask_types::Type::Var(_) => None,
            ty => Some(self.elem_type_of(&ty)),
        }
    }

    /// `container_elem_type` as a constructor argument: plain words when the
    /// checker has nothing here, which describes them as owning nothing.
    pub(super) fn container_elem_desc(&self, node_id: rask_ast::NodeId, index: usize) -> MirOperand {
        crate::elem_strs::elem(self.container_elem_type(node_id, index).unwrap_or(MirType::I64))
    }

    /// An element's checked type the way its container describes it.
    ///
    /// Through `payload_to_mir`, because a nested container is a bare `Ptr` in
    /// MIR like every other pointer: a `Map<string, Vec<i32>>` described that
    /// way freed its keys and left every value vector to nobody.
    pub(super) fn elem_type_of(&self, ty: &rask_types::Type) -> MirType {
        match ty {
            // `Map<string, func(i64) -> i64>.new()` records its argument as a
            // name and nothing resolves it further, so the values looked like
            // plain words: the map never held the closure's block, the block
            // went at the end of the statement that inserted it, and calling
            // what came back out answered 13 for `|x| x + 1` applied to 5
            // (#1151).
            rask_types::Type::UnresolvedNamed(name) if name.trim_start().starts_with("func(") => {
                MirType::FuncPtr(crate::types::SignatureId(0))
            }
            _ => self.ctx.payload_to_mir(ty),
        }
    }

    pub(super) fn head_name(&self, ty: &rask_types::Type) -> Option<String> {
        ty.head_name(self.ctx.type_names).map(str::to_string)
    }

    /// How wide one channel element is, for the receive buffer.
    ///
    /// The tracked size comes from the `Channel.buffered()` call site, which
    /// only reaches a variable bound directly to it. A receiver pulled out of
    /// the returned pair (`let rx = ch.1`) has no such record, so fall back
    /// to its own `Receiver<T>` type — otherwise a 24-byte struct was received
    /// into an 8-byte buffer and smashed the stack (#360).
    pub(super) fn channel_elem_size(&self, object: &rask_ast::expr::Expr) -> Result<i64, LoweringError> {
        if let Some(var_name) = object.name() {
            if let Some(size) = self.meta(var_name).and_then(|m| m.channel_elem_size) {
                return Ok(size);
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
        layout: &rask_types::layout::EnumLayout,
        src: &MirOperand,
        obj_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(obj_ty.clone());

        // Shallow copy: the whole value, at its real size. Copying it a word
        // at a time read and wrote past the end of any enum whose size isn't
        // a multiple of eight.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result,
            offset: 0,
            value: src.clone(),
            store_size: layout.size,
            field: None,
        }));

        let needs_switch = layout.variants.iter().any(|v| {
            v.fields.iter().any(|f| Self::clone_fn_for_type(&f.ty).is_some())
        });

        if needs_switch {
            // The tag at its own offset and width, the way a match reads it.
            let tag = self.emit_enum_tag(MirOperand::Local(result));

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
                                byte_offset: Some(abs_offset),
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
                            store_size: MirType::Ptr.size(),
                            field: None,
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

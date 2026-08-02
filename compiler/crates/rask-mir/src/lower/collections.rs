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
        let mut elem_ty = MirType::I64;
        let mut lowered = Vec::new();
        for (i, elem) in elems.iter().enumerate() {
            let (op, ty) = self.lower_expr(elem)?;
            if i == 0 {
                elem_ty = ty;
            }
            lowered.push(op);
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

            // A `Vec<T>` field is an array, not a number. Without this the field
            // went through json_buf_add_i64 and `{"items": [...]}` came out as
            // `{"items":{}}`.
            let vec_args = match &field.ty {
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
                        MirOperand::Constant(MirConst::String(field.name.clone())),
                        arr_json,
                    ],
                }));
                continue;
            }

            // A `T?` field is the value or `null`. It used to go through
            // json_buf_add_i64, which wrote the address of the option's storage —
            // `"assignee":140204924960672` where the answer is `null`.
            if matches!(&field.ty, Type::Result { err, .. } if **err == Type::None) {
                self.emit_json_optional_field(buf, &field.name, field_val)?;
                continue;
            }

            let nested_struct = match &field.ty {
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
                        MirOperand::Constant(MirConst::String(field.name.clone())),
                        nested_json,
                    ],
                }));
                continue;
            }

            let helper = match &field.ty {
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
                    MirOperand::Constant(MirConst::String(field.name.clone())),
                    MirOperand::Local(field_val),
                ],
            }));
        }

        // The StringOutParam adapter writes a 16-byte RaskStr and hands back its
        // address, so this is a string — typing it I64 made `const j =
        // json.encode(v)` print a pointer (#478).
        let result = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal("json_buf_finish".to_string()),
            args: vec![MirOperand::Local(buf)],
        }));

        Ok((MirOperand::Local(result), MirType::String))
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
        let opt_ty = self.builder.local_type(opt_local).unwrap_or(MirType::I64);
        let payload_ty = Self::payload_of_mir(&opt_ty).unwrap_or(MirType::I64);

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
        // address, so this is a string — typing it I64 made `const j =
        // json.encode(v)` print a pointer (#478).
        let result = self.builder.alloc_temp(MirType::String);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal("json_buf_finish_array".to_string()),
            args: vec![MirOperand::Local(arr_buf)],
        }));

        Ok((MirOperand::Local(result), MirType::String))
    }

    /// Expand `json.decode<T>(str)` into json_parse + field extraction.
    pub(super) fn lower_json_decode_struct(
        &mut self,
        str_op: MirOperand,
        layout: StructLayout,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_types::Type;

        let parsed = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(parsed),
            func: FunctionRef::internal("json_parse".to_string()),
            args: vec![str_op],
        }));

        let struct_id = self.ctx.find_struct(&layout.name)
            .map(|(id, sl)| StructLayoutId::new(id, sl.size, sl.align));
        let struct_ty = struct_id
            .map(MirType::Struct)
            .unwrap_or(MirType::I64);

        let result = self.builder.alloc_temp(struct_ty.clone());
        for (_idx, field) in layout.fields.iter().enumerate() {
            let helper = match &field.ty {
                Type::String => "json_get_string",
                Type::Bool => "json_get_bool",
                Type::F32 | Type::F64 => "json_get_f64",
                _ => "json_get_i64",
            };

            let field_val = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(field_val),
                func: FunctionRef::internal(helper.to_string()),
                args: vec![
                    MirOperand::Local(parsed),
                    MirOperand::Constant(MirConst::String(field.name.clone())),
                ],
            }));

            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: result,
                offset: field.offset,
                value: MirOperand::Local(field_val),
                store_size: None,
            }));
        }

        Ok((MirOperand::Local(result), struct_ty))
    }

    /// Size in bytes for a MIR type (used for runtime allocation).
    pub(super) fn elem_size_for_type(&self, ty: &MirType) -> i64 {
        match ty {
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr
            | MirType::FuncPtr(_) | MirType::Handle => 8,
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

    /// How wide one channel element is, for the receive buffer.
    ///
    /// The tracked size comes from the `Channel.buffered()` call site, which
    /// only reaches a variable bound directly to it. A receiver pulled out of
    /// the returned pair (`const rx = ch.1`) has no such record, so fall back
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
                    byte_offset: None,
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

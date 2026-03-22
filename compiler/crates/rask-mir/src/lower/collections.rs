// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Collection construction and cloning lowering:
//! Vec.from, Map.from, JSON encode/decode, enum clone.

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
        let elem_size = elem_ty.size();
        let array_ty = MirType::Array {
            elem: Box::new(elem_ty.clone()),
            len: elems.len() as u32,
        };
        let arr_local = self.builder.alloc_temp(array_ty);
        for (i, op) in lowered.into_iter().enumerate() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: arr_local,
                offset: i as u32 * elem_size,
                value: op,
                store_size: None,
            }));
        }

        let vec_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(vec_local),
            func: FunctionRef::internal("rask_vec_from_static".to_string()),
            args: vec![
                MirOperand::Local(arr_local),
                MirOperand::Constant(MirConst::Int(elems.len() as i64)),
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
            let field_val = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: field_val,
                rvalue: MirRValue::Field {
                    base: struct_op.clone(),
                    field_index: idx as u32,
                    byte_offset: None,
                    field_size: None,
                },
            }));

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

        let result = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal("json_buf_finish".to_string()),
            args: vec![MirOperand::Local(buf)],
        }));

        Ok((MirOperand::Local(result), MirType::I64))
    }

    /// Expand `json.encode(vec)` into a loop that encodes each element.
    pub(super) fn lower_json_encode_vec(
        &mut self,
        vec_op: MirOperand,
        elem_ty: Option<rask_types::Type>,
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

        let elem = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(elem),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
        }));

        let elem_ref = &elem_ty;
        let nested_struct = match elem_ref {
            Some(Type::UnresolvedNamed(name)) => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
            Some(Type::UnresolvedGeneric { name, .. }) => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
            Some(Type::Named(type_id)) => {
                self.ctx.type_names.get(type_id)
                    .and_then(|name| self.ctx.find_struct(name).map(|(_, l)| l.clone()))
            }
            _ => None,
        };

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
                _ => "json_buf_array_add_i64",
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
        let result = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result),
            func: FunctionRef::internal("json_buf_finish_array".to_string()),
            args: vec![MirOperand::Local(arr_buf)],
        }));

        Ok((MirOperand::Local(result), MirType::I64))
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

    /// Size of the Nth generic type parameter in a name like "Vec<string>" or "Map<string, i64>".
    /// Returns 16 for string, struct layout size for structs, 8 otherwise.
    pub(super) fn generic_type_param_size(&self, generic_name: &str, index: usize) -> i64 {
        let inner = generic_name.split('<').nth(1)
            .and_then(|s| s.strip_suffix('>'));
        if let Some(params_str) = inner {
            let params: Vec<&str> = params_str.split(',').map(|s| s.trim()).collect();
            if let Some(type_name) = params.get(index) {
                if *type_name == "string" {
                    return 16;
                }
                if let Some((_, layout)) = self.ctx.find_struct(type_name) {
                    return layout.size as i64;
                }
            }
        }
        8 // scalar default
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
                    field_size: None,
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
                    field_size: None,
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
                                field_size: None,
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

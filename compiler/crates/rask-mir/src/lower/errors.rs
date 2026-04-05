// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Error handling lowering: try, try-else, map_err.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
    types::RESULT_PAYLOAD_OFFSET,
};
use rask_ast::expr::{Expr, ExprKind, TryElse};

impl<'a> MirLowerer<'a> {
    /// Try expression lowering (spec L3).
    pub(super) fn lower_try(&mut self, inner: &Expr) -> Result<TypedOperand, LoweringError> {
        let (result, result_ty) = self.lower_expr(inner)?;

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag {
                value: result.clone(),
            },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // Err path — construct Result.Err with origin and return
        self.builder.switch_to_block(err_block);
        let err_ty = self.extract_err_type(inner)
            .or_else(|| match &result_ty {
                MirType::Result { err, .. } => Some(err.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(MirType::I64);
        let err_store_size = if err_ty.size() > 8 { Some(err_ty.size()) } else { None };
        let err_val = self.builder.alloc_temp(err_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
                byte_offset: None,
                field_size: None,
            },
        }));

        // Construct full Result.Err with origin (ER15)
        let ret_result = self.builder.alloc_temp(result_ty.clone());
        // Tag = 1 (Err)
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::RESULT_TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));
        // Origin: check if source result already has origin, otherwise set from current span.
        // For now, always set origin from the try site (first-propagation is handled by
        // checking existing origin at runtime or via a conditional store).
        // Store file pointer as 0 — codegen will fill with actual source_file ptr.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        // Store line number from current span (codegen resolves byte offset → line)
        let span_start = self.builder.current_span().start as i64;
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(span_start)),
            store_size: None,
        }));
        // Payload — use store_size for aggregates (strings are 16 bytes)
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(err_val),
            store_size: err_store_size,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Local(ret_result)),
        }));

        // Ok path
        self.builder.switch_to_block(ok_block);
        let ok_ty = self.extract_payload_type(inner)
            .or_else(|| match &result_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                _ => None,
            })
            .or_else(|| {
                // Walk through method chains to find the base function call
                let mut expr = inner;
                loop {
                    match &expr.kind {
                        ExprKind::MethodCall { object, method, .. } => {
                            if let ExprKind::Ident(mod_name) = &object.kind {
                                if super::is_type_constructor_name(mod_name) {
                                    let func_name = format!("{}_{}", mod_name, method);
                                    let ret = self.func_sigs.get(&func_name)
                                        .map(|s| s.ret_ty.clone())
                                        .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                                    return match ret {
                                        MirType::Result { ok, .. } => Some(*ok),
                                        MirType::Option(inner) => Some(*inner),
                                        _ => None,
                                    };
                                }
                            }
                            expr = object;
                        }
                        ExprKind::Call { func, .. } => {
                            let name = match &func.kind {
                                ExprKind::Ident(n) => n.clone(),
                                ExprKind::Field { object: o, field: f } => {
                                    if let ExprKind::Ident(mod_name) = &o.kind {
                                        format!("{}_{}", mod_name, f)
                                    } else { break; }
                                }
                                _ => break,
                            };
                            let ret = self.func_sigs.get(&name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&name));
                            return match ret {
                                MirType::Result { ok, .. } => Some(*ok),
                                MirType::Option(inner) => Some(*inner),
                                _ => None,
                            };
                        }
                        _ => break,
                    }
                }
                None
            })
            .unwrap_or(MirType::I64);
        let ok_val = self.builder.alloc_temp(ok_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ok_val,
            rvalue: MirRValue::Field {
                base: result,
                field_index: 0,
                byte_offset: None,
                field_size: None,
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(ok_val), ok_ty))
    }

    /// Try-else expression: `try expr else |e| { transform(e) }`
    pub(super) fn lower_try_else(&mut self, inner: &Expr, try_else: &TryElse) -> Result<TypedOperand, LoweringError> {
        let (result, result_ty) = self.lower_expr(inner)?;

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag {
                value: result.clone(),
            },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // Err path — bind error to param, evaluate else body, return transformed error
        self.builder.switch_to_block(err_block);
        let err_ty = self.extract_err_type(inner)
            .or_else(|| match &result_ty {
                MirType::Result { err, .. } => Some(err.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(MirType::I64);
        let err_val = self.builder.alloc_temp(err_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
                byte_offset: None,
                field_size: None,
            },
        }));

        let err_binding = &try_else.error_binding;
        self.locals.insert(err_binding.clone(), (err_val, err_ty));

        let (transformed_op, transformed_ty) = self.lower_expr(&try_else.body)?;

        // Only emit return if body didn't already terminate (e.g. bare `return` in body)
        if self.builder.current_block_unterminated() {
            // Construct full Result.Err with origin (ER15)
            let ret_result = self.builder.alloc_temp(result_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: crate::types::RESULT_TAG_OFFSET,
                value: MirOperand::Constant(MirConst::Int(1)),
                store_size: None,
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
                value: MirOperand::Constant(MirConst::Int(0)),
                store_size: None,
            }));
            let span_start = self.builder.current_span().start as i64;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
                value: MirOperand::Constant(MirConst::Int(span_start)),
                store_size: None,
            }));
            let transformed_store_size = if transformed_ty.size() > 8 { Some(transformed_ty.size()) } else { None };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: RESULT_PAYLOAD_OFFSET,
                value: transformed_op,
                store_size: transformed_store_size,
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: Some(MirOperand::Local(ret_result)),
            }));
        }

        // Ok path — extract payload
        self.builder.switch_to_block(ok_block);
        let ok_ty = self.extract_payload_type(inner)
            .or_else(|| match &result_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(MirType::I64);
        let ok_val = self.builder.alloc_temp(ok_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ok_val,
            rvalue: MirRValue::Field {
                base: result,
                field_index: 0,
                byte_offset: None,
                field_size: None,
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(ok_val), ok_ty))
    }

    /// Inline expansion of `result.map_err(|e| transform(e))`.
    pub(super) fn lower_map_err(
        &mut self,
        result_op: MirOperand,
        result_ty: &MirType,
        closure_expr: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (closure_op, _) = self.lower_expr(closure_expr)?;
        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: closure_local,
            rvalue: MirRValue::Use(closure_op),
        }));

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: result_op.clone() },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        let out_ty = result_ty.clone();
        let out = self.builder.alloc_temp(out_ty.clone());

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // Ok path: pass through unchanged
        self.builder.switch_to_block(ok_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: out,
            rvalue: MirRValue::Use(result_op.clone()),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // Err path: extract payload, call closure, wrap as Err
        self.builder.switch_to_block(err_block);
        let err_payload = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_payload,
            rvalue: MirRValue::Field { base: result_op, field_index: 0, byte_offset: None, field_size: None },
        }));
        let new_err = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
            dst: Some(new_err),
            closure: closure_local,
            args: vec![MirOperand::Local(err_payload)],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: None,
        }));
        // Zero origin — map_err transforms don't set origin (preserves existing)
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(new_err),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(out), out_ty))
    }

    /// Inline expansion of `result.map_err(VariantConstructor)`.
    pub(super) fn lower_map_err_constructor(
        &mut self,
        result_op: MirOperand,
        result_ty: &MirType,
        constructor_name: &str,
    ) -> Result<TypedOperand, LoweringError> {
        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: result_op.clone() },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        let out_ty = result_ty.clone();
        let out = self.builder.alloc_temp(out_ty.clone());

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // Ok path: pass through unchanged
        self.builder.switch_to_block(ok_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: out,
            rvalue: MirRValue::Use(result_op.clone()),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // Err path: extract payload, wrap with constructor, re-wrap as Err
        self.builder.switch_to_block(err_block);
        let err_payload = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_payload,
            rvalue: MirRValue::Field { base: result_op, field_index: 0, byte_offset: None, field_size: None },
        }));
        let constructor_tag = self.variant_tag(constructor_name);
        let wrapped = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: wrapped,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(constructor_tag)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: wrapped,
            offset: 8,
            value: MirOperand::Local(err_payload),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)), // Err tag
            store_size: None,
        }));
        // Zero origin — constructor wrapping doesn't set origin
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: out,
            offset: RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(wrapped),
            store_size: None,
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(out), out_ty))
    }
}

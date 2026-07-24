// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Error handling lowering: try, try-else, map_err.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
    types::RESULT_PAYLOAD_OFFSET,
};
use rask_ast::expr::{CallArg, Expr, ExprKind, TryElse};

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
        // Origin (ER15): copy from source Result, then set if not already set.
        // Err(...) construction zeros origin, so first try site wins.
        let src_origin_line = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: src_origin_line,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 1, // origin_line is the second field (after tag, before payload)
                byte_offset: Some(crate::types::RESULT_ORIGIN_LINE_OFFSET),
                field_size: Some(8),
            },
        }));
        // Compute this try site's line number
        let try_line = self.ctx.line_map
            .map(|lm| lm.offset_to_line_col(self.builder.current_span().start).0 as i64)
            .unwrap_or(0);

        // If source origin_line is 0 (unset), use this try site; otherwise preserve source.
        // MIR doesn't have select/cmov, so use branch.
        let origin_set_block = self.builder.create_block();
        let origin_unset_block = self.builder.create_block();
        let origin_merge_block = self.builder.create_block();
        let origin_line_local = self.builder.alloc_temp(MirType::I64);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(src_origin_line),
            then_block: origin_set_block,
            else_block: origin_unset_block,
        }));

        // Source had origin → copy it
        self.builder.switch_to_block(origin_set_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: origin_line_local,
            rvalue: MirRValue::Use(MirOperand::Local(src_origin_line)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: origin_merge_block,
        }));

        // Source had no origin → set from this try site
        self.builder.switch_to_block(origin_unset_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: origin_line_local,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(try_line))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: origin_merge_block,
        }));

        self.builder.switch_to_block(origin_merge_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
            value: MirOperand::Local(origin_line_local),
            store_size: None,
        }));
        // Payload — use store_size for aggregates (strings are 16 bytes)
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(err_val),
            store_size: err_store_size,
        }));
        if self.ensure_stack.is_empty() {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: Some(MirOperand::Local(ret_result)),
            }));
        } else {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::CleanupReturn {
                value: Some(MirOperand::Local(ret_result)),
                cleanup_chain: self.cleanup_chain(),
            }));
        }

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

        // Read source origin before transform body (ER15 first-propagation)
        let src_origin_line = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: src_origin_line,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 1,
                byte_offset: Some(crate::types::RESULT_ORIGIN_LINE_OFFSET),
                field_size: Some(8),
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
            // Preserve source origin if set, otherwise use this try site
            let try_line = self.ctx.line_map
                .map(|lm| lm.offset_to_line_col(self.builder.current_span().start).0 as i64)
                .unwrap_or(0);
            let origin_set_blk = self.builder.create_block();
            let origin_unset_blk = self.builder.create_block();
            let origin_merge_blk = self.builder.create_block();
            let origin_line_local = self.builder.alloc_temp(MirType::I64);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(src_origin_line),
                then_block: origin_set_blk,
                else_block: origin_unset_blk,
            }));
            self.builder.switch_to_block(origin_set_blk);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: origin_line_local,
                rvalue: MirRValue::Use(MirOperand::Local(src_origin_line)),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: origin_merge_blk }));
            self.builder.switch_to_block(origin_unset_blk);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: origin_line_local,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(try_line))),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: origin_merge_blk }));
            self.builder.switch_to_block(origin_merge_blk);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
                value: MirOperand::Constant(MirConst::Int(0)),
                store_size: None,
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
                value: MirOperand::Local(origin_line_local),
                store_size: None,
            }));
            let transformed_store_size = if transformed_ty.size() > 8 { Some(transformed_ty.size()) } else { None };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_result,
                offset: RESULT_PAYLOAD_OFFSET,
                value: transformed_op,
                store_size: transformed_store_size,
            }));
            if self.ensure_stack.is_empty() {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(ret_result)),
                }));
            } else {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::CleanupReturn {
                    value: Some(MirOperand::Local(ret_result)),
                    cleanup_chain: self.cleanup_chain(),
                }));
            }
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

    /// Inline lowering for Result/Option methods that have stdlib stubs but
    /// no runtime implementation (`.map`, `.ok`, `.filter`).
    ///
    /// Returns `Ok(Some(operand))` when the call was inlined; `Ok(None)`
    /// when the receiver isn't a Result/Option or the method isn't one we
    /// inline. Falling through lets the normal method-dispatch path handle
    /// other calls.
    pub(super) fn try_lower_result_option_method(
        &mut self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[CallArg],
        obj_op: &MirOperand,
        obj_ty: &MirType,
    ) -> Result<Option<super::TypedOperand>, LoweringError> {
        let raw_ty = match self.ctx.lookup_raw_type(object.id) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        let is_result = matches!(&raw_ty, rask_types::Type::Result { err, .. }
            if **err != rask_types::Type::None && !matches!(**err, rask_types::Type::Var(_)));
        let is_option = matches!(&raw_ty, rask_types::Type::Result { err, .. } if **err == rask_types::Type::None);
        if !is_result && !is_option {
            return Ok(None);
        }

        // Skip when the type checker couldn't fully resolve the err side
        // (unresolved type variables) — MIR lowering may end up with a
        // non-Result MirType and the inline lowering will fail to match.
        let result = match (is_result, method, args.len()) {
            (true, "map", 1) => self.lower_result_map(expr, obj_op.clone(), obj_ty.clone(), &args[0].expr).map(Some),
            (true, "ok", 0) => self.lower_result_ok(expr, obj_op.clone(), obj_ty.clone()).map(Some),
            (false, "map", 1) => self.lower_option_map(expr, obj_op.clone(), obj_ty.clone(), &args[0].expr).map(Some),
            (false, "filter", 1) => self.lower_option_filter(expr, obj_op.clone(), obj_ty.clone(), &args[0].expr).map(Some),
            _ => Ok(None),
        };
        // If the inline lowering fails because the receiver's MIR type
        // doesn't actually match Result/Option (type checker unresolved),
        // fall through to the regular dispatch path instead of erroring.
        match result {
            Err(LoweringError::InvalidConstruct(msg)) if msg.contains("receiver must be") => Ok(None),
            other => other,
        }
    }

    /// Inline `result.map(closure)` for Result<T, E>:
    ///   if Ok(t): result = Ok(closure(t))
    ///   if Err(e): result = Err(e)  (copy through)
    fn lower_result_map(
        &mut self,
        expr: &Expr,
        obj_op: MirOperand,
        obj_ty: MirType,
        closure: &Expr,
    ) -> Result<super::TypedOperand, LoweringError> {
        let (closure_op, _) = self.lower_expr(closure)?;
        let closure_local = match closure_op {
            MirOperand::Local(id) => id,
            _ => return Err(LoweringError::InvalidConstruct(
                "Result.map closure must be a local".to_string(),
            )),
        };

        // Result types: in T, err E; out U, err E.
        let (in_ok_ty, err_ty) = match &obj_ty {
            MirType::Result { ok, err } => ((**ok).clone(), (**err).clone()),
            _ => return Err(LoweringError::InvalidConstruct(
                "Result.map receiver must be Result".to_string(),
            )),
        };
        let result_ty = self.lookup_expr_type(expr)
            .unwrap_or(MirType::Result { ok: Box::new(MirType::I64), err: Box::new(err_ty.clone()) });
        let out_ok_ty = match &result_ty {
            MirType::Result { ok, .. } => (**ok).clone(),
            _ => MirType::I64,
        };

        let result_local = self.builder.alloc_temp(result_ty.clone());

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: obj_op.clone() },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // Ok branch: read T payload, call closure, store new Ok. Scalars read at
        // RESULT_PAYLOAD_OFFSET; aggregates use the None fast-path (#350).
        self.builder.switch_to_block(ok_block);
        let payload_local = self.builder.alloc_temp(in_ok_ty.clone());
        let in_is_aggregate = matches!(
            in_ok_ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::String
        );
        let in_byte_offset = if in_is_aggregate { None } else { Some(RESULT_PAYLOAD_OFFSET) };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload_local,
            rvalue: MirRValue::Field {
                base: obj_op.clone(),
                field_index: 0,
                byte_offset: in_byte_offset,
                field_size: None,
            },
        }));
        let mapped_local = self.builder.alloc_temp(out_ok_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
            dst: Some(mapped_local),
            closure: closure_local,
            args: vec![MirOperand::Local(payload_local)],
        }));
        // tag = 0, zero origin, payload = mapped value.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: Some(8),
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 8,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: Some(8),
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 16,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: Some(8),
        }));
        let payload_size = out_ok_ty.size().max(8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local,
            offset: RESULT_PAYLOAD_OFFSET,
            value: MirOperand::Local(mapped_local),
            store_size: Some(payload_size),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // Err branch: copy whole source Result to result_local (tag=1, origin, err
        // payload are preserved). Same MIR shape works because both Result types
        // share the err side.
        self.builder.switch_to_block(err_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result_local,
            rvalue: MirRValue::Use(obj_op),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Inline `result.ok()` for Result<T, E>: Result<T, E> → T?
    ///   if Ok(t): Some(t); if Err: None
    fn lower_result_ok(
        &mut self,
        expr: &Expr,
        obj_op: MirOperand,
        obj_ty: MirType,
    ) -> Result<super::TypedOperand, LoweringError> {
        let in_ok_ty = match &obj_ty {
            MirType::Result { ok, .. } => (**ok).clone(),
            _ => return Err(LoweringError::InvalidConstruct(
                "Result.ok receiver must be Result".to_string(),
            )),
        };
        let result_ty = self.lookup_expr_type(expr)
            .unwrap_or(MirType::Option(Box::new(in_ok_ty.clone())));
        let result_local = self.builder.alloc_temp(result_ty.clone());

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: obj_op.clone() },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // Ok branch: result = Some(payload). Read the source Result's ok payload
        // at RESULT_PAYLOAD_OFFSET (scalars); aggregates use the None fast-path
        // (the field access yields the payload slot address). Mirrors the `!`
        // unwrap in emit_option_payload — without the explicit offset the read
        // lands on the origin fields and returns garbage (#350).
        self.builder.switch_to_block(ok_block);
        let payload_local = self.builder.alloc_temp(in_ok_ty.clone());
        let ok_is_aggregate = matches!(
            in_ok_ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::String
        );
        let ok_byte_offset = if ok_is_aggregate { None } else { Some(RESULT_PAYLOAD_OFFSET) };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload_local,
            rvalue: MirRValue::Field {
                base: obj_op.clone(),
                field_index: 0,
                byte_offset: ok_byte_offset,
                field_size: None,
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: Some(8),
        }));
        let payload_size = in_ok_ty.size().max(8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 8,
            value: MirOperand::Local(payload_local),
            store_size: Some(payload_size),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // Err branch: result = None (tag=1)
        self.builder.switch_to_block(err_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: Some(8),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Inline `option.map(closure)` for Option<T>: T? → U?
    fn lower_option_map(
        &mut self,
        expr: &Expr,
        obj_op: MirOperand,
        obj_ty: MirType,
        closure: &Expr,
    ) -> Result<super::TypedOperand, LoweringError> {
        let (closure_op, _) = self.lower_expr(closure)?;
        let closure_local = match closure_op {
            MirOperand::Local(id) => id,
            _ => return Err(LoweringError::InvalidConstruct(
                "Option.map closure must be a local".to_string(),
            )),
        };
        let in_ty = match &obj_ty {
            MirType::Option(inner) => (**inner).clone(),
            MirType::Result { ok, err } if **err == MirType::Void => (**ok).clone(),
            _ => return Err(LoweringError::InvalidConstruct(
                "Option.map receiver must be Option".to_string(),
            )),
        };
        let result_ty = self.lookup_expr_type(expr)
            .unwrap_or(MirType::Option(Box::new(MirType::I64)));
        let out_ty = match &result_ty {
            MirType::Option(inner) => (**inner).clone(),
            _ => MirType::I64,
        };

        let result_local = self.builder.alloc_temp(result_ty.clone());

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: obj_op.clone() },
        }));

        let some_block = self.builder.create_block();
        let none_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: none_block,
            else_block: some_block,
        }));

        // Some branch: closure(payload), result = Some(mapped)
        self.builder.switch_to_block(some_block);
        let payload_local = self.builder.alloc_temp(in_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload_local,
            rvalue: MirRValue::Field {
                base: obj_op.clone(),
                field_index: 0,
                byte_offset: None,
                field_size: None,
            },
        }));
        let mapped_local = self.builder.alloc_temp(out_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
            dst: Some(mapped_local),
            closure: closure_local,
            args: vec![MirOperand::Local(payload_local)],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)),
            store_size: Some(8),
        }));
        let payload_size = out_ty.size().max(8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 8,
            value: MirOperand::Local(mapped_local),
            store_size: Some(payload_size),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // None: result = None
        self.builder.switch_to_block(none_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: Some(8),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Inline `option.filter(closure)` for Option<T>: T? → T?
    ///   if Some(t) and closure(t): Some(t); else: None
    fn lower_option_filter(
        &mut self,
        expr: &Expr,
        obj_op: MirOperand,
        obj_ty: MirType,
        closure: &Expr,
    ) -> Result<super::TypedOperand, LoweringError> {
        let (closure_op, _) = self.lower_expr(closure)?;
        let closure_local = match closure_op {
            MirOperand::Local(id) => id,
            _ => return Err(LoweringError::InvalidConstruct(
                "Option.filter closure must be a local".to_string(),
            )),
        };
        let in_ty = match &obj_ty {
            MirType::Option(inner) => (**inner).clone(),
            MirType::Result { ok, err } if **err == MirType::Void => (**ok).clone(),
            _ => return Err(LoweringError::InvalidConstruct(
                "Option.filter receiver must be Option".to_string(),
            )),
        };
        let result_ty = self.lookup_expr_type(expr).unwrap_or(obj_ty.clone());

        let result_local = self.builder.alloc_temp(result_ty.clone());

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: obj_op.clone() },
        }));

        let some_block = self.builder.create_block();
        let none_block = self.builder.create_block();
        let keep_block = self.builder.create_block();
        let drop_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: none_block,
            else_block: some_block,
        }));

        // Some branch: closure(payload) → if true keep, else drop
        self.builder.switch_to_block(some_block);
        let payload_local = self.builder.alloc_temp(in_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload_local,
            rvalue: MirRValue::Field {
                base: obj_op.clone(),
                field_index: 0,
                byte_offset: None,
                field_size: None,
            },
        }));
        let keep_local = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
            dst: Some(keep_local),
            closure: closure_local,
            args: vec![MirOperand::Local(payload_local)],
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(keep_local),
            then_block: keep_block,
            else_block: drop_block,
        }));

        // keep: result = source (copy)
        self.builder.switch_to_block(keep_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: result_local,
            rvalue: MirRValue::Use(obj_op.clone()),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // drop: result = None
        self.builder.switch_to_block(drop_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: Some(8),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        // None: result = None
        self.builder.switch_to_block(none_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: result_local, offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
            store_size: Some(8),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }
}

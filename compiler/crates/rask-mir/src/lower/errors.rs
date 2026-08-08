// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Error handling lowering: try, try-else, map_err.

use crate::FieldAccess;
use super::{LoweringError, MirLowerer, TypedOperand, HANDLE_NONE_SENTINEL};
use crate::{
    operand::{BinOp, MirConst}, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
    types::RESULT_PAYLOAD_OFFSET,
};
use rask_ast::expr::{CallArg, CatchClause, Expr, ExprKind};

/// ER31a: where a propagated error goes inside the caller's error enum.
struct ErrorWrapTarget {
    enum_ty: MirType,
    tag_offset: u32,
    tag: i64,
    /// Byte offset of the variant's single payload field.
    payload_offset: u32,
}

impl<'a> MirLowerer<'a> {
    /// The error type of a `try` target's Result, from whichever source
    /// resolved it. The checker's type for the whole `try` expression and the
    /// lowered MIR type of the inner can each be an unresolved `Ptr` (a
    /// cross-module lock chain leaves the checker type a var; a plain call can
    /// leave the MIR type bare), so take whichever landed a concrete type. A
    /// wrong type here sizes the error slot wrong and blocks method dispatch on
    /// the `else |e|` binding.
    fn resolved_err_type(&self, inner: &Expr, result_ty: &MirType) -> MirType {
        let err_from_result = match result_ty {
            MirType::Result { err, .. } => Some(err.as_ref().clone()),
            _ => None,
        };
        let candidates = [self.extract_err_type(inner), err_from_result];
        candidates.iter().flatten()
            .find(|t| !matches!(t, MirType::Ptr))
            .or_else(|| candidates.iter().flatten().next())
            .cloned()
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/errors:41"))
    }

    /// The ok payload type, from whichever source resolved it — same reasoning as
    /// `resolved_err_type`, for the other side.
    ///
    /// `Ptr` is what both sources produce when they couldn't work the type out,
    /// and the checker's answer is not automatically the better one: a lock chain
    /// like `store.lock().view_task(id)` left the checker with a `Ptr` ok type
    /// while the lowered result type knew it was a `TaskView`. Taking the
    /// checker's `Ptr` typed the binding as an opaque pointer, so
    /// `json.encode(view)` fell through to `json_encode_i64` and `/tasks/1`
    /// answered with the task's id instead of the task.
    pub(super) fn better_payload_ty(from_checker: Option<MirType>, from_result: Option<MirType>) -> Option<MirType> {
        let candidates = [from_checker, from_result];
        candidates.iter().flatten()
            .find(|t| !matches!(t, MirType::Ptr))
            .or_else(|| candidates.iter().flatten().next())
            .cloned()
    }

    /// True when `inner` is a `comptime` expression whose evaluation already
    /// unwrapped a `T or E`/`T?` down to its bare ok payload (CT45, #403).
    /// `ExprKind::Comptime` lowering returns a raw scalar for `ComptimeValue::
    /// Bool/I64/String` and only falls back to normal (Result-shaped) lowering
    /// for everything else, so a scalar `result_ty` here means the wrapper is
    /// already gone — there's no tag byte left to read.
    fn is_collapsed_comptime_ok(inner: &Expr, result_ty: &MirType) -> bool {
        matches!(inner.kind, ExprKind::Comptime { .. })
            && !matches!(result_ty, MirType::Result { .. } | MirType::Option(_))
    }

    /// ER31a: where this `try` puts its error inside the caller's error enum.
    /// The checker picked the variant; this resolves it against the enum layout.
    /// `None` when the error propagates as-is.
    fn error_wrap_target(&self, try_id: rask_ast::NodeId) -> Option<ErrorWrapTarget> {
        let wrap = self.ctx.error_wraps.get(&try_id)?;
        let (idx, layout) = self.ctx.find_enum(&wrap.enum_name)?;
        let variant = layout.variants.iter().find(|v| v.name == wrap.variant)?;
        let field = variant.fields.first();
        Some(ErrorWrapTarget {
            enum_ty: MirType::Enum(crate::types::EnumLayoutId::new(idx, layout.size, layout.align)),
            tag_offset: layout.tag_offset,
            tag: variant.tag as i64,
            payload_offset: variant.payload_offset + field.map_or(0, |f| f.offset),
        })
    }

    /// Try expression lowering (spec L3).
    pub(super) fn lower_try(
        &mut self,
        try_id: rask_ast::NodeId,
        inner: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (result, result_ty) = self.lower_expr(inner)?;

        // `try comptime f()`: comptime evaluation already collapsed the `T or E`
        // to its known-success payload — a plain scalar, not the tagged union
        // `try` normally reads a byte from. There's no runtime tag to check, and
        // treating the scalar as if it were a pointer to that union reads
        // whatever memory the value happens to look like (CT45, #403).
        if Self::is_collapsed_comptime_ok(inner, &result_ty) {
            return Ok((result, result_ty));
        }

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
        // Inside a `try { … } else |e| …` block the error belongs to the
        // handler, not to the caller (ER18), so read it straight into the
        // handler's binding at the type the handler will dispatch on.
        let handler = self.catch_frames.last().cloned();
        let err_ty = match &handler {
            Some(frame) => frame.err_ty.clone(),
            None => self.resolved_err_type(inner, &result_ty),
        };
        let err_store_size = if err_ty.size() > 8 { Some(err_ty.size()) } else { None };
        let err_byte_offset = self.payload_byte_offset(&err_ty);
        // ER31a only applies when the error leaves this function, so a `try`
        // caught by an enclosing `try { … } else` handler keeps its own type.
        let wrap = match &handler {
            Some(_) => None,
            None => self.error_wrap_target(try_id),
        };
        let err_val = match &handler {
            Some(frame) => frame.err_val,
            None => self.builder.alloc_temp(err_ty.clone()),
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
                // Explicit offset so a scalar err payload loads its value even when
                // the ok side is an aggregate (same ambiguity as #389's ok-path fix).
                byte_offset: err_byte_offset,
                // Wrapping copies the error into an enum slot, so an aggregate
                // one has to come back as an address to copy from.
                access: match &wrap {
                    Some(_) => aggregate_payload_access(&err_ty),
                    None => FieldAccess::Word,
                },
            },
        }));

        if let Some(frame) = handler {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: frame.origin_line,
                rvalue: MirRValue::Field {
                    base: result.clone(),
                    field_index: 1,
                    byte_offset: Some(crate::types::RESULT_ORIGIN_LINE_OFFSET),
                    access: FieldAccess::Sized(8),
                },
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: frame.handler_block,
            }));
            return self.finish_try_ok_path(inner, &result, &result_ty, ok_block, merge_block);
        }

        // ER31a: hand the caller the error type it declared. A `try store.view(id)`
        // inside `-> TaskView or ApiError` builds `ApiError.Store(e)` right here —
        // the same value `else |e| ApiError.Store(e)` would have produced by hand.
        let (err_val, err_store_size) = match wrap {
            Some(w) => {
                let wrapped = self.builder.alloc_temp(w.enum_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: wrapped,
                    offset: w.tag_offset,
                    value: MirOperand::Constant(MirConst::Int(w.tag)),
                    store_size: None,
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: wrapped,
                    offset: w.payload_offset,
                    value: MirOperand::Local(err_val),
                    store_size: err_store_size,
                }));
                let size = w.enum_ty.size();
                (wrapped, if size > 8 { Some(size) } else { None })
            }
            None => (err_val, err_store_size),
        };

        // An optional-returning function propagates *absence*, and an Option is
        // a tag with nothing beside it — no origin fields, no payload. The
        // Result construction below writes origin at offsets 8 and 16 and a
        // payload at 24, which runs past the end of a 16-byte Option slot; and
        // the type match further down only knows about Result, so the slot got
        // typed after the *source* optional instead of the return. That's how
        // `try raw` in a `-> u16?` built a `string?`-shaped Result and the
        // caller's `??` read a payload of 1 where the default belonged (#608).
        if let MirType::Option(_) = self.builder.ret_ty() {
            let none_ty = self.builder.ret_ty().clone();
            let ret_none = self.builder.alloc_temp(none_ty);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: ret_none,
                offset: rask_mono::abi::OPTION_TAG_OFFSET,
                value: MirOperand::Constant(MirConst::Int(1)),
                store_size: None,
            }));
            if self.ensure_stack.is_empty() {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(ret_none)),
                }));
            } else {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::CleanupReturn {
                    value: Some(MirOperand::Local(ret_none)),
                    cleanup_chain: self.cleanup_chain(),
                }));
            }
            return self.finish_try_ok_path(inner, &result, &result_ty, ok_block, merge_block);
        }

        // Construct full Result.Err with origin (ER15). The slot has to be the
        // *enclosing* function's return type — this value is what `return`
        // hands back. Using the callee's Result type gave the caller a slot of
        // the wrong shape whenever the two differed, e.g. propagating a
        // `KV? or E` out of a `string or E` function.
        let ret_result_ty = match self.builder.ret_ty() {
            ty @ MirType::Result { .. } => ty.clone(),
            _ => result_ty.clone(),
        };
        let ret_result = self.builder.alloc_temp(ret_result_ty);
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
                access: FieldAccess::Sized(8),
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

        self.finish_try_ok_path(inner, &result, &result_ty, ok_block, merge_block)
    }

    /// The Ok side of a `try`: read the payload and continue at `merge_block`.
    /// Shared by the propagate-to-caller path and the ER18 handler path.
    fn finish_try_ok_path(
        &mut self,
        inner: &Expr,
        result: &MirOperand,
        result_ty: &MirType,
        ok_block: crate::BlockId,
        merge_block: crate::BlockId,
    ) -> Result<TypedOperand, LoweringError> {
        self.builder.switch_to_block(ok_block);
        let ok_ty = Self::better_payload_ty(
            self.extract_payload_type(inner),
            match result_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                _ => None,
            },
        )
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
            .unwrap_or_else(|| crate::fallback::i64_fallback("lower/errors:367"));
        let ok_val = self.builder.alloc_temp(ok_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ok_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
                // Explicit offset so codegen loads the value at RESULT_PAYLOAD_OFFSET
                // instead of guessing "aggregate" from the err side's type and handing
                // back the slot address (#389). The size goes with it when the
                // payload is an aggregate, so codegen knows to take its address
                // rather than load its first word (#383).
                byte_offset: self.payload_byte_offset(&ok_ty),
                access: aggregate_payload_access(&ok_ty),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(ok_val), ok_ty))
    }

    /// Try-else expression: `try expr else |e| { transform(e) }`
    /// ER18 block form: `try { … } else |e| handler`.
    ///
    /// The block itself produces the value; every `try` inside it hands its
    /// error to the handler instead of returning from the function. Lowering
    /// the block as if it were a Result read a tag off the block's own value —
    /// on `try { try s.parse<f64>() }` that meant tag-testing an f64, which the
    /// Cranelift verifier rejected outright (#480).
    pub(super) fn lower_catch_block(
        &mut self,
        block: &Expr,
        clause: &CatchClause,
    ) -> Result<TypedOperand, LoweringError> {
        // The handler binds `e` at the enclosing function's error type — that's
        // what the checker scopes it as.
        let err_ty = match self.builder.ret_ty() {
            MirType::Result { err, .. } => (**err).clone(),
            _ => MirType::I64,
        };
        let err_val = self.builder.alloc_temp(err_ty.clone());
        let origin_line = self.builder.alloc_temp(MirType::I64);
        let handler_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.catch_frames.push(super::CatchFrame {
            handler_block,
            err_val,
            err_ty: err_ty.clone(),
            origin_line,
        });
        let block_result = self.lower_expr(block);
        self.catch_frames.pop();
        let (block_op, block_ty) = block_result?;

        let value = self.builder.alloc_temp(block_ty.clone());
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: value,
                rvalue: MirRValue::Use(block_op),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        // Handler: `e` is in scope, and its value is the expression's value.
        // In practice the handler diverges (`return`), which terminates here.
        self.builder.switch_to_block(handler_block);
        let shadowed = if clause.is_discard() {
            None
        } else {
            self.locals.insert(clause.binder.clone(), (err_val, err_ty))
        };
        let (handler_op, _) = self.lower_expr(&clause.body)?;
        if !clause.is_discard() {
            match shadowed {
                Some(prev) => { self.locals.insert(clause.binder.clone(), prev); }
                None => { self.locals.remove(&clause.binder); }
            }
        }
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: value,
                rvalue: MirRValue::Use(handler_op),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(value), block_ty))
    }

    /// ER14: `r catch <binder> => <body>`. The success payload on the ok path,
    /// the handler's value on the err path — and the handler only runs there,
    /// which is what makes the right side lazy.
    pub(super) fn lower_catch(
        &mut self,
        node: rask_ast::NodeId,
        inner: &Expr,
        clause: &CatchClause,
    ) -> Result<TypedOperand, LoweringError> {
        // ER18: `try { … } catch e => …` — the handler covers the whole block,
        // so the first inner `try` that fails jumps straight to it.
        if let ExprKind::Try { expr: block } = &inner.kind {
            if matches!(block.kind, ExprKind::Block(_)) {
                return self.lower_catch_block(block, clause);
            }
        }

        let (result, result_ty) = self.lower_expr(inner)?;

        // Same comptime short-circuit as `lower_try` — a collapsed scalar can't
        // take the error branch, so there's no tag to check (CT45, #403).
        if Self::is_collapsed_comptime_ok(inner, &result_ty) {
            return Ok((result, result_ty));
        }

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: result.clone() },
        }));

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        }));

        // ER14a: when the handler's value is itself two-branch (`catch _ =>
        // none`), the whole expression stays wrapped — but in the *handler's*
        // shape, not the operand's, so the success has to be re-wrapped rather
        // than passed through. The two differ in what the other branch holds.
        let keeps_shape = self.ctx.fallback_keeps_shape.contains(&node);
        let payload_ty = Self::better_payload_ty(
            self.extract_payload_type(inner),
            match &result_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                _ => None,
            },
        ).unwrap_or_else(|| crate::fallback::i64_fallback("lower/errors:fallback_payload"));
        let ok_ty = if keeps_shape {
            MirType::Option(Box::new(payload_ty.clone()))
        } else {
            payload_ty.clone()
        };
        let value = self.builder.alloc_temp(ok_ty.clone());

        // Err path — bind the error (unless it's `_`) and run the handler.
        self.builder.switch_to_block(err_block);
        let err_ty = self.resolved_err_type(inner, &result_ty);
        let err_val = self.builder.alloc_temp(err_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
                byte_offset: self.payload_byte_offset(&err_ty),
                access: FieldAccess::Word,
            },
        }));
        let shadowed = if clause.is_discard() {
            None
        } else {
            self.locals.insert(clause.binder.clone(), (err_val, err_ty))
        };
        let (handler_op, _) = self.lower_expr(&clause.body)?;
        if !clause.is_discard() {
            match shadowed {
                Some(prev) => { self.locals.insert(clause.binder.clone(), prev); }
                None => { self.locals.remove(&clause.binder); }
            }
        }
        // A diverging handler (`catch e => return …`) already terminated.
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: value,
                rvalue: MirRValue::Use(handler_op),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        // Ok path — read the payload out, and re-wrap it when the shape stays.
        self.builder.switch_to_block(ok_block);
        let payload = self.builder.alloc_temp(payload_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: payload,
            rvalue: MirRValue::Field {
                base: result,
                field_index: 0,
                byte_offset: self.payload_byte_offset(&payload_ty),
                access: FieldAccess::Word,
            },
        }));
        // Assigning a bare payload into an option-typed slot is the wrap —
        // codegen and the interpreter both read the destination's type.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: MirRValue::Use(MirOperand::Local(payload)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(value), ok_ty))
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
            rvalue: MirRValue::Field { base: result_op, field_index: 0, byte_offset: None, access: FieldAccess::Word },
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
            rvalue: MirRValue::Field { base: result_op, field_index: 0, byte_offset: None, access: FieldAccess::Word },
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
            // Tag predicates. Stubbed for the checker, never implemented for
            // codegen — the call reached `Result_is_err`, which doesn't exist.
            (true, "is_err", 0) | (false, "is_none", 0) => {
                Ok(Some(self.lower_result_tag_test(obj_op.clone(), true)))
            }
            (true, "is_ok", 0) | (false, "is_some", 0) => {
                Ok(Some(self.lower_result_tag_test(obj_op.clone(), false)))
            }
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

    /// `is_ok`/`is_err` (and the `T?` spellings `is_some`/`is_none`) as a tag
    /// comparison. A nonzero tag is the error/absent side, matching how every
    /// other Result lowering here branches on it.
    fn lower_result_tag_test(&mut self, obj_op: MirOperand, want_err: bool) -> super::TypedOperand {
        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: obj_op },
        }));

        let out = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: out,
            rvalue: MirRValue::BinaryOp {
                op: if want_err { BinOp::Ne } else { BinOp::Eq },
                left: MirOperand::Local(tag_local),
                right: MirOperand::Constant(MirConst::Int(0)),
            },
        }));
        (MirOperand::Local(out), MirType::Bool)
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
                access: FieldAccess::Word,
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

        // `Handle?` is a niche — the sentinel *is* the None, with no tag or
        // struct slot beside it (same shape `lower_none`/`emit_option_payload`
        // use). Building the generic tagged-Option here — alloc a struct slot,
        // `Store` a tag byte and payload into it — asked codegen to take the
        // address of a value it never gave a stack slot, and the store landed
        // on whatever that defaulted to (#556-adjacent; caught via
        // `examples/game_loop.rk` segfaulting in `GameState.spawn_enemy(..).ok()`).
        let is_niche = self.option_is_niche(expr, &result_ty);
        if is_niche {
            let result_local = self.builder.alloc_temp(MirType::Handle);

            self.builder.switch_to_block(ok_block);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Field {
                    base: obj_op.clone(),
                    field_index: 0,
                    byte_offset: Some(RESULT_PAYLOAD_OFFSET),
                    access: FieldAccess::Word,
                },
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

            self.builder.switch_to_block(err_block);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(HANDLE_NONE_SENTINEL))),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

            self.builder.switch_to_block(merge_block);
            return Ok((MirOperand::Local(result_local), MirType::Handle));
        }

        let result_local = self.builder.alloc_temp(result_ty.clone());

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
                access: FieldAccess::Word,
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
                access: FieldAccess::Word,
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
                access: FieldAccess::Word,
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

/// How a `T or E` payload comes back. An aggregate one lives in place, so its
/// address is the answer — and that has to be said outright, because `ok` and
/// `err` can disagree and the size alone can't settle it (#383/#389).
fn aggregate_payload_access(ty: &MirType) -> FieldAccess {
    if ty.passed_by_address() {
        FieldAccess::InPlace(ty.size())
    } else {
        FieldAccess::Word
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Error handling lowering: try, try-else.

use crate::{FieldAccess, FunctionRef};
use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
};
use rask_ast::expr::{CatchClause, Expr, ExprKind};

/// What a `try` says when it has nowhere to propagate to. Both backends print
/// it, so it is declared once where both can see it.
pub use rask_stdlib::panic_messages::TRY_PROPAGATED_NOWHERE;

/// ER31a: where a propagated error goes inside the caller's error enum.
struct ErrorWrapTarget {
    enum_ty: MirType,
    tag_offset: u32,
    tag_bytes: u32,
    tag: i64,
    /// Byte offset of the variant's single payload field.
    payload_offset: u32,
    /// That field's width.
    payload_size: Option<u32>,
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
            .unwrap_or_else(|| crate::fallback::unknown_type("lower/errors:41"))
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
    /// A `Heap<T>` beats both. The checker erases `Heap<T>` to `T` (HP5), so
    /// where the lowered type says `heap<i64>` and the checker says `i64` they
    /// are describing the same value and only one of them knows there is a
    /// block — `let v = make() catch _ => return` then `*v` read the block's
    /// address as the number (#1256).
    pub(super) fn better_payload_ty(from_checker: Option<MirType>, from_result: Option<MirType>) -> Option<MirType> {
        let candidates = [from_checker, from_result];
        if let Some(heap) = candidates.iter().flatten().find(|t| matches!(t, MirType::Heap(_))) {
            return Some(heap.clone());
        }
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
            tag_bytes: layout.tag_bytes(),
            tag: variant.tag as i64,
            payload_offset: variant.payload_offset + field.map_or(0, |f| f.offset),
            payload_size: field.map(|f| f.size),
        })
    }

    /// Try expression lowering (spec L3).
    pub(super) fn lower_try(
        &mut self,
        try_id: rask_ast::NodeId,
        inner: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        // ER16a: the `try` may belong to a step inside the chain rather than to
        // the whole of it — `try read_file(p).len()` branches at the call and
        // hands `.len()` the payload. The checker worked out which step; arm it
        // here and `lower_expr` emits the branch when it gets there.
        if let Some(step) = self.ctx.try_chain_placement.get(&try_id).copied() {
            if step != inner.id {
                let saved = self.pending_try_step.replace((try_id, step));
                let lowered = self.lower_expr(inner);
                self.pending_try_step = saved;
                return lowered;
            }
        }
        let (result, result_ty) = self.lower_expr(inner)?;
        self.emit_try_branch(try_id, inner, result, result_ty)
    }

    /// The branch itself: read the tag, leave through the error path, carry on
    /// with the payload. Split out of `lower_try` so ER16a can emit it at a
    /// chain step instead of at the `try` node.
    pub(super) fn emit_try_branch(
        &mut self,
        try_id: rask_ast::NodeId,
        inner: &Expr,
        result: MirOperand,
        result_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
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
        // The error goes into the payload slot at its own size.
        let err_store_size = self.aggregate_size(&err_ty);
        // ER31a only applies when the error leaves this function, so a `try`
        // caught by an enclosing `try { … } else` handler keeps its own type.
        let wrap = match &handler {
            Some(_) => None,
            None => self.error_wrap_target(try_id),
        };
        // ER32: the enclosing function's error side is `any Interface`, so the
        // concrete error acquires a vtable on its way out. Same trigger as
        // ER31a's enum wrap and mutually exclusive with it — an error is either
        // named by a boundary enum or type-erased, not both.
        //
        // Without this the checker accepted the propagation and the payload
        // stored into the caller's slot was the bare concrete value. The caller
        // then read its first two words as {data, vtable} and called through
        // whatever the second one happened to be, which is a segfault as soon
        // as anyone asks for `.message()`.
        //
        // Unless it arrived erased already: `try g(n)` inside a
        // `-> i64 or any Error` whose `g` returns one too. Boxing a box asks
        // for the concrete type's name and an interface object hasn't got one, so
        // the vtable came out `.vtable.unknown__Error` and the first
        // `.message()` on it failed to link (#1106). The value is forwarded
        // whole instead — it already carries the right vtable.
        let already_boxed = |err: &MirType| matches!(
            (&err_ty, err),
            (MirType::InterfaceObject { interface_name: have }, MirType::InterfaceObject { interface_name: want })
                if have == want
        );
        let box_interface: Option<String> = match (&handler, &wrap, &self.return_ty()) {
            (None, None, MirType::Result { err, .. }) => match &**err {
                MirType::InterfaceObject { interface_name } if !already_boxed(err) => {
                    Some(interface_name.clone())
                }
                _ => None,
            },
            _ => None,
        };
        let err_val = match &handler {
            Some(frame) => frame.err_val,
            None => self.builder.alloc_temp(err_ty.clone()),
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: err_val,
            rvalue: self.payload_field(result.clone(), &result_ty, &err_ty),
        }));

        if let Some(frame) = handler {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: frame.origin_line,
                rvalue: MirRValue::Field {
                    base: result.clone(),
                    field_index: 1,
                    byte_offset: Some(crate::types::RESULT_ORIGIN_LINE_OFFSET),
                    access: FieldAccess::Sized(crate::types::RESULT_ORIGIN_LINE_BYTES),
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
                    store_size: w.tag_bytes,
                    field: None,
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: wrapped,
                    offset: w.payload_offset,
                    value: MirOperand::Local(err_val),
                    store_size: w.payload_size.unwrap_or(err_store_size),
                    field: None,
                }));
                (wrapped, w.enum_ty.size())
            }
            None => (err_val, err_store_size),
        };

        // ER32: box after any ER31a wrap, so the two compose in the one order
        // that makes sense — the enum names the error, the box erases it.
        let (err_val, err_store_size) = match &box_interface {
            Some(interface_name) => {
                let (boxed, boxed_ty): (MirOperand, MirType) = self.emit_interface_box(
                    MirOperand::Local(err_val),
                    &err_ty,
                    interface_name,
                );
                match boxed {
                    MirOperand::Local(id) => (id, boxed_ty.size()),
                    _ => (err_val, err_store_size),
                }
            }
            None => (err_val, err_store_size),
        };

        // Propagating a `T or A` out of a `T or (A | B)` — the error has to say
        // which member it is, because nothing in its bytes does (#776).
        let (err_val, err_store_size) =
            self.wrap_union_member(err_val, err_store_size, &err_ty);

        // Nothing to propagate into: a `test` block's body is lowered as a
        // void function, and the checker lets `try` through there. There is no
        // Result slot to build, so the error ends the test the way the
        // interpreter already ends it — as a failure naming what happened.
        // Building one anyway returned a value out of a void signature and the
        // Cranelift verifier rejected the whole function (#932).
        //
        // Asking whether this *is* a test body, not whether it returns void:
        // every function without a return annotation returns void, `main`
        // included, so the void test panicked in ordinary helpers with a
        // message about test blocks. Those are a compile error now (E0316), and
        // this arm is only for the one case that isn't.
        if self.in_test_body {
            // T20 asks for the error to be *reported*, not only for the test to
            // end. The value is right here and its type is known, so call its
            // `message()` the same way `r!` does — reachability queued the body
            // and left the name on this node. Without it a failing setup step
            // said only that something had failed, and finding out what meant
            // bisecting the test.
            let text = self
                .ctx
                .call_rewrites
                .get(&try_id)
                .cloned()
                .and_then(|msg_fn| self.emit_error_message_text(&msg_fn, err_val, &err_ty));
            match text {
                Some(text) => {
                    let joined = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(joined),
                        func: FunctionRef::internal("concat".to_string()),
                        args: vec![
                            MirOperand::Constant(MirConst::String(format!(
                                "{TRY_PROPAGATED_NOWHERE}: "
                            ))),
                            MirOperand::Local(text),
                        ],
                    }));
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("panic_str".to_string()),
                        args: vec![MirOperand::Local(joined)],
                    }));
                }
                // No `message()` to call — an optional's `none`, or an error
                // type whose body nothing queued. The bare line still names the
                // test and the line it failed on.
                None => {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("panic".to_string()),
                        args: vec![MirOperand::Constant(MirConst::String(
                            TRY_PROPAGATED_NOWHERE.to_string(),
                        ))],
                    }));
                }
            }
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
            return self.finish_try_ok_path(inner, &result, &result_ty, ok_block, merge_block);
        }

        // An optional-returning function propagates *absence*, and an Option is
        // a tag with nothing beside it — no origin fields, no payload. The
        // Result construction below writes origin at offsets 8 and 16 and a
        // payload at 24, which runs past the end of a 16-byte Option slot; and
        // the type match further down only knows about Result, so the slot got
        // typed after the *source* optional instead of the return. That's how
        // `try raw` in a `-> u16?` built a `string?`-shaped Result and the
        // caller's `??` read a payload of 1 where the default belonged (#608).
        if let none_ty @ MirType::Option(_) = self.return_ty() {
            let ret_none = self.builder.alloc_temp(none_ty.clone());
            self.store_wrapper_tag(ret_none, &none_ty, rask_mono::runtime_abi::RASK_TAG_ABSENT);
            self.terminate_return(Some(MirOperand::Local(ret_none)));
            return self.finish_try_ok_path(inner, &result, &result_ty, ok_block, merge_block);
        }

        // Construct full Result.Err with origin (ER15). The slot has to be the
        // *enclosing* function's return type — this value is what `return`
        // hands back. Using the callee's Result type gave the caller a slot of
        // the wrong shape whenever the two differed, e.g. propagating a
        // `KV? or E` out of a `string or E` function.
        let ret_result_ty = match self.return_ty() {
            ty @ MirType::Result { .. } => ty,
            _ => result_ty.clone(),
        };
        let ret_result = self.builder.alloc_temp(ret_result_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(rask_mono::runtime_abi::RASK_TAG_ABSENT)),
            store_size: crate::types::TAG_BYTES,
            field: None,
        }));
        // Origin (ER15): copy from source Result, then set if not already set.
        // Err(...) construction zeros origin, so first try site wins.
        let src_origin_line = self.builder.alloc_temp(MirType::U32);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: src_origin_line,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 1, // origin_line is the second field (after tag, before payload)
                byte_offset: Some(crate::types::RESULT_ORIGIN_LINE_OFFSET),
                access: FieldAccess::Sized(crate::types::RESULT_ORIGIN_LINE_BYTES),
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
        let origin_line_local = self.builder.alloc_temp(MirType::U32);

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
            store_size: 8,
            field: None,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
            value: MirOperand::Local(origin_line_local),
            store_size: crate::types::RESULT_ORIGIN_LINE_BYTES,
            field: None,
        }));
        // Payload, at the error's own size (a string is 16 bytes).
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: ret_result,
            offset: ret_result_ty.payload_offset(),
            value: MirOperand::Local(err_val),
            store_size: err_store_size,
            field: None,
        }));
        self.terminate_return(Some(MirOperand::Local(ret_result)));

        self.finish_try_ok_path(inner, &result, &result_ty, ok_block, merge_block)
    }

    /// Put a concrete error into the union slot the enclosing function returns.
    ///
    /// A union's members are nominally distinct types with nothing in their bytes
    /// to tell them apart, so which one is held is written alongside them:
    /// `[member:8][member bytes]`. Anything that discriminates — `is Member`,
    /// method dispatch — reads that index.
    ///
    /// A no-op unless the return's err side really is a union and `err_ty` really
    /// is one of its members. A `T or (A | B)` propagated whole into another
    /// `T or (A | B)` is already indexed and passes straight through.
    fn wrap_union_member(
        &mut self,
        err_val: crate::LocalId,
        err_store_size: u32,
        err_ty: &MirType,
    ) -> (crate::LocalId, u32) {
        let MirType::Result { err: ret_err, .. } = self.return_ty() else {
            return (err_val, err_store_size);
        };
        let MirType::Union(members) = ret_err.as_ref() else {
            return (err_val, err_store_size);
        };
        if matches!(err_ty, MirType::Union(_)) {
            return (err_val, err_store_size);
        }
        let union_ty = ret_err.as_ref().clone();
        let Some(index) = self.union_member_index(&union_ty, err_ty) else {
            return (err_val, err_store_size);
        };
        let Some(member) = members.get(index).cloned() else {
            return (err_val, err_store_size);
        };
        let slot = self.builder.alloc_temp(union_ty.clone());
        self.store_wrapper_tag(slot, &union_ty, index as i64);
        self.store_wrapper_payload(slot, &union_ty, MirOperand::Local(err_val), &member);
        (slot, union_ty.size())
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
                            if let Some(mod_name) = object.name() {
                                if super::is_type_constructor_name(mod_name) {
                                    let func_name = format!("{}_{}", mod_name, method);
                                    let ret = self.func_sigs.get(&func_name)
                                        .map(|s| s.ret_ty.clone())
                                        .unwrap_or_else(|| super::stdlib_return_mir_type_in(&func_name, Some(self.ctx)));
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
                                    if let Some(mod_name) = o.name() {
                                        format!("{}_{}", mod_name, f)
                                    } else { break; }
                                }
                                _ => break,
                            };
                            let ret = self.func_sigs.get(&name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type_in(&name, Some(self.ctx)));
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
            .unwrap_or_else(|| crate::fallback::unknown_type("lower/errors:367"));
        let ok_val = self.builder.alloc_temp(ok_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ok_val,
            rvalue: self.payload_field(result.clone(), result_ty, &ok_ty),
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
        let err_ty = match self.return_ty() {
            MirType::Result { err, .. } => *err,
            _ => MirType::I64,
        };
        let err_val = self.builder.alloc_temp(err_ty.clone());
        let origin_line = self.builder.alloc_temp(MirType::U32);
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
        ).unwrap_or_else(|| crate::fallback::unknown_type("lower/errors:fallback_payload"));
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
            rvalue: self.payload_field(result.clone(), &result_ty, &err_ty),
        }));
        let shadowed = if clause.is_discard() {
            None
        } else {
            self.locals.insert(clause.binder.clone(), (err_val, err_ty))
        };
        let (handler_op, handler_ty) = self.lower_expr(&clause.body)?;
        if !clause.is_discard() {
            match shadowed {
                Some(prev) => { self.locals.insert(clause.binder.clone(), prev); }
                None => { self.locals.remove(&clause.binder); }
            }
        }
        // A diverging handler (`catch e => return …`) already terminated.
        if self.builder.current_block_unterminated() {
            // `catch _ => 5` on a shape that stays wrapped hands back a bare
            // payload for a `T?` slot, so the arm is a coercion position like
            // any other.
            let handler_op = self.coerce_into_wrapper(
                rask_ast::coercion::CoercionSite::CatchArm,
                handler_op, &handler_ty, &ok_ty,
            );
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
            rvalue: self.payload_field(result, &result_ty, &payload_ty),
        }));
        // ER14a: when the shape stays, the payload read out of the operand is a
        // bare `T` going into a `T?` slot and gains the layer here. This used to
        // be left to codegen noticing the destination's type, which is the habit
        // that let four positions drift apart (#701).
        let payload_op = self.coerce_into_wrapper(
            rask_ast::coercion::CoercionSite::CatchArm,
            MirOperand::Local(payload), &payload_ty, &ok_ty,
        );
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: MirRValue::Use(payload_op),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: merge_block,
        }));

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(value), ok_ty))
    }
}

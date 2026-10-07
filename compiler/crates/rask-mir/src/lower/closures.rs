// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure and spawn lowering.

use rask_ast::ty::TypeExpr;
use super::{LoweringError, MirLowerer, TypedOperand};
use rask_ast::NodeId;
use crate::{
    stmt::ClosureCapture, BlockBuilder, FunctionRef, LocalId, MirOperand,
    MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::{
    expr::Expr,
    stmt::Stmt,
};

impl<'a> MirLowerer<'a> {
    /// A named function used as a value (`apply(double, 21)`, or the handler
    /// `http.serve` takes). Callers invoke it through
    /// `closure_call`, which passes an environment pointer first, but a
    /// top-level function has no such parameter — so wrap it in one that does
    /// and hand back a closure with an empty environment.
    ///
    /// Without this, lowering treated the bare name as a variable lookup and
    /// gave up: the flagship's `http.serve("0.0.0.0:8080", handle)`
    /// failed native compilation with "unresolved variable `handle`" while the
    /// interpreter ran it.
    ///
    /// Returns `None` if the name isn't a known function, so the caller can
    /// report its own unresolved-variable error.
    pub(super) fn lower_fn_as_value(&mut self, name: &str) -> Option<TypedOperand> {
        let sig = self.func_sigs.get(name)?.clone();
        let ret_ty = sig.ret_ty.clone();
        let param_tys = sig.param_tys.clone();

        // Named per use site, the way closure bodies are. A single global
        // `<name>__fnval` looks tidier but the dedup that would need is
        // per-lowerer, so passing the same function from two places emitted
        // the wrapper twice and Cranelift rejected the duplicate definition.
        let wrapper_name = format!("{}__fnval_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        {
            let mut wb = BlockBuilder::new(wrapper_name.clone(), ret_ty.clone());
            wb.add_param("__env".to_string(), MirType::Ptr);

            let mut args = Vec::new();
            for (i, ty_str) in param_tys.iter().enumerate() {
                // A scalar `mutate` parameter arrives as the caller's address
                // and goes on to the function as one.
                let ty = if sig.scalar_mutate_params.get(i).is_some_and(Option::is_some) {
                    MirType::Ptr
                } else {
                    ty_str
                        .as_ref()
                        .map(|t| self.ctx.resolve_type_expr(t))
                        .unwrap_or_else(|| crate::fallback::unknown_type("lower/closures:fnval_param"))
                };
                let id = wb.add_param(format!("__a{}", i), ty);
                args.push(MirOperand::Local(id));
            }

            let call_dst = if ret_ty == MirType::Void {
                None
            } else {
                Some(wb.alloc_temp(ret_ty.clone()))
            };
            wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: call_dst,
                func: FunctionRef::internal(name.to_string()),
                args,
            }));
            wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: call_dst.map(MirOperand::Local),
            }));

            self.func_sigs.insert(wrapper_name.clone(), super::FuncSig {
                ret_ty,
                scalar_mutate_params: Vec::new(),
                aggregate_mutate_params: Vec::new(),
                ret_vec_elem: None,
                param_tys: Vec::new(),
            });
            self.synthesized_functions.push(wb.finish());
        }

        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: wrapper_name,
            captures: Vec::new(),
            heap: false,
            task_bound: false,
        }));
        Some((MirOperand::Local(result_local), MirType::Ptr))
    }

    /// Wrap a type's `compare` into a comparator closure for `sort_by`.
    ///
    /// `sort()` is defined as `T: Comparable` (std.collections/SO3), so an
    /// element type that has a `compare` has to be sorted by it. This is the
    /// bridge: the sort runtime takes a closure, `compare` is a plain function,
    /// and the two disagree about the answer's shape — `compare` produces an
    /// `Ordering` while the C comparator ABI reads an integer. Same conversion
    /// the closure path does, for the same reason.
    ///
    /// Returns `None` when the type has no `compare`, or has one that doesn't
    /// answer with an `Ordering`, leaving the caller on the byte-comparing
    /// default. Bailing out sorts by the wrong key; guessing at an unknown
    /// return shape reads a tag out of whatever it is, which crashes.
    pub(super) fn lower_compare_as_comparator(&mut self, name: &str) -> Option<TypedOperand> {
        let sig = self.func_sigs.get(name)?;
        let param_tys = sig.param_tys.clone();
        let ret_ty = sig.ret_ty.clone();
        if param_tys.len() != 2 || !self.is_ordering_ty(&ret_ty) {
            return None;
        }

        let wrapper_name = format!("{}__cmpval_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        {
            let mut wb = BlockBuilder::new(wrapper_name.clone(), MirType::I64);
            wb.add_param("__env".to_string(), MirType::Ptr);

            let mut args = Vec::new();
            for (i, ty_str) in param_tys.iter().enumerate() {
                let ty = ty_str
                    .as_ref()
                    .map(|t| self.ctx.resolve_type_expr(t))
                    .unwrap_or_else(|| crate::fallback::unknown_type("lower/closures:cmp_param"));
                let id = wb.add_param(format!("__c{}", i), ty);
                args.push(MirOperand::Local(id));
            }

            let result = wb.alloc_temp(ret_ty.clone());
            wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(result),
                func: FunctionRef::internal(name.to_string()),
                args,
            }));

            // `compare` answers with an `Ordering`; the C comparator ABI reads
            // an integer. Hand over the tag — Less 0, Equal 1, Greater 2 — and
            // the adapter's `tag - 1` turns it back into a sign.
            let saved = std::mem::replace(&mut self.builder, wb);
            let normalized = self.emit_ordering_tag_i64(MirOperand::Local(result));
            let mut wb = std::mem::replace(&mut self.builder, saved);
            wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: Some(MirOperand::Local(normalized)),
            }));

            self.func_sigs.insert(wrapper_name.clone(), super::FuncSig {
                ret_ty: MirType::I64,
                scalar_mutate_params: Vec::new(),
                aggregate_mutate_params: Vec::new(),
                ret_vec_elem: None,
                param_tys: Vec::new(),
            });
            self.synthesized_functions.push(wb.finish());
        }

        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: wrapper_name,
            captures: Vec::new(),
            heap: false,
            task_bound: false,
        }));
        Some((MirOperand::Local(result_local), MirType::Ptr))
    }

    /// The `hash` and `eq` a map built at `node` calls on its keys, as the two
    /// function addresses the keyed constructor takes — `None` when the key's
    /// bytes are its identity and the runtime hashes it itself.
    ///
    /// Monomorphization chose the functions and queued them (#1391); this only
    /// adapts them to the runtime's callback shape. The runtime hands a key's
    /// *address* and its size, and wants `int` back from `eq`. A struct, enum,
    /// tuple or wrapper is passed by address anyway, so its address is the
    /// argument; a `Vec` key is a handle, loaded out of the slot.
    pub(super) fn map_key_fn_addrs(
        &mut self,
        node: NodeId,
        key_ty: &MirType,
    ) -> Option<(MirOperand, MirOperand)> {
        let fns = self.ctx.map_key_fns.get(&node)?.clone();
        let hash = self.key_callback(&fns.hash, key_ty, false);
        let eq = self.key_callback(&fns.eq, key_ty, true);
        let addr = |this: &mut Self, name: String| {
            let local = this.builder.alloc_temp(MirType::I64);
            this.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: local,
                rvalue: MirRValue::FuncAddr(name),
            }));
            MirOperand::Local(local)
        };
        Some((addr(self, hash), addr(self, eq)))
    }

    /// `uint64_t (*)(const void *key, int64_t size)` around `target(key)`, or
    /// `int (*)(const void *a, const void *b, int64_t size)` around
    /// `target(a, b)` when `is_eq`.
    fn key_callback(&mut self, target: &str, key_ty: &MirType, is_eq: bool) -> String {
        let name = format!(
            "{}__key_{}_{}",
            self.parent_name,
            if is_eq { "eq" } else { "hash" },
            self.closure_counter
        );
        self.closure_counter += 1;
        // Everything codegen passes by address: the slot holds the value, so
        // the slot's address is the argument.
        let by_address = matches!(
            key_ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Option(_) | MirType::Result { .. }
        );
        let ret_ty = if is_eq { MirType::I32 } else { MirType::U64 };
        let mut wb = BlockBuilder::new(name.clone(), ret_ty.clone());
        let mut args = Vec::new();
        for i in 0..(if is_eq { 2 } else { 1 }) {
            if by_address {
                args.push(MirOperand::Local(wb.add_param(format!("__k{i}"), key_ty.clone())));
            } else {
                let slot = wb.add_param(format!("__k{i}"), MirType::Ptr);
                let key = wb.alloc_temp(key_ty.clone());
                wb.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: key,
                    rvalue: MirRValue::Deref(MirOperand::Local(slot)),
                }));
                args.push(MirOperand::Local(key));
            }
        }
        wb.add_param("__size".to_string(), MirType::I64);
        let answer = wb.alloc_temp(if is_eq { MirType::Bool } else { MirType::U64 });
        wb.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(answer),
            func: FunctionRef::internal(target.to_string()),
            args,
        }));
        let value = if is_eq {
            let widened = wb.alloc_temp(MirType::I32);
            wb.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: widened,
                rvalue: MirRValue::Cast { value: MirOperand::Local(answer), target_ty: MirType::I32 },
            }));
            widened
        } else {
            answer
        };
        wb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Local(value)),
        }));
        self.func_sigs.insert(name.clone(), super::FuncSig {
            ret_ty,
            scalar_mutate_params: Vec::new(),
            aggregate_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_tys: Vec::new(),
        });
        self.synthesized_functions.push(wb.finish());
        name
    }

    /// Closure lowering: synthesize a separate MIR function for the body,
    /// build the environment, and emit ClosureCreate in the enclosing function.
    ///
    /// `carries` mirrors `mem.closures/CM1`: a closure that outlives its frame
    /// carries its captures and starts heap-allocated; one that stays puts its
    /// environment on the stack. `closure_carries` reads the answer off the
    /// ownership pass.
    /// `closure_id` is the closure expression's own node, which carries the
    /// checker's `Fn` type — the return type an unannotated closure would
    /// otherwise have to guess.
    pub(super) fn lower_closure(
        &mut self,
        params: &[rask_ast::expr::ClosureParam],
        ret_ty: Option<&TypeExpr>,
        body: &Expr,
        carries: bool,
        closure_id: Option<NodeId>,
    ) -> Result<TypedOperand, LoweringError> {
        self.lower_closure_expecting(params, ret_ty, body, carries, &[], closure_id, false)
    }

    /// CM1: whether this closure outlives the frame that built it, as the
    /// ownership pass worked out.
    pub(super) fn closure_carries(&self, closure_id: Option<NodeId>) -> bool {
        closure_id.is_some_and(|id| self.ctx.escaping_closures.contains(&id))
    }

    /// As `lower_closure`, with the parameter types the callee declares for this
    /// argument position. An unannotated closure parameter takes its type from
    /// there — otherwise it defaults to i64 and field access and method dispatch
    /// inside the body run against the wrong type (`|req| req.method` on a
    /// `func(Request) -> Response` parameter read a pointer instead of the tag).
    pub(super) fn lower_closure_expecting(
        &mut self,
        params: &[rask_ast::expr::ClosureParam],
        ret_ty: Option<&TypeExpr>,
        body: &Expr,
        carries: bool,
        expected_param_tys: &[TypeExpr],
        closure_id: Option<NodeId>,
        for_spawn: bool,
    ) -> Result<TypedOperand, LoweringError> {
        // 1. Collect free variables (captures from enclosing scope)
        let free_vars = self.collect_free_vars(body, params);

        // 2. Generate unique name for the closure function
        let closure_name = format!("{}__closure_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        // 3. Build the closure environment layout.
        //
        // A closure that stays in its frame borrows what it captures
        // (mem.closures/MC1): the env holds each variable's address and the
        // body reads and writes through it, so `let bump = || { n = n + 1 }`
        // bumps the caller's `n`. Copying instead is what lost every such write
        // on native (#1038).
        //
        // One that outlives its frame carries the values instead — pointing
        // into a dead frame is exactly what it must not do. Which one a closure
        // is comes from the ownership pass (`escaping_closures`, CM1), not from
        // a word at the literal. Same split the interpreter draws between
        // `capture_shared` and `capture_snapshot`.
        //
        // Borrowing is safe here because a borrowing closure is never heap
        // allocated (`heap: carries`, step 5) and so cannot outlive the frame
        // it points into.
        let by_ref = !carries && !for_spawn;
        // The environment slot holds a pointer for a borrow, and the value
        // itself otherwise — but for a carrying closure the value in the slot
        // *is* the variable, so the body has to write back into it rather than
        // into a loaded copy. A task's copy is its own and dies with it, so
        // there is nothing to write back to.
        let capture_access = if for_spawn {
            crate::CaptureAccess::Value
        } else if carries {
            crate::CaptureAccess::Owned
        } else {
            crate::CaptureAccess::Borrowed
        };
        let mut captures = Vec::new();
        let mut env_offset = 0u32;
        for (_name, local_id, ty, copy) in &free_vars {
            // An address is a word regardless of what it points at.
            let size = if by_ref { 8 } else { ty.size() };
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
                by_ref,
                copy: *copy,
            });
            env_offset = aligned_offset + size;
        }

        // 4. Synthesize a MIR function for the closure body.
        //
        // Prefer the written annotation, then what the checker inferred for the
        // closure, and only then guess. Guessing means i64, and i64 is only right
        // for a payload that already fits a machine word: an unannotated
        // `|| { return captured }` over a bool printed 1, over a char 120, over a
        // string or a struct its address. The checker had the type all along —
        // this just asks it.
        let inferred_void = ret_ty.is_none() && Self::body_has_bare_return(body);
        // Void counts as an answer. Filtering it out sent `|| { }` — nothing to
        // return, and the checker says so — down to the guess instead.
        let checked_ret = closure_id
            .and_then(|id| self.ctx.lookup_raw_type(id))
            .and_then(|ty| match ty {
                rask_types::Type::Fn { ret, .. } => Some(self.ctx.type_to_mir(ret.as_ref())),
                _ => None,
            });
        let closure_ret = ret_ty
            .map(|t| self.ctx.resolve_type_expr(t))
            .or(checked_ret)
            .unwrap_or_else(|| if inferred_void {
                MirType::Void
            } else {
                crate::fallback::unknown_type("lower/closures:closure_ret")
            });
        // A comparator closure hands its answer to C code that reads a plain
        // integer — `rask_vec_sort_by`'s adapter tests the return against zero.
        // Returning an aggregate would return its address instead (#729).
        let returns_ordering = self.is_ordering_ty(&closure_ret);
        let closure_ret = if returns_ordering { MirType::I64 } else { closure_ret };
        let mut closure_builder = BlockBuilder::new(closure_name.clone(), closure_ret.clone());

        let env_param_id = closure_builder.add_param("__env".to_string(), MirType::Ptr);

        // The checker's parameter list for this closure, for a parameter that
        // was neither annotated nor pinned by the callee's signature.
        let checked_params: Vec<MirType> = closure_id
            .and_then(|id| self.ctx.lookup_raw_type(id))
            .and_then(|ty| match ty {
                rask_types::Type::Fn { params, .. } => Some(
                    params.iter().map(|p| self.ctx.type_to_mir(&p.ty)).collect()
                ),
                _ => None,
            })
            .unwrap_or_default();

        // The parameters' metadata is the closure's own. One flat table holds
        // every name, so an outer binding the parameter shadows gets its own
        // back once the body is lowered.
        let outer_param_names = self.save_names(params.iter().map(|p| p.name.as_str()).collect());
        let mut closure_locals = std::collections::HashMap::new();
        for (i, param) in params.iter().enumerate() {
            // Written annotation first, then the type the callee declares for
            // this position, then what the checker inferred.
            let written = param.ty.clone()
                .or_else(|| expected_param_tys.get(i).cloned());
            let param_ty = written.as_ref()
                .map(|t| self.ctx.resolve_type_expr(t))
                .or_else(|| checked_params.get(i).cloned())
                .unwrap_or_else(|| crate::fallback::unknown_type("lower/closures:param"));
            // A `mutate` parameter takes the caller's address the way a
            // declared function's does, so one function type means one calling
            // convention whichever kind of function is behind the value.
            let mode = rask_ast::ty::ParamMode::from_flags(param.is_take, param.is_mutate);
            let (scalar_mutate, _) = super::mutate_param_passing(mode, &param.name, &param_ty);
            let local_ty = if scalar_mutate.is_some() { MirType::Ptr } else { param_ty.clone() };
            let param_id = closure_builder.add_param(param.name.clone(), local_ty.clone());
            closure_locals.insert(param.name.clone(), (param_id, local_ty));
            if param.is_mutate {
                self.meta_mut(&param.name).assigns_through = true;
            }
            self.meta_mut(&param.name).scalar_through_ptr = scalar_mutate;
            if let Some(prefix) = self.mir_type_name(&param_ty) {
                self.meta_mut(&param.name).type_prefix = Some(prefix);
            } else if let Some(t) = written.as_ref() {
                if let Some(prefix) = super::type_prefix_of(t) {
                    self.meta_mut(&param.name).type_prefix = Some(prefix);
                }
            }
        }

        // Emit LoadCapture for each free variable
        let mut addressed_captures = std::collections::HashSet::new();
        for (i, (name, _outer_id, ty, _)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            if capture_access.is_addressed() {
                addressed_captures.insert(local_id);
            }
            closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
                access: capture_access,
            }));
            closure_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        // Lower the closure body using a temporary lowerer
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_captures = std::mem::replace(&mut self.addressed_captures, addressed_captures);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);
            // The cleanup chain belongs to the enclosing function, and its
            // blocks live in that function's MIR. A `return` inside this body
            // would drain it here, branching to block ids this function does
            // not have — Cranelift reports it as `invalid block reference`.
            let saved_ensure_stack = std::mem::take(&mut self.ensure_stack);

            // The tag read has to be emitted while the closure's own builder is
            // still installed, so it lands inside the closure body. An explicit
            // `return` in the body is converted by `terminate_return` instead.
            let body_result = self.lower_expr(body).map(|(op, ty)| {
                if self.is_ordering_ty(&ty) && closure_ret == MirType::I64 {
                    let tag = self.emit_ordering_tag_i64(op);
                    (MirOperand::Local(tag), MirType::I64)
                } else {
                    (op, ty)
                }
            });

            closure_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.addressed_captures = saved_captures;
            self.loop_stack = saved_loop_stack;
            self.ensure_stack = saved_ensure_stack;
            self.restore_names(outer_param_names);

            let (body_val, _body_ty) = body_result?;

            if closure_builder.current_block_unterminated() {
                let ret_value = if closure_ret == MirType::Void { None } else { Some(body_val) };
                closure_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                    value: ret_value,
                }));
            }
        }

        let closure_fn = closure_builder.finish();

        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: closure_ret.clone(),
            scalar_mutate_params: Vec::new(),
            aggregate_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_tys: Vec::new(),
        });

        self.synthesized_functions.push(closure_fn);

        // A spawned closure whose result doesn't fit the runtime's one-word
        // result slot gets a thunk in front of it: same environment, calls the
        // real closure, puts the answer on the heap and hands back its address.
        // The join side copies through that address.
        //
        // Without this the runtime called the closure through
        // `int64_t (*)(void *)` and kept whatever was in the integer return
        // register — a stale pointer for a task returning `2.5f64`, which read
        // back as 3.3e-310, and nothing at all for a string.
        let boxes_result = for_spawn && crate::types::spawn_payload_is_boxed(&closure_ret);
        let entry_name = if boxes_result {
            self.synthesize_spawn_box_thunk(&closure_name, &closure_ret)
        } else {
            closure_name
        };
        // Handed straight to the spawn call being lowered, which is the next
        // thing that happens. The runtime has to know whether the word the task
        // hands back is a box it must free — see `GreenTask::result_owned` — and
        // this is the only place that knows, because it's the place that decided.
        if for_spawn {
            self.spawn_result_boxed = boxes_result;
        }

        // 5. In the parent function, emit ClosureCreate.
        // Own closures may escape — start heap-allocated so escape analysis can
        // decide whether to downgrade. Scope-limited closures never escape; stack only.
        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: entry_name,
            captures,
            heap: carries,
            task_bound: closure_id.is_some_and(|id| self.ctx.task_bound_closures.contains(&id)),
        }));

        Ok((MirOperand::Local(result_local), MirType::Ptr))
    }



    /// The element type of a `Sequence<T>` / `SequenceMut<T>` iterable, or
    /// `None` for anything else.
    ///
    /// The checker's own type is the authority: `Sequence` is nominal (SEQ1), so
    /// a closure that fills the slot arrives already unified with the sequence
    /// shape and reads back as `Sequence<T>` rather than as its function type.
    pub(super) fn sequence_elem_ty(&self, iter_expr: &Expr) -> Option<MirType> {
        let ty = self.ctx.lookup_raw_type(iter_expr.id)?;
        // `type_prefix` is the shared answer to "what is this type called", and
        // it knows about stdlib types the local name table doesn't carry.
        let name = super::MirContext::type_prefix(ty, self.ctx.type_names)?;
        if name != "Sequence" && name != "SequenceMut" {
            return None;
        }
        let args = match ty {
            rask_types::Type::Generic { args, .. }
            | rask_types::Type::UnresolvedGeneric { args, .. } => args,
            _ => return None,
        };
        match args.first()? {
            rask_types::GenericArg::Type(t) => Some(self.ctx.type_to_mir(t)),
            rask_types::GenericArg::ConstUsize(_) => None,
        }
    }

    /// `for x in seq { … }` over a `Sequence<T>` (type.sequence/SEQ6).
    ///
    /// Iterating a sequence is *calling* it. The loop synthesizes a yield
    /// closure whose body is the loop body, hands it to the sequence, and waits;
    /// the sequence walks itself in its own frame, so nothing has to be stored
    /// between items (SEQ38).
    ///
    /// The control-flow translation (SEQ7, SEQ8) falls out of block structure
    /// rather than needing new machinery:
    ///
    /// - fall off the end → `return true`
    /// - `break` → the loop's exit block, which is `return false`
    /// - `continue` → the loop's continue block, which is `return true`
    /// - `return v` → write `v` and a flag through captures the enclosing frame
    ///   owns, then `return false`; the frame tests the flag after the call
    ///
    /// The last one is why SEQ8 records the answer beside the loop instead of
    /// unwinding: unwinding would cross adapter frames that would each have to
    /// know to pass it on, and `SEQ13a` is already load-bearing enough.
    ///
    /// Writing through a capture is exactly what #1038 unblocked, and so is the
    /// ordinary accumulating body — `for x in seq { total = total + x }` reaches
    /// `total` because the yield closure borrows it.
    pub(super) fn lower_for_sequence(
        &mut self,
        label: Option<&str>,
        binding: &rask_ast::stmt::ForBinding,
        iter_expr: &Expr,
        body: &[Stmt],
        elem_ty: MirType,
    ) -> Result<(), LoweringError> {
        use rask_ast::stmt::ForBinding;

        let (seq_op, _) = self.lower_expr(iter_expr)?;
        let seq_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: seq_local,
            rvalue: MirRValue::Use(seq_op),
        }));

        // Where a `return` inside the body leaves its answer. The flag is
        // separate from the value because the value's type is the enclosing
        // function's return type, and `void` has no value to test.
        let outer_ret = self.builder.ret_ty().clone();
        let ret_flag = self.builder.alloc_local(
            format!("__seq_ret_flag_{}", self.closure_counter), MirType::I64,
        );
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: ret_flag,
            rvalue: MirRValue::Use(MirOperand::Constant(crate::operand::MirConst::Int(0))),
        }));
        let ret_value = if outer_ret == MirType::Void {
            None
        } else {
            Some(self.builder.alloc_local(
                format!("__seq_ret_value_{}", self.closure_counter), outer_ret.clone(),
            ))
        };

        let closure_name = format!("{}__yield_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        // The yield takes one parameter: the item. A tuple binding unpacks it
        // inside the body, so the parameter needs a name of its own — using the
        // first of the tuple's names would shadow it, and `for (k, v) in seq`
        // then had no `v` at all.
        let names: Vec<String> = match binding {
            ForBinding::Single(name) => vec![name.clone()],
            ForBinding::Tuple(names) => names.clone(),
        };
        let param_name = match binding {
            ForBinding::Single(name) => name.clone(),
            ForBinding::Tuple(_) => format!("__seq_item_{}", self.closure_counter),
        };
        // The body's free variables, minus whatever the binding introduces.
        let mut free_vars: Vec<(String, LocalId, MirType, bool)> = self
            .collect_free_vars_block(body)
            .into_iter()
            .filter(|(name, _, _, _)| !names.contains(name))
            .collect();
        // The two the loop just made are captured like any other local, so a
        // `return` in the body writes the enclosing frame's storage.
        free_vars.push((format!("__flag_{closure_name}"), ret_flag, MirType::I64, true));
        if let Some(v) = ret_value {
            free_vars.push((format!("__value_{closure_name}"), v, outer_ret.clone(), false));
        }

        let mut captures = Vec::new();
        let mut env_offset = 0u32;
        for _ in &free_vars {
            // A scope-limited closure borrows (mem.closures/MC1), and an address
            // is a word. The sequence only calls it, so it never outlives this
            // frame — see `transform::addr_taken` for why that matters.
            captures.push(crate::stmt::ClosureCapture {
                local_id: LocalId(0), // filled in below
                offset: env_offset,
                size: 8,
                by_ref: true,
                copy: false, // filled in below
            });
            env_offset += 8;
        }
        for (cap, (_, id, _, copy)) in captures.iter_mut().zip(free_vars.iter()) {
            cap.local_id = *id;
            cap.copy = *copy;
        }

        let mut yb = BlockBuilder::new(closure_name.clone(), MirType::Bool);
        let env_param = yb.add_param("__env".to_string(), MirType::Ptr);
        let item_param = yb.add_param(param_name.clone(), elem_ty.clone());

        let mut yield_locals = std::collections::HashMap::new();
        if matches!(binding, ForBinding::Single(_)) {
            yield_locals.insert(param_name.clone(), (item_param, elem_ty.clone()));
        }

        let mut inner_flag = None;
        let mut inner_value = None;
        for (i, (name, outer_id, ty, _)) in free_vars.iter().enumerate() {
            let dst = yb.alloc_local(name.clone(), ty.clone());
            yb.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst,
                env_ptr: env_param,
                offset: captures[i].offset,
                access: crate::CaptureAccess::Borrowed,
            }));
            if *outer_id == ret_flag {
                inner_flag = Some(dst);
            } else if Some(*outer_id) == ret_value {
                inner_value = Some(dst);
            } else {
                yield_locals.insert(name.clone(), (dst, ty.clone()));
            }
        }

        // Three exits, each a block that returns the right answer.
        let continue_block = yb.create_block();
        let exit_block = yb.create_block();
        let nonlocal_block = yb.create_block();

        let mut body_result: Result<(), LoweringError> = Ok(());
        {
            let saved_builder = std::mem::replace(&mut self.builder, yb);
            let saved_locals = std::mem::replace(&mut self.locals, yield_locals);
            let saved_loops = std::mem::take(&mut self.loop_stack);
            // The cleanup chain belongs to the enclosing function, and its
            // blocks live in that function's MIR. A `return` inside this body
            // would drain it here, branching to block ids this function does
            // not have — Cranelift reports it as `invalid block reference`.
            let saved_ensure_stack = std::mem::take(&mut self.ensure_stack);
            let saved_inline = self.inline_return_target.take();
            // Element write-backs are the enclosing function's for the same reason.
            let saved_write_backs = std::mem::take(&mut self.pending_write_backs);

            // `break` and `continue` in the body are this loop's, and this loop
            // is one call of the yield.
            self.loop_stack.push(super::LoopContext {
                label: label.map(|s| s.to_string()),
                continue_block,
                exit_block,
                result_local: None,
                ensure_depth: self.ensure_stack.len(),
                writeback_depth: self.pending_write_backs.len(),
            });
            // `return v` assigns to the captured value local and jumps to the
            // block that raises the flag. Writing the local *is* a store through
            // the capture pointer — `transform::addr_taken` rewrites it.
            self.inline_return_target = inner_value
                .or(inner_flag)
                .map(|dst| (dst, nonlocal_block, 0, Some(outer_ret.clone())));

            // `for (k, v) in seq` — read the names off the item, the same way
            // the index loop reads them off a Map entry. Has to happen with the
            // closure's builder and locals installed, so the reads land in the
            // yield body and the names resolve there.
            if let ForBinding::Tuple(tuple_names) = binding {
                let pats: Vec<rask_ast::stmt::TuplePat> = tuple_names
                    .iter()
                    .map(|n| rask_ast::stmt::TuplePat::Name(n.clone()))
                    .collect();
                if let Err(e) = self.destructure_tuple_pattern(
                    &pats, &MirOperand::Local(item_param), &elem_ty, None,
                ) {
                    body_result = Err(e);
                }
            }

            for stmt in body {
                if let Err(e) = self.lower_stmt(stmt) {
                    body_result = Err(e);
                    break;
                }
            }
            // Fell off the end of the body: another item, please (SEQ3).
            if self.builder.current_block_unterminated() {
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: continue_block,
                }));
            }

            self.loop_stack = saved_loops;
            self.ensure_stack = saved_ensure_stack;
            self.pending_write_backs = saved_write_backs;
            self.inline_return_target = saved_inline;
            yb = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
        }
        body_result?;

        // Whether any `return` in the body compiled to a jump into the
        // non-local exit. Read off what lowering emitted rather than scanned for
        // in the source: `try`, `??` and a `return` inside a nested `match` arm
        // all reach it, and a list of the constructs that do would go stale.
        let body_returns = yb.any_jump_to(nonlocal_block);

        yb.switch_to_block(continue_block);
        yb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Constant(crate::operand::MirConst::Int(1))),
        }));
        yb.switch_to_block(exit_block);
        yb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Constant(crate::operand::MirConst::Int(0))),
        }));
        yb.switch_to_block(nonlocal_block);
        if let Some(flag) = inner_flag {
            yb.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: flag,
                rvalue: MirRValue::Use(MirOperand::Constant(crate::operand::MirConst::Int(1))),
            }));
        }
        yb.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Constant(crate::operand::MirConst::Int(0))),
        }));

        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: MirType::Bool,
            scalar_mutate_params: Vec::new(),
            aggregate_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_tys: Vec::new(),
        });
        self.synthesized_functions.push(yb.finish());

        // Build the closure and hand it to the sequence.
        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: closure_local,
            func_name: closure_name,
            captures,
            heap: false,
            task_bound: false,
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
            dst: None,
            closure: seq_local,
            args: vec![MirOperand::Local(closure_local)],
        }));

        // SEQ8: the body asked to leave the enclosing function. Test the flag
        // the yield raised and do it here, where `return` means what it says.
        //
        // Only when the body actually asked. Most loop bodies don't return, and
        // emitting the test anyway left the frame with a second `Return` — of a
        // slot nothing ever wrote — that no path could reach. It cost every
        // sequence terminal its drop: `functions_that_hand_a_container_back`
        // needs *every* returning path to hand back a container this frame made,
        // and that one handed back an unwritten slot, so `to_vec` and `to_map`
        // were freed by nobody (#1060).
        if !body_returns {
            return Ok(());
        }
        let taken = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(ret_flag),
            then_block: taken,
            else_block: done,
        }));
        self.builder.switch_to_block(taken);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: ret_value.map(MirOperand::Local),
        }));
        self.builder.switch_to_block(done);
        Ok(())
    }

    /// `spawn_with(arg, f)`, or a `Thread`/`ThreadPool` twin: a task that runs
    /// `f(arg)` once. `target` is the plain spawn form the task goes through
    /// (`spawn`, `Thread_spawn`, `ThreadPool_spawn`).
    ///
    /// Built the way the closure `|| f(arg)` would be, by hand, because that
    /// closure consumes its capture and the language refuses it
    /// (`mem.closures/CM4`). Here it runs once by construction: the runtime
    /// calls a spawned body exactly once. `arg` goes into a heap block, the
    /// way `Heap(arg)` boxes it, so the task's environment holds an address
    /// and the value leaves it exactly once, as the call's `take` argument.
    pub(super) fn lower_spawn_with(
        &mut self,
        call: &Expr,
        args: &[rask_ast::expr::CallArg],
        target: &str,
    ) -> Result<TypedOperand, LoweringError> {
        let [arg, f] = args else {
            return Err(LoweringError::InvalidConstruct(
                "spawn_with takes the value to hand over and the task's body".to_string(),
            ));
        };
        let (arg_op, arg_ty) = self.lower_expr(&arg.expr)?;
        // Which container a bare pointer is decides its free, and only the
        // checker's type says.
        let arg_ty = match (&arg_ty, self.ctx.lookup_raw_type(arg.expr.id)) {
            (MirType::Ptr, Some(t)) => self.ctx.payload_to_mir(t),
            _ => arg_ty,
        };
        let (f_op, _) = match &f.expr.kind {
            rask_ast::expr::ExprKind::Closure { params, ret_ty, body } => self.lower_closure_expecting(
                params, ret_ty.as_ref(), body, true, &[], Some(f.expr.id), false,
            )?,
            _ => self.lower_expr(&f.expr)?,
        };
        let ret = self
            .ctx
            .lookup_raw_type(f.expr.id)
            .and_then(|t| self.ctx.callable_ret_ty(t, self.ctx.type_names))
            .unwrap_or_else(|| crate::fallback::unknown_type("lower/closures:spawn_with_ret"));
        let f_local = self.as_local(f_op);
        // A body that holds a link or a `Local` box may not cross, whichever
        // way it got here. The task's own closure is checked when it is
        // adopted; this one sits inside it.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("rask_closure_refuse_crossing".to_string()),
            args: vec![MirOperand::Local(f_local)],
        }));
        let block = self.box_into_owned(arg_op, &arg_ty);
        let block_local = self.as_local(block);
        let block_ty = MirType::Heap(Box::new(arg_ty.clone()));

        let boxes_result = crate::types::spawn_payload_is_boxed(&ret);
        let thunk_name = format!("{}__spawn_with_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        let thunk_ret = if boxes_result { MirType::I64 } else { ret.clone() };
        let mut b = BlockBuilder::new(thunk_name.clone(), thunk_ret.clone());
        let env = b.add_param("__env".to_string(), MirType::Ptr);
        let body_fn = b.alloc_local("__f".to_string(), MirType::Ptr);
        b.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
            dst: body_fn,
            env_ptr: env,
            offset: 0,
            access: crate::CaptureAccess::Value,
        }));
        let held = b.alloc_local("__handed".to_string(), block_ty);
        b.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
            dst: held,
            env_ptr: env,
            offset: 8,
            access: crate::CaptureAccess::Value,
        }));
        // Taken out of the block the way a field is moved out of its slot
        // (`Field_take`): the thunk owns what it read, so it answers for it
        // like any caller of a `take` parameter — freed after the call unless
        // the body kept it.
        let handed = b.alloc_local("__arg".to_string(), arg_ty.clone());
        b.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(handed),
            func: FunctionRef::internal("Field_take".to_string()),
            args: vec![
                MirOperand::Local(held),
                MirOperand::Constant(crate::operand::MirConst::Int(arg_ty.size() as i64)),
            ],
        }));
        b.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("rask_free".to_string()),
            args: vec![MirOperand::Local(held)],
        }));
        let handed = MirOperand::Local(handed);
        let result = (ret != MirType::Void).then(|| b.alloc_local("__value".to_string(), ret.clone()));
        b.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
            dst: result,
            closure: body_fn,
            args: vec![handed],
        }));
        let returned = match result {
            Some(value) if boxes_result => {
                let boxed = b.alloc_local("__boxed".to_string(), MirType::Ptr);
                b.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(boxed),
                    func: FunctionRef::internal("rask_alloc".to_string()),
                    args: vec![MirOperand::Constant(crate::operand::MirConst::Int(ret.size().max(8) as i64))],
                }));
                b.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: boxed,
                    offset: 0,
                    value: MirOperand::Local(value),
                    store_size: Some(ret.size()),
                }));
                Some(MirOperand::Local(boxed))
            }
            other => other.map(MirOperand::Local),
        };
        b.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: returned }));
        self.func_sigs.insert(thunk_name.clone(), super::FuncSig {
            ret_ty: thunk_ret,
            scalar_mutate_params: Vec::new(),
            aggregate_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_tys: Vec::new(),
        });
        self.synthesized_functions.push(b.finish());

        let task = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: task,
            func_name: thunk_name,
            captures: vec![
                ClosureCapture { local_id: f_local, offset: 0, size: 8, by_ref: false, copy: false },
                ClosureCapture { local_id: block_local, offset: 8, size: 8, by_ref: false, copy: false },
            ],
            heap: true,
            task_bound: self.ctx.task_bound_closures.contains(&call.id),
        }));
        let handle_ty = self
            .func_sigs
            .get(target)
            .map(|s| s.ret_ty.clone())
            .unwrap_or(MirType::Ptr);
        let handle = self.builder.alloc_temp(handle_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(handle),
            func: FunctionRef::internal(target.to_string()),
            args: vec![
                MirOperand::Local(task),
                MirOperand::Constant(crate::operand::MirConst::Int(i64::from(boxes_result))),
            ],
        }));
        Ok((MirOperand::Local(handle), handle_ty))
    }

    /// Build the one-word entry point for a spawned closure whose result is
    /// wider than a word, and return its name.
    ///
    /// Same environment pointer, forwarded untouched — the captures the caller
    /// already built are still the ones the real closure reads.
    fn synthesize_spawn_box_thunk(&mut self, closure_name: &str, ret: &MirType) -> String {
        let thunk_name = format!("{closure_name}__spawn_box");
        let mut b = BlockBuilder::new(thunk_name.clone(), MirType::I64);
        let env = b.add_param("__env".to_string(), MirType::Ptr);

        let value = b.alloc_local("__value".to_string(), ret.clone());
        b.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(value),
            func: FunctionRef::internal(closure_name.to_string()),
            args: vec![MirOperand::Local(env)],
        }));

        let size = ret.size().max(8) as i64;
        let boxed = b.alloc_local("__boxed".to_string(), MirType::Ptr);
        b.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(boxed),
            func: FunctionRef::internal("rask_alloc".to_string()),
            args: vec![MirOperand::Constant(crate::operand::MirConst::Int(size))],
        }));
        b.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: boxed,
            offset: 0,
            value: MirOperand::Local(value),
            store_size: Some(ret.size()),
        }));
        b.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Local(boxed)),
        }));

        self.func_sigs.insert(thunk_name.clone(), super::FuncSig {
            ret_ty: MirType::I64,
            scalar_mutate_params: Vec::new(),
            aggregate_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_tys: Vec::new(),
        });
        self.synthesized_functions.push(b.finish());
        thunk_name
    }

    /// Collect free variables from a block of statements (no params to bind).
    pub(super) fn collect_free_vars_block(
        &self,
        body: &[Stmt],
    ) -> Vec<(String, LocalId, MirType, bool)> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let bound = std::collections::HashSet::new();
        self.walk_free_vars_block(body, &bound, &mut seen, &mut free);
        free
    }

    /// Check if a closure body contains bare return statements (return without value).
    fn body_has_bare_return(expr: &rask_ast::expr::Expr) -> bool {
        use rask_ast::expr::ExprKind;
        match &expr.kind {
            ExprKind::Block(stmts) => stmts.iter().any(|s| Self::stmt_has_bare_return(s)),
            ExprKind::If { then_branch, else_branch, .. }
            | ExprKind::IfLet { then_branch, else_branch, .. } => {
                Self::body_has_bare_return(then_branch)
                || else_branch.as_ref().map_or(false, |e| Self::body_has_bare_return(e))
            }
            _ => false,
        }
    }

    fn stmt_has_bare_return(stmt: &rask_ast::stmt::Stmt) -> bool {
        use rask_ast::stmt::StmtKind;
        match &stmt.kind {
            StmtKind::Return(None) => true,
            StmtKind::Expr(e) => Self::body_has_bare_return(e),
            _ => false,
        }
    }
}

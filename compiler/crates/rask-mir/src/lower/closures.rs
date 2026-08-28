// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure and spawn lowering.

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
        let sig = self.func_sigs.get(name)?;
        let ret_ty = sig.ret_ty.clone();
        let param_ty_strs = sig.param_ty_strs.clone();

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
            for (i, ty_str) in param_ty_strs.iter().enumerate() {
                let ty = ty_str
                    .as_deref()
                    .map(|s| self.ctx.resolve_type_str(s))
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/closures:fnval_param"));
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
                param_ty_strs: Vec::new(),
            });
            self.synthesized_functions.push(wb.finish());
        }

        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: wrapper_name,
            captures: Vec::new(),
            heap: false,
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
        let param_ty_strs = sig.param_ty_strs.clone();
        let ret_ty = sig.ret_ty.clone();
        if param_ty_strs.len() != 2 || !self.is_ordering_ty(&ret_ty) {
            return None;
        }

        let wrapper_name = format!("{}__cmpval_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;
        {
            let mut wb = BlockBuilder::new(wrapper_name.clone(), MirType::I64);
            wb.add_param("__env".to_string(), MirType::Ptr);

            let mut args = Vec::new();
            for (i, ty_str) in param_ty_strs.iter().enumerate() {
                let ty = ty_str
                    .as_deref()
                    .map(|s| self.ctx.resolve_type_str(s))
                    .unwrap_or_else(|| crate::fallback::i64_fallback("lower/closures:cmp_param"));
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
                param_ty_strs: Vec::new(),
            });
            self.synthesized_functions.push(wb.finish());
        }

        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: wrapper_name,
            captures: Vec::new(),
            heap: false,
        }));
        Some((MirOperand::Local(result_local), MirType::Ptr))
    }

    /// Closure lowering: synthesize a separate MIR function for the body,
    /// build the environment, and emit ClosureCreate in the enclosing function.
    ///
    /// `is_own` mirrors `mem.closures`: owned closures start heap-allocated (may
    /// escape); scope-limited closures are always stack-allocated.
    /// `closure_id` is the closure expression's own node, which carries the
    /// checker's `Fn` type — the return type an unannotated closure would
    /// otherwise have to guess.
    pub(super) fn lower_closure(
        &mut self,
        params: &[rask_ast::expr::ClosureParam],
        ret_ty: Option<&str>,
        body: &Expr,
        is_own: bool,
        closure_id: Option<NodeId>,
    ) -> Result<TypedOperand, LoweringError> {
        self.lower_closure_expecting(params, ret_ty, body, is_own, &[], closure_id, false)
    }

    /// As `lower_closure`, with the parameter types the callee declares for this
    /// argument position. An unannotated closure parameter takes its type from
    /// there — otherwise it defaults to i64 and field access and method dispatch
    /// inside the body run against the wrong type (`|req| req.method` on a
    /// `func(Request) -> Response` parameter read a pointer instead of the tag).
    pub(super) fn lower_closure_expecting(
        &mut self,
        params: &[rask_ast::expr::ClosureParam],
        ret_ty: Option<&str>,
        body: &Expr,
        is_own: bool,
        expected_param_tys: &[String],
        closure_id: Option<NodeId>,
        for_spawn: bool,
    ) -> Result<TypedOperand, LoweringError> {
        // 1. Collect free variables (captures from enclosing scope)
        let free_vars = self.collect_free_vars(body, params);

        // 2. Generate unique name for the closure function
        let closure_name = format!("{}__closure_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        // 3. Build the closure environment layout
        let mut captures = Vec::new();
        let mut env_offset = 0u32;
        for (_name, local_id, ty) in &free_vars {
            let size = ty.size();
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
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
            .map(|s| self.ctx.resolve_type_str(s))
            .or(checked_ret)
            .unwrap_or_else(|| if inferred_void {
                MirType::Void
            } else {
                crate::fallback::i64_fallback("lower/closures:closure_ret")
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
                    params.iter().map(|p| self.ctx.type_to_mir(p)).collect()
                ),
                _ => None,
            })
            .unwrap_or_default();

        // The same parameter list, unconverted. A function type has no MIR shape
        // beyond "pointer", so whether a parameter holds one is only visible
        // before the conversion.
        let checked_param_tys: Vec<rask_types::Type> = closure_id
            .and_then(|id| self.ctx.lookup_raw_type(id))
            .and_then(|ty| match ty {
                rask_types::Type::Fn { params, .. } => Some(params.clone()),
                _ => None,
            })
            .unwrap_or_default();
        // Parameter names this closure registered as callable, so they can be
        // taken back out of the outer lowerer's sets afterwards.
        let mut callable_params: Vec<String> = Vec::new();

        let mut closure_locals = std::collections::HashMap::new();
        for (i, param) in params.iter().enumerate() {
            // Written annotation first, then the type the callee declares for
            // this position, then what the checker inferred.
            let ty_str = param.ty.clone()
                .or_else(|| expected_param_tys.get(i).cloned());
            let param_ty = ty_str.as_deref()
                .map(|s| self.ctx.resolve_type_str(s))
                .or_else(|| checked_params.get(i).cloned())
                .unwrap_or_else(|| crate::fallback::i64_fallback("lower/closures:param"));
            let param_id = closure_builder.add_param(param.name.clone(), param_ty.clone());
            closure_locals.insert(param.name.clone(), (param_id, param_ty.clone()));
            // A parameter that holds a function — `fs.map(|f| { return f(3) })`.
            // Registering it is what makes the call site emit an indirect call
            // instead of looking for a function named `f`; without it, lowering
            // found no signature and gave up on the return type (#870). The same
            // registration a `for` binding gets in #869, one level down.
            if let Some(rask_types::Type::Fn { ret, .. }) = checked_param_tys.get(i) {
                let ret_mir = self.ctx.type_to_mir(ret.as_ref());
                if self.closure_locals.insert(param.name.clone()) {
                    callable_params.push(param.name.clone());
                }
                self.func_sigs.insert(
                    param.name.clone(),
                    super::FuncSig {
                        ret_ty: ret_mir,
                        scalar_mutate_params: Vec::new(),
                        aggregate_mutate_params: Vec::new(),
                        ret_vec_elem: None,
                        param_ty_strs: Vec::new(),
                    },
                );
            }
            if let Some(prefix) = self.mir_type_name(&param_ty) {
                self.meta_mut(&param.name).type_prefix = Some(prefix);
            } else if let Some(s) = ty_str.as_deref() {
                if let Some(prefix) = super::type_prefix_from_str(s) {
                    self.meta_mut(&param.name).type_prefix = Some(prefix);
                }
            }
        }

        // Emit LoadCapture for each free variable
        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
                by_ref: false,
            }));
            closure_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        // Lower the closure body using a temporary lowerer
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

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
            self.loop_stack = saved_loop_stack;
            // The closure's parameters are out of scope again — don't leave a
            // name like `f` registered as callable for the enclosing function.
            for name in &callable_params {
                self.closure_locals.remove(name);
                self.func_sigs.remove(name);
            }

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
            param_ty_strs: Vec::new(),
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
        let entry_name = if for_spawn && crate::types::spawn_payload_is_boxed(&closure_ret) {
            self.synthesize_spawn_box_thunk(&closure_name, &closure_ret)
        } else {
            closure_name
        };

        // 5. In the parent function, emit ClosureCreate.
        // Own closures may escape — start heap-allocated so escape analysis can
        // decide whether to downgrade. Scope-limited closures never escape; stack only.
        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: entry_name,
            captures,
            heap: is_own,
        }));

        Ok((MirOperand::Local(result_local), MirType::Ptr))
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
            param_ty_strs: Vec::new(),
        });
        self.synthesized_functions.push(b.finish());
        thunk_name
    }

    /// Spawn lowering: synthesize a closure function from the body block,
    /// emit ClosureCreate + Call to rask_closure_spawn.
    pub(super) fn lower_spawn(
        &mut self,
        body: &[Stmt],
    ) -> Result<TypedOperand, LoweringError> {
        // 1. Collect free variables from the spawn body block
        let free_vars = self.collect_free_vars_block(body);

        // 2. Generate unique name for the spawn function
        let spawn_name = format!("{}__spawn_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        // 3. Build the closure environment layout
        let mut captures = Vec::new();
        let mut env_offset = 0u32;
        for (_name, local_id, ty) in &free_vars {
            let size = ty.size();
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
            });
            env_offset = aligned_offset + size;
        }

        // 4. Synthesize a MIR function for the spawn body.
        let mut spawn_builder = BlockBuilder::new(spawn_name.clone(), MirType::Void);

        let env_param_id = spawn_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut spawn_locals = std::collections::HashMap::new();
        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = spawn_builder.alloc_local(name.clone(), ty.clone());
            spawn_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
                by_ref: false,
            }));
            spawn_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        // Lower the body statements using a temporary lowerer
        {
            let saved_builder = std::mem::replace(&mut self.builder, spawn_builder);
            let saved_locals = std::mem::replace(&mut self.locals, spawn_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            let mut body_result = Ok(());
            for stmt in body {
                if let Err(e) = self.lower_stmt(stmt) {
                    body_result = Err(e);
                    break;
                }
            }

            spawn_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;

            body_result?;

            if spawn_builder.current_block_unterminated() {
                spawn_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
            }
        }

        let spawn_fn = spawn_builder.finish();

        // Try the state machine transform for yield-point-containing spawns
        if let Some(sm_result) = crate::transform::state_machine::transform(&spawn_fn) {
            let poll_name = sm_result.poll_fn.name.clone();
            self.func_sigs.insert(poll_name.clone(), super::FuncSig {
                ret_ty: MirType::I32,
                scalar_mutate_params: Vec::new(),
                aggregate_mutate_params: Vec::new(),
                ret_vec_elem: None,
                param_ty_strs: Vec::new(),
            });
            self.synthesized_functions.push(sm_result.poll_fn);

            let state_ptr = self.builder.alloc_temp(MirType::Ptr);
            let state_size_val = sm_result.state_size as i64;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(state_ptr),
                func: FunctionRef::internal("rask_alloc".to_string()),
                args: vec![MirOperand::Constant(crate::operand::MirConst::Int(state_size_val))],
            }));

            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: state_ptr,
                offset: 0,
                value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                store_size: None,
            }));

            for &(env_offset, state_offset) in &sm_result.capture_stores {
                if let Some(cap) = captures.iter().find(|c| c.offset == env_offset) {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: state_ptr,
                        offset: state_offset,
                        value: MirOperand::Local(cap.local_id),
                        store_size: None,
                    }));
                }
            }

            let poll_fn_ptr = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: poll_fn_ptr,
                rvalue: MirRValue::Use(MirOperand::Constant(
                    crate::operand::MirConst::String(poll_name),
                )),
            }));

            let handle_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(handle_local),
                func: FunctionRef::internal("rask_green_spawn".to_string()),
                args: vec![
                    MirOperand::Local(poll_fn_ptr),
                    MirOperand::Local(state_ptr),
                    MirOperand::Constant(crate::operand::MirConst::Int(state_size_val)),
                ],
            }));

            Ok((MirOperand::Local(handle_local), MirType::Ptr))
        } else {
            self.func_sigs.insert(spawn_name.clone(), super::FuncSig {
                ret_ty: MirType::Void,
                scalar_mutate_params: Vec::new(),
                aggregate_mutate_params: Vec::new(),
                ret_vec_elem: None,
                param_ty_strs: Vec::new(),
            });
            self.synthesized_functions.push(spawn_fn);

            let closure_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
                dst: closure_local,
                func_name: spawn_name,
                captures,
                heap: true,
            }));

            let handle_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(handle_local),
                func: FunctionRef::internal("spawn".to_string()),
                args: vec![MirOperand::Local(closure_local)],
            }));

            Ok((MirOperand::Local(handle_local), MirType::Ptr))
        }
    }

    /// Collect free variables from a block of statements (no params to bind).
    pub(super) fn collect_free_vars_block(
        &self,
        body: &[Stmt],
    ) -> Vec<(String, LocalId, MirType)> {
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

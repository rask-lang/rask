// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Concurrency lowering: Shared read/write blocks, Mutex lock blocks.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    operand::MirConst, stmt::ClosureCapture, types::StructLayoutId, BlockBuilder, FunctionRef,
    MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{CallArg, Expr, ExprKind};
use rask_ast::{NodeId, Span};

/// Which runtime calls a box's `with` block takes and drops access through.
///
/// Every box in the family hands back the payload's address the same way, so the
/// block itself is one piece of code; only these three names differ. `release` is
/// `None` for a box holding no lock — a `Cell` is single-task by construction
/// (mem.cell/CE1), so there's nothing to unlock on the way out.
pub(super) struct BoxWithSyms {
    /// Takes exclusive access, returns the payload's address.
    pub acquire: &'static str,
    /// The payload's address again, for the write-back — acquire consumed its own
    /// result. Must not re-take access.
    pub data: &'static str,
    /// Drops access. `None` when the box holds none.
    pub release: Option<&'static str>,
}

impl BoxWithSyms {
    pub(super) const MUTEX: Self = Self {
        acquire: "Mutex_acquire",
        data: "Mutex_data",
        release: Some("Mutex_release"),
    };

    /// `Cell_acquire` and `Cell_data` both land on `rask_cell_get` — the slot's
    /// address, no lock involved. They stay distinct MIR names so this path and
    /// the `cell.get()` path (CE6, which copies out) can't drift into each other.
    pub(super) const CELL: Self = Self {
        acquire: "Cell_acquire",
        data: "Cell_data",
        release: None,
    };
}

impl<'a> MirLowerer<'a> {
    /// Extract the inner type name from a Shared/Mutex expression — the `T` in
    /// `Shared<T>`/`Mutex<T>`, whether the checker left it a resolved `Generic`
    /// or an `UnresolvedGeneric`.
    pub(super) fn resolve_shared_inner_type_name(&self, object: &Expr) -> Option<String> {
        if let Some(raw_ty) = self.ctx.lookup_raw_type(object.id) {
            let args = match raw_ty {
                rask_types::Type::UnresolvedGeneric { args, .. }
                | rask_types::Type::Generic { args, .. } => Some(args),
                _ => None,
            };
            if let Some(args) = args {
                if let Some(rask_types::GenericArg::Type(inner)) = args.first() {
                    if let rask_types::Type::UnresolvedNamed(name) = inner.as_ref() {
                        return Some(name.clone());
                    }
                    if let Some(prefix) = super::MirContext::type_prefix(inner, self.ctx.type_names) {
                        return Some(prefix);
                    }
                }
            }
        }
        if let ExprKind::Ident(var_name) = &object.kind {
            if let Some(full_type) = self.meta(var_name).and_then(|m| m.full_type.as_deref()) {
                let inner = full_type.split('<').nth(1)
                    .and_then(|s| s.strip_suffix('>'));
                if let Some(name) = inner {
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    /// The MIR type of what a sync box holds: the `T` of `Mutex<T>`/`Shared<T>`.
    /// Only the type name was resolved before, so anything that isn't a named
    /// struct — a `string`, a tuple — fell back to `i64` and got treated as a
    /// word-sized payload. The lock then handed back an address and codegen
    /// loaded eight bytes out of it, which for a `Mutex<string>` is the string's
    /// first half read as a pointer.
    pub(super) fn resolve_sync_payload_mir(&self, object: &Expr) -> Option<MirType> {
        let raw_ty = self.ctx.lookup_raw_type(object.id)?;
        let args = match raw_ty {
            rask_types::Type::UnresolvedGeneric { args, .. }
            | rask_types::Type::Generic { args, .. } => args,
            _ => return None,
        };
        let rask_types::GenericArg::Type(inner) = args.first()? else { return None };
        Some(self.ctx.type_to_mir(inner))
    }

    /// Lower `with shared.read() as d { body }` / `with shared.write() as d { body }`.
    pub(super) fn lower_shared_with_block(
        &mut self,
        object: &Expr,
        method: &str,
        binding_name: &str,
        body: &[rask_ast::stmt::Stmt],
    ) -> Result<TypedOperand, LoweringError> {
        let (shared_op, _) = self.lower_expr(object)?;

        let mut free_vars = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut bound = std::collections::HashSet::new();
        bound.insert(binding_name.to_string());
        self.walk_free_vars_block(body, &bound, &mut seen, &mut free_vars);

        let closure_name = format!("{}__with_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

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

        let mut closure_builder = BlockBuilder::new(closure_name.clone(), MirType::I64);
        let env_param_id = closure_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut data_param_ty = self.resolve_sync_payload_mir(object).unwrap_or_else(|| crate::fallback::i64_fallback("lower/concurrency:132"));
        let inner_type_name = self.resolve_shared_inner_type_name(object);
        if let Some(ref type_name) = inner_type_name {
            if let Some((layout_idx, sl)) = self.ctx.find_struct(type_name) {
                data_param_ty = MirType::Struct(StructLayoutId::new(layout_idx, sl.size, sl.align));
            }
            self.meta_mut(binding_name).type_prefix = Some(type_name.clone());
        }

        // The box stores the payload's bytes; the lock hands the closure their
        // address. When the payload lives in its own storage that address *is*
        // the value, but anything that fits in a word — a Map or Vec handle, an
        // integer — has to be loaded out. Passed straight through, `with
        // mutex.lock() as m { m.insert(…) }` on a `Mutex<Map>` handed the map
        // runtime a pointer to a pointer and crashed on the first hash (#477).
        // A word-sized payload is loaded into a local, so whatever the body did
        // to it has to be written back before the guard drops — otherwise
        // `with m.lock() as v { v += 5 }` incremented a copy (#268).
        let mut writeback_slot = None;
        let data_param_id = if data_param_ty.passed_by_address() {
            closure_builder.add_param(binding_name.to_string(), data_param_ty.clone())
        } else {
            let slot = closure_builder.add_param("__guard_slot".to_string(), MirType::Ptr);
            let loaded = closure_builder.alloc_local(binding_name.to_string(), data_param_ty.clone());
            closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: loaded,
                rvalue: MirRValue::Deref(MirOperand::Local(slot)),
            }));
            writeback_slot = Some((slot, loaded, data_param_ty.size()));
            loaded
        };

        let mut closure_locals = std::collections::HashMap::new();
        closure_locals.insert(binding_name.to_string(), (data_param_id, data_param_ty));

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

        let body_result;
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            body_result = self.lower_block(body);

            closure_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;
        }

        let (body_val, _) = body_result?;

        if let Some((slot, loaded, size)) = writeback_slot {
            if closure_builder.current_block_unterminated() {
                closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: slot,
                    offset: 0,
                    value: MirOperand::Local(loaded),
                    store_size: Some(size),
                }));
            }
        }

        if closure_builder.current_block_unterminated() {
            closure_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: Some(body_val),
            }));
        }

        let closure_fn = closure_builder.finish();
        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: MirType::I64,
            scalar_mutate_params: Vec::new(),
            ret_vec_elem: None,
            param_ty_strs: Vec::new(),
        });
        self.synthesized_functions.push(closure_fn);

        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: closure_local,
            func_name: closure_name,
            captures,
            heap: false,
        }));

        let func_name = if method == "read" {
            "Shared_read".to_string()
        } else {
            "Shared_write".to_string()
        };

        let result_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_local),
            func: FunctionRef::internal(func_name),
            args: vec![shared_op, MirOperand::Local(closure_local)],
        }));

        Ok((MirOperand::Local(result_local), MirType::I64))
    }

    /// Lower `with <box> as v { body }` for a box whose payload is reached by
    /// address — `Mutex` (via `.lock()` or bare) and `Cell`.
    ///
    /// The body runs in this frame, between acquire and release, not in a
    /// synthesized closure. The closure form got two things wrong that no amount
    /// of patching inside it could fix: a `return` in the body returned from the
    /// closure instead of the enclosing function, so the function fell through to
    /// whatever came after the `with`; and the write-back of a word-sized payload
    /// was skipped whenever the body ended terminated, so the mutation was lost
    /// too.
    ///
    /// ```text
    /// func bump(m: Mutex<i64>) -> i64 {
    ///     with m.lock() as v { v = v + 5; return v }
    ///     return -1
    /// }
    /// // was: returns -1, mutex still holds 1.  now: returns 6, holds 6.
    /// ```
    ///
    /// Release (and the write-back) go in a cleanup block on the ensure stack,
    /// so `return`, `try`, `break` and `continue` all run them on the way out.
    pub(super) fn lower_box_with_block(
        &mut self,
        object: &Expr,
        binding_name: &str,
        body: &[rask_ast::stmt::Stmt],
        syms: &BoxWithSyms,
    ) -> Result<TypedOperand, LoweringError> {
        let (box_op, _) = self.lower_expr(object)?;

        // A payload that lives in its own storage is aliased through the pointer
        // acquire hands back; anything word-sized is loaded into the local
        // (codegen does the load), which is why it needs writing back.
        let inner_type_name = self.resolve_shared_inner_type_name(object);
        let mut guard_ty = self.resolve_sync_payload_mir(object).unwrap_or_else(|| crate::fallback::i64_fallback("lower/concurrency:278"));
        if let Some(ref type_name) = inner_type_name {
            if let Some((layout_idx, sl)) = self.ctx.find_struct(type_name) {
                guard_ty = MirType::Struct(StructLayoutId::new(layout_idx, sl.size, sl.align));
            }
        }
        let by_address = guard_ty.passed_by_address();

        let guard_local = self.builder.alloc_local(binding_name.to_string(), guard_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(guard_local),
            func: FunctionRef::internal(syms.acquire.to_string()),
            args: vec![box_op.clone()],
        }));

        let saved_binding = self.locals.insert(binding_name.to_string(), (guard_local, guard_ty.clone()));
        if let Some(ref type_name) = inner_type_name {
            self.meta_mut(binding_name).type_prefix = Some(type_name.clone());
        }

        let writeback = (!by_address).then(|| guard_ty.size());
        let depth = self.ensure_stack.len();
        self.push_guard_cleanup(&box_op, guard_local, writeback, syms);

        let result = self.lower_block(body);

        // Park the block's value in a local of its own before the guard is
        // released. A `with` block is an expression (mem.borrowing, the
        // `const name = with pool[h] as entity { entity.name }` form), and the
        // value has to be read out while the lock is still held.
        let result = result.map(|(op, ty)| {
            if matches!(ty, MirType::Void) || !self.builder.current_block_unterminated() {
                return (op, ty);
            }
            let out = self.builder.alloc_temp(ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: out,
                rvalue: crate::MirRValue::Use(op),
            }));
            (MirOperand::Local(out), ty)
        });

        // Fall-through exit runs the same cleanup inline. Early exits already
        // chained through the block on the ensure stack — but they left the
        // current block terminated, and whatever follows the `with` still has to
        // be lowered somewhere. Without a fresh block it landed on top of the
        // body's own terminator and erased it.
        if self.builder.current_block_unterminated() {
            self.emit_loop_cleanup(depth);
        } else {
            let unreachable = self.builder.create_block();
            self.builder.switch_to_block(unreachable);
        }
        self.ensure_stack.truncate(depth);
        match saved_binding {
            Some(prev) => { self.locals.insert(binding_name.to_string(), prev); }
            None => { self.locals.remove(binding_name); }
        }

        result
    }

    /// Schedule "write the guard back, then unlock" as a scope cleanup, the same
    /// shape `ensure` uses: the main flow skips the block, and every exit path
    /// chains through it.
    fn push_guard_cleanup(
        &mut self,
        box_op: &MirOperand,
        guard_local: crate::LocalId,
        writeback: Option<u32>,
        syms: &BoxWithSyms,
    ) {
        let cleanup_block = self.builder.create_block();
        let continue_block = self.builder.create_block();

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::EnsurePush { cleanup_block }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: continue_block }));

        self.builder.switch_to_block(cleanup_block);
        if let Some(size) = writeback {
            let slot = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(slot),
                func: FunctionRef::internal(syms.data.to_string()),
                args: vec![box_op.clone()],
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: slot,
                offset: 0,
                value: MirOperand::Local(guard_local),
                store_size: Some(size),
            }));
        }
        if let Some(release) = syms.release {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(release.to_string()),
                args: vec![box_op.clone()],
            }));
        }
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

        self.ensure_stack.push(cleanup_block);
        self.builder.switch_to_block(continue_block);
    }

    /// True when `object` has type `{box_name}<T>` (e.g. "Mutex", "Shared").
    /// Resolves the prefix from the checker's type (resolved `Generic` or
    /// `UnresolvedGeneric`, generics stripped) so it covers a field receiver
    /// like `self.store`; falls back to a tracked `type_prefix` on a local.
    pub(super) fn is_sync_box_expr(&self, object: &Expr, box_name: &str) -> bool {
        let from_type = self.ctx.lookup_raw_type(object.id)
            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
            .map(|p| p.split('<').next().unwrap_or(&p).trim() == box_name)
            .unwrap_or(false);
        let from_prefix = if let ExprKind::Ident(var_name) = &object.kind {
            self.meta(var_name)
                .and_then(|m| m.type_prefix.as_deref())
                .map(|p| p == box_name)
                .unwrap_or(false)
        } else {
            false
        };
        from_type || from_prefix
    }

    /// If `object` is a no-arg guard access on a sync box —
    /// `mutex.lock()`, `shared.read()`, `shared.write()` — return the box
    /// expression and the acquire/release runtime functions for it. The caller
    /// runs the trailing method or field access on the guard between them.
    pub(super) fn sync_guard<'e>(
        &self,
        object: &'e Expr,
    ) -> Option<(&'e Expr, &'static str, &'static str)> {
        let ExprKind::MethodCall { object: box_obj, method, args, .. } = &object.kind else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        match method.as_str() {
            "lock" if self.is_sync_box_expr(box_obj, "Mutex") => {
                Some((box_obj, "Mutex_acquire", "Mutex_release"))
            }
            "read" if self.is_sync_box_expr(box_obj, "Shared") => {
                Some((box_obj, "Shared_read_acquire", "Shared_release"))
            }
            "write" if self.is_sync_box_expr(box_obj, "Shared") => {
                Some((box_obj, "Shared_write_acquire", "Shared_release"))
            }
            _ => None,
        }
    }

    /// Lower a guard access on a sync box: `box.lock()/.read()/.write()`
    /// followed by a method call or field access. Acquire the lock and take a
    /// pointer to the inner value, run the trailing operation on that pointer
    /// in this frame, then release. Running in-frame (rather than in a closure,
    /// as the `with` form does) lets the operation return an aggregate — a
    /// `T or E` result — through the normal ABI. A `mutate self` method writes
    /// through to the real value; the lock is held for exactly the operation.
    ///
    /// `make_op` builds the trailing operation given the guard as an ident:
    /// `|g| g.method(args)` or `|g| g.field`.
    pub(super) fn lower_sync_guard_access(
        &mut self,
        box_obj: &Expr,
        acquire: &str,
        release: &str,
        ret_hint: Option<MirType>,
        make_op: impl FnOnce(Expr) -> Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (box_op, _) = self.lower_expr(box_obj)?;

        // The guard aliases the box's inner value — the acquire call returns a
        // pointer to it. Type the local as the inner struct so method dispatch
        // and field offsets resolve, exactly like a `with pool[h] as e`
        // binding. Codegen special-cases the acquire functions to bind the
        // returned pointer directly (a struct pointer-alias), so it isn't
        // copied into a fresh slot — a `mutate self` method then writes through
        // to the real value.
        let inner_name = self.resolve_shared_inner_type_name(box_obj);
        let guard_ty = inner_name.as_ref()
            .and_then(|n| self.ctx.find_struct(n))
            .map(|(idx, sl)| MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)))
            .unwrap_or(MirType::Ptr);
        let guard_name = format!("__lock_guard_{}", self.closure_counter);
        self.closure_counter += 1;
        let guard_local = self.builder.alloc_local(guard_name.clone(), guard_ty.clone());
        // Codegen loads through the acquire result for a non-struct payload, so
        // the guard already holds the value here — no deref at this level.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(guard_local),
            func: FunctionRef::internal(acquire.to_string()),
            args: vec![box_op.clone()],
        }));
        self.locals.insert(guard_name.clone(), (guard_local, guard_ty));
        if let Some(n) = &inner_name {
            self.meta_mut(&guard_name).type_prefix = Some(n.clone());
        }

        // Lower the trailing operation on the guard through the normal path.
        let guard_ident = Expr {
            id: NodeId::DUMMY,
            span: Span::new(0, 0),
            kind: ExprKind::Ident(guard_name.clone()),
        };
        let (result, inner_ret_ty) = self.lower_expr(&make_op(guard_ident))?;

        // Release. The operation's value is a copy (or lives in a caller slot),
        // so it stays valid after the lock is released.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(release.to_string()),
            args: vec![box_op],
        }));

        // Pick the return type. The inner op's type (from the method's own
        // signature) is authoritative — it carries a resolved `T or E` result,
        // which the outer expression's checker type often doesn't (a lock chain
        // is frequently left an inference var, collapsing to Ptr). Fall back to
        // the checker hint only when the inner type is an unresolved bare Ptr.
        let ret_ty = if matches!(inner_ret_ty, MirType::Ptr) {
            ret_hint
                .filter(|t| !matches!(t, MirType::Void | MirType::Ptr))
                .unwrap_or(inner_ret_ty)
        } else {
            inner_ret_ty
        };
        Ok((result, ret_ty))
    }

}

// ─── select ────────────────────────────────────────────────────────────────

impl<'a> MirLowerer<'a> {
    /// `select { rx -> v: …, tx <- msg: …, _: … }` (conc.select).
    ///
    /// Compiles to a poll loop over the arms. Each arm gets one non-blocking
    /// probe per round; the first that succeeds jumps to its body. With a
    /// `_:` arm, a round where nothing was ready falls into it (A3). Without
    /// one, the loop yields and goes round again — unless every channel came
    /// back closed, which is the end of the road (CL1).
    ///
    /// The old lowering fell straight into arm 0 and ran every body in
    /// sequence, so a receive binding was never defined and MIR failed with
    /// `UnresolvedVariable("v")`.
    pub(super) fn lower_select(
        &mut self,
        arms: &[rask_ast::expr::SelectArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::SelectArmKind;

        if arms.is_empty() {
            return Err(LoweringError::InvalidConstruct(
                "select with zero arms [conc.select/P3]".to_string(),
            ));
        }

        // Channel operands and send payloads are evaluated once, before the
        // loop — polling must not re-run them each round.
        enum Probe {
            Recv { rx: MirOperand, buf: crate::LocalId, elem: MirType, binding: String, arm: usize },
            Send { tx: MirOperand, value: MirOperand, arm: usize },
        }
        let mut probes = Vec::new();
        let mut default_arm: Option<usize> = None;

        for (i, arm) in arms.iter().enumerate() {
            match &arm.kind {
                SelectArmKind::Recv { channel, binding } => {
                    let (rx, _) = self.lower_expr(channel)?;
                    let elem = self.channel_elem_type(channel);
                    // One-element array so the buffer is a real stack slot the
                    // runtime can write into; a scalar local would spill to a
                    // fresh slot per `Ref` and lose the value.
                    let buf = self.builder.alloc_temp(MirType::Array {
                        elem: Box::new(elem.clone()),
                        len: 1,
                    });
                    probes.push(Probe::Recv { rx, buf, elem, binding: binding.clone(), arm: i });
                }
                SelectArmKind::Send { channel, value } => {
                    let (tx, _) = self.lower_expr(channel)?;
                    let (value, _) = self.lower_expr(value)?;
                    probes.push(Probe::Send { tx, value, arm: i });
                }
                SelectArmKind::Default => default_arm = Some(i),
            }
        }

        let merge_block = self.builder.create_block();
        let poll_block = self.builder.create_block();
        let arm_blocks: Vec<crate::BlockId> =
            arms.iter().map(|_| self.builder.create_block()).collect();
        // One probe block per arm plus the "nothing was ready" landing.
        let probe_blocks: Vec<crate::BlockId> =
            (0..=probes.len()).map(|_| self.builder.create_block()).collect();

        // Tracks whether any channel could still deliver. Reset every round so
        // a channel closing mid-wait is noticed.
        let any_open = self.builder.alloc_temp(MirType::Bool);
        let result_local = self.builder.alloc_temp(MirType::I64);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: poll_block }));
        self.builder.switch_to_block(poll_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: any_open,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: probe_blocks[0],
        }));

        for (p, probe) in probes.iter().enumerate() {
            self.builder.switch_to_block(probe_blocks[p]);
            let next = probe_blocks[p + 1];
            let (status, arm_idx) = match probe {
                Probe::Recv { rx, buf, .. } => {
                    let addr = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: addr,
                        rvalue: MirRValue::Ref(*buf),
                    }));
                    let status = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(status),
                        func: FunctionRef::internal("rask_channel_try_recv_into".to_string()),
                        args: vec![rx.clone(), MirOperand::Local(addr)],
                    }));
                    (status, *match probe { Probe::Recv { arm, .. } => arm, _ => unreachable!() })
                }
                Probe::Send { tx, value, arm } => {
                    let status = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(status),
                        func: FunctionRef::internal("rask_channel_try_send_i64".to_string()),
                        args: vec![tx.clone(), value.clone()],
                    }));
                    (status, *arm)
                }
            };

            // RASK_CHAN_OK is 0; CLOSED is -1; FULL/EMPTY are -2/-3.
            let ready = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: ready,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: MirOperand::Local(status),
                    right: MirOperand::Constant(MirConst::Int(0)),
                },
            }));
            let not_ready = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(ready),
                then_block: arm_blocks[arm_idx],
                else_block: not_ready,
            }));

            // Not ready: still open unless the channel reported CLOSED (CL2 —
            // a closed channel is skipped, the others keep being waited on).
            self.builder.switch_to_block(not_ready);
            let closed = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: closed,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: MirOperand::Local(status),
                    right: MirOperand::Constant(MirConst::Int(-1)),
                },
            }));
            let mark_open = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(closed),
                then_block: next,
                else_block: mark_open,
            }));
            self.builder.switch_to_block(mark_open);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: any_open,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: next }));
        }

        // Nothing fired this round.
        self.builder.switch_to_block(probe_blocks[probes.len()]);
        if let Some(idx) = default_arm {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: arm_blocks[idx],
            }));
        } else {
            let wait = self.builder.create_block();
            let all_closed = self.builder.create_block();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(any_open),
                then_block: wait,
                else_block: all_closed,
            }));

            self.builder.switch_to_block(wait);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("rask_yield".to_string()),
                args: vec![],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: poll_block,
            }));

            self.builder.switch_to_block(all_closed);
            let msg = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(msg),
                func: FunctionRef::internal("panic".to_string()),
                args: vec![MirOperand::Constant(MirConst::String(
                    "select: every channel is closed [conc.select/CL1]".to_string(),
                ))],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
        }

        // Arm bodies. A receive arm binds its value from the probe's buffer.
        let mut result_ty = MirType::Void;
        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);
            if let Some(Probe::Recv { buf, elem, binding, .. }) =
                probes.iter().find(|p| matches!(p, Probe::Recv { arm, .. } if *arm == i))
            {
                let bound = self.builder.alloc_local(binding.clone(), elem.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: bound,
                    rvalue: MirRValue::ArrayIndex {
                        base: MirOperand::Local(*buf),
                        index: MirOperand::Constant(MirConst::Int(0)),
                        elem_size: elem.size(),
                    },
                }));
                self.locals.insert(binding.clone(), (bound, elem.clone()));
            }
            let (arm_val, arm_ty) = self.lower_expr(&arm.body)?;
            if !matches!(arm_ty, MirType::Void) {
                result_ty = arm_ty;
            }
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(arm_val),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Element type behind a `Receiver<T>` expression, defaulting to a word.
    fn channel_elem_type(&self, channel: &Expr) -> MirType {
        let args = match self.ctx.lookup_raw_type(channel.id) {
            Some(rask_types::Type::UnresolvedGeneric { args, .. }) => args.clone(),
            Some(rask_types::Type::Generic { args, .. }) => args.clone(),
            _ => return MirType::I64,
        };
        match args.first() {
            Some(rask_types::GenericArg::Type(t)) => self.ctx.type_to_mir(t),
            _ => MirType::I64,
        }
    }
}

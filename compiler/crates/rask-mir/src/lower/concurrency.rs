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

/// Which synchronization a `Shared<T, S>` takes (`conc.sync/SH1`). The strategy
/// is a type argument resolved here, at lowering, so nothing about it survives
/// into the emitted code — a `Local` box calls the no-lock runtime directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SharedStrategy {
    /// No lock at all. Several accessors, one task.
    Local,
    /// Read-write lock: many readers at once, or one writer.
    Readers,
    /// Plain lock: one holder at a time, reader or writer.
    Mutex,
}

impl SharedStrategy {
    /// Read it off a type argument's spelling. Anything unrecognised — a bare
    /// `Shared<T>`, an unresolved variable — is `Local`, which is the
    /// conservative answer (SH8): you can never accidentally skip
    /// synchronization you needed, because sending a `Local` box doesn't
    /// compile.
    pub(super) fn from_name(name: &str) -> Self {
        match name.trim() {
            "Readers" => SharedStrategy::Readers,
            "Mutex" => SharedStrategy::Mutex,
            _ => SharedStrategy::Local,
        }
    }

    /// The MIR prefix its runtime family answers to. The three families already
    /// existed as three types; the merge is that one type now picks between
    /// them instead of the user doing it at declaration time.
    pub(super) fn prefix(self) -> &'static str {
        match self {
            SharedStrategy::Local => "Cell",
            SharedStrategy::Readers => "Shared",
            SharedStrategy::Mutex => "Mutex",
        }
    }

    /// The acquire/data/release triple for a `with` block over this strategy.
    /// `write` picks the exclusive side where the strategy has two.
    pub(super) fn with_syms(self, write: bool) -> BoxWithSyms {
        match self {
            SharedStrategy::Local => BoxWithSyms::CELL,
            SharedStrategy::Mutex => BoxWithSyms::MUTEX,
            SharedStrategy::Readers => {
                if write { BoxWithSyms::SHARED_WRITE } else { BoxWithSyms::SHARED_READ }
            }
        }
    }

    /// Acquire and release for an inline guard access.
    pub(super) fn guard_syms(self, write: bool) -> (&'static str, &'static str) {
        match self {
            SharedStrategy::Local => ("Cell_acquire", "Cell_noop_release"),
            SharedStrategy::Mutex => ("Mutex_acquire", "Mutex_release"),
            SharedStrategy::Readers => {
                if write {
                    ("Shared_write_acquire", "Shared_release")
                } else {
                    ("Shared_read_acquire", "Shared_release")
                }
            }
        }
    }
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

    pub(super) const SHARED_READ: Self = Self {
        acquire: "Shared_read_acquire",
        data: "Shared_data",
        release: Some("Shared_release"),
    };

    pub(super) const SHARED_WRITE: Self = Self {
        acquire: "Shared_write_acquire",
        data: "Shared_data",
        release: Some("Shared_release"),
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

    /// The strategy of a `Shared<T, S>` receiver — the second type argument.
    ///
    /// Absent means `Local`: bare `Shared<T>` is `Shared<T, Local>` in every
    /// position (SH3), and an unresolved receiver gets the conservative answer
    /// for the same reason the default is conservative (SH8).
    pub(super) fn shared_strategy(&self, object: &Expr) -> SharedStrategy {
        if let Some(raw_ty) = self.ctx.lookup_raw_type(object.id) {
            let args = match raw_ty {
                rask_types::Type::UnresolvedGeneric { args, .. }
                | rask_types::Type::Generic { args, .. } => Some(args.as_slice()),
                _ => None,
            };
            if let Some(rask_types::GenericArg::Type(s)) = args.and_then(|a| a.get(1)) {
                if let Some(name) = super::MirContext::type_prefix(s, self.ctx.type_names) {
                    return SharedStrategy::from_name(&name);
                }
            }
        }
        // The declared spelling, when the checker's type didn't survive:
        // "Shared<Queue, Mutex>" -> Mutex.
        if let ExprKind::Ident(var_name) = &object.kind {
            if let Some(full) = self.meta(var_name).and_then(|m| m.full_type.as_deref()) {
                if let Some(inner) = full.split_once('<').and_then(|(_, r)| r.strip_suffix('>')) {
                    if let Some((_, strategy)) = inner.rsplit_once(',') {
                        return SharedStrategy::from_name(strategy);
                    }
                }
            }
        }
        SharedStrategy::Local
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
            // `read`/`write` on a `Shared<T, S>`: which lock, if any, is the
            // strategy's business (SH5). All three answer to both verbs — a
            // `read()` under `Mutex` takes the exclusive lock, slower than
            // `Readers` would be there and never wrong.
            "read" | "write" if self.is_sync_box_expr(box_obj, "Shared") => {
                let (acquire, release) = self
                    .shared_strategy(box_obj)
                    .guard_syms(method == "write");
                Some((box_obj, acquire, release))
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
        // What the payload holds, when the payload is a collection. The guard is
        // a synthetic local with no checker type of its own, so
        // `self.counters.lock().get(k)` on a `Mutex<Map<string, u64>>` had
        // nothing to resolve the value type from once `.lock()` desugared away.
        if let Some(elem) = self.ctx.lookup_raw_type(box_obj.id)
            .and_then(|ty| self.collection_elem_of_checker_type(ty))
        {
            self.meta_mut(&guard_name).elem_type = Some(elem);
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
        is_priority: bool,
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
        let landing_block = self.builder.create_block();
        let arm_blocks: Vec<crate::BlockId> =
            arms.iter().map(|_| self.builder.create_block()).collect();
        let probe_blocks: Vec<crate::BlockId> =
            (0..probes.len()).map(|_| self.builder.create_block()).collect();
        // One "move to the next arm in the cycle, or stop" block per probe —
        // see the rotation comment below.
        let advance_blocks: Vec<crate::BlockId> =
            (0..probes.len()).map(|_| self.builder.create_block()).collect();

        // Tracks whether any channel could still deliver. Reset every round so
        // a channel closing mid-wait is noticed.
        let any_open = self.builder.alloc_temp(MirType::Bool);
        let result_local = self.builder.alloc_temp(MirType::I64);
        // How many arms this round has probed — the cycle stops once every
        // arm's had exactly one turn, wherever it started.
        let visited = self.builder.alloc_temp(MirType::I64);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: poll_block }));
        self.builder.switch_to_block(poll_block);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: any_open,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: visited,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        // conc.select/P1: a plain `select` picks uniformly among the ready arms
        // so one busy channel can't starve the rest. A full shuffle every poll
        // is more than that guarantee needs — starting the probe cycle at a
        // rotating offset removes the starvation for the cost of one runtime
        // counter bump. `select_priority` keeps listed order, so it always
        // starts at arm 0.
        if probes.is_empty() {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: landing_block,
            }));
        } else if is_priority || probes.len() == 1 {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: probe_blocks[0],
            }));
        } else {
            let start = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(start),
                func: FunctionRef::internal("rask_select_rotate".to_string()),
                args: vec![MirOperand::Constant(MirConst::Int(probes.len() as i64))],
            }));
            let cases = (0..probes.len()).map(|i| (i as u64, probe_blocks[i])).collect();
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Switch {
                value: MirOperand::Local(start),
                cases,
                default: probe_blocks[0],
            }));
        }

        for (p, probe) in probes.iter().enumerate() {
            self.builder.switch_to_block(probe_blocks[p]);
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
                then_block: advance_blocks[p],
                else_block: mark_open,
            }));
            self.builder.switch_to_block(mark_open);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: any_open,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: advance_blocks[p] }));

            // Count this arm as checked; once every arm's had its turn this
            // round, land — otherwise carry on to the next arm in the cycle
            // (wrapping past the end, since the cycle can start anywhere).
            self.builder.switch_to_block(advance_blocks[p]);
            let new_visited = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: new_visited,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Add,
                    left: MirOperand::Local(visited),
                    right: MirOperand::Constant(MirConst::Int(1)),
                },
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: visited,
                rvalue: MirRValue::Use(MirOperand::Local(new_visited)),
            }));
            let done = self.builder.alloc_temp(MirType::Bool);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: done,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: MirOperand::Local(new_visited),
                    right: MirOperand::Constant(MirConst::Int(probes.len() as i64)),
                },
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(done),
                then_block: landing_block,
                else_block: probe_blocks[(p + 1) % probes.len()],
            }));
        }

        // Nothing fired this round.
        self.builder.switch_to_block(landing_block);
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

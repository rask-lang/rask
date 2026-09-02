// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Give a captured scalar a home in memory.
//!
//! A scope-limited closure *borrows* what it captures: the environment holds
//! the variable's address and the body reads and writes through it, so a write
//! inside the closure is a write to the enclosing variable (mem.closures/MC1).
//!
//! For anything passed by address that costs nothing — the local's Cranelift
//! variable already holds the address. For a scalar it can't work at all: an
//! SSA value has no address. `Ref` on a scalar used to spill a *copy* to a
//! fresh stack slot and hand back that, so every write through it landed on
//! the copy and vanished (#1038).
//!
//! This pass makes those scalars memory-resident. A read becomes a load, a
//! write becomes a store, and the local itself stops being used as a value:
//!
//! ```text
//! _5 = _5 + 1        →    _t0 = Deref(_5)
//!                         _t1 = _t0 + 1
//!                         Store { addr: _5, value: _t1 }
//! ```
//!
//! `_5` is now only ever an address, which frees its Cranelift variable to
//! hold the stack slot rather than the value — the same convention a struct
//! local already uses. Codegen needs no notion of a local that is secretly in
//! memory.
//!
//! Doing it here rather than in codegen is also what keeps the write path
//! honest. MIR has one place that knows how a statement touches a local
//! (`analysis::uses`) and every pass already depends on it; the Cranelift
//! builder has twenty-seven unmarked `def_var` calls, and missing one leaves
//! the variable's memory stale while the SSA copy moves on — a silently wrong
//! answer, which is the failure mode that let #1038 hide in the first place.
//!
//! Runs before SSA construction, so no phi is ever built over a variable that
//! lives in memory. That is what a mem2reg pass refuses to promote, reached
//! from the other side.

use std::collections::HashSet;

use crate::analysis::uses::{self, UseKind};
use crate::{
    LocalId, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt, MirStmtKind, MirType,
};

/// The locals a function addresses rather than holds, split by who owns the
/// storage.
#[derive(Debug, Default)]
pub struct AddrTaken {
    /// Scalars whose address is handed to a closure environment built here.
    /// This frame owns the storage, so each needs a stack slot — an aggregate
    /// has one already, which is why only scalars are listed.
    pub owned: HashSet<LocalId>,
    /// Locals whose address arrived *from* a closure environment (a by-ref
    /// `LoadCapture`), of any type. The storage belongs to the frame that
    /// created the closure, so these must get no slot of their own: a local
    /// listed in `stack_slot_map` while its variable points somewhere else is
    /// read through whichever of the two a given site happens to consult, and
    /// a word-sized struct came back from `|| { return s }` as the address
    /// instead of the value.
    pub borrowed: HashSet<LocalId>,
}

impl AddrTaken {
    pub fn contains(&self, id: LocalId) -> bool {
        self.owned.contains(&id) || self.borrowed.contains(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.owned.is_empty() && self.borrowed.is_empty()
    }
}

/// Which locals something wants the *address* of, before the rewrite runs.
///
/// This is the rewrite's own input. It says who needs memory, not who has it
/// yet — `analyze` is the post-rewrite question, and codegen asks that one.
pub fn wants_address(func: &MirFunction) -> AddrTaken {
    let mut found = AddrTaken::default();

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::ClosureCreate { captures, .. }
                | MirStmtKind::EnsureHookRegister { captures, .. } => {
                    for c in captures.iter().filter(|c| c.by_ref) {
                        found.owned.insert(c.local_id);
                    }
                }
                MirStmtKind::LoadCapture { dst, by_ref: true, .. } => {
                    found.borrowed.insert(*dst);
                }
                // Any scalar whose address is taken needs a stable one. `Ref` on
                // a scalar used to spill a copy and hand back the copy's
                // address, which is a lie the moment anything writes through it
                // — that is #899 as well as #1038.
                MirStmtKind::Assign { rvalue: MirRValue::Ref(id), .. } => {
                    if is_scalar(func, *id) {
                        found.owned.insert(*id);
                    }
                }
                _ => {}
            }
        }
    }

    // A closure body that captures one of its own by-ref captures names the
    // same local both ways. The pointer it already holds is the address to
    // pass on, so there is nothing for this frame to own.
    for id in &found.borrowed {
        found.owned.remove(id);
    }
    found
}

/// Which locals of `func` actually live in memory — what codegen needs to know
/// to type their variables and hand them slots.
///
/// A local qualifies when something wants its address *and* nothing writes it
/// as a value any more. The second half is what the rewrite establishes, so
/// asking for it here rather than assuming it means codegen reads the MIR in
/// front of it instead of trusting that a pass ran. MIR that never went through
/// `run` keeps its old meaning — `Ref` spills a copy — rather than being
/// miscompiled, which is what tests and other MIR producers depend on.
pub fn analyze(func: &MirFunction) -> AddrTaken {
    let wanted = wants_address(func);
    if wanted.is_empty() {
        return wanted;
    }
    let written = locals_written_as_values(func);
    AddrTaken {
        // Only a scalar needs a slot of its own here; an aggregate local
        // already has one and its variable already holds the address.
        owned: wanted
            .owned
            .difference(&written)
            .copied()
            .filter(|id| is_scalar(func, *id))
            .collect(),
        borrowed: wanted.borrowed.difference(&written).copied().collect(),
    }
}

/// Locals some statement still assigns a value to.
///
/// A by-ref `LoadCapture` is excluded: it establishes the local's address, and
/// that is the one write a memory-resident local keeps.
fn locals_written_as_values(func: &MirFunction) -> HashSet<LocalId> {
    let mut written = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if matches!(&stmt.kind, MirStmtKind::LoadCapture { by_ref: true, .. }) {
                continue;
            }
            if let Some(dst) = uses::stmt_def(stmt) {
                written.insert(dst);
            }
        }
    }
    written
}

/// A by-ref capture whose local codegen would not treat as memory-resident.
///
/// There is no safe way to compile that: the environment would hold the
/// scalar's value while the closure body loads through it as a pointer, which
/// is a wrong answer rather than a crash. It means a pass moved a
/// `ClosureCreate` into a function the rewrite had already finished with —
/// `transform::inline` declines such functions for exactly this reason.
pub fn unprepared_capture(func: &MirFunction) -> Option<LocalId> {
    let wanted = wants_address(func);
    if wanted.is_empty() {
        return None;
    }
    let written = locals_written_as_values(func);
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::ClosureCreate { captures, .. } = &stmt.kind else { continue };
            for c in captures.iter().filter(|c| c.by_ref) {
                if wanted.owned.contains(&c.local_id) && written.contains(&c.local_id) {
                    return Some(c.local_id);
                }
            }
        }
    }
    None
}

/// True when the local is held in an SSA value rather than addressed.
///
/// `passed_by_address` is the shared answer to "does the variable hold the
/// data or a pointer to it" — its own doc records what went wrong when callers
/// spelled out their own lists instead.
fn is_scalar(func: &MirFunction, id: LocalId) -> bool {
    local_ty(func, id).is_some_and(|ty| !ty.passed_by_address() && *ty != MirType::Void)
}

fn local_ty(func: &MirFunction, id: LocalId) -> Option<&MirType> {
    func.locals
        .iter()
        .chain(func.params.iter())
        .find(|l| l.id == id)
        .map(|l| &l.ty)
}

/// Prepare every function: withdraw the borrow from closures that outlive the
/// frame they were built in, then make the remaining captured scalars
/// memory-resident.
///
/// The withdrawal has to happen across the whole program, because the flag
/// lives in two places that must agree — the `ClosureCreate` that builds the
/// environment and the `LoadCapture`s in the closure's own function, which is
/// a separate `MirFunction`.
pub fn run_all(fns: &mut [MirFunction]) {
    let by_value = closures_outliving_their_frame(fns);
    if !by_value.is_empty() {
        withdraw_borrows(fns, &by_value);
    }
    for func in fns.iter_mut() {
        run(func);
    }
}

/// Closure functions whose environment may outlive the frame that built it.
///
/// Borrowing is only safe while the frame the addresses point into is alive.
/// `func adder(k: i32) -> func(i32) -> i32 { return |x| x + k }` hands its
/// environment to the caller, so `k`'s address is stale the moment `adder`
/// returns — those captures have to be copies.
///
/// Conservative: a closure the function returns, stores through a pointer, puts
/// in an array, or boxes as a trait object loses its borrow. One passed to a
/// call keeps it, which is what the sequence protocol needs — `seq(|x| { … })`
/// hands the closure to something that only calls it, and that is the shape
/// every `for` loop over a sequence lowers to.
///
/// A closure that escapes is separately unsound today: lowering marks only
/// `own` closures as heap, so a returned scope-limited closure is stack
/// allocated and dangles whether its captures are copies or addresses (#1045).
/// This keeps such a program behaving as it did rather than making it worse.
fn closures_outliving_their_frame(fns: &[MirFunction]) -> HashSet<String> {
    let mut escaping = HashSet::new();

    for func in fns {
        // Which closure function each local holds, following plain copies —
        // lowering routinely assigns the `ClosureCreate` result on before
        // returning it, and reading only the original destination missed that.
        let mut holds: Vec<(LocalId, String)> = Vec::new();
        let mut changed = true;
        while changed {
            changed = false;
            for block in &func.blocks {
                for stmt in &block.statements {
                    let (dst, name) = match &stmt.kind {
                        MirStmtKind::ClosureCreate { dst, func_name, captures, .. }
                            if captures.iter().any(|c| c.by_ref) =>
                        {
                            (*dst, func_name.clone())
                        }
                        MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                            match holds.iter().find(|(id, _)| id == src) {
                                Some((_, name)) => (*dst, name.clone()),
                                None => continue,
                            }
                        }
                        _ => continue,
                    };
                    if !holds.iter().any(|(id, n)| *id == dst && *n == name) {
                        holds.push((dst, name));
                        changed = true;
                    }
                }
            }
        }
        if holds.is_empty() {
            continue;
        }
        let held_by = |id: LocalId| holds.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone());

        for block in &func.blocks {
            for stmt in &block.statements {
                let leaked = match &stmt.kind {
                    MirStmtKind::Store { value: MirOperand::Local(id), .. }
                    | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                    | MirStmtKind::TraitBox { value: MirOperand::Local(id), .. } => held_by(*id),
                    _ => None,
                };
                escaping.extend(leaked);
            }
            let returned = match &block.terminator.kind {
                crate::MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
                | crate::MirTerminatorKind::CleanupReturn {
                    value: Some(MirOperand::Local(id)), ..
                } => held_by(*id),
                _ => None,
            };
            escaping.extend(returned);
        }
    }

    escaping
}

/// Turn the named closures' captures back into copies, on both sides.
fn withdraw_borrows(fns: &mut [MirFunction], by_value: &HashSet<String>) {
    // The environment layout changes with the flag — a by-ref slot is a word,
    // a by-value one is as wide as the type — so the offsets have to be rebuilt
    // to match on both sides. Rebuilding from the local's own MIR type is what
    // lowering did.
    let mut layouts: Vec<(String, Vec<u32>)> = Vec::new();

    for func in fns.iter_mut() {
        let types: Vec<(LocalId, MirType)> = func
            .locals
            .iter()
            .chain(func.params.iter())
            .map(|l| (l.id, l.ty.clone()))
            .collect();
        for block in &mut func.blocks {
            for stmt in &mut block.statements {
                let MirStmtKind::ClosureCreate { func_name, captures, .. } = &mut stmt.kind else {
                    continue;
                };
                if !by_value.contains(func_name) {
                    continue;
                }
                let mut offset = 0u32;
                let mut offsets = Vec::with_capacity(captures.len());
                for c in captures.iter_mut() {
                    let size = types
                        .iter()
                        .find(|(id, _)| *id == c.local_id)
                        .map_or(8, |(_, ty)| ty.size());
                    c.by_ref = false;
                    c.size = size;
                    c.offset = (offset + 7) & !7;
                    offset = c.offset + size;
                    offsets.push(c.offset);
                }
                layouts.push((func_name.clone(), offsets));
            }
        }
    }

    for func in fns.iter_mut() {
        let Some((_, offsets)) = layouts.iter().find(|(n, _)| *n == func.name) else {
            continue;
        };
        // The body emits one `LoadCapture` per capture, in capture order, at the
        // top of the entry block — a nested closure has a function of its own —
        // so the nth one here is the nth capture there. Counting only the by-ref
        // ones would misalign the moment a closure mixed the two.
        let mut nth = 0usize;
        for block in &mut func.blocks {
            for stmt in &mut block.statements {
                if let MirStmtKind::LoadCapture { offset, by_ref, .. } = &mut stmt.kind {
                    if let Some(o) = offsets.get(nth) {
                        *offset = *o;
                    }
                    *by_ref = false;
                    nth += 1;
                }
            }
        }
    }
}

/// Rewrite every read of an address-taken scalar into a load and every write
/// into a store. Returns the locals that were made memory-resident.
pub fn run(func: &mut MirFunction) -> AddrTaken {
    let taken = wants_address(func);
    if taken.is_empty() {
        return taken;
    }

    let mut next_local = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| l.id.0 + 1)
        .max()
        .unwrap_or(0);
    let mut fresh: Vec<MirLocal> = Vec::new();

    // A fresh temp of the same type, to carry the loaded or about-to-be-stored
    // value. Named after its source so `--dump-mir` stays readable.
    let mut new_temp = |ty: &MirType, of: LocalId, fresh: &mut Vec<MirLocal>| {
        let id = LocalId(next_local);
        next_local += 1;
        fresh.push(MirLocal {
            id,
            name: Some(format!("__mem{}_{}", of.0, id.0)),
            ty: ty.clone(),
            is_param: false,
        });
        id
    };

    // Every address-taken local has to stop being written as a value, whatever
    // its type. Otherwise SSA versions it, each version gets storage of its
    // own, and the capture points at one while the write lands in another —
    // which is how `mut hit: T? = none` written from inside a closure stayed
    // `none` while the same write to an `i32` worked.
    //
    // Reads are the part that differs by type. A scalar's variable holds its
    // value, so a read becomes a load. An aggregate's variable holds its
    // address already and every read of one goes through that address, so
    // reads are left as they are.
    let types: Vec<(LocalId, MirType)> = func
        .locals
        .iter()
        .chain(func.params.iter())
        .filter(|l| taken.contains(l.id))
        .filter(|l| l.ty != MirType::Void)
        .map(|l| (l.id, l.ty.clone()))
        .collect();
    if types.is_empty() {
        return taken;
    }
    let rewritable: HashSet<LocalId> = types.iter().map(|(id, _)| *id).collect();
    let load_on_read: HashSet<LocalId> = types
        .iter()
        .filter(|(_, ty)| !ty.passed_by_address())
        .map(|(id, _)| *id)
        .collect();
    let ty_of = |id: LocalId| -> MirType {
        types.iter().find(|(i, _)| *i == id).map(|(_, t)| t.clone()).unwrap()
    };

    for block in &mut func.blocks {
        let mut out: Vec<MirStmt> = Vec::with_capacity(block.statements.len());

        for mut stmt in block.statements.drain(..) {
            // The by-ref LoadCapture is what *establishes* the address. Leave
            // it alone: rewriting its destination into a store would store the
            // pointer through itself.
            if matches!(&stmt.kind, MirStmtKind::LoadCapture { dst, by_ref: true, .. }
                if rewritable.contains(dst))
            {
                out.push(stmt);
                continue;
            }

            // Reads first, so a statement that both reads and writes the same
            // local loads the old value before storing the new one.
            let span = stmt.span;
            uses::visit_stmt_use_locals_mut(&mut stmt, &mut |id, kind| {
                if kind == UseKind::AddressOf || !load_on_read.contains(id) {
                    return;
                }
                let ty = ty_of(*id);
                let tmp = new_temp(&ty, *id, &mut fresh);
                out.push(MirStmt::new(
                    MirStmtKind::Assign {
                        dst: tmp,
                        rvalue: MirRValue::Deref(MirOperand::Local(*id)),
                    },
                    span,
                ));
                *id = tmp;
            });

            // Then the write, redirected into a temp the store reads back.
            match uses::stmt_def(&stmt) {
                Some(dst) if rewritable.contains(&dst) => {
                    let ty = ty_of(dst);
                    let tmp = new_temp(&ty, dst, &mut fresh);
                    redirect_def(&mut stmt, tmp);
                    out.push(stmt);
                    out.push(MirStmt::new(
                        MirStmtKind::Store {
                            addr: dst,
                            offset: 0,
                            value: MirOperand::Local(tmp),
                            // The slot is a word, but a narrower scalar has to
                            // be stored at its own width or the load reads
                            // bytes the store never wrote — an f32 stored as a
                            // promoted f64 reads back as noise.
                            store_size: Some(ty.size()),
                        },
                        span,
                    ));
                }
                _ => out.push(stmt),
            }
        }

        block.statements = out;

        // A terminator reads its operand from the block it ends, so its loads
        // go at the end of that block's statements.
        let mut loads: Vec<MirStmt> = Vec::new();
        let span = block.terminator.span;
        uses::visit_terminator_use_locals_mut(&mut block.terminator, &mut |id, kind| {
            if kind == UseKind::AddressOf || !load_on_read.contains(id) {
                return;
            }
            let ty = ty_of(*id);
            let tmp = new_temp(&ty, *id, &mut fresh);
            loads.push(MirStmt::new(
                MirStmtKind::Assign {
                    dst: tmp,
                    rvalue: MirRValue::Deref(MirOperand::Local(*id)),
                },
                span,
            ));
            *id = tmp;
        });
        block.statements.extend(loads);
    }

    func.locals.extend(fresh);
    taken
}

/// Point a statement's write at a different local.
///
/// The destination positions are the ones `uses::stmt_def` reports, kept in
/// step with it by construction — both match on the same statement kinds.
fn redirect_def(stmt: &mut MirStmt, to: LocalId) {
    match &mut stmt.kind {
        MirStmtKind::Assign { dst, .. }
        | MirStmtKind::Phi { dst, .. }
        | MirStmtKind::PoolCheckedAccess { dst, .. }
        | MirStmtKind::ClosureCreate { dst, .. }
        | MirStmtKind::LoadCapture { dst, .. }
        | MirStmtKind::ResourceRegister { dst, .. }
        | MirStmtKind::GlobalRef { dst, .. }
        | MirStmtKind::TraitBox { dst, .. } => *dst = to,
        MirStmtKind::Call { dst: Some(d), .. }
        | MirStmtKind::ClosureCall { dst: Some(d), .. }
        | MirStmtKind::TraitCall { dst: Some(d), .. } => *d = to,
        _ => {}
    }
}

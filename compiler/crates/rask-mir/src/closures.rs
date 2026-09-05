// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure optimization pass — escape analysis, ownership transfer, and drop insertion.
//!
//! Entry points: `optimize_all_closures(fns)` decides stack vs heap, and
//! `insert_all_closure_drops(fns)` frees what each frame is left holding. They
//! run either side of inlining — see `insert_all_closure_drops` for why.
//!
//! Per function, using cross-function callee escape info:
//! 1. Identifies closure locals (destinations of ClosureCreate)
//! 2. Determines which closures escape — passed to unknown or escaping callees,
//!    stored to memory, or returned. Borrow-only callees (param doesn't escape)
//!    don't count as escaping → closure stays on the stack.
//! 3. Downgrades non-escaping closures to stack allocation (heap: false)
//! 4. Identifies transferred closures (escaping Call arg or Store, no local use)
//! 5. Inserts ClosureDrop before Return terminators for heap-allocated
//!    closures that aren't returned and weren't transferred

use std::collections::{HashMap, HashSet};

use crate::analysis::uses;
use crate::{LocalId, MirFunction, MirOperand, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind};

/// Optimize closures across all functions with cross-function analysis.
///
/// Builds a callee escape map: for each function, which parameters escape?
/// This lets the per-function pass distinguish borrow (callee only calls the
/// closure locally → stack-allocate) from ownership transfer (callee stores/
/// returns/forwards → heap-allocate, suppress caller drop).
///
/// Unknown callees (runtime functions, external) are assumed to take ownership.
pub fn optimize_all_closures(fns: &mut [MirFunction]) {
    let callee_escapes = build_callee_escape_map(fns);

    for func in fns.iter_mut() {
        decide_allocation(func, &callee_escapes);
    }
}

/// Free the heap closures each frame is left holding.
///
/// Split out of `optimize_all_closures` and run **after** inlining, which is
/// the whole reason a chain leaked. `v.filter(p)` is three small stdlib
/// functions, so the inliner takes all of them — and it copies their
/// `ClosureCreate`s into the caller *after* the ownership analysis has already
/// run. Every environment in an inlined chain therefore reached codegen having
/// been analysed only in a frame it no longer lives in, and `main` was never
/// looked at at all: no owner, no drop, three allocations a call (#1045).
///
/// The allocation decision above still runs before inlining and has to — it
/// answers "does this outlive its frame", which is a question about the frame
/// the closure was *written* in. The flag rides along when the statement is
/// copied. Ownership is the opposite kind of question: it is about the frame
/// that ends up holding the thing, so it can only be asked once inlining has
/// settled which frame that is.
pub fn insert_all_closure_drops(fns: &mut [MirFunction]) {
    let callee_escapes = build_callee_escape_map(fns);

    // A function that hands a heap closure back makes its caller the owner —
    // `let tick = counter()` is the caller receiving a block nobody else will
    // free. Which functions those are can only be read off the finished
    // allocation decisions, so it waits for every function to have one.
    let hands_back = functions_handing_back_a_closure(fns);

    for func in fns.iter_mut() {
        insert_drops(func, &callee_escapes, &hands_back);
    }
}

/// Heap exactly when the closure outlives this frame.
///
/// This used to only downgrade. Lowering picks the initial answer from `own`,
/// so a scope-limited closure that escaped anyway — by being returned, which is
/// every sequence source and every adapter — kept a stack environment in a
/// frame that had already been popped. It read back whatever was left there:
/// the right answer in a small program, a wrong one or a segfault in a real
/// one (#1045).
fn decide_allocation(func: &mut MirFunction, callee_escapes: &HashMap<String, Vec<bool>>) {
    let created = created_closures(func);
    if created.is_empty() {
        return;
    }
    let escaping = find_escaping_closures(func, &created, callee_escapes);

    for block in &mut func.blocks {
        for stmt in &mut block.statements {
            if let MirStmtKind::ClosureCreate { dst, heap, .. } = &mut stmt.kind {
                *heap = escaping.contains(dst);
            }
        }
    }
}

/// Free the heap closures this frame is left holding.
///
/// Two ways to be left holding one: build it here, or take one back from a
/// call. Both are owned values with a single owner like anything else in Rask,
/// so the frame that still has one when it returns is the frame that frees it
/// (mem.ownership/O1). A closure it handed on — returned, stored, or passed to
/// something that keeps it — belongs to whoever took it.
fn insert_drops(
    func: &mut MirFunction,
    callee_escapes: &HashMap<String, Vec<bool>>,
    hands_back: &HashSet<String>,
) {
    let mut owned: HashMap<LocalId, bool> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::ClosureCreate { dst, heap: true, .. } => {
                    owned.insert(*dst, true);
                }
                MirStmtKind::Call { dst: Some(dst), func: callee, .. }
                    if hands_back.contains(&callee.name) =>
                {
                    owned.insert(*dst, true);
                }
                _ => {}
            }
        }
    }

    if owned.is_empty() {
        return;
    }

    let aliases = closure_aliases(func, &owned);
    let transferred = find_transferred_closures(func, &owned, callee_escapes);
    let heap_closures: HashSet<LocalId> = owned
        .keys()
        .filter(|id| !transferred.contains(id))
        .copied()
        .collect();

    if heap_closures.is_empty() {
        return;
    }

    // The chain's inner environments are excluded from `heap_closures` above —
    // captured by an escaping closure, so not this frame's. That is right, and
    // it stopped there: the owner was named and never asked to free them, so a
    // two-adapter chain leaked four allocations a call (#1045).
    let owned_by = captured_environments(func, &owned, &aliases);
    insert_closure_drops(func, &heap_closures, &owned_by, &aliases);
}

/// The `ClosureCreate` destinations in a function.
fn created_closures(func: &MirFunction) -> HashMap<LocalId, bool> {
    let mut found = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::ClosureCreate { dst, heap, .. } = &stmt.kind {
                found.insert(*dst, *heap);
            }
        }
    }
    found
}

/// Functions whose return value is a heap closure the caller now owns.
fn functions_handing_back_a_closure(fns: &[MirFunction]) -> HashSet<String> {
    let mut names = HashSet::new();
    for func in fns {
        let heap: HashMap<LocalId, bool> = created_closures(func)
            .into_iter()
            .filter(|(_, heap)| *heap)
            .collect();
        if heap.is_empty() {
            continue;
        }
        let aliases = closure_aliases(func, &heap);
        for block in &func.blocks {
            let returned = match &block.terminator.kind {
                MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
                | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => *id,
                _ => continue,
            };
            if aliases.contains_key(&returned) {
                names.insert(func.name.clone());
            }
        }
    }
    names
}

/// Build a map of callee name → per-parameter escape info.
///
/// For each function, checks whether each parameter escapes (appears in
/// Call args, Store, or Return within the function body). A non-escaping
/// parameter means the function only uses it locally (e.g., via ClosureCall).
fn build_callee_escape_map(fns: &[MirFunction]) -> HashMap<String, Vec<bool>> {
    let mut map = HashMap::new();
    for func in fns {
        let escapes: Vec<bool> = func.params.iter()
            .map(|p| param_escapes_from(func, p.id))
            .collect();
        map.insert(func.name.clone(), escapes);
    }
    map
}

/// Check if a parameter escapes from its function.
///
/// A parameter "escapes" if it appears in a Call arg, Store value, or Return.
/// If it only appears in ClosureCall position, the function merely borrows it.
fn param_escapes_from(func: &MirFunction, param_id: LocalId) -> bool {
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                // A parameter captured by a closure leaves with it. This was
                // missing, and it is how a caller came to free a closure the
                // callee had handed on: every adapter captures the sequence it
                // wraps, so `Sequence_filter(self, pred)` reported that neither
                // parameter escaped, callers read "borrow", and the frame
                // dropped the source while the returned adapter still pointed
                // at it (#1051).
                //
                // Conservative on purpose — this map is built before allocation
                // is decided, so there is no "does the closure escape" to ask
                // yet. Erring toward escaping costs a leak; erring the other way
                // costs a use-after-free.
                MirStmtKind::ClosureCreate { captures, .. } => {
                    if captures.iter().any(|c| c.local_id == param_id) {
                        return true;
                    }
                }
                MirStmtKind::Call { args, .. } => {
                    if args.iter().any(|a| uses::operand_reads(a, param_id)) {
                        return true;
                    }
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. } if *id == param_id => {
                    return true;
                }
                _ => {}
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. }
                if *id == param_id => return true,
            _ => {}
        }
    }
    false
}

/// Scan all blocks to find closure locals that escape.
///
/// A closure escapes if it appears in:
/// - A Return/CleanupReturn terminator as the return value
/// - A Call arg where the callee is unknown or the param escapes from the callee
/// - A Store statement as the stored value
///
/// A closure passed to a known callee whose corresponding parameter doesn't
/// escape is NOT escaping — the callee merely borrows it.
/// Every local that names a closure, mapped to the closure it names.
///
/// `ClosureCreate` writes one local, and the analyses below matched on exactly
/// that one. A closure bound to a name and used later reaches its use through a
/// copy — `let f = own || …` lowers to `_12 = closure(…)` then `_13 = _12`, and
/// `spawn(_13)` was invisible to the escape check. The closure was downgraded to
/// a stack allocation and `spawn` then freed a stack address: `free(): invalid
/// pointer` (#1008).
///
/// Copies are followed to a fixpoint, so a chain of them resolves to the one
/// `ClosureCreate` at the root.
fn find_escaping_closures(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> HashSet<LocalId> {
    // Lowering routinely copies the `ClosureCreate` result on before returning
    // it, so reading only the original destination missed the escape. Every
    // alias answers for the closure it came from, and the answer is recorded
    // against that closure — `decide_allocation` looks up the `ClosureCreate`
    // destination, which is the origin, never a copy of it.
    let aliases = closure_aliases(func, closure_locals);
    let mut escaping: HashSet<LocalId> = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { func: callee, args, .. } => {
                    for (arg_idx, arg) in args.iter().enumerate() {
                        if let Some(id) = uses::operand_local(arg) {
                            if let Some(origin) = aliases.get(&id).copied() {
                                let is_borrow = callee_escapes.get(&callee.name)
                                    .and_then(|e| e.get(arg_idx))
                                    .map(|escapes| !escapes)
                                    .unwrap_or(false);

                                if !is_borrow {
                                    escaping.insert(origin);
                                }
                            }
                        }
                    }
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. } => {
                    if let Some(origin) = aliases.get(id).copied() {
                        escaping.insert(origin);
                    }
                }
                _ => {}
            }
        }

        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                if let Some(origin) = aliases.get(id).copied() {
                    escaping.insert(origin);
                }
            }
            _ => {}
        }
    }

    // An escaping closure takes its captures with it, so a capture that is
    // itself a closure has to outlive the frame too.
    //
    // Without this the outer closure went to the heap and the one it wraps
    // stayed on the stack, so what came back pointed into a dead frame. An
    // adapter chain returned from a function is the shape that finds it —
    //
    //     func chained(v: Vec<i32>) -> Sequence<i32> {
    //         let src: Sequence<i32> = v.as_sequence()
    //         return src.filter(|x| x > 1)
    //     }
    //
    // — where `src` is a local nothing else escapes, and calling the result
    // segfaulted (#1051). One level works and always did, which is why it went
    // unnoticed: a closure built directly over a parameter has no inner
    // environment to leave behind.
    //
    // A fixpoint rather than one pass: chains nest arbitrarily deep, and each
    // adapter captures the one before it.
    let captured_closures: Vec<(LocalId, Vec<LocalId>)> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::ClosureCreate { dst, captures, .. } => Some((
                *dst,
                captures.iter().map(|c| c.local_id).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect();

    loop {
        let mut grew = false;
        for (dst, captures) in &captured_closures {
            if !escaping.contains(dst) {
                continue;
            }
            for cap in captures {
                if let Some(inner) = aliases.get(cap).copied() {
                    grew |= escaping.insert(inner);
                }
            }
        }
        if !grew {
            break;
        }
    }

    escaping
}

/// Every local that holds one of this function's closures, mapped to the
/// `ClosureCreate` destination it came from — itself, for the original.
fn closure_aliases(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
) -> HashMap<LocalId, LocalId> {
    let mut holds: HashMap<LocalId, LocalId> = closure_locals.keys().map(|id| (*id, *id)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } =
                    &stmt.kind
                else {
                    continue;
                };
                if let Some(origin) = holds.get(src).copied() {
                    if holds.insert(*dst, origin) != Some(origin) {
                        changed = true;
                    }
                }
            }
        }
    }
    holds
}

/// Find closures whose ownership was transferred out of the function.
///
/// A closure is "transferred" if it's passed to a callee that forwards/stores
/// the parameter, or to an unknown callee (runtime function). Closures passed
/// to callees that only use the parameter locally (borrow) are NOT transferred.
///
/// Closures also used locally via ClosureCall are excluded — the caller still
/// needs them, so we assume borrow semantics and keep the drop.
fn find_transferred_closures(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> HashSet<LocalId> {
    let mut passed_or_stored = HashSet::new();
    let mut used_locally = HashSet::new();
    // Lowering copies a closure on before capturing it, so the capture names an
    // alias rather than the `ClosureCreate` destination this pass is keyed by.
    let aliases = closure_aliases(func, closure_locals);

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                // A closure captured by an *escaping* closure went with it: the
                // environment now holding it outlives this frame, so this frame
                // is not the one that frees it. `decide_allocation` has already
                // run, so `heap` here means exactly "escapes".
                //
                // `ClosureCreate` is neither a call nor a store, so nothing saw
                // the transfer and the frame dropped a closure the returned one
                // still pointed at. An adapter chain returned from a function is
                // the shape: `return src.filter(p)` emitted `closure_drop(src)`
                // immediately before `return`, and calling the result read freed
                // memory (#1051).
                MirStmtKind::ClosureCreate { heap: true, captures, .. } => {
                    for cap in captures {
                        if let Some(origin) = aliases.get(&cap.local_id).copied() {
                            passed_or_stored.insert(origin);
                        }
                    }
                }
                MirStmtKind::Call { func: callee, args, .. } => {
                    for (arg_idx, arg) in args.iter().enumerate() {
                        if let Some(id) = uses::operand_local(arg) {
                            if let Some(origin) = aliases.get(&id).copied() {
                                let is_borrow = callee_escapes.get(&callee.name)
                                    .and_then(|e| e.get(arg_idx))
                                    .map(|escapes| !escapes)
                                    .unwrap_or(false);

                                if !is_borrow {
                                    passed_or_stored.insert(origin);
                                }
                            }
                        }
                    }
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. } => {
                    if let Some(origin) = aliases.get(id).copied() {
                        passed_or_stored.insert(origin);
                    }
                }
                MirStmtKind::ClosureCall { closure, .. } => {
                    if let Some(origin) = aliases.get(closure).copied() {
                        used_locally.insert(origin);
                    }
                }
                _ => {}
            }
        }
    }

    passed_or_stored.difference(&used_locally).copied().collect()
}

/// How many environments captured each one.
///
/// This is the ownership test, and it is deliberately not "did a `let` name
/// it". A MIR local's name is not only a binding — after inlining, the callee's
/// parameter names land in the caller, so `self` and `f` from an inlined
/// adapter look exactly like a user's `let`. Counting capturers asks the
/// question directly instead.
///
/// Captured once: that capturer owns it, and frees it when it is itself freed.
/// Captured more than once, as in
///
/// ```text
/// let src = counter(1)
/// let a = src.map(f)
/// let b = src.map(g)
/// ```
///
/// no single environment owns `src`, and picking either would free it twice.
/// Those keep today's behaviour — this frame does not free them, so they leak
/// rather than double-free. The enclosing scope is the right owner there and
/// that is a separate change; erring toward a leak is the same call
/// `find_transferred_closures` already makes.
fn capture_counts(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
    aliases: &HashMap<LocalId, LocalId>,
) -> HashMap<LocalId, usize> {
    let mut counts: HashMap<LocalId, usize> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::ClosureCreate { dst, captures, heap: true, .. } = &stmt.kind else {
                continue;
            };
            let owner = aliases.get(dst).copied().unwrap_or(*dst);
            let mut seen_here: HashSet<LocalId> = HashSet::new();
            for cap in captures {
                let Some(inner) = aliases.get(&cap.local_id).copied() else { continue };
                if inner == owner || !closure_locals.contains_key(&inner) {
                    continue;
                }
                // One environment capturing the same thing at two offsets is
                // still one owner.
                if seen_here.insert(inner) {
                    *counts.entry(inner).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

/// What each environment captured that it therefore owns.
///
/// An adapter's environment holds the sequence it wraps and the closure it was
/// given — `closure[heap](Sequence_filter…, [_36@0, _37@8])` — and both are
/// environments of their own. `find_transferred_closures` already worked this
/// out and used it to say "not this frame's to free"; this is the other half of
/// the same fact, which nothing was asking for: they are the *owner's* to free.
///
/// Named captures are left out, per `named_closures`.
fn captured_environments(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
    aliases: &HashMap<LocalId, LocalId>,
) -> HashMap<LocalId, Vec<LocalId>> {
    let counts = capture_counts(func, closure_locals, aliases);
    let mut owned: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::ClosureCreate { dst, captures, heap: true, .. } = &stmt.kind else {
                continue;
            };
            let owner = aliases.get(dst).copied().unwrap_or(*dst);
            for cap in captures {
                let Some(inner) = aliases.get(&cap.local_id).copied() else { continue };
                if inner == owner || !closure_locals.contains_key(&inner) {
                    continue;
                }
                if counts.get(&inner).copied().unwrap_or(0) != 1 {
                    continue;
                }
                let slot = owned.entry(owner).or_default();
                if !slot.contains(&inner) {
                    slot.push(inner);
                }
            }
        }
    }
    owned
}

/// Everything `roots` transitively owns, the roots included, innermost first.
///
/// Innermost first because freeing an environment releases its block, and the
/// inner ones are reached through what that block holds. They are separate MIR
/// locals so the order is not strictly required, but emitting the other way
/// round is a use-after-free waiting for the first person who changes how a
/// capture is read.
fn expand_owned(roots: &[LocalId], owned: &HashMap<LocalId, Vec<LocalId>>) -> Vec<LocalId> {
    let mut out: Vec<LocalId> = Vec::new();
    let mut seen: HashSet<LocalId> = HashSet::new();
    fn walk(
        id: LocalId,
        owned: &HashMap<LocalId, Vec<LocalId>>,
        seen: &mut HashSet<LocalId>,
        out: &mut Vec<LocalId>,
    ) {
        if !seen.insert(id) {
            return;
        }
        for inner in owned.get(&id).map(|v| v.as_slice()).unwrap_or(&[]) {
            walk(*inner, owned, seen, out);
        }
        out.push(id);
    }
    for root in roots {
        walk(*root, owned, &mut seen, &mut out);
    }
    out
}

/// Insert ClosureDrop statements before Return terminators and loop back-edges
/// for heap-allocated closures that aren't the return value on that path.
fn insert_closure_drops(
    func: &mut MirFunction,
    heap_closures: &HashSet<LocalId>,
    owned_by: &HashMap<LocalId, Vec<LocalId>>,
    aliases: &HashMap<LocalId, LocalId>,
) {
    // Which block each owned closure arrives in — built here, made there, or
    // handed back by a call.
    //
    // This used to record `ClosureCreate` destinations only, and a closure that
    // came from a *call* — the whole point of
    // `functions_handing_back_a_closure` — had no entry. The back-edge filter
    // read that absence as "not made in the loop" and skipped it, so a loop
    // calling a function that returns a closure freed exactly one environment:
    // the one live at the return. Everything else leaked, one allocation per
    // iteration (#1045).
    let mut closure_block: HashMap<LocalId, usize> = HashMap::new();
    for (idx, block) in func.blocks.iter().enumerate() {
        for stmt in &block.statements {
            let dst = match &stmt.kind {
                MirStmtKind::ClosureCreate { dst, .. } => Some(*dst),
                MirStmtKind::Call { dst: Some(dst), .. } => Some(*dst),
                _ => None,
            };
            if let Some(dst) = dst.filter(|d| heap_closures.contains(d)) {
                closure_block.insert(dst, idx);
            }
        }
    }

    // Placement is decided by dominance, the same way `container_drop` decides
    // it, and for the same reason: a closure made on one path can't be freed on
    // another, where it was never made.
    //
    // The return case used to drop *every* owned closure at *every* return,
    // filtered only by "isn't the value being returned". That was survivable
    // while the back-edge case couldn't see closures received from a call —
    // one drop fired, and it happened to be the live one. Making the back-edge
    // see them turned it into a double free: the loop body's closure was freed
    // at the back-edge and again on the way out.
    let dom = crate::analysis::dominators::DominatorTree::build(func);
    let mut drops_to_insert: Vec<(usize, Vec<LocalId>)> = Vec::new();

    for (block_idx, block) in func.blocks.iter().enumerate() {
        let mut back_edge_drops = |target: &crate::BlockId, out: &mut Vec<(usize, Vec<LocalId>)>| {
            if !dom.dominates(*target, block.id) {
                return;
            }
            let to_drop: Vec<LocalId> = heap_closures
                .iter()
                .filter(|id| {
                    closure_block.get(id).is_some_and(|&cidx| {
                        let def = func.blocks[cidx].id;
                        dom.dominates(*target, def) && dom.dominates(def, block.id)
                    })
                })
                .copied()
                .collect();
            if !to_drop.is_empty() {
                out.push((block_idx, to_drop));
            }
        };

        match &block.terminator.kind {
            // Return: drop what this path actually made, minus the value going
            // back to the caller.
            MirTerminatorKind::Return { value } | MirTerminatorKind::CleanupReturn { value, .. } => {
                // Through the alias, not the bare local. Lowering copies a
                // closure before returning it — `_5 = _8; return _5` — so
                // comparing the returned name against the `ClosureCreate`
                // destination never matched, and the frame freed the
                // environment it was handing back. `|x| { return upto(x) }`
                // as a `flat_map` callback segfaulted: every element built a
                // sequence and freed it on the way out.
                let returned_local = match value {
                    Some(MirOperand::Local(id)) => {
                        Some(aliases.get(id).copied().unwrap_or(*id))
                    }
                    _ => None,
                };
                let to_drop: Vec<LocalId> = heap_closures
                    .iter()
                    .filter(|id| Some(**id) != returned_local)
                    .filter(|id| {
                        closure_block.get(id).is_some_and(|&cidx| {
                            dom.dominates(func.blocks[cidx].id, block.id)
                        })
                    })
                    .copied()
                    .collect();
                if !to_drop.is_empty() {
                    drops_to_insert.push((block_idx, to_drop));
                }
            }

            MirTerminatorKind::Goto { target } => back_edge_drops(target, &mut drops_to_insert),
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                let mut out = Vec::new();
                back_edge_drops(then_block, &mut out);
                back_edge_drops(else_block, &mut out);
                drops_to_insert.extend(out);
            }

            _ => {}
        }
    }

    for (block_idx, locals) in drops_to_insert {
        for local_id in expand_owned(&locals, owned_by) {
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::ClosureDrop {
                closure: local_id,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, MirBlock, MirConst, MirLocal, MirType};
    use crate::operand::FunctionRef;
    use crate::MirTerminatorKind;

    /// Both halves, in pipeline order.
    ///
    /// The real pipeline runs allocation before inlining and drop insertion
    /// after it, because an inlined chain's environments land in a frame the
    /// first pass never saw (#1045). A test that called only the first half
    /// would assert on drops that nothing had inserted yet.
    fn run_closure_passes(fns: &mut Vec<MirFunction>) {
        optimize_all_closures(fns);
        insert_all_closure_drops(fns);
    }

    fn temp(id: u32, ty: MirType) -> MirLocal {
        MirLocal { id: LocalId(id), name: None, ty, is_param: false }
    }

    fn param(id: u32, ty: MirType) -> MirLocal {
        MirLocal { id: LocalId(id), name: None, ty, is_param: true }
    }

    fn block(id: u32, stmts: Vec<MirStmt>, term: MirTerminator) -> MirBlock {
        MirBlock { id: BlockId(id), statements: stmts, terminator: term }
    }

    fn ret(val: Option<MirOperand>) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: val })
    }

    fn get_heap(func: &MirFunction) -> bool {
        func.blocks[0].statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap()
    }

    fn has_drop(func: &MirFunction) -> bool {
        func.blocks[0].statements.iter().any(|s| matches!(s.kind, MirStmtKind::ClosureDrop { .. }))
    }

    #[test]
    fn local_only_closure_gets_stack() {
        // Closure used only in ClosureCall → stack, no drop
        let func = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let mut fns = vec![func];
        run_closure_passes(&mut fns);
        let func = &fns[0];

        assert!(!get_heap(func), "non-escaping closure should be stack-allocated");
        assert!(!has_drop(func), "stack closure should not have drop");
    }

    #[test]
    fn returned_closure_stays_heap() {
        let mut fns = vec![MirFunction {
            name: "make".to_string(),
            params: vec![],
            ret_ty: MirType::Ptr,
            locals: vec![temp(0, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "make__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                ], ret(Some(MirOperand::Local(LocalId(0))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        assert!(get_heap(&fns[0]), "returned closure must stay heap");
        assert!(!has_drop(&fns[0]), "returned closure should not be dropped");
    }

    #[test]
    fn unknown_callee_assumes_transfer() {
        // Closure passed to spawn (not in fn set) → heap, no drop
        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![temp(0, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("spawn".to_string()),
                        args: vec![MirOperand::Local(LocalId(0))],
                    }),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        assert!(get_heap(&fns[0]), "closure to unknown callee must be heap");
        assert!(!has_drop(&fns[0]), "ownership transferred to unknown callee");
    }

    #[test]
    fn borrow_callee_gets_stack_and_no_drop() {
        // apply() only does ClosureCall on its param → borrow.
        // Closure doesn't escape, gets stack-allocated. No drop needed.
        let apply_fn = MirFunction {
            name: "apply".to_string(),
            params: vec![param(0, MirType::Ptr)],
            ret_ty: MirType::I64,
            locals: vec![param(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let caller_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(LocalId(1)),
                        func: FunctionRef::internal("apply".to_string()),
                        args: vec![MirOperand::Local(LocalId(0))],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let mut fns = vec![apply_fn, caller_fn];
        run_closure_passes(&mut fns);
        let main = fns.iter().find(|f| f.name == "main").unwrap();

        assert!(!get_heap(main), "closure to borrow-only callee should be stack");
        assert!(!has_drop(main), "stack closure needs no drop");
    }

    #[test]
    fn escaping_callee_gets_heap_and_no_drop() {
        // store_it() stores the param → escapes. Heap, ownership transferred, no drop.
        let store_fn = MirFunction {
            name: "store_it".to_string(),
            params: vec![param(0, MirType::Ptr)],
            ret_ty: MirType::Void,
            locals: vec![param(0, MirType::Ptr), temp(1, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::Store {
                        addr: LocalId(1),
                        offset: 0,
                        value: MirOperand::Local(LocalId(0)),
                        store_size: None,
                    }),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let caller_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![temp(0, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("store_it".to_string()),
                        args: vec![MirOperand::Local(LocalId(0))],
                    }),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let mut fns = vec![store_fn, caller_fn];
        run_closure_passes(&mut fns);
        let main = fns.iter().find(|f| f.name == "main").unwrap();

        assert!(get_heap(main), "closure to escaping callee must be heap");
        assert!(!has_drop(main), "ownership transferred — no drop");
    }

    #[test]
    fn unknown_callee_plus_local_use_gets_drop() {
        // Closure passed to unknown `run` AND used via ClosureCall.
        // Unknown → escaping → heap. Also used locally → not transferred. Drop inserted.
        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("run".to_string()),
                        args: vec![MirOperand::Local(LocalId(0))],
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        assert!(get_heap(&fns[0]), "unknown callee forces heap");
        assert!(has_drop(&fns[0]), "local use prevents transfer — drop needed");
    }

    // ═══════════════════════════════════════════════════════════
    // Edge cases: nested closures
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn nested_closures_both_local_only() {
        // Outer closure and inner closure, both only used via ClosureCall.
        // Both should be downgraded to stack, no drops.
        //
        //   f__closure_0(env) -> i64 {
        //     _1 = ClosureCreate[heap] { func: "f__closure_1" }   // inner
        //     _2 = ClosureCall(_1)
        //     return _2
        //   }
        //   f() -> i64 {
        //     _0 = ClosureCreate[heap] { func: "f__closure_0" }   // outer
        //     _1 = ClosureCall(_0)
        //     return _1
        //   }

        let outer_closure = MirFunction {
            name: "f__closure_0".to_string(),
            params: vec![param(0, MirType::Ptr)],
            ret_ty: MirType::I64,
            locals: vec![
                param(0, MirType::Ptr),
                temp(1, MirType::Ptr),   // inner closure
                temp(2, MirType::I64),   // call result
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_1".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(2))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let f_fn = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let mut fns = vec![outer_closure, f_fn];
        run_closure_passes(&mut fns);

        let outer = &fns[0];
        let f = &fns[1];

        // Inner closure (in outer_closure body) → stack
        let inner_heap = outer.blocks[0].statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();
        assert!(!inner_heap, "inner closure should be stack-allocated");

        // Outer closure (in f) → stack
        assert!(!get_heap(f), "outer closure should be stack-allocated");
    }

    #[test]
    fn nested_closure_inner_returned_from_outer() {
        // Inner closure returned from outer → inner must stay heap.
        // Outer only used via ClosureCall → stack.
        //
        //   f__closure_0(env) -> ptr {
        //     _1 = ClosureCreate[heap] { func: "f__closure_1" }
        //     return _1   // ← inner escapes
        //   }
        //   f() -> i64 {
        //     _0 = ClosureCreate[heap] { func: "f__closure_0" }
        //     _1 = ClosureCall(_0)   // returns ptr to inner
        //     _2 = ClosureCall(_1)   // call the inner
        //     return _2
        //   }

        let outer_closure = MirFunction {
            name: "f__closure_0".to_string(),
            params: vec![param(0, MirType::Ptr)],
            ret_ty: MirType::Ptr,
            locals: vec![
                param(0, MirType::Ptr),
                temp(1, MirType::Ptr),   // inner closure
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_1".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),  // return inner
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let f_fn = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::Ptr),  // outer closure
                temp(1, MirType::Ptr),  // inner (from ClosureCall)
                temp(2, MirType::I64),  // final result
            ],
            blocks: vec![
                block(0, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    }),
                ], ret(Some(MirOperand::Local(LocalId(2))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };

        let mut fns = vec![outer_closure, f_fn];
        run_closure_passes(&mut fns);

        let outer = &fns[0];
        let f = &fns[1];

        // Inner closure returned from outer → must stay heap
        let inner_heap = outer.blocks[0].statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();
        assert!(inner_heap, "inner closure returned from outer must stay heap");

        // Outer closure only used via ClosureCall → stack
        assert!(!get_heap(f), "outer closure (only ClosureCall) should be stack");
    }

    // ═══════════════════════════════════════════════════════════
    // Edge cases: closures in loops
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn closure_in_loop_body_local_only() {
        // Closure created in a loop body, only used via ClosureCall.
        // Should be stack-allocated (no leak concern with stack).
        //
        //   f() -> i64 {
        //     block0: _0 = 0; goto block1
        //     block1: branch(_0 < 10, block2, block3)
        //     block2:
        //       _1 = ClosureCreate[heap] { captures: [] }
        //       _2 = ClosureCall(_1)
        //       _0 = _0 + 1
        //       goto block1
        //     block3: return _0
        //   }

        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![
                temp(0, MirType::I64),  // counter
                temp(1, MirType::Ptr),  // closure
                temp(2, MirType::I64),  // call result
            ],
            blocks: vec![
                block(0, vec![], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) })),
                block(1, vec![], MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(LocalId(0)),
                    then_block: BlockId(2),
                    else_block: BlockId(3),
                })),
                block(2, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    }),
                ], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) })),
                block(3, vec![], ret(Some(MirOperand::Local(LocalId(0))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        // Closure only used in ClosureCall → stack (safe even in loop)
        let loop_block = &fns[0].blocks[2];
        let heap = loop_block.statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();
        assert!(!heap, "loop-body closure with only local use should be stack");
    }

    #[test]
    fn closure_in_loop_body_transferred() {
        // Closure in loop body passed to unknown callee (e.g., register_callback).
        // Must be heap-allocated. Ownership transferred each iteration → no drop.
        //
        //   block2:
        //     _1 = ClosureCreate[heap] { captures: [] }
        //     Call(register, [_1])
        //     goto block1

        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                temp(0, MirType::I64),
                temp(1, MirType::Ptr),
            ],
            blocks: vec![
                block(0, vec![], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) })),
                block(1, vec![], MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(LocalId(0)),
                    then_block: BlockId(2),
                    else_block: BlockId(3),
                })),
                block(2, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("register".to_string()),
                        args: vec![MirOperand::Local(LocalId(1))],
                    }),
                ], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) })),
                block(3, vec![], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        let loop_block = &fns[0].blocks[2];
        let heap = loop_block.statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();
        assert!(heap, "closure passed to unknown callee must stay heap");

        // Ownership transferred to register → no drop anywhere
        let any_drop = fns[0].blocks.iter()
            .flat_map(|b| &b.statements)
            .any(|s| matches!(s.kind, MirStmtKind::ClosureDrop { .. }));
        assert!(!any_drop, "ownership transferred — no drop needed");
    }

    // ═══════════════════════════════════════════════════════════
    // Edge cases: closures in match arms
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn closures_in_different_match_arms_local_only() {
        // Two closures created in different match arms, both only used
        // via ClosureCall. Both should be stack-allocated.
        //
        //   block0: switch(x, [(0, block1), (1, block2)], block3)
        //   block1: _1 = ClosureCreate; _2 = ClosureCall(_1); goto block3
        //   block2: _3 = ClosureCreate; _4 = ClosureCall(_3); goto block3
        //   block3: return 0

        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![param(0, MirType::I64)],
            ret_ty: MirType::I64,
            locals: vec![
                param(0, MirType::I64),
                temp(1, MirType::Ptr),   // closure in arm 1
                temp(2, MirType::I64),   // call result 1
                temp(3, MirType::Ptr),   // closure in arm 2
                temp(4, MirType::I64),   // call result 2
            ],
            blocks: vec![
                block(0, vec![], MirTerminator::dummy(MirTerminatorKind::Switch {
                    value: MirOperand::Local(LocalId(0)),
                    cases: vec![(0, BlockId(1)), (1, BlockId(2))],
                    default: BlockId(3),
                })),
                block(1, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    }),
                ], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(3) })),
                block(2, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(3),
                        func_name: "f__closure_1".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(4)),
                        closure: LocalId(3),
                        args: vec![],
                    }),
                ], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(3) })),
                block(3, vec![], ret(Some(MirOperand::Constant(MirConst::Int(0))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        // Both closures only used in ClosureCall → both stack
        let arm1_heap = fns[0].blocks[1].statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();
        let arm2_heap = fns[0].blocks[2].statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();

        assert!(!arm1_heap, "match arm 1 closure should be stack");
        assert!(!arm2_heap, "match arm 2 closure should be stack");
    }

    #[test]
    fn closure_in_match_arm_escaping() {
        // One match arm returns a closure, the other doesn't.
        // The returned closure must stay heap.
        //
        //   block0: branch(x, block1, block2)
        //   block1: _1 = ClosureCreate[heap]; return _1   ← escapes
        //   block2: return null_ptr

        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![param(0, MirType::I64)],
            ret_ty: MirType::Ptr,
            locals: vec![
                param(0, MirType::I64),
                temp(1, MirType::Ptr),  // closure
            ],
            blocks: vec![
                block(0, vec![], MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(LocalId(0)),
                    then_block: BlockId(1),
                    else_block: BlockId(2),
                })),
                block(1, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
                block(2, vec![],
                    ret(Some(MirOperand::Constant(MirConst::Int(0))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        let heap = fns[0].blocks[1].statements.iter().find_map(|s| {
            if let MirStmtKind::ClosureCreate { heap, .. } = &s.kind { Some(*heap) } else { None }
        }).unwrap();
        assert!(heap, "closure returned from match arm must stay heap");
    }

    // ═══════════════════════════════════════════════════════════
    // Loop back-edge drops
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn closure_in_loop_escaping_and_local_gets_back_edge_drop() {
        // Closure in loop body: passed to unknown callee AND used locally.
        // Must be heap. Not transferred (local use). Drop at back-edge.
        //
        //   block0: goto block1
        //   block1: branch(cond, block2, block3)
        //   block2:
        //     _1 = ClosureCreate[heap]
        //     Call(run, [_1])         ← unknown callee → escaping
        //     _2 = ClosureCall(_1)    ← local use → not transferred
        //     goto block1             ← back-edge: drop _1 here
        //   block3: return void      ← and *not* here: block2 doesn't dominate it

        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                temp(0, MirType::I64),
                temp(1, MirType::Ptr),
                temp(2, MirType::I64),
            ],
            blocks: vec![
                block(0, vec![], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) })),
                block(1, vec![], MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(LocalId(0)),
                    then_block: BlockId(2),
                    else_block: BlockId(3),
                })),
                block(2, vec![
                    MirStmt::dummy(MirStmtKind::ClosureCreate {
                        dst: LocalId(1),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("run".to_string()),
                        args: vec![MirOperand::Local(LocalId(1))],
                    }),
                    MirStmt::dummy(MirStmtKind::ClosureCall {
                        dst: Some(LocalId(2)),
                        closure: LocalId(1),
                        args: vec![],
                    }),
                ], MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) })),
                block(3, vec![], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        let loop_block = &fns[0].blocks[2];
        assert!(
            loop_block.statements.iter().any(|s| matches!(s.kind, MirStmtKind::ClosureDrop { .. })),
            "back-edge block should have ClosureDrop for leaked closure"
        );

        // And *not* at the return. Block 2 is the loop body; the exit path
        // 0 → 1 → 3 never runs it, so there is nothing to free there — and on a
        // path that did run it, the back-edge drop above already freed it. This
        // assertion used to demand the second drop, which was a double free
        // waiting for the back-edge case to start working (#1045).
        let exit_block = &fns[0].blocks[3];
        assert!(
            !exit_block.statements.iter().any(|s| matches!(s.kind, MirStmtKind::ClosureDrop { .. })),
            "return block must not drop a closure the loop body made"
        );
    }
}

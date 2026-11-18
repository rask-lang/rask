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
use crate::{LocalId, MirFunction, MirOperand, MirStmt, MirStmtKind, MirTerminatorKind};

/// Optimize closures across all functions with cross-function analysis.
///
/// Builds a callee escape map: for each function, which parameters escape?
/// This lets the per-function pass distinguish borrow (callee only calls the
/// closure locally → stack-allocate) from ownership transfer (callee stores/
/// returns/forwards → heap-allocate, suppress caller drop).
///
/// Unknown callees (runtime functions, external) are assumed to take ownership.
pub fn optimize_all_closures(fns: &mut [MirFunction]) {
    let callee_escapes = build_callee_escape_map(fns, CaptureRule::EscapingClosure);
    let kept = KeptThroughCalls::build(fns);
    let heap_slots = captures_holding_heap_closures(fns);
    let lent = LentCells::build(fns, heap_slots);

    for func in fns.iter_mut() {
        decide_allocation(func, &callee_escapes, &kept, &lent);
    }
}

/// Where a closure stored in a variable passes to another frame that may
/// replace it, so every closure stored there has to be a heap one: the other
/// frame releases the one it replaces.
struct LentCells {
    /// (closure body, offset) of a captured variable the body assigns to or
    /// hands on (`captures_holding_heap_closures`).
    slots: HashSet<(String, u32)>,
    /// (function, index) of each `mutate` parameter. `setit(mutate g)` lends
    /// `g` itself, and a body that assigns `g = || n` stores through it
    /// (`container_drop::release_before_param_stores`).
    mutate_params: HashSet<(String, usize)>,
}

impl LentCells {
    fn build(fns: &[MirFunction], slots: HashSet<(String, u32)>) -> Self {
        let mutate_params = fns
            .iter()
            .flat_map(|f| {
                f.param_modes
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| **m == rask_ast::ty::ParamMode::Mutate)
                    .map(|(i, _)| (f.name.clone(), i))
            })
            .collect();
        LentCells { slots, mutate_params }
    }
}

/// The environment slots, as (closure body, offset), whose captured closure
/// variable has to hold heap closures: the body assigns a closure to it, or
/// hands the closure in it on.
///
/// Such a variable trades closures across two frames: the creating frame put
/// one in it, the body replaces it and the replaced one is released. So every
/// closure that lands in it is a heap one, the first included. Left on the
/// stack, the first one was freed by the body as if it had been allocated, or
/// pointed into the body's frame after it returned.
///
/// Handed on is the same trade the other way: `fs.push(g)` in a loop body
/// gives the vector a reference of its own to what `g` holds
/// (`retain_loads_handed_on`), and a stack closure has nothing to count with.
/// Any call counts, since which calls keep an argument isn't settled yet when
/// this is asked.
///
/// A body that hands the variable on to a body that does either counts too:
/// nested loops write the outer function's variable from the inner body, and
/// the outer body only passes the address along.
fn captures_holding_heap_closures(fns: &[MirFunction]) -> HashSet<(String, u32)> {
    let mut out = HashSet::new();
    // (body, offset) the slot came in at, and (inner body, offset) it went to.
    let mut passed: Vec<((String, u32), (String, u32))> = Vec::new();
    for func in fns {
        let slots: HashMap<LocalId, u32> = func
            .blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|stmt| match &stmt.kind {
                MirStmtKind::LoadCapture { dst, offset, .. }
                    if matches!(func.local_ty(*dst), Some(crate::MirType::FuncPtr(_))) =>
                {
                    Some((*dst, *offset))
                }
                _ => None,
            })
            .collect();
        if slots.is_empty() {
            continue;
        }
        // Each slot's variable and the plain copies made of it.
        let mut holders: HashMap<LocalId, u32> = slots.clone();
        loop {
            let before = holders.len();
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                if let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
                    if let Some(offset) = holders.get(src).copied().filter(|_| !slots.contains_key(dst)) {
                        holders.entry(*dst).or_insert(offset);
                    }
                }
            }
            if holders.len() == before {
                break;
            }
        }
        let held = |op: &MirOperand| match op {
            MirOperand::Local(l) => holders.get(l).copied(),
            _ => None,
        };
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let handed: Vec<u32> = match &stmt.kind {
                MirStmtKind::Call { args, .. }
                | MirStmtKind::InterfaceCall { args, .. }
                | MirStmtKind::ClosureCall { args, .. } => args.iter().filter_map(held).collect(),
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::InterfaceBox { value, .. } => held(value).into_iter().collect(),
                MirStmtKind::ClosureCreate { captures, .. } => captures
                    .iter()
                    .filter(|c| !c.by_ref)
                    .filter_map(|c| holders.get(&c.local_id).copied())
                    .collect(),
                _ => Vec::new(),
            };
            out.extend(handed.into_iter().map(|offset| (func.name.clone(), offset)));
            // `g = h` with both captured: `h`'s closure now sits in a variable
            // the body writes, so it has to be a heap one as well.
            if let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(op) } = &stmt.kind {
                if slots.contains_key(dst) {
                    out.extend(held(op).map(|offset| (func.name.clone(), offset)));
                }
            }
            if let MirStmtKind::ClosureCreate { func_name, captures, .. } = &stmt.kind {
                for c in captures.iter().filter(|c| c.by_ref) {
                    if let Some(offset) = slots.get(&c.local_id) {
                        passed.push(((func.name.clone(), *offset), (func_name.clone(), c.offset)));
                    }
                }
            }
            if matches!(stmt.kind, MirStmtKind::LoadCapture { .. }) {
                continue;
            }
            if let Some(offset) = uses::stmt_def(stmt).and_then(|d| slots.get(&d)) {
                out.insert((func.name.clone(), *offset));
            }
        }
    }
    loop {
        let before = out.len();
        for (from, to) in &passed {
            if out.contains(to) {
                out.insert(from.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// Which calls *through* a closure may keep a closure they are handed.
///
/// A yield lends its item, and a terminal that keeps one clones it (CV2),
/// which needs a block with a header. A closure literal yielded straight out of a sequence body —
///
/// ```text
/// func pair() -> Sequence<func(i64) -> i64> {
///     return |yield| { if !yield(|n| n + 5) { return } … }
/// }
/// pair().to_vec()
/// ```
///
/// — was given a stack environment, because a call through a closure never
/// counted as somewhere a closure could go. `to_vec` then retained a block with
/// no header and pushed a pointer into a frame that was about to be popped.
///
/// The bodies a call reaches say whether they keep the argument. A body
/// nobody can place might be one of the keepers, unless the program has no
/// closure body that keeps a closure parameter at all, which is nearly every
/// program: that is what keeps an ordinary `for x in seq` from allocating its
/// yield.
pub(crate) struct KeptThroughCalls {
    targets: crate::closure_targets::ClosureTargets,
    /// The plain walk, the one `container_drop` reads: a variable is only
    /// worth heap closures when the pass that releases them sees the same
    /// call targets.
    plain_targets: crate::closure_targets::ClosureTargets,
    routes: HashMap<String, Vec<Route>>,
    any_keeper: bool,
}

impl KeptThroughCalls {
    pub(crate) fn build(fns: &[MirFunction]) -> Self {
        let targets = crate::closure_targets::ClosureTargets::build_following_returns(fns);
        let routes = build_param_routes(fns, &targets);
        let bodies: HashSet<&str> = fns
            .iter()
            .flat_map(|f| f.blocks.iter().flat_map(|b| b.statements.iter()))
            .filter_map(|stmt| match &stmt.kind {
                MirStmtKind::ClosureCreate { func_name, .. } => Some(func_name.as_str()),
                _ => None,
            })
            .collect();
        let any_keeper = fns.iter().filter(|f| bodies.contains(f.name.as_str())).any(|f| {
            f.params.iter().enumerate().skip(1).any(|(i, p)| {
                matches!(p.ty, crate::MirType::FuncPtr(_))
                    && routes.get(&f.name).and_then(|r| r.get(i)) == Some(&Route::Away)
            })
        });
        Self {
            targets,
            plain_targets: crate::closure_targets::ClosureTargets::build(fns),
            routes,
            any_keeper,
        }
    }

    /// Might `closure(args)` in `func` hand argument `i` to a body's `mutate`
    /// parameter? Only bodies the call is known to reach count: through an
    /// unknown one, the caller's variable isn't taken as owning its closure
    /// (`container_drop::cell_used_as_variable`), so nothing replaces it.
    fn lends_to_mutate(&self, func: &str, closure: LocalId, i: usize, lent: &LentCells) -> bool {
        self.plain_targets
            .known(func, closure)
            .is_some_and(|bodies| bodies.iter().any(|b| lent.mutate_params.contains(&(b.clone(), i + 1))))
    }

    /// May `closure(args)` in `func` keep argument `i`?
    fn keeps(&self, func: &str, closure: LocalId, i: usize) -> bool {
        if !self.any_keeper {
            return false;
        }
        match self.targets.known(func, closure) {
            // Argument `i` is parameter `i + 1`, after the environment.
            Some(bodies) => bodies.iter().any(|b| {
                self.routes.get(b).and_then(|r| r.get(i + 1)).is_none_or(|r| *r == Route::Away)
            }),
            None => true,
        }
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
    let callee_escapes = build_callee_escape_map(fns, CaptureRule::HeapClosure);

    // A function that hands a heap closure back makes its caller the owner —
    // `let tick = counter()` is the caller receiving a block nobody else will
    // free. Which functions those are can only be read off the finished
    // allocation decisions, so it waits for every function to have one.
    // Which bodies a call *through* a closure can reach, so the same question
    // can be asked of a callback. `flat_map(|x| upto(x))` builds a sequence per
    // element and hands it back through a `ClosureCall`, which has no name to
    // look up — so nothing owned any of them and each element leaked its
    // environment (#1045).
    let targets = crate::closure_targets::ClosureTargets::build(fns);
    let hands_back = functions_handing_back_a_closure(fns, &targets);

    for func in fns.iter_mut() {
        insert_drops(func, &callee_escapes, &hands_back, &targets);
    }
}

/// Does a call keep argument `i`? A callee with no body of its own keeps what
/// it isn't known to borrow: its declared shape, or a runtime call that uses
/// its callback up.
fn callee_keeps(callee_escapes: &HashMap<String, Vec<bool>>, callee: &crate::FunctionRef, i: usize) -> bool {
    callee_escapes
        .get(&callee.name)
        .and_then(|e| e.get(i))
        .copied()
        .unwrap_or_else(|| !uses_up_callback(callee) && callee.shape.map_or(true, |s| s.gives(i)))
}

/// A bodiless callee that is done with a closure it's handed before it
/// returns: one written down as using its callback up, or a runtime call
/// lowering made for itself, which only reads what it's handed (#1555).
fn uses_up_callback(callee: &crate::FunctionRef) -> bool {
    callee.runtime || callee.shape.is_some_and(|s| s.is_eager_callback())
}

/// The closures `func` hands to a task: the first argument of each task
/// block's runtime entry (`lower_spawn`).
fn task_closures(func: &MirFunction) -> HashSet<LocalId> {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Call { func: callee, args, .. }
                if crate::TASK_ENTRIES.contains(&callee.name.as_str()) =>
            {
                args.first().and_then(uses::operand_local)
            }
            _ => None,
        })
        .collect()
}

/// Heap exactly when the closure outlives this frame.
///
/// This used to only downgrade. Lowering picks the initial answer from `own`,
/// so a scope-limited closure that escaped anyway — by being returned, which is
/// every sequence source and every adapter — kept a stack environment in a
/// frame that had already been popped. It read back whatever was left there:
/// the right answer in a small program, a wrong one or a segfault in a real
/// one (#1045).
fn decide_allocation(
    func: &mut MirFunction,
    callee_escapes: &HashMap<String, Vec<bool>>,
    kept: &KeptThroughCalls,
    lent: &LentCells,
) {
    let created = created_closures(func);
    if created.is_empty() {
        return;
    }
    let escaping = find_escaping_closures(func, &created, callee_escapes, kept, lent);

    for block in &mut func.blocks {
        for stmt in &mut block.statements {
            if let MirStmtKind::ClosureCreate { dst, heap, .. } = &mut stmt.kind {
                *heap = escaping.contains(dst);
            }
        }
    }
}

/// The closure functions whose environment the frame stops being able to vouch
/// for — so their captures cannot be addresses into that frame.
///
/// Borrowing is what a scope-limited closure does (mem.closures/MC1), and it is
/// sound exactly while the frame the addresses point into is alive and the
/// compiler can see the closure's whole life inside it. Two things end that:
///
///   - the closure leaves by name — returned, stored through a pointer, put in
///     an array, boxed as an interface object, or captured by another closure that
///     leaves;
///   - the closure is handed to a call that *keeps* it. `fns.push(|x| …)` is
///     this one: the vector holds the closure, `fns[0]` reads it back out under
///     a name nothing connects to the create, and the frame's release for the
///     captured string had already run (#1160).
///
/// A call that only *calls* the closure is not either of those, and that
/// distinction is the whole point of asking the question this way rather than
/// off the heap flag. `upto(4).for_each(|x| { total = total + x })` puts its
/// environment on the heap because `for_each` holds the block for the duration
/// of the call — but it hands nothing on, so `total` stays a borrow and the
/// write lands in `main`'s variable. Copying there answered 0.
///
/// Keyed by function name, because the two flags that have to agree — `by_ref`
/// on the create and the access on each `LoadCapture` — live in two separate
/// `MirFunction`s that share nothing but the name. So a closure handed on in one
/// frame captures by value in all of them.
pub(crate) fn closures_handed_on(fns: &[MirFunction]) -> HashSet<String> {
    let callee_escapes = build_callee_escape_map(fns, CaptureRule::HeapClosure);
    let kept = KeptThroughCalls::build(fns);
    let routes = &kept.routes;
    let mut names = HashSet::new();

    for func in fns {
        let created = created_closures(func);
        if created.is_empty() {
            continue;
        }
        let aliases = closure_aliases(func, &created);
        let leaving = leaves_the_frame(func, routes, &kept.targets);
        // Only a closure that actually borrows something has a borrow to
        // withdraw, so the map is built from those creates alone.
        let borrows: HashMap<LocalId, &str> = func
            .blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|stmt| match &stmt.kind {
                MirStmtKind::ClosureCreate { dst, func_name, captures, .. }
                    if captures.iter().any(|c| c.by_ref) =>
                {
                    Some((*dst, func_name.as_str()))
                }
                _ => None,
            })
            .collect();
        if borrows.is_empty() {
            continue;
        }
        let name_of = |id: LocalId| -> Vec<String> {
            aliases
                .origins(&id)
                .iter()
                .filter_map(|origin| borrows.get(origin).map(|n| n.to_string()))
                .collect()
        };

        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Call { dst, func: callee, args } => {
                        for (idx, arg) in args.iter().enumerate() {
                            let Some(id) = uses::operand_local(arg) else { continue };
                            // A callee nobody wrote down might keep it, and
                            // "might" has to mean "does": guessing borrow costs
                            // the buffer, guessing keep costs a copy.
                            let keeps = callee_escapes
                                .get(&callee.name)
                                .and_then(|e| e.get(idx))
                                .copied()
                                .unwrap_or_else(|| {
                                    !uses_up_callback(callee)
                                });
                            // A callee that keeps it only inside what it hands
                            // back keeps it exactly as long as that result
                            // lives (mem.closures/SL4). `filter(pred)` returns
                            // an adapter holding `pred`; consumed by `count()`
                            // on the same line, the adapter dies in this frame
                            // and `pred` never left it, so its write to
                            // `total` has to land (#1279). Returned or stored,
                            // the adapter takes `pred` with it.
                            let only_in_result = routes
                                .get(&callee.name)
                                .and_then(|r| r.get(idx))
                                .is_some_and(|r| *r == Route::IntoResult);
                            let result_stays = dst.is_none_or(|d| !leaving.contains(&d));
                            if keeps && !(only_in_result && result_stays) {
                                names.extend(name_of(id));
                            }
                        }
                    }
                    MirStmtKind::Store { value: MirOperand::Local(id), .. }
                    | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                    | MirStmtKind::InterfaceBox { value: MirOperand::Local(id), .. } => {
                        names.extend(name_of(*id));
                    }
                    // Kept by a body a call through a closure reaches: it
                    // outlives the frame, so it can't point into it.
                    MirStmtKind::ClosureCall { closure, args, .. } => {
                        for (i, arg) in args.iter().enumerate() {
                            let Some(id) = uses::operand_local(arg) else { continue };
                            if kept.keeps(&func.name, *closure, i) {
                                names.extend(name_of(id));
                            }
                        }
                    }
                    // An environment that goes with an escaping closure is as
                    // gone as the closure is.
                    MirStmtKind::ClosureCreate { captures, heap: true, .. } => {
                        for cap in captures {
                            names.extend(name_of(cap.local_id));
                        }
                    }
                    _ => {}
                }
            }
            match &block.terminator.kind {
                MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
                | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                    names.extend(name_of(*id));
                }
                _ => {}
            }
        }
    }

    names
}

/// Where a parameter can go once the callee has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Route {
    /// Nowhere: the callee is done with it when it returns.
    Stays,
    /// Only into the value the callee returns — captured by a closure it hands
    /// back, or handed back itself. It lives as long as the caller keeps that
    /// result (mem.closures/SL4).
    IntoResult,
    /// Somewhere the caller can't follow: stored, boxed, or given to a callee
    /// that keeps it.
    Away,
}

/// `Route` for every parameter of every function, as a fixed point — a
/// parameter handed to another function goes wherever that one sends it,
/// called by name or through a closure (`targets`).
fn build_param_routes(
    fns: &[MirFunction],
    targets: &crate::closure_targets::ClosureTargets,
) -> HashMap<String, Vec<Route>> {
    let mut routes: HashMap<String, Vec<Route>> = fns
        .iter()
        .map(|f| (f.name.clone(), vec![Route::Stays; f.params.len()]))
        .collect();
    // Routes only rise, so this settles.
    loop {
        let mut changed = false;
        for func in fns {
            let (into_result, away) = frame_routes(func, &routes, targets);
            for (i, p) in func.params.iter().enumerate() {
                let r = if away.contains(&p.id) {
                    Route::Away
                } else if into_result.contains(&p.id) {
                    Route::IntoResult
                } else {
                    Route::Stays
                };
                let Some(slot) = routes.get_mut(&func.name).and_then(|r| r.get_mut(i)) else {
                    continue;
                };
                if r > *slot {
                    *slot = r;
                    changed = true;
                }
            }
        }
        if !changed {
            return routes;
        }
    }
}

/// The locals in `func` whose value may outlive it: returned, stored, or
/// handed on to something that keeps it.
fn leaves_the_frame(
    func: &MirFunction,
    routes: &HashMap<String, Vec<Route>>,
    targets: &crate::closure_targets::ClosureTargets,
) -> HashSet<LocalId> {
    let (into_result, away) = frame_routes(func, routes, targets);
    into_result.union(&away).copied().collect()
}

/// Which locals of `func` reach its return value, and which go somewhere else
/// that outlives the call.
///
/// Walked backwards from where values leave: whatever is copied into, captured
/// by, or handed to a callee alongside a leaving value leaves with it. A
/// callee's `IntoResult` parameter leaves exactly the way the call's own result
/// does, which is the part a plain "does it escape" can't say.
fn frame_routes(
    func: &MirFunction,
    routes: &HashMap<String, Vec<Route>>,
    targets: &crate::closure_targets::ClosureTargets,
) -> (HashSet<LocalId>, HashSet<LocalId>) {
    let mut into_result: HashSet<LocalId> = HashSet::new();
    let mut away: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                into_result.insert(*id);
            }
            _ => {}
        }
    }
    // `src` leaves wherever `dst` does.
    fn follow(
        into_result: &mut HashSet<LocalId>,
        away: &mut HashSet<LocalId>,
        dst: LocalId,
        src: LocalId,
    ) -> bool {
        let mut grew = false;
        if into_result.contains(&dst) {
            grew |= into_result.insert(src);
        }
        if away.contains(&dst) {
            grew |= away.insert(src);
        }
        grew
    }
    let mut changed = true;
    while changed {
        changed = false;
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            match &stmt.kind {
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                | MirStmtKind::InterfaceBox { value: MirOperand::Local(id), .. } => {
                    changed |= away.insert(*id);
                }
                MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } => {
                    changed |= follow(&mut into_result, &mut away, *dst, *src);
                }
                MirStmtKind::ClosureCreate { dst, captures, .. } => {
                    for cap in captures {
                        changed |= follow(&mut into_result, &mut away, *dst, cap.local_id);
                    }
                }
                MirStmtKind::Call { dst, func: callee, args } => {
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        let route = match routes.get(&callee.name) {
                            Some(r) => r.get(i).copied().unwrap_or(Route::Away),
                            None if uses_up_callback(callee) => {
                                Route::Stays
                            }
                            None => Route::Away,
                        };
                        match (route, dst) {
                            (Route::Stays, _) => {}
                            (Route::Away, _) => changed |= away.insert(id),
                            (Route::IntoResult, Some(d)) => {
                                changed |= follow(&mut into_result, &mut away, *d, id);
                            }
                            (Route::IntoResult, None) => {}
                        }
                    }
                }
                // Through a closure, argument `i` goes wherever parameter
                // `i + 1` of each body it reaches sends it, after the
                // environment. `let s = set_to; s(mutate g, h)` keeps `h` the
                // way `set_to(mutate g, h)` does; read as staying, the caller
                // built `h` on its stack and the frame that took it freed it.
                // A call nobody can place is left as staying, the same answer
                // `KeptThroughCalls::keeps` gives when no body keeps a closure.
                MirStmtKind::ClosureCall { dst, closure, args } => {
                    let Some(bodies) = targets.known(&func.name, *closure) else { continue };
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        let route = bodies
                            .iter()
                            .map(|b| routes.get(b).map_or(Route::Away, |r| r.get(i + 1).copied().unwrap_or(Route::Away)))
                            .max()
                            .unwrap_or(Route::Stays);
                        match (route, dst) {
                            (Route::Stays, _) => {}
                            (Route::Away, _) => changed |= away.insert(id),
                            (Route::IntoResult, Some(d)) => {
                                changed |= follow(&mut into_result, &mut away, *d, id);
                            }
                            (Route::IntoResult, None) => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (into_result, away)
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
    targets: &crate::closure_targets::ClosureTargets,
) {
    let mut owned: HashMap<LocalId, bool> =
        retain_loads_handed_on(func, callee_escapes).into_iter().map(|l| (l, true)).collect();
    owned.extend(clone_lent_params_handed_on(func, callee_escapes).into_iter().map(|l| (l, true)));
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                // A heap closure, and a stack one whose environment owns what
                // it carried: dropping it is what releases that.
                MirStmtKind::ClosureCreate { dst, heap, captures, .. }
                    if *heap || captures.iter().any(|c| c.owns.is_some()) =>
                {
                    owned.insert(*dst, true);
                }
                // A closure value a task block captured is the task's: the
                // block runs once and frees it unless it hands it on. The
                // frame that started the task gave it a reference of its own.
                MirStmtKind::LoadCapture { dst, access: crate::CaptureAccess::Taken, .. }
                    if matches!(func.local_ty(*dst), Some(crate::MirType::FuncPtr(_))) =>
                {
                    owned.insert(*dst, true);
                }
                MirStmtKind::Call { dst: Some(dst), func: callee, .. }
                    if hands_back.contains(&callee.name) || callee.name == "rask_closure_clone" =>
                {
                    owned.insert(*dst, true);
                }
                // A closure taken out of a container (`fs.remove(i)`,
                // `fs.replace(i, g)`, and so `fs[i] = g`) is the caller's now,
                // by the declaration's `@moves_out`. Nothing here made it, so
                // without this nothing freed it.
                MirStmtKind::Call { dst: Some(dst), func: callee, .. }
                    if matches!(func.local_ty(*dst), Some(crate::MirType::FuncPtr(_)))
                        && callee.moves_out() =>
                {
                    owned.insert(*dst, true);
                }
                // A third way: take one back from a call *through* a closure.
                // Same rule as the named case, asked of every body the call can
                // reach — one that hands back somebody else's closure is the
                // whole set's answer, the way `flat_map(|k| SHARED_SEQ)` would
                // be.
                MirStmtKind::ClosureCall { dst: Some(dst), closure, .. } => {
                    if let Some(bodies) = targets.known(&func.name, *closure) {
                        if bodies.iter().all(|b| hands_back.contains(b)) {
                            owned.insert(*dst, true);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if owned.is_empty() {
        return;
    }

    let aliases = closure_aliases(func, &owned);
    let facts = closure_facts(func, &owned, &aliases, callee_escapes);
    let plan = crate::analysis::ownership::plan(
        func,
        &facts,
        crate::analysis::ownership::Placement::ScopeEnd,
    );
    let drops_for = |name: LocalId, made: Option<LocalId>| -> Vec<MirStmt> {
        vec![MirStmt::dummy(MirStmtKind::ClosureDrop { closure: name, made })]
    };
    let guards = crate::analysis::ownership::Guards::new(func, &plan);
    let (_, unwind_edges) = crate::analysis::ownership::place_unwind(
        func,
        plan.unwind,
        &guards,
        &mut |_: &mut MirFunction, name: LocalId, made: Option<LocalId>| drops_for(name, made),
    );
    let mut at_end: Vec<(usize, LocalId, Option<LocalId>, Option<usize>)> = Vec::new();
    let mut on_edges: Vec<(crate::BlockId, crate::BlockId, Vec<MirStmt>)> = Vec::new();
    for r in plan.releases {
        match r {
            crate::analysis::ownership::Release::At { block, name, made, guard, .. } => {
                at_end.push((block, name, made, guard))
            }
            crate::analysis::ownership::Release::OnEdge { from, to, name, made, guard } => {
                on_edges.push((from, to, guards.wrap(guard, drops_for(name, made))))
            }
        }
    }
    at_end.sort_by_key(|(b, l, _, _)| (*b, l.0));
    for (block, name, made, guard) in at_end {
        let drops = guards.wrap(guard, drops_for(name, made));
        func.blocks[block].statements.extend(drops);
    }
    let flag_edges = guards.edges(func);
    crate::analysis::ownership::insert_on_edges(
        func,
        crate::analysis::ownership::merge_edges(
            crate::analysis::ownership::merge_edges(unwind_edges, on_edges),
            flag_edges,
        ),
    );
    guards.finish(func);
}

/// A closure read out of a variable's cell and handed to something that keeps
/// it gets a copy of its own, made at the read.
///
/// A closure variable a closure body writes lives in a cell, and the cell owns
/// the closure it holds: an overwrite releases the old one and the frame
/// releases the last (`container_drop::closure_cell_is_a_variable`). Reading
/// the cell copies the pointer, so `fs.push(g)` in a loop body hands the vector
/// the block the cell still holds, and the next `g = || 7` freed it under the vector. The
/// cell doesn't know the read moved its closure out, so the read clones it
/// (CV2), and this frame owns the copy like any other: handed over where it
/// goes, released where it doesn't. The cell goes on releasing its own.
///
/// The copy is made at the read, not the hand-off, because a write to the
/// cell in between (`let r = g; g = || 3; return r`) releases the cell's.
fn retain_loads_handed_on(func: &mut MirFunction, callee_escapes: &HashMap<String, Vec<bool>>) -> HashSet<LocalId> {
    let is_closure = |func: &MirFunction, l: &LocalId| matches!(func.local_ty(*l), Some(crate::MirType::FuncPtr(_)));
    let mut handed: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::ClosureCreate { heap: true, captures, .. } => {
                    handed.extend(captures.iter().filter(|c| !c.by_ref).map(|c| c.local_id));
                }
                MirStmtKind::Call { func: callee, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if let Some(id) = uses::operand_local(arg).filter(|_| callee_keeps(callee_escapes, callee, i)) {
                            handed.insert(id);
                        }
                    }
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                | MirStmtKind::InterfaceBox { value: MirOperand::Local(id), .. } => {
                    handed.insert(*id);
                }
                _ => {}
            }
        }
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } = &block.terminator.kind
        {
            handed.insert(*id);
        }
    }
    // A read kept in a variable (`let r = g`) may be used after the cell is
    // written, which releases the cell's reference, so it takes its own too.
    // A read used only to call it right away needs none.
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { rvalue: crate::MirRValue::Use(MirOperand::Local(src)), .. } = &stmt.kind {
            handed.insert(*src);
        }
    }
    loop {
        let before = handed.len();
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
                if handed.contains(dst) {
                    handed.insert(*src);
                }
            }
        }
        if handed.len() == before {
            break;
        }
    }
    // Only a variable's cell: one this function writes, or a loop body's
    // view of the enclosing frame's. The slot a body's `return` fills is read
    // once by the frame and handed over, so a read of it takes nothing; a
    // reference taken there was one nobody released.
    let variables: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Store { addr, offset: 0, .. } => Some(*addr),
            MirStmtKind::LoadCapture { dst, .. } => Some(*dst),
            _ => None,
        })
        .collect();
    let mut next = func.locals.iter().chain(func.params.iter()).map(|l| l.id.0).max().map_or(0, |m| m + 1);
    let mut cloned = HashSet::new();
    for bi in 0..func.blocks.len() {
        let mut si = 0;
        while si < func.blocks[bi].statements.len() {
            if let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Deref(MirOperand::Local(cell)) } =
                &func.blocks[bi].statements[si].kind
            {
                let dst = *dst;
                if handed.contains(&dst) && is_closure(func, &dst) && is_closure(func, cell) && variables.contains(cell) {
                    let read = LocalId(next);
                    next += 1;
                    let ty = func.local_ty(dst).cloned().unwrap_or(crate::MirType::Ptr);
                    func.locals.push(crate::MirLocal { id: read, name: None, ty, is_param: false });
                    let MirStmtKind::Assign { dst: d, .. } = &mut func.blocks[bi].statements[si].kind else {
                        unreachable!()
                    };
                    *d = read;
                    func.blocks[bi].statements.insert(
                        si + 1,
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(dst),
                            func: crate::FunctionRef::borrowing("rask_closure_clone"),
                            args: vec![MirOperand::Local(read)],
                        }),
                    );
                    cloned.insert(dst);
                    si += 1;
                }
            }
            si += 1;
        }
    }
    cloned
}

/// A closure parameter the function only borrows, handed to something that
/// keeps it: the keeper gets a clone (mem.closures/CV2), made at the hand-off
/// and owned by this frame until then.
///
/// The checker rules this out in a body someone wrote (PM6), so what reaches
/// here is a loop body lowering made. A yield lends its item (SEQ34), and a
/// body that keeps one keeps a copy unless every call reaching it hands the
/// item over (`container_drop::settle_loop_item_modes`). `to_vec` over a
/// `map(keep_i)` pushed the closure `map` was about to free.
fn clone_lent_params_handed_on(func: &mut MirFunction, callee_escapes: &HashMap<String, Vec<bool>>) -> HashSet<LocalId> {
    let mut lent: HashSet<LocalId> = func
        .params
        .iter()
        .zip(&func.param_modes)
        .filter(|(p, m)| **m == rask_ast::ty::ParamMode::Borrow && matches!(p.ty, crate::MirType::FuncPtr(_)))
        .map(|(p, _)| p.id)
        .collect();
    let mut cloned = HashSet::new();
    if lent.is_empty() {
        return cloned;
    }
    loop {
        let before = lent.len();
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
                if lent.contains(src) {
                    lent.insert(*dst);
                }
            }
        }
        if lent.len() == before {
            break;
        }
    }
    let mut next = func.locals.iter().chain(func.params.iter()).map(|l| l.id.0).max().map_or(0, |m| m + 1);
    let mut fresh = |func: &mut MirFunction, of: LocalId| {
        let id = LocalId(next);
        next += 1;
        let ty = func.local_ty(of).cloned().unwrap_or(crate::MirType::Ptr);
        func.locals.push(crate::MirLocal { id, name: None, ty, is_param: false });
        id
    };
    let clone_of = |dst: LocalId, of: LocalId| {
        MirStmt::dummy(MirStmtKind::Call {
            dst: Some(dst),
            func: crate::FunctionRef::borrowing("rask_closure_clone"),
            args: vec![MirOperand::Local(of)],
        })
    };
    for bi in 0..func.blocks.len() {
        let mut si = 0;
        while si < func.blocks[bi].statements.len() {
            let mut swaps: Vec<LocalId> = Vec::new();
            match &func.blocks[bi].statements[si].kind {
                MirStmtKind::ClosureCreate { heap: true, captures, .. } => {
                    swaps.extend(captures.iter().filter(|c| !c.by_ref && !c.lent).map(|c| c.local_id));
                }
                MirStmtKind::Call { func: callee, args, .. } => {
                    swaps.extend(
                        args.iter()
                            .enumerate()
                            .filter(|(i, _)| callee_keeps(callee_escapes, callee, *i))
                            .filter_map(|(_, a)| uses::operand_local(a)),
                    );
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                | MirStmtKind::InterfaceBox { value: MirOperand::Local(id), .. } => swaps.push(*id),
                _ => {}
            }
            swaps.retain(|id| lent.contains(id));
            for of in swaps {
                let copy = fresh(func, of);
                func.blocks[bi].statements.insert(si, clone_of(copy, of));
                si += 1;
                let swap = |l: &mut LocalId| {
                    if *l == of {
                        *l = copy;
                    }
                };
                let swap_op = |op: &mut MirOperand| {
                    if let MirOperand::Local(l) = op {
                        swap(l);
                    }
                };
                match &mut func.blocks[bi].statements[si].kind {
                    MirStmtKind::ClosureCreate { captures, .. } => captures.iter_mut().for_each(|c| swap(&mut c.local_id)),
                    MirStmtKind::Call { args, .. } => args.iter_mut().for_each(swap_op),
                    MirStmtKind::Store { value, .. }
                    | MirStmtKind::ArrayStore { value, .. }
                    | MirStmtKind::InterfaceBox { value, .. } => swap_op(value),
                    _ => {}
                }
                cloned.insert(copy);
            }
            si += 1;
        }
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) } = &func.blocks[bi].terminator.kind {
            let of = *id;
            if lent.contains(&of) {
                let copy = fresh(func, of);
                func.blocks[bi].statements.push(clone_of(copy, of));
                func.blocks[bi].terminator.kind = MirTerminatorKind::Return { value: Some(MirOperand::Local(copy)) };
                cloned.insert(copy);
            }
        }
    }
    cloned
}

/// What each statement does to the closures this frame may hold.
///
/// Made here: built, or handed back by a call. Handed over: captured by an
/// escaping closure (its environment now holds it, and frees it with itself),
/// stored, returned, or passed to a callee that keeps the argument. A callee
/// whose body says it keeps nothing leaves the closure to this frame; no
/// answer means it might keep it. Calling a closure is a borrow.
///
fn closure_facts(
    func: &MirFunction,
    made: &HashMap<LocalId, bool>,
    aliases: &ClosureAliases,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> crate::analysis::ownership::Facts {
    use crate::analysis::ownership::Event;
    let tracked: std::collections::BTreeSet<LocalId> = aliases.map.keys().copied().collect();
    let is = |l: &LocalId| tracked.contains(l);
    let mut facts = crate::analysis::ownership::Facts {
        moved_parts: Default::default(),
        names: tracked.clone(),
        events: Vec::new(),
        terminator_events: Vec::new(),
        reads: Vec::new(),
        kills: Vec::new(),
        terminator_reads: Vec::new(),
        foreign: func.params.iter().map(|p| p.id).filter(|p| is(p)).collect(),
        owned: Vec::new(),
        caller_slots: Vec::new(),
    };
    for block in &func.blocks {
        let (mut events, mut reads, mut kills) = (Vec::new(), Vec::new(), Vec::new());
        for stmt in &block.statements {
            let mut ev: Vec<Event> = Vec::new();
            let give = |ev: &mut Vec<Event>, id: LocalId| {
                if is(&id) {
                    ev.push(Event::HandOver(id));
                }
            };
            match &stmt.kind {
                // Carried in: the environment owns it now (CM2).
                MirStmtKind::ClosureCreate { captures, .. } => {
                    for cap in captures.iter().filter(|c| !c.by_ref && !c.lent) {
                        give(&mut ev, cap.local_id);
                    }
                }
                MirStmtKind::Call { func: callee, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if callee_keeps(callee_escapes, callee, i) {
                            give(&mut ev, id);
                        }
                    }
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                | MirStmtKind::InterfaceBox { value: MirOperand::Local(id), .. } => give(&mut ev, *id),
                MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) }
                    if is(dst) && is(src) && !made.contains_key(dst) =>
                {
                    ev.push(Event::Alias { dst: *dst, src: *src });
                }
                _ => {}
            }
            if let Some(d) = uses::stmt_def(stmt).filter(|d| is(d)) {
                let bound = ev.iter().any(|e| matches!(e, Event::Alias { dst, .. } if *dst == d));
                if !bound && !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                    ev.push(if made.contains_key(&d) { Event::Make(d) } else { Event::Other(d) });
                }
            }
            let (mut r, mut k) = (Vec::new(), Vec::new());
            if !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                for n in &tracked {
                    if uses::stmt_reads(stmt, *n) {
                        r.push(*n);
                    }
                    if uses::stmt_def(stmt) == Some(*n) {
                        k.push(*n);
                    }
                }
            }
            events.push(ev);
            reads.push(r);
            kills.push(k);
        }
        facts.events.push(events);
        facts.reads.push(reads);
        facts.kills.push(kills);
        let mut term = Vec::new();
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } =
            &block.terminator.kind
        {
            if is(id) {
                term.push(Event::HandOver(*id));
            }
        }
        facts.terminator_events.push(term);
        facts
            .terminator_reads
            .push(tracked.iter().copied().filter(|n| uses::terminator_reads(&block.terminator, *n)).collect());
    }
    facts
}

/// Closures a function reads out of a join's result and returns.
///
/// A task that ends in a closure hands it over through the join as a boxed
/// payload, and the joiner is the only one left holding it: the task's frame is
/// gone and the handle was consumed. `return h.join()!` reads it out of the
/// result wrapper with a field read, which no other rule saw as a source, so a
/// function written that way didn't count as handing a closure back and its
/// caller never freed the one it got.
fn closures_out_of_joins(func: &MirFunction) -> Vec<LocalId> {
    let mut joined: HashSet<LocalId> = HashSet::new();
    let mut out = Vec::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { dst: Some(dst), func: callee, .. }
                    if matches!(callee.name.as_str(), "Handle_join" | "Handle_cancel") =>
                {
                    joined.insert(*dst);
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: crate::MirRValue::Field { base: MirOperand::Local(base), .. },
                } if joined.contains(base)
                    && matches!(func.local_ty(*dst), Some(crate::MirType::FuncPtr(_))) =>
                {
                    out.push(*dst);
                }
                _ => {}
            }
        }
    }
    out
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
fn functions_handing_back_a_closure(
    fns: &[MirFunction],
    targets: &crate::closure_targets::ClosureTargets,
) -> HashSet<String> {
    let mut names = HashSet::new();
    // A fixed point, because handing one back is transitive. `|x| upto(x)`
    // creates no closure of its own — it calls `upto` and returns what came
    // back — so a single pass left it out, and `flat_map` over it owned nothing
    // and freed nothing. One environment per element (#1045).
    loop {
        let before = names.len();
        for func in fns {
            if names.contains(&func.name) {
                continue;
            }
            let mut owned: HashMap<LocalId, bool> = created_closures(func)
                .into_iter()
                .filter(|(_, heap)| *heap)
                .collect();
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                match &stmt.kind {
                    MirStmtKind::Call { dst: Some(dst), func: callee, .. }
                        if names.contains(&callee.name) =>
                    {
                        owned.insert(*dst, true);
                    }
                    MirStmtKind::ClosureCall { dst: Some(dst), closure, .. } => {
                        if let Some(bodies) = targets.known(&func.name, *closure) {
                            if bodies.iter().all(|b| names.contains(b)) {
                                owned.insert(*dst, true);
                            }
                        }
                    }
                    _ => {}
                }
            }
            for id in closures_out_of_joins(func) {
                owned.insert(id, true);
            }
            if owned.is_empty() {
                continue;
            }
            // A read out of a cell holding one of them holds it too, and so
            // does every copy of the read: `let r = g; …; return r`.
            let loads = loads_from_closure_cells(func, &closure_aliases(func, &owned));
            owned.extend(loads.into_iter().map(|dst| (dst, true)));
            let aliases = closure_aliases(func, &owned);
            for block in &func.blocks {
                let returned = match &block.terminator.kind {
                    MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
                    | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                        *id
                    }
                    _ => continue,
                };
                if aliases.holds_closure(&returned) {
                    names.insert(func.name.clone());
                }
            }
        }
        if names.len() == before {
            return names;
        }
    }
}

/// Loads out of a variable that lives in memory because a closure body assigns
/// it, where the variable holds closures made here.
///
/// `mut f = || 0; for v in seq { f = || v[2] }; return f` keeps `f` in a cell
/// the loop body writes through, and `return f` is `tmp = *cell; return tmp`.
/// Every closure that lands in such a cell is a heap one, and a read that is
/// handed on takes a reference of its own (`retain_loads_handed_on`), so the
/// one read back out at the return is handed back. Not seeing it, the caller
/// never freed it.
fn loads_from_closure_cells(func: &MirFunction, aliases: &ClosureAliases) -> Vec<LocalId> {
    let held: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::Store { addr, offset: 0, value: MirOperand::Local(v), .. }
                if matches!(func.local_ty(*addr), Some(crate::MirType::FuncPtr(_))) && aliases.holds_closure(v) =>
            {
                Some(*addr)
            }
            _ => None,
        })
        .collect();
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Deref(MirOperand::Local(cell)) }
                if held.contains(cell) =>
            {
                Some(*dst)
            }
            _ => None,
        })
        .collect()
}

/// How a parameter that a closure captures counts in the callee escape map.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureRule {
    /// Every capture counts, whatever the closure does. For a question that
    /// has no closure to ask about: a boxed interface value captured by any
    /// closure is left to it (a leak beats a double free).
    AnyClosure,
    /// Only a capture by a closure already on the heap. A stack environment
    /// dies with the frame, so the caller is still the owner.
    /// `seq.reduce(|a, b| a + b)` is that case: the `for x in self` desugar
    /// captures `f` into a scope-limited yield closure, so every closure
    /// passed to a terminal read as given away and nobody freed it.
    HeapClosure,
    /// Only a capture by a closure that leaves its frame, worked out from the
    /// map as it stands. For deciding stack or heap, when no closure has been
    /// put on either yet.
    ///
    /// Counting every capture here said that a function which only calls a
    /// closure it captured into a local closure (`let g = || { f() }; g()`)
    /// gives `f` away. Its caller then put `f` on the heap, which copies what
    /// it captures, and `f`'s write to the caller's variable landed in the
    /// copy (#1886).
    EscapingClosure,
}

/// Build a map of callee name → per-parameter escape info.
///
/// For each function, checks whether each parameter escapes (appears in
/// Call args, Store, or Return within the function body). A non-escaping
/// parameter means the function only uses it locally (e.g., via ClosureCall).
/// Which parameters each function gives away, by name.
///
/// `rule` picks how a parameter captured by a closure counts; see `CaptureRule`.
pub(crate) fn build_callee_escape_map(
    fns: &[MirFunction],
    rule: CaptureRule,
) -> HashMap<String, Vec<bool>> {
    let kept = (rule == CaptureRule::EscapingClosure).then(|| KeptThroughCalls::build(fns));
    // Start from "nothing escapes" and keep adding until nothing new appears.
    // Handing a parameter on is only an escape if the callee lets it escape,
    // and that answer is this same map — so one pass can't compute it. A single
    // pass read every call argument as an escape, which is what put a closure
    // on the heap the moment it was passed through one function into another:
    //
    //     func inner(f: func(i32)) { f(1) }       // borrows f
    //     func outer(f: func(i32)) { inner(f) }   // read as: gives f away
    //
    // A heap environment copies its captures, so `outer(|x| { total = total + x })`
    // added to a copy and the caller's `total` stayed 0 (#1038 again, from the
    // other end). Only escapes are ever added, so the loop settles, and a
    // recursive pair with no escape of its own correctly comes out borrowing.
    let mut map: HashMap<String, Vec<bool>> = fns
        .iter()
        .map(|f| (f.name.clone(), vec![false; f.params.len()]))
        .collect();
    let lent = LentCells::build(fns, captures_holding_heap_closures(fns));
    let targets = crate::closure_targets::ClosureTargets::build_following_returns(fns);
    loop {
        let mut changed = false;
        for func in fns {
            // Which of this frame's closures leave it, under the answers so far.
            // They only ever grow, so the loop still settles.
            let leaving = match &kept {
                Some(kept) => {
                    let created = created_closures(func);
                    if created.is_empty() {
                        HashSet::new()
                    } else {
                        find_escaping_closures(func, &created, &map, kept, &lent)
                    }
                }
                None => HashSet::new(),
            };
            for (i, p) in func.params.iter().enumerate() {
                if map.get(&func.name).is_some_and(|e| e[i]) {
                    continue;
                }
                if param_escapes_from(func, p.id, rule, &leaving, &map, &lent.slots, &targets) {
                    if let Some(e) = map.get_mut(&func.name) {
                        e[i] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return map;
        }
    }
}

/// Check if a parameter escapes from its function.
///
/// A parameter "escapes" if it appears in a Call arg, Store value, or Return,
/// or is an argument of a closure call that reaches a body keeping it. Called
/// itself, the function merely borrows it.
fn param_escapes_from(
    func: &MirFunction,
    param_id: LocalId,
    rule: CaptureRule,
    leaving: &HashSet<LocalId>,
    known: &HashMap<String, Vec<bool>>,
    heap_slots: &HashSet<(String, u32)>,
    targets: &crate::closure_targets::ClosureTargets,
) -> bool {
    // A task block captures a lent closure parameter as a view, so for who
    // frees what the caller's closure stays the caller's. It still has to be
    // on the heap, which is the other question this map answers.
    let tasks = if rule == CaptureRule::HeapClosure { task_closures(func) } else { HashSet::new() };
    // `g = h` lowers through a temporary, so a copy of the parameter going
    // somewhere is the parameter going there.
    let mut names = HashSet::from([param_id]);
    loop {
        let before = names.len();
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
                if names.contains(src) {
                    names.insert(*dst);
                }
            }
        }
        if names.len() == before {
            break;
        }
    }
    let is_it = |l: &LocalId| names.contains(l);
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::ClosureCreate { dst, .. } if tasks.contains(dst) => {}
                // A parameter captured by a closure leaves with it. This was
                // missing, and it is how a caller came to free a closure the
                // callee had handed on: every adapter captures the sequence it
                // wraps, so `Sequence_filter(self, pred)` reported that neither
                // parameter escaped, callers read "borrow", and the frame
                // dropped the source while the returned adapter still pointed
                // at it (#1051).
                //
                // Which closures count is the rule's (`CaptureRule`). Erring
                // toward escaping costs a leak; erring the other way costs a
                // use-after-free.
                // Captured by a body that keeps closures in it, as in a loop
                // body doing `g = h`: the body takes a reference of its own,
                // which only a heap closure has.
                MirStmtKind::ClosureCreate { func_name, captures, .. }
                    if rule == CaptureRule::EscapingClosure
                        && captures.iter().any(|c| {
                            c.by_ref && is_it(&c.local_id) && heap_slots.contains(&(func_name.clone(), c.offset))
                        }) =>
                {
                    return true;
                }
                MirStmtKind::ClosureCreate { dst, captures, heap, .. } => {
                    let takes_it = match rule {
                        CaptureRule::AnyClosure => true,
                        CaptureRule::HeapClosure => *heap,
                        CaptureRule::EscapingClosure => leaving.contains(dst),
                    };
                    if takes_it && captures.iter().any(|c| is_it(&c.local_id)) {
                        return true;
                    }
                }
                // Passed on. Whether that gives it away is the callee's
                // answer, in the same position — the map being built. A callee
                // with no body of its own (a runtime helper) has no answer, and
                // "unaccounted for" has to mean "might keep it"; the ones that
                // demonstrably don't are written down.
                MirStmtKind::Call { func: callee, args, .. } => {
                    for (arg_idx, arg) in args.iter().enumerate() {
                        if !names.iter().any(|n| uses::operand_reads(arg, *n)) {
                            continue;
                        }
                        let borrows = known
                            .get(&callee.name)
                            .and_then(|e| e.get(arg_idx))
                            .map(|escapes| !escapes)
                            .unwrap_or_else(|| {
                                uses_up_callback(callee)
                            });
                        if !borrows {
                            return true;
                        }
                    }
                }
                // Passed through a closure to bodies that are known: argument
                // `i` is each body's parameter `i + 1`, after the environment,
                // and escapes where that parameter does. `let s = set_to;
                // s(mutate g, h)` keeps `h` as `set_to(mutate g, h)` does; read
                // as a borrow, the caller built `h` on its stack and the frame
                // that took it freed it. A call nobody can place stays a
                // borrow, as before.
                MirStmtKind::ClosureCall { closure, args, .. } => {
                    let Some(bodies) = targets.known(&func.name, *closure) else { continue };
                    for (i, arg) in args.iter().enumerate() {
                        if !names.iter().any(|n| uses::operand_reads(arg, *n)) {
                            continue;
                        }
                        if bodies.iter().any(|b| known.get(b).and_then(|e| e.get(i + 1)).copied().unwrap_or(true)) {
                            return true;
                        }
                    }
                }
                // Storing a lent closure into a variable that owns one is
                // refused by the checker (E0902, fix `f.clone()`), so a store
                // here leaves the caller's closure the caller's. It still has
                // to be on the heap.
                MirStmtKind::Store { addr, value: MirOperand::Local(id), .. }
                    if is_it(id)
                        && rule == CaptureRule::HeapClosure
                        && matches!(func.local_ty(*addr), Some(crate::MirType::FuncPtr(_))) => {}
                MirStmtKind::Store { value: MirOperand::Local(id), .. } if is_it(id) => {
                    return true;
                }
                _ => {}
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. }
                if is_it(id) => return true,
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
    kept: &KeptThroughCalls,
    lent: &LentCells,
) -> HashSet<LocalId> {
    // Lowering routinely copies the `ClosureCreate` result on before returning
    // it, so reading only the original destination missed the escape. Every
    // alias answers for the closure it came from, and the answer is recorded
    // against that closure — `decide_allocation` looks up the `ClosureCreate`
    // destination, which is the origin, never a copy of it.
    let aliases = closure_aliases(func, closure_locals);
    let mut escaping: HashSet<LocalId> = HashSet::new();
    let addr_of: HashMap<LocalId, LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Ref(src) } => Some((*dst, *src)),
            _ => None,
        })
        .collect();

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { func: callee, args, .. } => {
                    for (arg_idx, arg) in args.iter().enumerate() {
                        if let Some(id) = uses::operand_local(arg) {
                            for &origin in aliases.origins(&id) {
                                // A bodiless runtime helper has no escape map
                                // to read, and "unaccounted for" has to mean
                                // "might keep it". The ones that demonstrably
                                // don't are written down instead.
                                let is_borrow = callee_escapes.get(&callee.name)
                                    .and_then(|e| e.get(arg_idx))
                                    .map(|escapes| !escapes)
                                    .unwrap_or_else(|| {
                                        uses_up_callback(callee)
                                    });

                                if !is_borrow {
                                    escaping.insert(origin);
                                }
                            }
                            // The variable itself lent to a `mutate` parameter:
                            // the callee may replace what it holds.
                            if let Some(var) = addr_of.get(&id) {
                                if lent.mutate_params.contains(&(callee.name.clone(), arg_idx)) {
                                    escaping.extend(aliases.origins(var).iter().copied());
                                }
                            }
                        }
                    }
                }
                MirStmtKind::ClosureCall { closure, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if aliases.holds_closure(&id) && kept.keeps(&func.name, *closure, i) {
                            escaping.extend(aliases.origins(&id).iter().copied());
                        }
                        // The variable lent to a body's `mutate` parameter, as
                        // with a call by name above (#2017).
                        if let Some(var) = addr_of.get(&id) {
                            if kept.lends_to_mutate(&func.name, *closure, i, lent) {
                                escaping.extend(aliases.origins(var).iter().copied());
                            }
                        }
                    }
                }
                // Three ways to put a closure somewhere the frame does not
                // control: through a pointer, into a fixed-size array, or
                // inside an interface box.
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. }
                | MirStmtKind::InterfaceBox { value: MirOperand::Local(id), .. } => {
                    escaping.extend(aliases.origins(id).iter().copied());
                }
                // A captured variable belongs to the frame that made the
                // environment, so a closure assigned to it outlives this one.
                // `f = || v[2]` in a `for` body over a sequence is this: the
                // body is its own function, and the write only becomes a store
                // once `addr_taken` runs, after this decision. Left on the
                // stack, `f` pointed into the body's frame after it returned.
                MirStmtKind::LoadCapture { dst, .. } => {
                    escaping.extend(aliases.origins(dst).iter().copied());
                }
                // The other side of the same trade: what this frame puts in a
                // variable a body it creates assigns closures to or hands on
                // (`captures_holding_heap_closures`).
                MirStmtKind::ClosureCreate { func_name, captures, .. } => {
                    for c in captures.iter().filter(|c| c.by_ref && lent.slots.contains(&(func_name.clone(), c.offset))) {
                        escaping.extend(aliases.origins(&c.local_id).iter().copied());
                    }
                }
                _ => {}
            }
        }

        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                escaping.extend(aliases.origins(id).iter().copied());
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
                for &inner in aliases.origins(cap) {
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
/// `ClosureCreate` destinations it may have come from — itself, for an
/// original.
///
/// A set, not one origin: a `mut` name reassigned from one closure to another
/// holds either, depending on where you are. With one origin per local the
/// fixed point below never settled — each pass moved the name from the first
/// closure to the second and back, and `rask compile` spun forever on
///
/// ```text
/// mut f = || { dropped += 1 }
/// f = || { seen += 1 }
/// spawn { f() }
/// ```
///
/// (#1335). The analyses that read this are all "might": a closure a local
/// might hold escapes if the local does, so each answers for every origin.
pub(crate) struct ClosureAliases {
    map: HashMap<LocalId, Vec<LocalId>>,
}

impl ClosureAliases {
    /// The closures `id` may hold; empty when it holds none.
    pub(crate) fn origins(&self, id: &LocalId) -> &[LocalId] {
        self.map.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn holds_closure(&self, id: &LocalId) -> bool {
        !self.origins(id).is_empty()
    }
}

fn closure_aliases(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
) -> ClosureAliases {
    let mut map: HashMap<LocalId, Vec<LocalId>> =
        closure_locals.keys().map(|id| (*id, vec![*id])).collect();
    // Sets only grow, so this settles.
    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                // A phi is a copy from whichever edge came in. Once SSA has
                // run, `f = || …` in a loop is `f = phi [first, made_here]`,
                // and skipping it lost track of what `f` holds: the drop pass
                // freed the new closure at the end of the turn and the first
                // one at the return, and handed back the freed one.
                let (dst, srcs): (&LocalId, Vec<LocalId>) = match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: crate::MirRValue::Use(MirOperand::Local(src)) } => {
                        (dst, vec![*src])
                    }
                    MirStmtKind::Phi { dst, args } => (
                        dst,
                        args.iter()
                            .filter_map(|(_, op)| match op {
                                MirOperand::Local(l) => Some(*l),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => continue,
                };
                let from: Vec<LocalId> =
                    srcs.iter().filter_map(|s| map.get(s)).flatten().copied().collect();
                if from.is_empty() {
                    continue;
                }
                let into = map.entry(*dst).or_default();
                for origin in from {
                    if !into.contains(&origin) {
                        into.push(origin);
                        changed = true;
                    }
                }
            }
        }
    }
    ClosureAliases { map }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, MirBlock, MirConst, MirLocal, MirTerminator, MirType};
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

    fn temp(id: u32, ty: MirType) -> MirLocal { MirLocal { id: LocalId(id), name: None, ty, is_param: false } }

    fn param(id: u32, ty: MirType) -> MirLocal { MirLocal { id: LocalId(id), name: None, ty, is_param: true } }

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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
                    }),
                ], ret(Some(MirOperand::Local(LocalId(0))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
            source_file: None,
        }];

        run_closure_passes(&mut fns);

        assert!(get_heap(&fns[0]), "returned closure must stay heap");
        assert!(!has_drop(&fns[0]), "returned closure should not be dropped");
    }

    #[test]
    fn unknown_callee_assumes_transfer() {
        // Closure passed to a callee not in the fn set → heap, no drop
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
                        task_bound: false,
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("keep_it".to_string()),
                        args: vec![MirOperand::Local(LocalId(0))],
                    }),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        store_size: 8,
                        field: None,
                    }),
                ], ret(None)),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
            source_file: None,
        };

        let mut fns = vec![store_fn, caller_fn];
        run_closure_passes(&mut fns);
        let main = fns.iter().find(|f| f.name == "main").unwrap();

        assert!(get_heap(main), "closure to escaping callee must be heap");
        assert!(!has_drop(main), "ownership transferred — no drop");
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),  // return inner
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
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
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
                        task_bound: false,
                    }),
                ], ret(Some(MirOperand::Local(LocalId(1))))),
                block(2, vec![],
                    ret(Some(MirOperand::Constant(MirConst::Int(0))))),
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
    fn closure_in_loop_owning_capture_gets_back_edge_drop() {
        // Closure in loop body whose environment owns what it carried, called
        // locally. Stack, but dropping it releases the capture: drop at the
        // back-edge.
        //
        //   block0: goto block1
        //   block1: branch(cond, block2, block3)
        //   block2:
        //     _1 = ClosureCreate[_3 owns]
        //     _2 = ClosureCall(_1)
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
                temp(3, MirType::Ptr),
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
                        captures: vec![crate::ClosureCapture {
                            local_id: LocalId(3),
                            offset: 0,
                            size: 8,
                            by_ref: false,
                            copy: false,
                            lent: false,
                            owns: Some(MirType::Ptr),
                        }],
                        heap: false,
                        task_bound: false,
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
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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

    #[test]
    fn drop_under_a_copy_names_its_create() {
        // _0 = closure; _1 = _0; call _1 — the frame frees it, under whichever
        // name, and says it was made as _0.
        let func = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::Ptr)],
            blocks: vec![block(0, vec![
                MirStmt::dummy(MirStmtKind::ClosureCreate {
                    dst: LocalId(0),
                    func_name: "f__closure_0".to_string(),
                    captures: vec![],
                    heap: true,
                    task_bound: false,
                }),
                MirStmt::dummy(MirStmtKind::Assign {
                    dst: LocalId(1),
                    rvalue: crate::MirRValue::Use(MirOperand::Local(LocalId(0))),
                }),
                MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("keep_nothing".to_string()),
                    args: vec![MirOperand::Local(LocalId(1))],
                }),
            ], ret(None))],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
            source_file: None,
        };
        let mut fns = vec![func];
        let escapes = HashMap::from([("keep_nothing".to_string(), vec![false])]);
        insert_drops(&mut fns[0], &escapes, &HashSet::new(), &crate::closure_targets::ClosureTargets::build(&[]));
        let made: Vec<Option<LocalId>> = fns[0].blocks[0]
            .statements
            .iter()
            .filter_map(|s| match &s.kind {
                MirStmtKind::ClosureDrop { made, .. } => Some(*made),
                _ => None,
            })
            .collect();
        assert_eq!(made, vec![Some(LocalId(0))]);
    }
}

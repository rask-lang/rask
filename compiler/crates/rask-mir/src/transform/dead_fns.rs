// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Removes the functions inlining left without a caller.
//!
//! Inlining splices a body into every call site and leaves the callee in
//! place, so a function called once (IN3) outlives its last call. It still
//! counts for every whole-program question a later pass asks. Closure
//! specialization gives it `__for` copies of the closures it builds, and those
//! copies call the same shared bodies the live code does: a dead
//! `Sequence_map__yield_1__for1` that kept its `Vec` item made the live
//! `yield_1` behind the same `as_sequence` take it too, and the rows of
//! `g.map(|r| r.map(f)).flatten().to_vec()` were freed twice.
//!
//! Liveness starts from two kinds of root. One is what the caller says runs
//! from outside the program's own calls (`PassContext::outside_roots`). The
//! other is every function no body named before inlining: lowering made it for
//! something other than a call (a const's init thunk, drop glue codegen calls
//! by name), and inlining can't have been what orphaned it.

use std::collections::HashSet;

use crate::{MirFunction, MirRValue, MirStmtKind};

/// Every function `f` names: calls, addresses taken, closures and ensure hooks
/// built. A boxed interface's methods are roots of their own (the vtable is
/// built from monomorphization's table, not from the box).
pub(crate) fn named_by(f: &MirFunction) -> impl Iterator<Item = &str> {
    f.blocks.iter().flat_map(|b| &b.statements).filter_map(|stmt| match &stmt.kind {
        MirStmtKind::Call { func, .. } => Some(func.name.as_str()),
        MirStmtKind::Assign { rvalue: MirRValue::FuncAddr(n), .. } => Some(n.as_str()),
        MirStmtKind::ClosureCreate { func_name, .. } => Some(func_name.as_str()),
        MirStmtKind::EnsureHookRegister { thunk, .. } => Some(thunk.as_str()),
        _ => None,
    })
}

/// Every name any function in `fns` names.
pub(crate) fn named_anywhere(fns: &[MirFunction]) -> HashSet<String> {
    fns.iter().flat_map(named_by).map(str::to_string).collect()
}

/// Drop every function no root reaches. `named_before` is `named_anywhere`
/// taken before inlining.
pub(crate) fn remove_dead_functions(
    fns: &mut Vec<MirFunction>,
    named_before: &HashSet<String>,
    outside_roots: &HashSet<String>,
) {
    let mut live: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = fns
        .iter()
        .filter(|f| f.is_extern_c || !named_before.contains(&f.name) || outside_roots.contains(&f.name))
        .map(|f| f.name.as_str())
        .collect();
    let by_name: std::collections::HashMap<&str, &MirFunction> = fns.iter().map(|f| (f.name.as_str(), f)).collect();
    while let Some(name) = queue.pop() {
        if !live.insert(name) {
            continue;
        }
        if let Some(f) = by_name.get(name) {
            queue.extend(named_by(f));
        }
    }
    let live: HashSet<String> = live.into_iter().map(str::to_string).collect();
    fns.retain(|f| live.contains(&f.name));
}

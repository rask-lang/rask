// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Which closure bodies a call through a closure value can reach.
//!
//! The drop pass decides "this call handed me a container I now own" by looking
//! the callee up by name (`functions_that_hand_a_container_back`). A call
//! through a closure has no name, so the answer was always "somebody else's" —
//! and the container leaked. That is #943: `flat_map`'s closure builds a `Vec`
//! per element and nothing frees any of them.
//!
//! Freeing it unconditionally is a use-after-free, which is what makes this an
//! analysis rather than a one-liner. The issue's own example:
//!
//! ```text
//! const SHARED: Vec<i64> = [7, 8, 9]
//! func lookup() -> Vec<i64> { return SHARED }
//! keys.flat_map(|k| lookup())      // hands back SHARED itself
//! ```
//!
//! So the question is which bodies the call can reach, and the by-name machinery
//! answers the rest.
//!
//! Everything here leans to "unknown", because unknown costs a leak and a wrong
//! answer costs a free of memory somebody still holds:
//!
//!   - a local defined by anything this walk doesn't model is unknown
//!   - a closure body's own parameters are unknown, always: it is reached
//!     indirectly, so its call sites can't be enumerated by name
//!   - a target set is only usable when it is known *and* non-empty
//!
//! What is modelled is the one path that matters — a closure made in one
//! function, passed by name to another, captured by the adapter that function
//! returns, and called from inside it. That is every sequence adapter in
//! `stdlib/sequence.rk`.
//!
//! And one more: a closure kept in a struct field, `Plan { run: || … }` read
//! back as `p.run` (#1526). Each field is its own set: lowering records on a
//! store which field it writes, so `o.inner.run = f`, a store at `Outer`'s
//! byte 16, still counts for `Inner.run`. A store of a closure that lowering
//! didn't name a field for goes into a set every field read includes, unless
//! it lands in something no struct field read can see — an optional's payload,
//! an enum's, a tuple's.

use std::collections::{HashMap, HashSet};

use crate::{BlockId, FieldSlot, LocalId, MirFunction, MirOperand, MirRValue, MirStmtKind, MirType};

/// What a value might be, as far as this walk can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Flow {
    /// Every closure body that can reach here.
    Known(HashSet<String>),
    /// A flow this walk doesn't model reached here.
    Unknown,
}

impl Flow {
    fn merge(&mut self, other: &Flow) -> bool {
        match (&mut *self, other) {
            (Flow::Unknown, _) => false,
            (_, Flow::Unknown) => {
                *self = Flow::Unknown;
                true
            }
            (Flow::Known(mine), Flow::Known(theirs)) => {
                let before = mine.len();
                mine.extend(theirs.iter().cloned());
                mine.len() != before
            }
        }
    }
}

/// The closure bodies each `ClosureCall` in the program can reach.
pub struct ClosureTargets {
    locals: HashMap<(String, LocalId), Flow>,
}

impl ClosureTargets {
    pub fn build(fns: &[MirFunction]) -> Self {
        Self::walk(fns, false)
    }

    /// `build`, also following a closure out of the function that returns it:
    /// `pair()` hands back `pair`'s adapter, so `for f in pair()` calls a body
    /// this can name. The plain walk calls every call result unknown.
    ///
    /// Separate because the passes reading `build` act on a known set — they
    /// free, specialise, adopt — and were tuned against what it knows. The one
    /// reader of this answers "might this call keep its argument", where a
    /// wider known set only ever means fewer guesses.
    pub fn build_following_returns(fns: &[MirFunction]) -> Self {
        Self::walk(fns, true)
    }

    fn walk(fns: &[MirFunction], follow_returns: bool) -> Self {
        let with_body: HashSet<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        let mut returns: HashMap<String, Flow> = HashMap::new();
        // A function named by a `ClosureCreate` is reached through a pointer, so
        // its arguments come from call sites this walk can't name. Its own
        // parameters are unknown for good.
        let mut is_closure_body: HashSet<&str> = HashSet::new();
        for func in fns {
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                if let MirStmtKind::ClosureCreate { func_name, .. } = &stmt.kind {
                    is_closure_body.insert(func_name.as_str());
                }
            }
        }

        let mut locals: HashMap<(String, LocalId), Flow> = HashMap::new();
        let mut params: HashMap<(String, usize), Flow> = HashMap::new();
        let mut captures: HashMap<(String, u32), Flow> = HashMap::new();
        // What each struct field may hold, by layout and field index.
        let mut fields: HashMap<FieldSlot, Flow> = HashMap::new();
        // Closures stored where no field was named, which any field may hold.
        let mut unattributed = Flow::Known(HashSet::new());

        for func in fns {
            if is_closure_body.contains(func.name.as_str()) {
                for (i, _) in func.params.iter().enumerate() {
                    params.insert((func.name.clone(), i), Flow::Unknown);
                }
            }
            // A local whose definition this walk doesn't model can hold
            // anything. Recorded first so a later merge can't downgrade it back
            // to a known set.
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                let Some(dst) = crate::analysis::uses::stmt_def(stmt) else { continue };
                let followed_return = follow_returns
                    && matches!(&stmt.kind, MirStmtKind::Call { func: f, .. } if with_body.contains(f.name.as_str()));
                if !modelled_def(func, stmt) && !followed_return {
                    locals.insert((func.name.clone(), dst), Flow::Unknown);
                }
            }
        }

        // A closure variable lent to a call (`swap(mutate s)`) may come back
        // holding whatever the callee stored through its parameter. What each
        // function stores through each pointer parameter, `*(p) = f`, by
        // function and parameter index. Read as still holding what it held,
        // `s(mutate g, 3)` after `swap` put another function in `s` was taken
        // for a call of the old one.
        let mut param_stores: HashMap<(String, usize), Flow> = HashMap::new();
        let lending: HashMap<&str, Lending> = fns.iter().map(|f| (f.name.as_str(), Lending::of(f))).collect();
        // Calls through a closure whose target isn't known yet. Once the walk
        // settles, any still unknown could have stored anything.
        let mut settled_unknown: HashSet<(String, LocalId, usize)> = HashSet::new();

        loop {
            let mut grew = false;
            for func in fns {
                let name = &func.name;
                let lend = &lending[name.as_str()];
                // Writes a call made through argument `a` land where `a`
                // points: the caller's variable, or the caller's own pointer
                // parameter, which passes them on to its caller.
                let mut land = |locals: &mut HashMap<(String, LocalId), Flow>,
                                param_stores: &mut HashMap<(String, usize), Flow>,
                                a: LocalId,
                                written: &Flow|
                 -> bool {
                    let mut grew = false;
                    if let Some(src) = lend.refs.get(&a).filter(|src| is_closure(func, **src)) {
                        grew |= merge_into(locals, (name.clone(), *src), written);
                    }
                    if let Some(i) = lend.ptr_params.get(&a) {
                        grew |= merge_into(param_stores, (name.clone(), *i), written);
                    }
                    grew
                };
                // Parameters seed their locals.
                for (i, p) in func.params.iter().enumerate() {
                    if let Some(flow) = params.get(&(name.clone(), i)).cloned() {
                        grew |= merge_into(&mut locals, (name.clone(), p.id), &flow);
                    }
                }
                for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                    match &stmt.kind {
                        MirStmtKind::ClosureCreate { dst, func_name, captures: caps, .. } => {
                            let mine = Flow::Known([func_name.clone()].into_iter().collect());
                            grew |= merge_into(&mut locals, (name.clone(), *dst), &mine);
                            for cap in caps {
                                let Some(flow) =
                                    locals.get(&(name.clone(), cap.local_id)).cloned()
                                else {
                                    continue;
                                };
                                grew |= merge_into(
                                    &mut captures,
                                    (func_name.clone(), cap.offset),
                                    &flow,
                                );
                            }
                        }
                        MirStmtKind::LoadCapture { dst, offset, .. } => {
                            if let Some(flow) = captures.get(&(name.clone(), *offset)).cloned() {
                                grew |= merge_into(&mut locals, (name.clone(), *dst), &flow);
                            }
                        }
                        MirStmtKind::Assign { dst, rvalue } if is_closure(func, *dst) => {
                            if let Some(slot) = struct_field_read(func, rvalue) {
                                let mut flow = fields.get(&slot).cloned().unwrap_or(Flow::Known(HashSet::new()));
                                flow.merge(&unattributed);
                                grew |= merge_into(&mut locals, (name.clone(), *dst), &flow);
                                continue;
                            }
                            let Some(src) = copied_from(rvalue) else { continue };
                            if let Some(flow) = locals.get(&(name.clone(), src)).cloned() {
                                grew |= merge_into(&mut locals, (name.clone(), *dst), &flow);
                            }
                        }
                        MirStmtKind::Store { addr, value: MirOperand::Local(v), field, .. }
                            if is_closure(func, *v) =>
                        {
                            let Some(flow) = locals.get(&(name.clone(), *v)).cloned() else { continue };
                            grew |= land(&mut locals, &mut param_stores, *addr, &flow);
                            match field {
                                Some(slot) => grew |= merge_into(&mut fields, *slot, &flow),
                                None if no_field_sees(func, *addr) => {}
                                None => grew |= unattributed.merge(&flow),
                            }
                        }
                        MirStmtKind::Assign { dst, rvalue } => {
                            let Some(src) = copied_from(rvalue) else { continue };
                            if let Some(flow) = locals.get(&(name.clone(), src)).cloned() {
                                grew |= merge_into(&mut locals, (name.clone(), *dst), &flow);
                            }
                        }
                        MirStmtKind::Phi { dst, args } => {
                            for (_, arg) in args {
                                let MirOperand::Local(src) = arg else { continue };
                                let Some(flow) = locals.get(&(name.clone(), *src)).cloned()
                                else {
                                    continue;
                                };
                                grew |= merge_into(&mut locals, (name.clone(), *dst), &flow);
                            }
                        }
                        MirStmtKind::Call { func: fref, args, dst } => {
                            if let Some((d, src)) = clone_of(stmt) {
                                if let Some(flow) = locals.get(&(name.clone(), src)).cloned() {
                                    grew |= merge_into(&mut locals, (name.clone(), d), &flow);
                                }
                            }
                            if follow_returns {
                                if let (Some(d), Some(flow)) = (dst, returns.get(&fref.name).cloned()) {
                                    grew |= merge_into(&mut locals, (name.clone(), *d), &flow);
                                }
                            }
                            for (i, arg) in args.iter().enumerate() {
                                let MirOperand::Local(a) = arg else { continue };
                                if lend.lends(*a) {
                                    let written = if with_body.contains(fref.name.as_str()) {
                                        param_stores.get(&(fref.name.clone(), i)).cloned()
                                    } else {
                                        Some(Flow::Unknown)
                                    };
                                    if let Some(w) = written {
                                        grew |= land(&mut locals, &mut param_stores, *a, &w);
                                    }
                                }
                                let Some(flow) = locals.get(&(name.clone(), *a)).cloned() else {
                                    continue;
                                };
                                grew |=
                                    merge_into(&mut params, (fref.name.clone(), i), &flow);
                            }
                        }
                        // Through a closure: argument `i` is parameter `i + 1`
                        // of each body it reaches, after the environment.
                        MirStmtKind::ClosureCall { closure, args, .. } => {
                            let bodies = match locals.get(&(name.clone(), *closure)) {
                                Some(Flow::Known(set)) if !set.is_empty() => Some(set.clone()),
                                _ => None,
                            };
                            for (i, arg) in args.iter().enumerate() {
                                let MirOperand::Local(a) = arg else { continue };
                                if !lend.lends(*a) {
                                    continue;
                                }
                                let written = match &bodies {
                                    Some(set) => {
                                        let mut w = Flow::Known(HashSet::new());
                                        for b in set {
                                            if let Some(f) = param_stores.get(&(b.clone(), i + 1)) {
                                                w.merge(f);
                                            }
                                        }
                                        w
                                    }
                                    None if settled_unknown.contains(&(name.clone(), *closure, i)) => Flow::Unknown,
                                    None => continue,
                                };
                                grew |= land(&mut locals, &mut param_stores, *a, &written);
                            }
                        }
                        _ => {}
                    }
                }
                if follow_returns {
                    for block in &func.blocks {
                        let returned = match &block.terminator.kind {
                            crate::MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
                            | crate::MirTerminatorKind::CleanupReturn {
                                value: Some(MirOperand::Local(id)),
                                ..
                            } => *id,
                            _ => continue,
                        };
                        // Not a closure, or not one this walk can follow: the
                        // caller's result is unknown, the same as before.
                        let flow =
                            locals.get(&(name.clone(), returned)).cloned().unwrap_or(Flow::Unknown);
                        grew |= merge_into(&mut returns, name.clone(), &flow);
                    }
                }
            }
            if !grew {
                // Settled. A call whose target is still unknown could have
                // stored anything through what it was lent; walk again with
                // that.
                let before = settled_unknown.len();
                for func in fns {
                    let lend = &lending[func.name.as_str()];
                    for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                        let MirStmtKind::ClosureCall { closure, args, .. } = &st.kind else { continue };
                        if matches!(locals.get(&(func.name.clone(), *closure)), Some(Flow::Known(set)) if !set.is_empty()) {
                            continue;
                        }
                        for (i, arg) in args.iter().enumerate() {
                            if matches!(arg, MirOperand::Local(a) if lend.lends(*a)) {
                                settled_unknown.insert((func.name.clone(), *closure, i));
                            }
                        }
                    }
                }
                if settled_unknown.len() == before {
                    break;
                }
            }
        }

        Self { locals }
    }

    /// Every `ClosureCall` that can reach each closure body, as (function,
    /// block, statement), for the bodies where all of them can be named.
    ///
    /// The forward question ("which bodies can this call reach") answers the
    /// backward one only when every call that could reach the body answers
    /// it. A call through a value this walk lost track of could be calling
    /// any body that takes as many arguments, so those bodies are left out:
    /// `g()` on a closure variable rules out every body with no parameters,
    /// and nothing else. A body handed to a function
    /// with no MIR (the runtime, `spawn`) is called from places nobody can
    /// list, so it is left out, and so is every body when a closure the walk
    /// lost track of is handed there.
    pub fn calls_reaching(&self, fns: &[MirFunction]) -> Option<HashMap<String, Vec<(String, BlockId, usize)>>> {
        let with_body: HashSet<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        let mut sites: HashMap<String, Vec<(String, BlockId, usize)>> = HashMap::new();
        let mut external: HashSet<String> = HashSet::new();
        // A call the walk lost track of could reach any body that takes as
        // many arguments as it passes, and none that doesn't.
        let mut unknown_arities: HashSet<usize> = HashSet::new();
        for func in fns {
            for block in &func.blocks {
                for (si, st) in block.statements.iter().enumerate() {
                    match &st.kind {
                        MirStmtKind::ClosureCall { closure, args, .. } => match self.known(&func.name, *closure) {
                            Some(bodies) => {
                                for body in bodies {
                                    sites.entry(body.clone()).or_default().push((func.name.clone(), block.id, si));
                                }
                            }
                            None => {
                                unknown_arities.insert(args.len());
                            }
                        },
                        MirStmtKind::Call { func: callee, args, .. } if !with_body.contains(callee.name.as_str()) => {
                            for arg in args {
                                let MirOperand::Local(a) = arg else { continue };
                                if !matches!(func.local_ty(*a), Some(MirType::FuncPtr(_))) {
                                    continue;
                                }
                                external.extend(self.known(&func.name, *a)?.iter().cloned());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        // A body no call reaches still has a (empty) listing, unless it got out.
        for func in fns {
            for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                if let MirStmtKind::ClosureCreate { func_name, .. } = &st.kind {
                    sites.entry(func_name.clone()).or_default();
                }
            }
        }
        let arity = |body: &str| fns.iter().find(|f| f.name == body).map(|f| f.params.len().saturating_sub(1));
        sites.retain(|body, _| {
            !external.contains(body) && arity(body).is_some_and(|n| !unknown_arities.contains(&n))
        });
        Some(sites)
    }

    /// The bodies `local` in `func` may hold, when that is fully known and not
    /// empty. `None` is the answer for everything else, and the caller's cue to
    /// leave the value alone.
    pub fn known(&self, func: &str, local: LocalId) -> Option<&HashSet<String>> {
        match self.locals.get(&(func.to_string(), local)) {
            Some(Flow::Known(set)) if !set.is_empty() => Some(set),
            _ => None,
        }
    }
}

fn merge_into<K: std::hash::Hash + Eq>(
    map: &mut HashMap<K, Flow>,
    key: K,
    flow: &Flow,
) -> bool {
    match map.get_mut(&key) {
        Some(existing) => existing.merge(flow),
        None => {
            map.insert(key, flow.clone());
            true
        }
    }
}

/// The local a value was copied from, for the shapes that carry a closure
/// through unchanged. A by-ref capture holds the slot's address, so the body
/// reads it with a `LoadCapture` and then a `Deref`.
/// Where a function's pointers point, for following stores through them:
/// `&x` taken into a local, and the pointer parameters.
struct Lending {
    /// `p = &x`: `p` to `x`.
    refs: HashMap<LocalId, LocalId>,
    /// Pointer parameters, to their index.
    ptr_params: HashMap<LocalId, usize>,
}

impl Lending {
    fn of(func: &MirFunction) -> Self {
        let refs = func
            .blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|st| match &st.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } => Some((*dst, *src)),
                _ => None,
            })
            .collect();
        let ptr_params = func
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.ty, MirType::Ptr))
            .map(|(i, p)| (p.id, i))
            .collect();
        Self { refs, ptr_params }
    }

    /// Does handing `a` to a call let the callee write somewhere this walk
    /// tracks?
    fn lends(&self, a: LocalId) -> bool {
        self.refs.contains_key(&a) || self.ptr_params.contains_key(&a)
    }
}

fn copied_from(rvalue: &MirRValue) -> Option<LocalId> {
    match rvalue {
        MirRValue::Use(MirOperand::Local(src)) | MirRValue::Deref(MirOperand::Local(src)) => {
            Some(*src)
        }
        MirRValue::Ref(src) => Some(*src),
        _ => None,
    }
}

/// Is this statement's definition of its destination one the walk follows?
///
/// Everything else makes the destination unknown, which is what keeps a set
/// from being *partly* right — a local written on one path by a
/// `ClosureCreate` and on another by something unmodelled would otherwise look
/// like it could only be the one closure.
fn modelled_def(func: &MirFunction, stmt: &crate::MirStmt) -> bool {
    match &stmt.kind {
        MirStmtKind::ClosureCreate { .. } | MirStmtKind::LoadCapture { .. } => true,
        MirStmtKind::Assign { dst, rvalue } => {
            copied_from(rvalue).is_some()
                || (is_closure(func, *dst) && struct_field_read(func, rvalue).is_some())
        }
        MirStmtKind::Phi { .. } => true,
        _ => clone_of(stmt).is_some(),
    }
}

/// `dst = rask_closure_clone(src)`: a copy of the environment, running the
/// same body (mem.closures/CV2). `let src = p.run.clone()` hid which closure
/// `src()` called, so its result had no owner.
fn clone_of(stmt: &crate::MirStmt) -> Option<(LocalId, LocalId)> {
    match &stmt.kind {
        MirStmtKind::Call { dst: Some(d), func, args } if func.name == "rask_closure_clone" => {
            match args.first() {
                Some(MirOperand::Local(src)) => Some((*d, *src)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_closure(func: &MirFunction, local: LocalId) -> bool {
    matches!(func.local_ty(local), Some(MirType::FuncPtr(_)))
}

/// The field `rvalue` reads, when it reads one out of a struct.
fn struct_field_read(func: &MirFunction, rvalue: &MirRValue) -> Option<FieldSlot> {
    let MirRValue::Field { base: MirOperand::Local(base), field_index, .. } = rvalue else {
        return None;
    };
    let Some(MirType::Struct(sid)) = func.local_ty(*base) else { return None };
    Some(FieldSlot { layout: sid.id, field: *field_index })
}

/// Memory no struct field read can see: a wrapper's or an enum's payload, a
/// tuple's slot, an array's element. A closure stored there comes back out
/// through that shape's own read, which this walk doesn't follow.
fn no_field_sees(func: &MirFunction, addr: LocalId) -> bool {
    matches!(
        func.local_ty(addr),
        Some(
            MirType::Option(_)
                | MirType::Result { .. }
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Array { .. }
        )
    )
}

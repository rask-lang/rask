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

use std::collections::{HashMap, HashSet};

use crate::{LocalId, MirFunction, MirOperand, MirRValue, MirStmtKind};

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
                if !modelled_def(stmt) {
                    locals.insert((func.name.clone(), dst), Flow::Unknown);
                }
            }
        }

        loop {
            let mut grew = false;
            for func in fns {
                let name = &func.name;
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
                        MirStmtKind::Call { func: fref, args, .. } => {
                            for (i, arg) in args.iter().enumerate() {
                                let MirOperand::Local(a) = arg else { continue };
                                let Some(flow) = locals.get(&(name.clone(), *a)).cloned() else {
                                    continue;
                                };
                                grew |=
                                    merge_into(&mut params, (fref.name.clone(), i), &flow);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if !grew {
                break;
            }
        }

        Self { locals }
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
fn modelled_def(stmt: &crate::MirStmt) -> bool {
    match &stmt.kind {
        MirStmtKind::ClosureCreate { .. } | MirStmtKind::LoadCapture { .. } => true,
        MirStmtKind::Assign { rvalue, .. } => copied_from(rvalue).is_some(),
        MirStmtKind::Phi { .. } => true,
        _ => false,
    }
}

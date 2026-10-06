// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Every closure body a call through a closure value might reach, as a
//! superset, for every call in the program.
//!
//! `ClosureTargets` answers "which bodies, exactly" and gives up whenever a
//! value travels somewhere it doesn't follow. That is the right answer for a
//! pass that acts on the target, and no answer at all for a question that only
//! needs a bound: "does any body this call could reach keep its argument?"
//! A closure loaded out of a `Vec` has no exact answer, so the argument was
//! counted as kept and leaked (#1469).
//!
//! The bound comes from noticing that a function value only comes into being
//! at a `ClosureCreate` (a closure literal, or the wrapper lowering builds for
//! a named function used as a value), and the program is lowered whole. So a
//! value this walk loses track of can only be a body that, somewhere, went
//! where the walk doesn't follow: into memory, into the runtime, through a
//! vtable. Those bodies are the *escaped* set, and a value the walk can't
//! name is any one of them.
//!
//! The invariant everything below keeps: whenever a known value reaches a
//! place the walk doesn't model, its bodies join the escaped set. A loop body
//! handed straight to the sequence that calls it never escapes, which is what
//! keeps `to_vec`'s body (it keeps its item) out of the answer for an
//! unrelated closure pulled from a `Vec`.
//!
//! A local with no entry at all was never given a closure: every definition
//! the walk doesn't follow is recorded as unknown up front, so what's left
//! unrecorded is a constant, or a parameter nobody passes one to. That covers
//! a function nothing calls, too, which fusion leaves behind for every chain it
//! folds into a loop. Its body never runs, so it hands nothing anywhere.

use std::collections::{HashMap, HashSet};

use crate::{
    LocalId, MirFunction, MirOperand, MirRValue, MirStmtKind, MirTerminatorKind,
};

/// What a local may hold.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Held {
    /// These bodies. `aliased` means the slot's address is out, so a store
    /// through it may also have left any escaped body there.
    Known { bodies: HashSet<String>, aliased: bool },
    /// Any escaped body.
    Unknown,
}

impl Held {
    fn of(body: &str) -> Self {
        Held::Known { bodies: [body.to_string()].into_iter().collect(), aliased: false }
    }

    fn nothing() -> Self {
        Held::Known { bodies: HashSet::new(), aliased: false }
    }

    fn aliased() -> Self {
        Held::Known { bodies: HashSet::new(), aliased: true }
    }

    fn bodies(&self) -> Option<&HashSet<String>> {
        match self {
            Held::Known { bodies, .. } => Some(bodies),
            Held::Unknown => None,
        }
    }

    /// Calling through this can reach a body the walk didn't route to.
    fn reaches_unrouted(&self) -> bool {
        matches!(self, Held::Unknown | Held::Known { aliased: true, .. })
    }
}

/// The result: what each local may hold, and the escaped set.
pub struct ClosureReach {
    locals: HashMap<(String, LocalId), Held>,
    escaped: HashSet<String>,
}

impl ClosureReach {
    /// Every body `local` in `func` may hold when it's called. A superset:
    /// a body outside it can't be there.
    pub fn may_hold(&self, func: &str, local: LocalId) -> HashSet<&str> {
        let mut out: HashSet<&str> = HashSet::new();
        let Some(held) = self.locals.get(&(func.to_string(), local)) else { return out };
        if let Some(bodies) = held.bodies() {
            out.extend(bodies.iter().map(String::as_str));
        }
        if held.reaches_unrouted() {
            out.extend(self.escaped.iter().map(String::as_str));
        }
        out
    }

    pub fn build(fns: &[MirFunction]) -> Self {
        Walk::new(fns).run()
    }
}

struct Walk<'a> {
    fns: &'a [MirFunction],
    with_body: HashSet<&'a str>,
    /// Called from somewhere this walk can't see: a vtable, the runtime, C.
    /// Their parameters are unknown and what they return escapes. Escaped
    /// closure bodies join them as they're found.
    entered_unseen: HashSet<String>,
    /// Functions whose environment a `ClosureCreate` or an ensure hook builds,
    /// so the walk knows what their captures hold.
    env_built: HashSet<&'a str>,
    locals: HashMap<(String, LocalId), Held>,
    params: HashMap<(String, usize), Held>,
    captures: HashMap<(String, u32), Held>,
    returns: HashMap<String, Held>,
    escaped: HashSet<String>,
    changed: bool,
}

impl<'a> Walk<'a> {
    fn new(fns: &'a [MirFunction]) -> Self {
        let with_body: HashSet<&str> = fns.iter().map(|f| f.name.as_str()).collect();

        let mut entered_unseen: HashSet<String> = HashSet::new();
        let mut interface_methods: HashSet<&str> = HashSet::new();
        let mut env_built: HashSet<&str> = HashSet::new();
        for func in fns {
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                match &stmt.kind {
                    MirStmtKind::ClosureCreate { func_name, .. } => {
                        env_built.insert(func_name.as_str());
                    }
                    MirStmtKind::EnsureHookRegister { thunk, .. } => {
                        env_built.insert(thunk.as_str());
                        entered_unseen.insert(thunk.clone());
                    }
                    MirStmtKind::Assign { rvalue: MirRValue::FuncAddr(name), .. } => {
                        entered_unseen.insert(name.clone());
                    }
                    MirStmtKind::InterfaceCall { method_name, .. } => {
                        interface_methods.insert(method_name.as_str());
                    }
                    _ => {}
                }
            }
        }
        for func in fns {
            let name = func.name.as_str();
            // C can only hand back a closure it was given, so an escaped one.
            if func.is_extern_c {
                entered_unseen.insert(func.name.clone());
                continue;
            }
            // Anything that could sit in a vtable slot. By name, so a function
            // that only looks like an implementation is merely treated as
            // one, which costs precision and nothing else.
            let head = name.rsplit("::").next().unwrap_or(name);
            let base = head.split('$').next().unwrap_or(head);
            if interface_methods.iter().any(|m| base.ends_with(&format!("_{m}"))) {
                entered_unseen.insert(func.name.clone());
            }
        }

        Self {
            fns,
            with_body,
            entered_unseen,
            env_built,
            locals: HashMap::new(),
            params: HashMap::new(),
            captures: HashMap::new(),
            returns: HashMap::new(),
            escaped: HashSet::new(),
            changed: false,
        }
    }

    fn run(mut self) -> ClosureReach {
        // A definition this walk doesn't follow can hold anything.
        for func in self.fns {
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                let Some(dst) = crate::analysis::uses::stmt_def(stmt) else { continue };
                let followed = match &stmt.kind {
                    // An environment nothing visible built could hold anything.
                    MirStmtKind::LoadCapture { .. } => self.env_built.contains(func.name.as_str()),
                    MirStmtKind::ClosureCreate { .. }
                    | MirStmtKind::Phi { .. }
                    | MirStmtKind::ClosureCall { .. } => true,
                    MirStmtKind::Assign { rvalue, .. } => copied_from(rvalue).is_some(),
                    MirStmtKind::Call { func: f, .. } => self.with_body.contains(f.name.as_str()),
                    _ => false,
                };
                if !followed {
                    self.locals.insert((func.name.clone(), dst), Held::Unknown);
                }
            }
        }

        loop {
            self.changed = false;
            for func in self.fns {
                self.visit(func);
            }
            if !self.changed {
                break;
            }
        }
        ClosureReach { locals: self.locals, escaped: self.escaped }
    }

    fn unseen(&self, name: &str) -> bool {
        self.entered_unseen.contains(name) || self.escaped.contains(name)
    }

    fn held(&self, func: &str, op: &MirOperand) -> Option<Held> {
        let MirOperand::Local(id) = op else { return None };
        self.locals.get(&(func.to_string(), *id)).cloned()
    }

    fn escape(&mut self, held: Option<Held>) {
        let Some(Held::Known { bodies, .. }) = held else { return };
        for b in bodies {
            if self.escaped.insert(b) {
                self.changed = true;
            }
        }
    }

    fn escape_local(&mut self, func: &str, id: LocalId) {
        let held = self.locals.get(&(func.to_string(), id)).cloned();
        self.escape(held);
    }

    /// Merge `incoming` into a local. Known bodies merged into an unknown
    /// slot escape: whoever reads the slot treats it as any escaped body.
    fn into_local(&mut self, func: &str, id: LocalId, incoming: Held) {
        let key = (func.to_string(), id);
        let escaped = merge(&mut self.locals, key, incoming, &mut self.changed);
        self.escape(escaped);
    }

    fn into_param(&mut self, callee: &str, index: usize, incoming: Held) {
        let escaped = merge(&mut self.params, (callee.to_string(), index), incoming, &mut self.changed);
        self.escape(escaped);
    }

    fn into_return(&mut self, func: &str, incoming: Held) {
        let escaped = merge(&mut self.returns, func.to_string(), incoming, &mut self.changed);
        self.escape(escaped);
    }

    fn visit(&mut self, func: &MirFunction) {
        let name = func.name.as_str();
        let unseen = self.unseen(name);
        for (i, p) in func.params.iter().enumerate() {
            if unseen {
                self.into_local(name, p.id, Held::Unknown);
            }
            if let Some(h) = self.params.get(&(name.to_string(), i)).cloned() {
                self.into_local(name, p.id, h);
            }
        }

        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            match &stmt.kind {
                MirStmtKind::ClosureCreate { dst, func_name, captures, .. } => {
                    self.into_local(name, *dst, Held::of(func_name));
                    self.capture(name, func_name, captures);
                }
                MirStmtKind::EnsureHookRegister { thunk, captures } => {
                    self.capture(name, thunk, captures);
                }
                MirStmtKind::LoadCapture { dst, offset, access, .. } => {
                    if let Some(h) = self.captures.get(&(name.to_string(), *offset)).cloned() {
                        self.into_local(name, *dst, h);
                    }
                    if access.is_addressed() {
                        self.into_local(name, *dst, Held::aliased());
                    }
                }
                MirStmtKind::Assign { dst, rvalue } => match copied_from(rvalue) {
                    Some(src) => {
                        if let Some(h) = self.locals.get(&(name.to_string(), src)).cloned() {
                            self.into_local(name, *dst, h);
                        }
                        match rvalue {
                            // What a pointer reads is whatever was last stored there.
                            MirRValue::Deref(_) => self.into_local(name, *dst, Held::aliased()),
                            // The address is out, so a store may land in `src`.
                            MirRValue::Ref(_) => {
                                self.into_local(name, src, Held::aliased());
                                self.into_local(name, *dst, Held::aliased());
                            }
                            _ => {}
                        }
                    }
                    None => {
                        let mut used = Vec::new();
                        crate::analysis::uses::visit_rvalue_uses(rvalue, &mut |id| used.push(id));
                        for id in used {
                            self.escape_local(name, id);
                        }
                    }
                },
                MirStmtKind::Phi { dst, args } => {
                    for (_, op) in args {
                        if let Some(h) = self.held(name, op) {
                            self.into_local(name, *dst, h);
                        }
                    }
                }
                MirStmtKind::Call { dst, func: f, args } => {
                    if self.with_body.contains(f.name.as_str()) {
                        for (i, arg) in args.iter().enumerate() {
                            if let Some(h) = self.held(name, arg) {
                                self.into_param(&f.name, i, h);
                            }
                        }
                        if let (Some(d), Some(h)) = (dst, self.returns.get(&f.name).cloned()) {
                            self.into_local(name, *d, h);
                        }
                    } else {
                        for arg in args {
                            let h = self.held(name, arg);
                            self.escape(h);
                        }
                    }
                }
                MirStmtKind::ClosureCall { dst, closure, args } => {
                    let callee = self
                        .locals
                        .get(&(name.to_string(), *closure))
                        .cloned()
                        .unwrap_or_else(Held::nothing);
                    let mut unrouted = callee.reaches_unrouted();
                    for body in callee.bodies().cloned().unwrap_or_default() {
                        if !self.with_body.contains(body.as_str()) {
                            unrouted = true;
                            continue;
                        }
                        for (i, arg) in args.iter().enumerate() {
                            if let Some(h) = self.held(name, arg) {
                                self.into_param(&body, i + 1, h);
                            }
                        }
                        if let (Some(d), Some(h)) = (dst, self.returns.get(&body).cloned()) {
                            self.into_local(name, *d, h);
                        }
                    }
                    if unrouted {
                        for arg in args {
                            let h = self.held(name, arg);
                            self.escape(h);
                        }
                        if let Some(d) = dst {
                            self.into_local(name, *d, Held::Unknown);
                        }
                    }
                }
                // Bookkeeping on a value never hands it to anyone.
                MirStmtKind::ClosureDrop { .. }
                | MirStmtKind::ClosureRetain { .. }
                | MirStmtKind::RcInc { .. }
                | MirStmtKind::RcDec { .. }
                | MirStmtKind::RcDecContents { .. }
                | MirStmtKind::RcIncContents { .. } => {}
                // Writing to an address hands over the value, not the address.
                MirStmtKind::Store { value, .. } | MirStmtKind::ArrayStore { value, .. } => {
                    let h = self.held(name, value);
                    self.escape(h);
                }
                // Vtables, the runtime, everything else: what goes in escapes.
                _ => {
                    for id in crate::analysis::uses::stmt_uses(stmt) {
                        self.escape_local(name, id);
                    }
                }
            }
        }

        for block in &func.blocks {
            match &block.terminator.kind {
                MirTerminatorKind::Return { value: Some(op) }
                | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
                    if let Some(h) = self.held(name, op) {
                        if unseen {
                            self.escape(Some(h.clone()));
                        }
                        self.into_return(name, h);
                    }
                }
                _ => {
                    for id in crate::analysis::uses::terminator_uses(&block.terminator) {
                        self.escape_local(name, id);
                    }
                }
            }
        }
    }

    fn capture(&mut self, func: &str, body: &str, captures: &[crate::ClosureCapture]) {
        for cap in captures {
            if cap.by_ref {
                // The body can store through the address.
                self.into_local(func, cap.local_id, Held::aliased());
            }
            let Some(h) = self.locals.get(&(func.to_string(), cap.local_id)).cloned() else {
                continue;
            };
            let escaped =
                merge(&mut self.captures, (body.to_string(), cap.offset), h, &mut self.changed);
            self.escape(escaped);
        }
    }
}

/// Merge into `map[key]`. Returns what has to escape: known bodies arriving
/// at a slot that is already unknown.
fn merge<K: std::hash::Hash + Eq>(
    map: &mut HashMap<K, Held>,
    key: K,
    incoming: Held,
    changed: &mut bool,
) -> Option<Held> {
    let Some(existing) = map.get_mut(&key) else {
        map.insert(key, incoming);
        *changed = true;
        return None;
    };
    match (&mut *existing, incoming) {
        (Held::Unknown, incoming) => Some(incoming),
        (slot @ Held::Known { .. }, Held::Unknown) => {
            let old = std::mem::replace(slot, Held::Unknown);
            *changed = true;
            Some(old)
        }
        (
            Held::Known { bodies, aliased },
            Held::Known { bodies: more, aliased: more_aliased },
        ) => {
            let before = (bodies.len(), *aliased);
            bodies.extend(more);
            *aliased |= more_aliased;
            if (bodies.len(), *aliased) != before {
                *changed = true;
            }
            None
        }
    }
}

/// The local a value was copied from, for the shapes that carry a closure
/// through unchanged. Same shapes `ClosureTargets` follows.
fn copied_from(rvalue: &MirRValue) -> Option<LocalId> {
    match rvalue {
        MirRValue::Use(MirOperand::Local(src))
        | MirRValue::Deref(MirOperand::Local(src))
        | MirRValue::Ref(src) => Some(*src),
        _ => None,
    }
}

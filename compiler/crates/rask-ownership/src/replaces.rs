// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which parts of its parameters a function may write over, and which it may
//! read.
//!
//! A view of a field lasts until its block ends, and writing over the field
//! frees what the view reads (mem.borrowing/S1, S5). Between a field's move
//! and its refill, the field is empty, so nothing may read it (E0959). A call
//! does both through what it's handed, so the checker needs to know, per
//! parameter and per field path, what the callee may replace and what it may
//! read. Most calls touch little: `self.set_reg(…)` in a VM loop that holds a
//! view of `self.code` writes registers only. So the answer comes from the
//! bodies, worked out to a fixed point over the calls between them.
//!
//! A path is relative to the parameter: `["log"]` for `self.log = …`, `[]`
//! for the whole parameter. A call to something with no body here is taken to
//! read its arguments whole, and to replace each one it's given a `mutate`
//! marker for, and its receiver unless it's a stdlib container: `v.push` grows
//! a vector and replaces no handle. Being wrong that way can only refuse a
//! program, never let one through.

use std::collections::{HashMap, VecDeque};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{self, Visit};

use crate::OwnershipChecker;

/// A field path under a parameter.
pub(crate) type Path = Vec<String>;

/// What a function may do to one parameter.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct Effects {
    pub(crate) writes: Vec<Path>,
    pub(crate) reads: Vec<Path>,
}

/// Per function and method: each parameter in declaration order, by name.
pub(crate) type Replaces = HashMap<Key, Vec<(String, Effects)>>;

/// A free function by name, a method by the type it's declared on and its
/// name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Key {
    Fn(String),
    Method(String, String),
}

/// The longest path kept. A method that recurses through a field of its own
/// type (`self.next.clear()`) would grow its paths every round; a shorter path
/// covers everything under it, so cutting one short only refuses more.
const MAX_PATH: usize = 8;

impl<'a> OwnershipChecker<'a> {
    /// Fill `replaces` for every function and method in `groups`.
    ///
    /// A worklist: a body is worked out again only when a function it calls
    /// changed its answer, so a long chain of calls costs one pass per body,
    /// not one per link.
    pub(crate) fn infer_replaces(&mut self, groups: &[&[Decl]]) {
        let mut bodies: Vec<(Key, &FnDecl)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
                // A declaration whose body lives in the backend (`@native`,
                // `@builtin`) has none to read: it stays unknown, which reads
                // and replaces whole. An empty `{}` there isn't "does nothing".
                match &decl.kind {
                    DeclKind::Fn(f) if !f.body_lives_elsewhere() => bodies.push((Key::Fn(f.name.clone()), f)),
                    DeclKind::Impl(i) => {
                        let ty = i.target_ty.name().unwrap_or_default();
                        for m in i.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(ty.clone(), m.name.clone()), m));
                        }
                    }
                    DeclKind::Struct(st) => {
                        for m in st.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(st.name.clone(), m.name.clone()), m));
                        }
                    }
                    DeclKind::Enum(en) => {
                        for m in en.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(en.name.clone(), m.name.clone()), m));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut by_key: HashMap<Key, Vec<usize>> = HashMap::new();
        for (i, (key, _)) in bodies.iter().enumerate() {
            by_key.entry(key.clone()).or_default().push(i);
        }
        // Who calls whom, once: the keys a body's calls can reach.
        let mut callers: HashMap<Key, Vec<usize>> = HashMap::new();
        for (i, (_, f)) in bodies.iter().enumerate() {
            for callee in self.callees(&f.body) {
                callers.entry(callee).or_default().push(i);
            }
        }
        // Every declared function starts out touching nothing, so recursion
        // climbs from there: `ping` and `pong` calling each other end with
        // what they really write, not with "everything" for want of an answer.
        for (key, f) in &bodies {
            self.replaces
                .entry(key.clone())
                .or_insert_with(|| f.params.iter().map(|p| (p.name.clone(), Effects::default())).collect());
        }
        let mut results: Vec<Option<Vec<(String, Effects)>>> = vec![None; bodies.len()];
        let mut queued = vec![true; bodies.len()];
        let mut work: VecDeque<usize> = (0..bodies.len()).collect();
        while let Some(i) = work.pop_front() {
            queued[i] = false;
            let (key, f) = &bodies[i];
            let found: Vec<(String, Effects)> =
                f.params.iter().map(|p| (p.name.clone(), self.body_effects(&p.name, &f.body))).collect();
            if results[i].as_ref() == Some(&found) {
                continue;
            }
            results[i] = Some(found);
            let merged = merge(by_key[key].iter().filter_map(|j| results[*j].as_ref()));
            if self.replaces.get(key) == Some(&merged) {
                continue;
            }
            self.replaces.insert(key.clone(), merged);
            for &c in callers.get(key).into_iter().flatten() {
                if !queued[c] {
                    queued[c] = true;
                    work.push_back(c);
                }
            }
        }
    }

    /// The keys the calls in `body` can reach.
    fn callees(&self, body: &[Stmt]) -> Vec<Key> {
        let mut out = Vec::new();
        visit::walk_body(body, &mut |e| match &e.kind {
            ExprKind::Call { func, .. } => out.extend(self.fn_key(func)),
            ExprKind::MethodCall { object, method, .. } => {
                if let Some(ty) = self.receiver_type_name(object) {
                    out.push(Key::Method(ty, method.clone()));
                }
            }
            _ => {}
        });
        out
    }

    /// The function a call's callee names, when it names a declared one. A
    /// local or a parameter of the same name is a closure, not that function.
    fn fn_key(&self, func: &Expr) -> Option<Key> {
        let ExprKind::Ident(name) = &func.kind else { return None };
        let sym = self.program.resolutions.get(&func.id)?;
        let is_fn = self
            .program
            .symbols
            .get(*sym)
            .is_some_and(|s| matches!(s.kind, rask_resolve::SymbolKind::Function { .. }));
        is_fn.then(|| Key::Fn(name.clone()))
    }

    /// What `body` may write and read under `param`.
    ///
    /// A name shadowed by a local of the same name reads as the parameter
    /// here, which can only refuse a call that could have been allowed. A
    /// closure in the body counts as the body: what it writes or reads, the
    /// function does when it calls it.
    fn body_effects(&self, param: &str, body: &[Stmt]) -> Effects {
        struct Walk<'c, 'a> {
            checker: &'c OwnershipChecker<'a>,
            param: &'c str,
            fx: Effects,
        }
        impl Walk<'_, '_> {
            /// `place` and `under` as a path under the parameter, if it's one.
            fn path(&self, place: &Expr, under: &[String]) -> Option<Path> {
                let (Some(root), fields) = OwnershipChecker::extract_root_and_fields(place) else {
                    return None;
                };
                if root != self.param {
                    return None;
                }
                let mut path = fields.unwrap_or_default();
                path.extend(under.iter().cloned());
                path.truncate(MAX_PATH);
                Some(path)
            }
            fn call(&mut self, e: &Expr) {
                for (place, under) in self.checker.call_writes(e) {
                    if let Some(p) = self.path(place, &under) {
                        self.fx.writes.push(p);
                    }
                }
                for (place, under) in self.checker.call_reads(e) {
                    if let Some(p) = self.path(place, &under) {
                        self.fx.reads.push(p);
                    }
                }
            }
            fn args(&mut self, args: &[CallArg]) {
                for a in args {
                    if !is_place(&a.expr) {
                        visit::visit_expr(&a.expr, self);
                    }
                }
            }
        }
        impl<'e> Visit<'e> for Walk<'_, '_> {
            fn expr(&mut self, e: &'e Expr) -> bool {
                match &e.kind {
                    // The receiver and each argument that's a place are read
                    // as the callee reads them; the rest is walked as usual.
                    ExprKind::MethodCall { object, args, .. } => {
                        self.call(e);
                        if !is_place(object) {
                            visit::visit_expr(object, self);
                        }
                        self.args(args);
                        false
                    }
                    ExprKind::Call { func, args } => {
                        self.call(e);
                        visit::visit_expr(func, self);
                        self.args(args);
                        false
                    }
                    ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::OptionalField { .. } => {
                        if let Some(p) = self.path(e, &[]) {
                            self.fx.reads.push(p);
                            return false;
                        }
                        true
                    }
                    _ => true,
                }
            }
            fn stmt(&mut self, s: &'e Stmt) -> bool {
                let StmtKind::Assign { target, value, .. } = &s.kind else { return true };
                if let Some(p) = self.path(target, &[]) {
                    self.fx.writes.push(p);
                }
                // Writing a field doesn't read it; an index or a computed
                // part of the target still reads what it goes through.
                visit::visit_expr(value, self);
                if let ExprKind::Index { object, index } = &target.kind {
                    visit::visit_expr(object, self);
                    visit::visit_expr(index, self);
                }
                false
            }
        }
        let mut w = Walk { checker: self, param, fx: Effects::default() };
        visit::visit_body(body, &mut w);
        Effects { writes: minimal(w.fx.writes), reads: minimal(w.fx.reads) }
    }

    /// What a call may write over: each place it's handed to write through,
    /// with a path under that place the callee replaces. An empty path is the
    /// whole place.
    pub(crate) fn call_writes<'e>(&self, e: &'e Expr) -> Vec<(&'e Expr, Path)> {
        self.call_effects(e, true)
    }

    /// What a call may read: each place it's handed, receiver included, with
    /// a path under it the callee reads.
    pub(crate) fn call_reads<'e>(&self, e: &'e Expr) -> Vec<(&'e Expr, Path)> {
        self.call_effects(e, false)
    }

    fn call_effects<'e>(&self, e: &'e Expr, writes: bool) -> Vec<(&'e Expr, Path)> {
        let pick = |fx: &Effects| if writes { fx.writes.clone() } else { fx.reads.clone() };
        // Unknown callee: an argument is read whole, and replaced whole when
        // it's marked to be written through.
        let unknown = |a: &CallArg| !writes || matches!(a.mode, ArgMode::Mutate | ArgMode::Deleting);
        let mut out: Vec<(&'e Expr, Path)> = Vec::new();
        let mut each = |place: &'e Expr, paths: Vec<Path>| out.extend(paths.into_iter().map(|p| (place, p)));
        match &e.kind {
            ExprKind::Call { func, args } => {
                let known = self.fn_key(func).and_then(|k| self.replaces.get(&k));
                for (i, a) in args.iter().enumerate() {
                    match known.and_then(|params| params.get(i)) {
                        Some((_, fx)) => each(&a.expr, pick(fx)),
                        None if unknown(a) => each(&a.expr, vec![Vec::new()]),
                        None => {}
                    }
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                let known = self
                    .receiver_type_name(object)
                    .and_then(|ty| self.replaces.get(&Key::Method(ty, method.clone())));
                match known {
                    Some(params) => {
                        let mut rest = params.iter().peekable();
                        if let Some((_, fx)) = rest.next_if(|(name, _)| name == "self") {
                            each(object, pick(fx));
                        }
                        for (a, (_, fx)) in args.iter().zip(rest) {
                            each(&a.expr, pick(fx));
                        }
                    }
                    None => {
                        // No body here. A stdlib container's own method grows
                        // or empties it and replaces no handle; anything else
                        // (a type parameter, an interface, a stdlib handle) is
                        // taken to replace its receiver when it may write it.
                        let replaces_receiver = !self.is_container_receiver(object)
                            && !matches!(
                                self.self_param_of(object, method),
                                Some(rask_types::SelfParam::Value | rask_types::SelfParam::Take)
                            );
                        if !writes || replaces_receiver {
                            each(object, vec![Vec::new()]);
                        }
                        for a in args.iter().filter(|a| unknown(a)) {
                            each(&a.expr, vec![Vec::new()]);
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// A stdlib container, whose own methods change what it holds without
    /// replacing its handle.
    fn is_container_receiver(&self, object: &Expr) -> bool {
        let name = match self.node_ty(&object.id) {
            Some(rask_types::Type::Generic { base, .. }) => self.program.types.type_name(*base),
            Some(rask_types::Type::UnresolvedGeneric { name, .. }) => name.clone(),
            _ => return false,
        };
        matches!(name.as_str(), "Vec" | "Map" | "Set" | "Rack")
    }
}

/// A name or a chain of fields off one: what's read as a path, not walked.
fn is_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_) => true,
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => is_place(object),
        _ => false,
    }
}

/// The answers of every declaration under one key, merged. Different shapes
/// can't be merged by position, so then every parameter counts as written
/// and read whole.
fn merge<'r>(mut results: impl Iterator<Item = &'r Vec<(String, Effects)>>) -> Vec<(String, Effects)> {
    let Some(first) = results.next() else { return Vec::new() };
    let mut out = first.clone();
    for r in results {
        if r.len() != out.len() {
            let longer = if r.len() > out.len() { r.clone() } else { out };
            let whole = Effects { writes: vec![Vec::new()], reads: vec![Vec::new()] };
            return longer.into_iter().map(|(name, _)| (name, whole.clone())).collect();
        }
        for ((_, a), (_, b)) in out.iter_mut().zip(r) {
            a.writes = minimal(a.writes.iter().chain(&b.writes).cloned().collect());
            a.reads = minimal(a.reads.iter().chain(&b.reads).cloned().collect());
        }
    }
    out
}

/// Keep only the shortest paths: replacing `log` covers `log.inner`.
fn minimal(mut paths: Vec<Path>) -> Vec<Path> {
    paths.sort_by_key(|p| p.len());
    let mut out: Vec<Path> = Vec::new();
    for p in paths {
        if !out.iter().any(|q| p.starts_with(q)) {
            out.push(p);
        }
    }
    out.sort();
    out
}

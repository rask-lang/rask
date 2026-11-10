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

use std::collections::HashMap;

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
                        for m in i.methods.iter().filter(|m| m.body_lives_elsewhere()) {
                            self.bodyless_methods.insert(m.name.clone());
                        }
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
        // Who calls whom: body `i` calls the bodies in `edges[i]`.
        let edges: Vec<Vec<usize>> = bodies
            .iter()
            .map(|(_, f)| {
                let mut to: Vec<usize> =
                    self.callees(&f.body).iter().flat_map(|k| by_key.get(k).into_iter().flatten().copied()).collect();
                to.sort_unstable();
                to.dedup();
                to
            })
            .collect();
        // Every declared function starts out touching nothing, so recursion
        // climbs from there: `ping` and `pong` calling each other end with
        // what they really do, not with "everything" for want of an answer.
        for (key, f) in &bodies {
            self.replaces
                .entry(key.clone())
                .or_insert_with(|| f.params.iter().map(|p| (p.name.clone(), Effects::default())).collect());
        }
        // Callees before callers, a strongly connected group at a time, so a
        // body is worked out once its callees are settled. Only a group of
        // functions calling each other needs more than one pass. Working all
        // bodies at once instead pushed a chain's fields up it one round at a
        // time, cubic in its length.
        let mut results: Vec<Option<Vec<(String, Effects)>>> = vec![None; bodies.len()];
        for group in strongly_connected(&edges) {
            let recursive = group.len() > 1 || edges[group[0]].contains(&group[0]);
            loop {
                let mut changed = false;
                for &i in &group {
                    let (key, f) = &bodies[i];
                    let found: Vec<(String, Effects)> = f
                        .params
                        .iter()
                        .map(|p| (p.name.clone(), self.body_effects(&p.name, p.is_mutate || p.is_deleting, &f.body)))
                        .collect();
                    if results[i].as_ref() == Some(&found) {
                        continue;
                    }
                    results[i] = Some(found);
                    let merged = merge(by_key[key].iter().filter_map(|j| results[*j].as_ref()));
                    if self.replaces.get(key) != Some(&merged) {
                        self.replaces.insert(key.clone(), merged);
                        changed = true;
                    }
                }
                if !changed || !recursive {
                    break;
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
    fn body_effects(&self, param: &str, may_write: bool, body: &[Stmt]) -> Effects {
        struct Walk<'c, 'a> {
            checker: &'c OwnershipChecker<'a>,
            param: &'c str,
            fx: Effects,
        }
        impl Walk<'_, '_> {
            /// `place` and `under` as a path under the parameter, if it's one.
            fn path(&self, place: &Expr, under: &[String]) -> Option<Path> {
                let (root, mut path) = self.checker.written_place(place)?;
                if root != self.param {
                    return None;
                }
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
                    // `take p.f` reads the field out and leaves it empty.
                    ExprKind::Take { place } => {
                        if let Some(p) = self.path(place, &[]) {
                            self.fx.writes.push(p.clone());
                            self.fx.reads.push(p);
                            return false;
                        }
                        true
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
                // Writing a field doesn't read it; a target that isn't a plain
                // path (`self.slots[i].x`) reads what it goes through, the
                // index included.
                visit::visit_expr(value, self);
                if !is_place(target) {
                    visit::visit_expr(target, self);
                }
                false
            }
        }
        let mut w = Walk { checker: self, param, fx: Effects::default() };
        visit::visit_body(body, &mut w);
        // A parameter the function only borrows can't be written through it.
        let writes = if may_write { minimal(w.fx.writes) } else { Vec::new() };
        Effects { writes, reads: minimal(w.fx.reads) }
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
                    None if self.is_container_receiver(object) => {
                        // A stdlib container's own method grows or empties it
                        // and replaces no handle, but reads all of it.
                        if !writes {
                            each(object, vec![Vec::new()]);
                        }
                        for a in args.iter().filter(|a| unknown(a)) {
                            each(&a.expr, vec![Vec::new()]);
                        }
                    }
                    // A type parameter's or an interface's method comes from
                    // an interface, so when none declares `method` as
                    // `mutate self`, the call can't write its receiver.
                    None if writes && !self.some_interface_mutates(method) => {
                        for a in args.iter().filter(|a| unknown(a)) {
                            each(&a.expr, vec![Vec::new()]);
                        }
                    }
                    None => {
                        // Not one body: a type parameter or an interface, where
                        // any implementation of `method` could run. What all
                        // the ones with a body here do, together, unless one of
                        // that name has no body here, which counts as whole.
                        match self.any_method_named(method) {
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
                                each(object, vec![Vec::new()]);
                                for a in args.iter().filter(|a| unknown(a)) {
                                    each(&a.expr, vec![Vec::new()]);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Whether any interface declares `method` with `mutate self`.
    fn some_interface_mutates(&self, method: &str) -> bool {
        self.program.types.iter().any(|def| match def {
            rask_types::TypeDef::Interface { methods, .. } => methods
                .iter()
                .any(|m| m.name == method && matches!(m.self_param, rask_types::SelfParam::Mutate)),
            _ => false,
        })
    }

    /// Every summarised method called `method`, merged, when each one of that
    /// name has a body here.
    fn any_method_named(&self, method: &str) -> Option<Vec<(String, Effects)>> {
        if self.bodyless_methods.contains(method) {
            return None;
        }
        let all: Vec<&Vec<(String, Effects)>> = self
            .replaces
            .iter()
            .filter(|(k, _)| matches!(k, Key::Method(_, m) if m == method))
            .map(|(_, v)| v)
            .collect();
        if all.is_empty() {
            return None;
        }
        Some(merge(all.into_iter()))
    }

    /// The place a write through `e` lands in, as a root and a field path.
    /// It sees through `!` (`self.opt!.wipe()` writes `opt`) and into a
    /// fixed-size array, whose elements are its own storage: `self.slots[0]
    /// = …` writes `slots`. An element of a `Vec` isn't: a view of the vector
    /// shares its handle and sees the new element.
    pub(crate) fn written_place(&self, e: &Expr) -> Option<(String, Path)> {
        match &e.kind {
            ExprKind::Ident(name) => Some((name.clone(), Vec::new())),
            ExprKind::Field { object, field } | ExprKind::OptionalField { object, field } => {
                let (root, mut path) = self.written_place(object)?;
                path.push(field.clone());
                Some((root, path))
            }
            ExprKind::Unwrap { expr, .. } => self.written_place(expr),
            ExprKind::Index { object, .. }
                if matches!(self.node_ty(&object.id), Some(rask_types::Type::Array { .. })) =>
            {
                self.written_place(object)
            }
            _ => None,
        }
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
        ExprKind::Unwrap { expr, .. } => is_place(expr),
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

/// The most paths kept for one parameter. Derived methods touch every field,
/// and a caller's summary takes its callee's paths with a prefix, so a struct
/// seven levels deep and four fields wide would list 4^7 of them. Past this
/// many, paths collapse to their first field, then to the whole parameter;
/// a shorter path covers more, so that only refuses more.
const MAX_PATHS: usize = 32;

/// Keep only the shortest paths: replacing `log` covers `log.inner`.
fn minimal(mut paths: Vec<Path>) -> Vec<Path> {
    paths.sort_by_key(|p| p.len());
    let mut kept: std::collections::HashSet<Path> = std::collections::HashSet::new();
    let mut out: Vec<Path> = Vec::new();
    for p in paths {
        if (0..=p.len()).any(|n| kept.contains(&p[..n])) {
            continue;
        }
        kept.insert(p.clone());
        out.push(p);
    }
    if out.len() > MAX_PATHS {
        let firsts: Vec<Path> = out.iter().map(|p| p.iter().take(1).cloned().collect()).collect();
        out = minimal_unbounded(firsts);
        if out.len() > MAX_PATHS {
            out = vec![Vec::new()];
        }
    }
    out.sort();
    out
}

/// `minimal` without the cap, for paths already one field long.
fn minimal_unbounded(mut paths: Vec<Path>) -> Vec<Path> {
    paths.sort();
    paths.dedup();
    if paths.iter().any(|p| p.is_empty()) {
        return vec![Vec::new()];
    }
    paths
}

/// Tarjan's strongly connected components of a call graph, callees' groups
/// before their callers'. Iterative, so a long call chain can't run the
/// compiler's own stack out.
fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = edges.len();
    let mut index: Vec<Option<usize>> = vec![None; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut out: Vec<Vec<usize>> = Vec::new();
    for root in 0..n {
        if index[root].is_some() {
            continue;
        }
        // (node, how many of its edges have been followed)
        let mut frames: Vec<(usize, usize)> = vec![(root, 0)];
        index[root] = Some(next);
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&(v, at)) = frames.last() {
            if let Some(&w) = edges[v].get(at) {
                if let Some(top) = frames.last_mut() {
                    top.1 += 1;
                }
                match index[w] {
                    None => {
                        index[w] = Some(next);
                        low[w] = next;
                        next += 1;
                        stack.push(w);
                        on_stack[w] = true;
                        frames.push((w, 0));
                    }
                    Some(iw) if on_stack[w] => low[v] = low[v].min(iw),
                    Some(_) => {}
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if Some(low[v]) == index[v] {
                let mut group = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    group.push(w);
                    if w == v {
                        break;
                    }
                }
                out.push(group);
            }
        }
    }
    out
}

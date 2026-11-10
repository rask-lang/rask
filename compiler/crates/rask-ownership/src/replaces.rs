// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which parts of its `mutate` parameters a function can write over.
//!
//! A view of a field lasts until its block ends, and writing over the field
//! frees what the view reads (mem.borrowing/S1, S5). A call writes through a
//! `mutate` argument or a `mutate self` receiver, so the checker needs to know
//! which fields under it the callee may replace. Most calls replace nothing a
//! view holds: `self.set_reg(…)` in a VM loop that reads `self.code` writes
//! registers only. So the answer is per parameter and per field path, worked
//! out from the bodies to a fixed point over the calls between them, the way
//! `resize` works out which parameters a call can resize.
//!
//! A path is relative to the parameter: `["log"]` for `self.log = …`, `[]`
//! for the whole parameter. A call to something with no body here (an
//! interface method, a closure, a stdlib function) is taken to replace the
//! whole argument it's given a `mutate` marker for, which can only refuse a
//! program, never let one through. A stdlib container's own methods
//! (`v.push`, `m.clear()`) grow or empty the value without replacing its
//! handle, so they write no path.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{self, Visit};

use crate::OwnershipChecker;

/// Field paths a function may replace, per parameter in declaration order,
/// with the parameter's name.
pub(crate) type Replaces = HashMap<Key, Vec<(String, Vec<Vec<String>>)>>;

/// A free function by name, a method by the type it's declared on and its
/// name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Key {
    Fn(String),
    Method(String, String),
}

impl<'a> OwnershipChecker<'a> {
    /// Fill `replaces` for every function and method in `groups`.
    pub(crate) fn infer_replaces(&mut self, groups: &[&[Decl]]) {
        let mut bodies: Vec<(Key, &FnDecl)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
                match &decl.kind {
                    DeclKind::Fn(f) => bodies.push((Key::Fn(f.name.clone()), f)),
                    DeclKind::Impl(i) => {
                        let ty = i.target_ty.name().unwrap_or_default();
                        for m in &i.methods {
                            bodies.push((Key::Method(ty.clone(), m.name.clone()), m));
                        }
                    }
                    _ => {}
                }
            }
        }
        // Two declarations can share a key (the same method in two `extend`
        // blocks), so each round builds the whole map, merging those, and
        // stops when it matches the last. Each round can only add paths, and
        // paths are bounded (`MAX_PATH`), so it ends.
        loop {
            let mut next: Replaces = HashMap::new();
            for (key, f) in &bodies {
                let found: Vec<(String, Vec<Vec<String>>)> = f
                    .params
                    .iter()
                    .map(|p| {
                        let paths = if p.is_mutate || p.is_deleting {
                            self.body_replaces(&p.name, &f.body)
                        } else {
                            Vec::new()
                        };
                        (p.name.clone(), paths)
                    })
                    .collect();
                match next.get_mut(key) {
                    Some(have) if have.len() == found.len() => {
                        for ((_, a), (_, b)) in have.iter_mut().zip(found) {
                            a.extend(b);
                            *a = minimal(std::mem::take(a));
                        }
                    }
                    // Different shapes under one key can't be merged by
                    // position, so every parameter is taken as replaced whole.
                    Some(have) => {
                        let longer = if found.len() > have.len() { found } else { std::mem::take(have) };
                        *have = longer.into_iter().map(|(name, _)| (name, vec![Vec::new()])).collect();
                    }
                    None => {
                        next.insert(key.clone(), found);
                    }
                }
            }
            if next == self.replaces {
                break;
            }
            self.replaces = next;
        }
    }

    /// The paths under `param` that `body` may replace.
    ///
    /// A name shadowed by a local of the same name reads as the parameter
    /// here, which can only refuse a call that could have been allowed.
    fn body_replaces(&self, param: &str, body: &[Stmt]) -> Vec<Vec<String>> {
        struct Walk<'c, 'a> {
            checker: &'c OwnershipChecker<'a>,
            param: &'c str,
            out: Vec<Vec<String>>,
        }
        impl Walk<'_, '_> {
            fn add(&mut self, place: &Expr, suffix: &[String]) {
                let (Some(root), fields) = OwnershipChecker::extract_root_and_fields(place) else {
                    return;
                };
                if root != self.param {
                    return;
                }
                let mut path = fields.unwrap_or_default();
                path.extend(suffix.iter().cloned());
                // A method that recurses through a field of its own type
                // (`self.next.clear()`) would grow the path every round.
                // A shorter path covers everything under it, so cutting one
                // short only refuses more.
                path.truncate(MAX_PATH);
                self.out.push(path);
            }
        }
        impl<'e> Visit<'e> for Walk<'_, '_> {
            fn expr(&mut self, e: &'e Expr) -> bool {
                if let ExprKind::Closure { .. } = &e.kind {
                    return false;
                }
                for (place, path) in self.checker.call_writes(e) {
                    self.add(place, &path);
                }
                true
            }
            fn stmt(&mut self, s: &'e Stmt) -> bool {
                if let StmtKind::Assign { target, .. } = &s.kind {
                    self.add(target, &[]);
                }
                true
            }
        }
        let mut w = Walk { checker: self, param, out: Vec::new() };
        visit::visit_body(body, &mut w);
        minimal(w.out)
    }

    /// What a call may write over: each place it's given to write through,
    /// with a path under that place the callee replaces. An empty path is the
    /// whole place.
    pub(crate) fn call_writes<'e>(&self, e: &'e Expr) -> Vec<(&'e Expr, Vec<String>)> {
        let mut out = Vec::new();
        let marked = |a: &rask_ast::expr::CallArg| matches!(a.mode, ArgMode::Mutate | ArgMode::Deleting);
        match &e.kind {
            ExprKind::Call { func, args } => {
                let known = func.name().and_then(|n| self.replaces.get(&Key::Fn(n.to_string())));
                for (i, a) in args.iter().enumerate() {
                    if !marked(a) {
                        continue;
                    }
                    match known.and_then(|params| params.get(i)) {
                        Some((_, paths)) => out.extend(paths.iter().map(|p| (&a.expr, p.clone()))),
                        None => out.push((&a.expr, Vec::new())),
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
                        if let Some((_, paths)) = rest.next_if(|(name, _)| name == "self") {
                            out.extend(paths.iter().map(|p| (object.as_ref(), p.clone())));
                        }
                        for (a, (_, paths)) in args.iter().zip(rest) {
                            if marked(a) {
                                out.extend(paths.iter().map(|p| (&a.expr, p.clone())));
                            }
                        }
                    }
                    None => {
                        // No body here: a container's own method, which
                        // replaces nothing, or one behind an interface,
                        // which is taken to replace its receiver.
                        if self.interface_receiver(object) && self.is_mutate_self_method(object, method) {
                            out.push((object.as_ref(), Vec::new()));
                        }
                        for a in args.iter().filter(|a| marked(a)) {
                            out.push((&a.expr, Vec::new()));
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    fn interface_receiver(&self, object: &Expr) -> bool {
        matches!(self.node_ty(&object.id), Some(rask_types::Type::InterfaceObject { .. }))
    }
}

/// The longest path kept; see `body_replaces`.
const MAX_PATH: usize = 8;

/// Keep only the shortest paths: replacing `log` covers `log.inner`.
fn minimal(mut paths: Vec<Vec<String>>) -> Vec<Vec<String>> {
    paths.sort_by_key(|p| p.len());
    let mut out: Vec<Vec<String>> = Vec::new();
    for p in paths {
        if !out.iter().any(|q| p.starts_with(q)) {
            out.push(p);
        }
    }
    out.sort();
    out
}

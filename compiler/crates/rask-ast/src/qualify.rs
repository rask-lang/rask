// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Renaming declarations and every reference to them.
//!
//! Two passes give a declaration a new name: a dependency's names carry the
//! package they came from (`rask-compiler`'s `package_scope`), and a stdlib
//! module's private helpers carry the module (`rask-stdlib`'s stubs). Both
//! merge declarations into one flat namespace, so both need the same rename:
//! the declaration, the references to it, and not a local that happens to
//! share its spelling.

use std::collections::{HashMap, HashSet};

use crate::decl::{Decl, DeclKind};
use crate::expr::{Expr, ExprKind, Pattern};
use crate::rewrite::{self, Rewrite};

/// Rename every declaration in `map` and every reference to it in `decls`.
pub fn qualify_in_place(decls: &mut [Decl], map: &HashMap<String, String>) {
    if map.is_empty() {
        return;
    }
    rewrite_references(decls, map);
    for decl in decls.iter_mut() {
        rename_declaration(decl, map);
    }
}

/// Point every reference to a name in `map` at its new name, leaving the
/// declarations themselves alone.
///
/// A local can share a spelling with a function or const (both are
/// snake_case), and renaming the reference would point it at a declaration
/// instead of at the binding. The set of bound names is collected per
/// declaration and not per block, so a name bound anywhere in a body is left
/// alone throughout it: erring that way leaves a reference that resolve
/// reports, where erring the other way silently reads a local as a function.
pub fn rewrite_references(decls: &mut [Decl], map: &HashMap<String, String>) {
    if map.is_empty() {
        return;
    }
    for decl in decls.iter_mut() {
        let shadowed = names_bound_in(decl);
        let mut r = Qualifier { map, shadowed: &shadowed };
        rewrite::rewrite_decl(decl, &mut r);
    }
}

struct Qualifier<'a> {
    map: &'a HashMap<String, String>,
    shadowed: &'a HashSet<String>,
}

impl Qualifier<'_> {
    /// A name that names a declaration here, and isn't a local.
    fn lookup(&self, name: &str) -> Option<&String> {
        if self.shadowed.contains(name) {
            return None;
        }
        self.map.get(name)
    }
}

impl Rewrite for Qualifier<'_> {
    fn ty(&mut self, t: &mut String) {
        // A type is written, not parsed, at this stage — `Vec<Cat>`, `Cat?`,
        // `i64 or Cat` — so the substitution is by word. A type string can
        // never name a local, so the shadow set doesn't apply.
        let subst: Vec<(String, String)> =
            self.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        *t = crate::type_str::substitute_type_params(t, &subst);
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Ident(name) => {
                if let Some(q) = self.lookup(name) {
                    *name = q.clone();
                }
            }
            // A type name, so no shadow check.
            ExprKind::StructLit { name, .. } => {
                if let Some(q) = self.map.get(name.as_str()) {
                    *name = q.clone();
                }
            }
            // `spawn_raw { … }` and `using pool { … }` name a declaration the
            // same way a call does.
            ExprKind::BlockCall { name, .. } | ExprKind::UsingBlock { name, .. } => {
                if let Some(q) = self.lookup(name) {
                    *name = q.clone();
                }
            }
            _ => {}
        }
    }

    fn pattern(&mut self, p: &mut Pattern) {
        // `Colour.Red` in a pattern is the enum's name and the variant's; only
        // the enum is a declaration. A bare `Red` is a variant of whatever the
        // scrutinee is, and there is nothing here to rename.
        let name = match p {
            Pattern::Constructor { name, .. } | Pattern::Struct { name, .. } => name,
            Pattern::Wildcard
            | Pattern::Ident(_)
            | Pattern::Literal(_)
            | Pattern::Tuple(_)
            | Pattern::Or(_)
            | Pattern::Range { .. }
            | Pattern::TypePat { .. } => return,
        };
        match name.split_once('.') {
            Some((head, tail)) => {
                if let Some(q) = self.map.get(head) {
                    *name = format!("{}.{}", q, tail);
                }
            }
            None => {
                if let Some(q) = self.map.get(name.as_str()) {
                    *name = q.clone();
                }
            }
        }
    }
}

pub fn declared_name(decl: &Decl) -> Option<String> {
    match &decl.kind {
        DeclKind::Fn(f) => Some(f.name.clone()),
        DeclKind::Struct(s) => Some(s.name.clone()),
        DeclKind::Enum(e) => Some(e.name.clone()),
        DeclKind::Interface(t) => Some(t.name.clone()),
        DeclKind::Const(c) => Some(c.name.clone()),
        DeclKind::TypeAlias(a) => Some(a.name.clone()),
        DeclKind::Annotation(a) => Some(a.name.clone()),
        DeclKind::Union(u) => Some(u.name.clone()),
        _ => None,
    }
}

pub fn rename_declaration(decl: &mut Decl, map: &HashMap<String, String>) {
    let name: &mut String = match &mut decl.kind {
        DeclKind::Fn(f) => &mut f.name,
        DeclKind::Struct(s) => &mut s.name,
        DeclKind::Enum(e) => &mut e.name,
        DeclKind::Interface(t) => &mut t.name,
        DeclKind::Const(c) => &mut c.name,
        DeclKind::TypeAlias(a) => &mut a.name,
        DeclKind::Annotation(a) => &mut a.name,
        DeclKind::Union(u) => &mut u.name,
        _ => return,
    };
    if let Some(q) = map.get(name.as_str()) {
        *name = q.clone();
    }
}

/// Every name a declaration's bodies bind, anywhere inside them.
pub fn names_bound_in(decl: &Decl) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    let params_of = |f: &crate::decl::FnDecl, out: &mut HashSet<String>| {
        for p in &f.params {
            out.insert(p.name.clone());
        }
    };
    match &decl.kind {
        DeclKind::Fn(f) => params_of(f, &mut out),
        DeclKind::Struct(s) => s.methods.iter().for_each(|m| params_of(m, &mut out)),
        DeclKind::Enum(e) => e.methods.iter().for_each(|m| params_of(m, &mut out)),
        DeclKind::Interface(t) => t.methods.iter().for_each(|m| params_of(m, &mut out)),
        DeclKind::Impl(i) => i.methods.iter().for_each(|m| params_of(m, &mut out)),
        _ => {}
    }
    crate::visit::walk_decl(decl, &mut |e| match &e.kind {
        ExprKind::Closure { params, .. } => {
            for p in params {
                out.insert(p.name.clone());
            }
        }
        ExprKind::WithAs { bindings, .. } => {
            for b in bindings {
                out.insert(b.name.clone());
            }
        }
        ExprKind::Catch { clause, .. } => {
            out.insert(clause.binder.clone());
        }
        ExprKind::IfLet { pattern, else_binding, .. } => {
            out.extend(pattern.bound_names().into_iter().map(str::to_string));
            if let Some(b) = else_binding {
                out.insert(b.clone());
            }
        }
        ExprKind::GuardPattern { pattern, .. } | ExprKind::IsPattern { pattern, .. } => {
            out.extend(pattern.bound_names().into_iter().map(str::to_string));
        }
        ExprKind::Match { arms, .. } => {
            for arm in arms {
                out.extend(arm.pattern.bound_names().into_iter().map(str::to_string));
            }
        }
        _ => {}
    });
    bound_by_statements(decl, &mut out);
    out
}

fn bound_by_statements(decl: &Decl, out: &mut HashSet<String>) {
    fn body(stmts: &[crate::stmt::Stmt], out: &mut HashSet<String>) {
        use crate::stmt::{ForBinding, StmtKind, TuplePat};
        fn tuple(pats: &[TuplePat], out: &mut HashSet<String>) {
            for p in pats {
                match p {
                    TuplePat::Name(n) => {
                        out.insert(n.clone());
                    }
                    TuplePat::Wildcard => {}
                    TuplePat::Nested(inner) => tuple(inner, out),
                }
            }
        }
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Let { name, .. } | StmtKind::Mut { name, .. } => {
                    out.insert(name.clone());
                }
                StmtKind::LetTuple { patterns, .. } | StmtKind::MutTuple { patterns, .. } => {
                    tuple(patterns, out)
                }
                StmtKind::LetStruct { pattern, .. } => {
                    out.extend(pattern.bound_names().into_iter().map(str::to_string))
                }
                StmtKind::While { body: b, .. } | StmtKind::Loop { body: b, .. } => body(b, out),
                StmtKind::WhileLet { pattern, body: b, .. } => {
                    out.extend(pattern.bound_names().into_iter().map(str::to_string));
                    body(b, out);
                }
                StmtKind::For { binding, body: b, .. }
                | StmtKind::ComptimeFor { binding, body: b, .. } => {
                    match binding {
                        ForBinding::Single(n) => {
                            out.insert(n.clone());
                        }
                        ForBinding::Tuple(ns) => out.extend(ns.iter().cloned()),
                    }
                    body(b, out);
                }
                StmtKind::Comptime(b) => body(b, out),
                StmtKind::Ensure { body: b, else_handler } => {
                    body(b, out);
                    if let Some((param, handler)) = else_handler {
                        out.insert(param.clone());
                        body(handler, out);
                    }
                }
                StmtKind::Expr(_)
                | StmtKind::Assign { .. }
                | StmtKind::Return(_)
                | StmtKind::Break { .. }
                | StmtKind::Continue(_)
                | StmtKind::Discard { .. } => {}
            }
        }
    }
    match &decl.kind {
        DeclKind::Fn(f) => body(&f.body, out),
        DeclKind::Struct(s) => s.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Enum(e) => e.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Interface(t) => t.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Impl(i) => i.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Test(t) => body(&t.body, out),
        DeclKind::Benchmark(b) => body(&b.body, out),
        _ => {}
    }
}

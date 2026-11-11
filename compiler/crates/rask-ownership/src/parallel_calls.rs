// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function values a callee runs in parallel, checked where they're passed
//! (conc.fork/FK8).
//!
//! `func both(f: func(), g: func()) { fork { f() } and { g() } }` is fine on
//! its own. What makes it a race is the call: `both(h, h)` with an `h` that
//! writes what it captured runs one closure on two threads, and `both(|| n +=
//! 1, || print(n))` writes and reads `n` at once. Inside `both` nothing can
//! tell, so each function gets a summary of the parameter pairs it may use
//! at the same time, and every call is checked against it.
//!
//! A pair is a function-typed parameter one fork block names and any
//! parameter another block names, itself included. A function that hands its
//! own parameters to a callee with a pair takes the pair on.

use std::collections::{BTreeSet, HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, Expr, ExprKind};
use rask_ast::stmt::Stmt;
use rask_ast::visit;
use rask_ast::Span;
use rask_types::Type;

use crate::replaces::Key;
use crate::{ForkAccess, OwnershipChecker, OwnershipError, OwnershipErrorKind};

/// What a function may run at the same time, by parameter name.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct ParallelUse {
    /// Every parameter in order, `self` first for a method.
    pub(crate) params: Vec<String>,
    /// `(f, q)`: function value `f` may run while `q` is used elsewhere.
    pub(crate) pairs: BTreeSet<(String, String)>,
}

pub(crate) type ParallelUses = HashMap<Key, ParallelUse>;

/// What a function-typed argument is, as far as a caller can see.
enum FnArg<'e> {
    /// A declared function: it captures nothing.
    Declared,
    /// A closure literal, written here or bound to `name`.
    Literal { name: Option<String>, closure: &'e Expr },
    /// The caller's own parameter: the caller's callers decide.
    Param(String),
    /// From a field, an element or a call: what it captured is out of sight,
    /// and another value may be a copy of it.
    Opaque,
}

impl<'a> OwnershipChecker<'a> {
    pub(crate) fn infer_parallel_uses(&mut self, groups: &[&[Decl]]) {
        let mut bodies: Vec<(Key, &FnDecl)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
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
        for (key, f) in &bodies {
            let params = f.params.iter().map(|p| p.name.clone()).collect();
            self.parallel_uses.insert(key.clone(), ParallelUse { params, pairs: BTreeSet::new() });
        }
        loop {
            let mut changed = false;
            for (key, f) in &bodies {
                let pairs = self.body_parallel_pairs(f);
                let entry = self.parallel_uses.get_mut(key).expect("seeded above");
                if entry.pairs != pairs {
                    entry.pairs = pairs;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn body_parallel_pairs(&self, f: &FnDecl) -> BTreeSet<(String, String)> {
        let params: HashSet<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
        let fn_typed: HashSet<&str> = f
            .params
            .iter()
            .filter(|p| matches!(p.ty, Some(rask_ast::ty::TypeExpr::Func { .. })))
            .map(|p| p.name.as_str())
            .collect();
        let mut pairs = BTreeSet::new();
        for stmt in &f.body {
            visit::walk_stmt(stmt, &mut |e| match &e.kind {
                ExprKind::Fork { sides } => {
                    let named: Vec<HashSet<String>> = sides
                        .iter()
                        .map(|side| {
                            let mut out = HashSet::new();
                            visit::walk_expr(side, &mut |x| {
                                if let ExprKind::Ident(n) = &x.kind {
                                    if params.contains(n.as_str()) {
                                        out.insert(n.clone());
                                    }
                                }
                            });
                            out
                        })
                        .collect();
                    for (i, here) in named.iter().enumerate() {
                        for (j, there) in named.iter().enumerate() {
                            if i == j {
                                continue;
                            }
                            for p in here.iter().filter(|p| fn_typed.contains(p.as_str())) {
                                for q in there {
                                    pairs.insert((p.clone(), q.clone()));
                                }
                            }
                        }
                    }
                }
                ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                    for (a, b) in self.call_pairs(e) {
                        if let (ExprKind::Ident(an), ExprKind::Ident(bn)) = (&a.kind, &b.kind) {
                            if fn_typed.contains(an.as_str()) && params.contains(bn.as_str()) {
                                pairs.insert((an.clone(), bn.clone()));
                            }
                        }
                    }
                }
                _ => {}
            });
        }
        pairs
    }

    /// The callee's parallel pairs at this call, as the argument expressions.
    fn call_pairs<'e>(&self, call: &'e Expr) -> Vec<(&'e Expr, &'e Expr)> {
        let (key, receiver, args) = match &call.kind {
            ExprKind::Call { func, args } => match self.fn_key(func) {
                Some(k) => (k, None, args),
                None => return Vec::new(),
            },
            ExprKind::MethodCall { object, method, args, .. } => match self.receiver_type_name(object) {
                Some(ty) => (Key::Method(ty, method.clone()), Some(object.as_ref()), args),
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        let Some(uses) = self.parallel_uses.get(&key) else { return Vec::new() };
        if uses.pairs.is_empty() {
            return Vec::new();
        }
        let mut by_param: HashMap<&str, &'e Expr> = HashMap::new();
        let mut positional = uses.params.iter().filter(|p| receiver.is_none() || *p != "self");
        if let Some(r) = receiver {
            by_param.insert("self", r);
        }
        for a in args {
            let param = match &a.name {
                Some(label) => uses.params.iter().find(|p| *p == label),
                None => positional.next(),
            };
            if let Some(p) = param {
                by_param.insert(p.as_str(), &a.expr);
            }
        }
        uses.pairs
            .iter()
            .filter_map(|(p, q)| Some((*by_param.get(p.as_str())?, *by_param.get(q.as_str())?)))
            .collect()
    }

    fn fn_arg<'e>(&'e self, e: &'e Expr) -> FnArg<'e> {
        match &e.kind {
            ExprKind::Closure { .. } => FnArg::Literal { name: None, closure: e },
            ExprKind::GenericName { .. } => FnArg::Declared,
            ExprKind::Ident(name) => {
                if self.fn_key(e).is_some() || self.named_fn_values.contains(name) {
                    FnArg::Declared
                } else if let Some(lit) = self.closure_literals.get(name) {
                    FnArg::Literal { name: Some(name.clone()), closure: lit }
                } else if self.current_params.iter().any(|p| p == name) {
                    FnArg::Param(name.clone())
                } else {
                    FnArg::Opaque
                }
            }
            _ => FnArg::Opaque,
        }
    }

    /// What a closure literal reads and writes of what it captured.
    fn literal_accesses(&self, closure: &Expr) -> Vec<ForkAccess> {
        let ExprKind::Closure { params, body, .. } = &closure.kind else { return Vec::new() };
        let locals: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
        let mut out = Vec::new();
        self.collect_fork_accesses(body, &locals, &mut out, 0);
        out
    }

    /// What an argument that isn't a function reads, or writes when it goes
    /// to a `mutate` parameter.
    fn data_accesses(&self, arg: &Expr, mode: ArgMode) -> Vec<ForkAccess> {
        let write = matches!(mode, ArgMode::Mutate | ArgMode::Deleting);
        let mut out = Vec::new();
        if let Some((root, path)) = Self::fork_place(arg) {
            out.push(ForkAccess { root, path, write, span: arg.span });
            return out;
        }
        visit::walk_expr(arg, &mut |x| {
            if let ExprKind::Ident(n) = &x.kind {
                out.push(ForkAccess { root: n.clone(), path: Vec::new(), write: false, span: x.span });
            }
        });
        out
    }

    fn accesses_clash(a: &[ForkAccess], b: &[ForkAccess]) -> Option<(String, Span)> {
        for x in a {
            for y in b {
                if x.root == y.root && Self::paths_overlap(&x.path, &y.path) && (x.write || y.write) {
                    let at = if x.write { x.span } else { y.span };
                    return Some((Self::path_text(&x.root, &x.path), at));
                }
            }
        }
        None
    }

    /// FK8 at one call: every pair the callee may run at the same time.
    pub(crate) fn check_parallel_args(&mut self, call: &Expr) {
        let mode_of = |e: &Expr| -> ArgMode {
            let args = match &call.kind {
                ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args,
                _ => return ArgMode::Default,
            };
            args.iter().find(|a| std::ptr::eq(&a.expr, e)).map_or(ArgMode::Default, |a| a.mode)
        };
        let mut found: Option<(String, Span, Span)> = None;
        for (a, b) in self.call_pairs(call) {
            if found.is_some() {
                break;
            }
            let same = std::ptr::eq(a, b);
            let b_is_fn = matches!(self.node_ty(&b.id), Some(Type::Fn { .. }));
            match self.fn_arg(a) {
                FnArg::Declared | FnArg::Param(_) => {}
                FnArg::Opaque => {
                    // Another function value may be a copy of this one, and
                    // this one may write what it captured.
                    if same || b_is_fn {
                        found = Some(("this may be the same closure as the other, and may write what it captured".to_string(), a.span, b.span));
                    }
                }
                FnArg::Literal { name, closure } => {
                    let mine = self.literal_accesses(closure);
                    let writes = mine.iter().any(|x| x.write);
                    let theirs = if same {
                        if writes {
                            let what = name.clone().unwrap_or_else(|| "this closure".to_string());
                            found = Some((format!("`{what}` runs twice at once and writes what it captured"), a.span, b.span));
                        }
                        continue;
                    } else if b_is_fn {
                        match self.fn_arg(b) {
                            FnArg::Literal { name: other, closure: c } => {
                                if name.is_some() && name == other {
                                    if writes {
                                        found = Some((
                                            format!("`{}` runs twice at once and writes what it captured", name.clone().unwrap_or_default()),
                                            a.span,
                                            b.span,
                                        ));
                                    }
                                    continue;
                                }
                                self.literal_accesses(c)
                            }
                            _ => Vec::new(),
                        }
                    } else {
                        self.data_accesses(b, mode_of(b))
                    };
                    if let Some((what, _)) = Self::accesses_clash(&mine, &theirs) {
                        found = Some((format!("both touch `{what}`, and one writes it"), a.span, b.span));
                    }
                }
            }
        }
        if let Some((what, span, other_span)) = found {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ParallelArgsConflict { what, other_span },
                span,
            });
        }
    }

    /// FK8 for every call in a body, once the body has been walked and its
    /// closures are known.
    pub(crate) fn check_parallel_calls(&mut self, body: &[Stmt]) {
        let mut calls: Vec<Expr> = Vec::new();
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| {
                if matches!(e.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) && !self.call_pairs(e).is_empty() {
                    calls.push(e.clone());
                }
            });
        }
        for call in &calls {
            self.check_parallel_args(call);
        }
    }
}

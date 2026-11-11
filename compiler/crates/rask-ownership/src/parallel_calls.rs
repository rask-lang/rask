// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function values a callee runs in parallel, checked where they're passed
//! (conc.fork/FK8).
//!
//! `func both(f: func(), g: func()) { fork { f() } and { g() } }` is fine on
//! its own. What makes it a race is the call: `both(h, h)` with an `h` that
//! writes what it captured runs one closure on two threads, and `both(|| n +=
//! 1, || print(n))` writes and reads `n` at once. Inside `both` nothing can
//! tell, so each function gets a summary of the parameter pairs it may use
//! at the same time, and at every call the two arguments of each pair are
//! checked as the two blocks of a fork would be: the same conflicts (FK3),
//! the same one-block values (FK7), the same rules for a shared rack (RK16).
//!
//! A pair is a function-typed parameter one block names and any parameter
//! another block names, itself included. "Block" covers a fork's blocks and
//! the arguments of a call that may run them at once, so a function that
//! hands its parameters on, bare or inside a closure, takes the pair on. A
//! call through a function value nothing here can read may run its function
//! arguments at once, so it counts as one.

use std::collections::{BTreeSet, HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{visit, NodeId, Span};
use rask_types::Type;

use crate::replaces::Key;
use crate::{OwnershipChecker, OwnershipError, OwnershipErrorKind};

/// What a function may run at the same time, by parameter name.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct ParallelUse {
    /// Every parameter in order, `self` first for a method.
    pub(crate) params: Vec<String>,
    /// `(f, q)`: function value `f` may run while `q` is used elsewhere.
    pub(crate) pairs: BTreeSet<(String, String)>,
}

pub(crate) type ParallelUses = HashMap<Key, ParallelUse>;

/// A node id no expression has: the wrappers built to check an argument as a
/// fork block have no type of their own to look up.
const SYNTHETIC: NodeId = NodeId(u32::MAX);

impl<'a> OwnershipChecker<'a> {
    /// The function a callee expression names: a declared function, or a
    /// local bound to one (`let b = both`).
    pub(crate) fn callee_key(&self, func: &Expr) -> Option<Key> {
        if let Some(k) = self.fn_key(func) {
            return Some(k);
        }
        match &func.kind {
            ExprKind::Ident(name) => self.named_fn_values.get(name).cloned(),
            _ => None,
        }
    }

    /// Whether `e` names one of the parameters declared at `spans`, by what
    /// it resolves to: a local of the same name is something else.
    fn names_param_at(&self, e: &Expr, spans: &[Span]) -> bool {
        let ExprKind::Ident(_) = &e.kind else { return false };
        let Some(sym) = self.program.resolutions.get(&e.id) else { return false };
        self.program.symbols.get(*sym).is_some_and(|s| {
            matches!(s.kind, rask_resolve::SymbolKind::Parameter { .. }) && spans.contains(&s.span)
        })
    }

    /// Whether `e` names a parameter of the function being checked.
    pub(crate) fn is_param_ref(&self, e: &Expr) -> bool {
        self.names_param_at(e, &self.current_param_spans)
    }

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
        // Bodies sharing a key are joined, and a pair once found stays: both
        // keep the loop from trading answers between rounds.
        loop {
            let mut changed = false;
            for (key, f) in &bodies {
                let pairs = self.body_parallel_pairs(f);
                let entry = self.parallel_uses.get_mut(key).expect("seeded above");
                for p in pairs {
                    changed |= entry.pairs.insert(p);
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn body_parallel_pairs(&self, f: &FnDecl) -> BTreeSet<(String, String)> {
        let spans: Vec<Span> = f.params.iter().map(|p| p.name_span).collect();
        let fn_typed: HashSet<&str> = f
            .params
            .iter()
            .filter(|p| matches!(p.ty, Some(rask_ast::ty::TypeExpr::Func { .. })))
            .map(|p| p.name.as_str())
            .collect();
        // Closures the body binds to a name: `let w = || x()` names `x`
        // wherever `w` is handed on.
        let mut local_closures: HashMap<&str, &Expr> = HashMap::new();
        for stmt in &f.body {
            visit::walk_stmt(stmt, &mut |e| {
                for st in Self::own_stmts(e) {
                    if let StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } = &st.kind {
                        if matches!(init.kind, ExprKind::Closure { .. }) {
                            local_closures.insert(name.as_str(), init);
                        }
                    }
                }
            });
        }
        for st in &f.body {
            if let StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } = &st.kind {
                if matches!(init.kind, ExprKind::Closure { .. }) {
                    local_closures.insert(name.as_str(), init);
                }
            }
        }
        // The parameters an expression names, by resolution, through the
        // local closures it names.
        fn collect<'x>(
            this: &OwnershipChecker<'_>,
            e: &'x Expr,
            spans: &[Span],
            local_closures: &HashMap<&str, &'x Expr>,
            out: &mut HashSet<String>,
            depth: u32,
        ) {
            visit::walk_expr(e, &mut |x| {
                if let ExprKind::Ident(n) = &x.kind {
                    if this.names_param_at(x, spans) {
                        out.insert(n.clone());
                    } else if let Some(lit) = local_closures.get(n.as_str()) {
                        if depth < 8 {
                            collect(this, lit, spans, local_closures, out, depth + 1);
                        }
                    }
                }
            });
        }
        let named_in = |e: &Expr| -> HashSet<String> {
            let mut out = HashSet::new();
            collect(self, e, &spans, &local_closures, &mut out, 0);
            out
        };
        let mut pairs = BTreeSet::new();
        for stmt in &f.body {
            visit::walk_stmt(stmt, &mut |e| {
                let groups: Vec<(&Expr, &Expr)> = match &e.kind {
                    ExprKind::Fork { sides } => {
                        let mut out = Vec::new();
                        for (i, a) in sides.iter().enumerate() {
                            for (j, b) in sides.iter().enumerate() {
                                if i != j {
                                    out.push((a, b));
                                }
                            }
                        }
                        out
                    }
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. } => self.call_pairs(e),
                    _ => Vec::new(),
                };
                for (a, b) in groups {
                    let (here, there) = (named_in(a), named_in(b));
                    for p in here.iter().filter(|p| fn_typed.contains(p.as_str())) {
                        for q in &there {
                            pairs.insert((p.clone(), q.clone()));
                        }
                    }
                }
            });
        }
        pairs
    }

    /// What this call may run at the same time, as pairs of argument
    /// expressions: the callee's pairs, or for a call through a function value
    /// nothing here can read, every function argument against every argument.
    fn call_pairs<'e>(&self, call: &'e Expr) -> Vec<(&'e Expr, &'e Expr)> {
        let (key, receiver, args): (Option<Key>, Option<&'e Expr>, &'e [CallArg]) = match &call.kind {
            ExprKind::Call { func, args } => (self.callee_key(func), None, args),
            ExprKind::MethodCall { object, method, args, .. } => {
                (self.receiver_type_name(object).map(|ty| Key::Method(ty, method.clone())), Some(object.as_ref()), args)
            }
            _ => return Vec::new(),
        };
        let Some(uses) = key.as_ref().and_then(|k| self.parallel_uses.get(k)) else {
            return self.unknown_call_pairs(call, args);
        };
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

    /// A call through a function value with no body here: `r(a, a)` with `r`
    /// a parameter may be `both`. Only a call with a function argument can
    /// run anything at once.
    fn unknown_call_pairs<'e>(&self, call: &'e Expr, args: &'e [CallArg]) -> Vec<(&'e Expr, &'e Expr)> {
        let ExprKind::Call { func, .. } = &call.kind else { return Vec::new() };
        if !matches!(self.node_ty(&func.id), Some(Type::Fn { .. })) {
            return Vec::new();
        }
        if let ExprKind::Ident(name) = &func.kind {
            if self.closure_literals.contains_key(name) {
                return Vec::new();
            }
        }
        let is_fn = |e: &Expr| matches!(self.node_ty(&e.id), Some(Type::Fn { .. }));
        let mut out = Vec::new();
        for a in args.iter().filter(|a| is_fn(&a.expr)) {
            for b in args {
                out.push((&a.expr, &b.expr));
            }
        }
        out
    }

    /// An argument as the fork block it may run as. A closure is its own body;
    /// a closure bound to a name is the literal it was bound to; anything else
    /// is a block that uses it, writing it when it goes to `mutate`.
    fn as_fork_block(&self, arg: &Expr, mode: ArgMode) -> Expr {
        match &arg.kind {
            ExprKind::Closure { .. } => return arg.clone(),
            ExprKind::Ident(name) => {
                if let Some(lit) = self.closure_literals.get(name) {
                    return lit.clone();
                }
            }
            _ => {}
        }
        let stmt = if matches!(mode, ArgMode::Mutate | ArgMode::Deleting) {
            StmtKind::Assign { target: arg.clone(), value: arg.clone(), op: None }
        } else {
            StmtKind::Expr(arg.clone())
        };
        let body = Expr {
            id: SYNTHETIC,
            kind: ExprKind::Block(vec![Stmt { id: SYNTHETIC, kind: stmt, span: arg.span }]),
            span: arg.span,
        };
        Expr {
            id: SYNTHETIC,
            kind: ExprKind::Closure { params: Vec::new(), ret_ty: None, body: Box::new(body) },
            span: arg.span,
        }
    }

    /// FK8 at one call: each pair the callee may run at once, checked as two
    /// fork blocks. What that finds is reported at the call.
    pub(crate) fn check_parallel_args(&mut self, call: &Expr) {
        let args: &[CallArg] = match &call.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args,
            _ => return,
        };
        let mode_of = |e: &Expr| -> ArgMode {
            args.iter().find(|a| std::ptr::eq(&a.expr, e)).map_or(ArgMode::Default, |a| a.mode)
        };
        for (a, b) in self.call_pairs(call) {
            // A declared function captures nothing; a parameter's own callers
            // are checked for it.
            if self.callee_key(a).is_some() || self.is_param_ref(a) {
                continue;
            }
            let sides = [self.as_fork_block(a, mode_of(a)), self.as_fork_block(b, mode_of(b))];
            let before = self.errors.len();
            self.check_fork_sides(&sides);
            let found: Vec<OwnershipError> = self.errors.split_off(before);
            if let Some(first) = found.into_iter().next() {
                let what = first.kind.to_string();
                self.errors.push(OwnershipError {
                    kind: OwnershipErrorKind::ParallelArgsConflict { what, other_span: b.span },
                    span: a.span,
                });
                return;
            }
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

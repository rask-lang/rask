// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function values a callee runs in parallel, checked where they're passed
//! (conc.async/B4).
//!
//! `func both(f: func(), g: func()) { let h = ThreadPool.spawn { f() }  g()
//! h.join()! }` is fine on its own. What makes it a race is the call:
//! `both(t, t)` with a `t` that writes what it captured runs one closure on
//! two threads, and `both(|| n += 1, || print(n))` writes and reads `n` at
//! once. Inside `both` nothing can tell, so each function gets a summary of
//! the parameter pairs it may use at the same time, and at every call the
//! two arguments of each pair are checked as a borrowing task and the code
//! beside it would be: the same conflicts (B2), the same one-side values
//! (B3), the same rules for a shared rack (RK16).
//!
//! A pair is a function-typed parameter one side names and any parameter the
//! other side names, itself included. A side is a borrowing task or the code
//! before its join, or an argument of a call that may run them at once, so a
//! function that hands its parameters on, bare or inside a closure, takes the
//! pair on. A call through a function value nothing here can read may run its
//! function arguments at once, so it counts as one.

use std::collections::{BTreeSet, HashMap};

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
    /// The parameters that hold functions.
    pub(crate) fn_params: BTreeSet<String>,
    /// `(f, q)`: function value `f` may run while `q` is used elsewhere.
    pub(crate) pairs: BTreeSet<(String, String)>,
    /// `(f, args)`: function parameter `f` is called with these arguments,
    /// each the set of parameters it names. What that runs at once depends
    /// on what each caller passes for `f`.
    pub(crate) calls: BTreeSet<(String, Vec<BTreeSet<String>>)>,
}

pub(crate) type ParallelUses = HashMap<Key, ParallelUse>;

/// A node id no expression has: the wrappers built to check an argument, or
/// the code beside a task, as one side have no type of their own to look up.
pub(crate) const SYNTHETIC: NodeId = NodeId(u32::MAX);

impl<'a> OwnershipChecker<'a> {
    /// The function a callee expression names: a declared function, or a
    /// local bound to one (`let b = both`).
    pub(crate) fn callee_key(&self, func: &Expr) -> Option<Key> {
        if let Some(k) = self.fn_key(func) {
            return Some(k);
        }
        match &func.kind {
            ExprKind::Ident(name) if self.bound_once(name) => self.named_fn_values.get(name).cloned(),
            _ => None,
        }
    }

    /// Whether `name` is bound once in the body being checked, so the one
    /// binding is what every use of it holds.
    fn bound_once(&self, name: &str) -> bool {
        self.fn_name_bindings.get(name).is_none_or(|n| *n <= 1)
    }

    /// Whether `e` names a parameter: of the function being checked, or of a
    /// closure literal in it. Either way the callers say what it holds.
    fn names_any_param(&self, e: &Expr) -> bool {
        let ExprKind::Ident(_) = &e.kind else { return false };
        let Some(sym) = self.program.resolutions.get(&e.id) else { return false };
        self.program
            .symbols
            .get(*sym)
            .is_some_and(|s| matches!(s.kind, rask_resolve::SymbolKind::Parameter { .. }))
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
            self.parallel_uses.insert(key.clone(), ParallelUse { params, ..Default::default() });
        }
        // Bodies sharing a key are joined, and what is found once stays: both
        // keep the loop from trading answers between rounds.
        loop {
            let mut changed = false;
            for (key, f) in &bodies {
                let params: Vec<SummaryParam> = f
                    .params
                    .iter()
                    .map(|p| SummaryParam {
                        name: p.name.clone(),
                        span: p.name_span,
                        is_fn: matches!(p.ty, Some(rask_ast::ty::TypeExpr::Func { .. })),
                    })
                    .collect();
                let found = self.summarize(&params, Body::Stmts(&f.body));
                let entry = self.parallel_uses.get_mut(key).expect("seeded above");
                for p in found.fn_params {
                    changed |= entry.fn_params.insert(p);
                }
                for p in found.pairs {
                    changed |= entry.pairs.insert(p);
                }
                for c in found.calls {
                    changed |= entry.calls.insert(c);
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// What a body with these parameters may run at once, and which of its
    /// function parameters it calls with which of its parameters.
    fn summarize<'b>(&self, params: &[SummaryParam], body: Body<'b>) -> ParallelUse {
        let spans: Vec<Span> = params.iter().map(|p| p.span).collect();
        let mut fn_params: BTreeSet<String> =
            params.iter().filter(|p| p.is_fn).map(|p| p.name.clone()).collect();
        // Closures the body binds to a name: `let w = || x()` names `x`
        // wherever `w` is handed on, and `w(…)` runs the literal.
        let mut locals: HashMap<String, &'b Expr> = HashMap::new();
        let mut fns: HashMap<String, Key> = HashMap::new();
        let mut bindings: HashMap<String, u32> = HashMap::new();
        let mut bind = |name: &String, init: &'b Expr, locals: &mut HashMap<String, &'b Expr>, fns: &mut HashMap<String, Key>| {
            *bindings.entry(name.clone()).or_default() += 1;
            if matches!(init.kind, ExprKind::Closure { .. }) {
                locals.insert(name.clone(), init);
            } else if let Some(k) = self.fn_key(init) {
                fns.insert(name.clone(), k);
            } else if matches!(init.kind, ExprKind::Ident(_)) {
                // `let g = f`: `g` names whatever `f` names.
                locals.insert(name.clone(), init);
            }
        };
        let mut seen_stmts: std::collections::HashSet<*const Stmt> = std::collections::HashSet::new();
        body.each(&mut |e| {
            for st in Self::own_stmts(e) {
                if !seen_stmts.insert(st as *const Stmt) {
                    continue;
                }
                match &st.kind {
                    StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => {
                        bind(name, init, &mut locals, &mut fns)
                    }
                    StmtKind::Assign { target: Expr { kind: ExprKind::Ident(name), .. }, value, .. } => {
                        bind(name, value, &mut locals, &mut fns)
                    }
                    _ => {}
                }
            }
            if let ExprKind::Ident(n) = &e.kind {
                if self.names_param_at(e, &spans) && matches!(self.node_ty(&e.id), Some(Type::Fn { .. })) {
                    fn_params.insert(n.clone());
                }
            }
        });
        if let Body::Stmts(stmts) = body {
            for st in stmts {
                if !seen_stmts.insert(st as *const Stmt) {
                    continue;
                }
                match &st.kind {
                    StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => {
                        bind(name, init, &mut locals, &mut fns)
                    }
                    StmtKind::Assign { target: Expr { kind: ExprKind::Ident(name), .. }, value, .. } => {
                        bind(name, value, &mut locals, &mut fns)
                    }
                    _ => {}
                }
            }
        }
        // A name bound more than once holds different things at different
        // calls, so no one binding answers for it.
        locals.retain(|n, _| bindings.get(n).is_none_or(|c| *c <= 1));
        fns.retain(|n, _| bindings.get(n).is_none_or(|c| *c <= 1));
        // The parameters an expression names, by resolution, through the
        // local closures it names.
        fn collect<'x>(
            this: &OwnershipChecker<'_>,
            e: &'x Expr,
            spans: &[Span],
            locals: &HashMap<String, &'x Expr>,
            out: &mut BTreeSet<String>,
            depth: u32,
        ) {
            visit::walk_expr(e, &mut |x| {
                if let ExprKind::Ident(n) = &x.kind {
                    if this.names_param_at(x, spans) {
                        out.insert(n.clone());
                    } else if let Some(lit) = locals.get(n) {
                        if depth < 8 {
                            collect(this, lit, spans, locals, out, depth + 1);
                        }
                    }
                }
            });
        }
        let named_in = |e: &Expr| -> BTreeSet<String> {
            let mut out = BTreeSet::new();
            collect(self, e, &spans, &locals, &mut out, 0);
            out
        };
        let at = Site { spans: &spans, locals: &locals, fns: &fns };
        let mut pairs = BTreeSet::new();
        let mut calls = BTreeSet::new();
        body.each(&mut |e| {
            let mut groups: Vec<(&Expr, &Expr)> = Vec::new();
            match &e.kind {
                ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                    let found = self.parallel_effects(e, &at);
                    groups.extend(found.pairs);
                    for (via, args) in found.through {
                        let ExprKind::Ident(name) = &via.kind else { continue };
                        let args = args
                            .iter()
                            .map(|exprs| exprs.iter().flat_map(|x| named_in(x)).collect())
                            .collect();
                        calls.insert((name.clone(), args));
                    }
                }
                _ => {}
            }
            for (a, b) in groups {
                let (here, there) = (named_in(a), named_in(b));
                for p in here.iter().filter(|p| fn_params.contains(*p)) {
                    for q in &there {
                        pairs.insert((p.clone(), q.clone()));
                    }
                }
            }
        });
        // A task that borrows the frame runs beside the code up to its join
        // (conc.async/S5), the same as two blocks that may run at once.
        let windows = match body {
            Body::Stmts(stmts) => crate::borrowed_tasks::all_windows(stmts),
            Body::Expr(Expr { kind: ExprKind::Block(stmts), .. }) => crate::borrowed_tasks::all_windows(stmts),
            Body::Expr(_) => Vec::new(),
        };
        for w in &windows {
            let beside = crate::borrowed_tasks::beside(w, SYNTHETIC);
            for (a, b) in [(w.body, &beside), (&beside, w.body)] {
                let (here, there) = (named_in(a), named_in(b));
                for p in here.iter().filter(|p| fn_params.contains(*p)) {
                    for q in &there {
                        pairs.insert((p.clone(), q.clone()));
                    }
                }
            }
        }
        ParallelUse { params: params.iter().map(|p| p.name.clone()).collect(), fn_params, pairs, calls }
    }

    /// What this call may run at the same time, as pairs of argument
    /// expressions, plus the calls it makes through the parameters at `at`,
    /// which that function's callers answer for.
    fn parallel_effects<'e>(&self, call: &'e Expr, at: &Site<'_, 'e>) -> Effects<'e> {
        let mut out = Effects::default();
        match &call.kind {
            ExprKind::Call { func, args } => {
                if let Some(key) = self.callee_key(func).filter(|k| self.parallel_uses.contains_key(k)) {
                    let uses = &self.parallel_uses[&key];
                    let bind = bind_args(uses, None, args);
                    self.instantiate(uses, &bind, at, 0, &mut out);
                } else if matches!(self.node_ty(&func.id), Some(Type::Fn { .. })) {
                    let args = args.iter().map(|a| vec![&a.expr]).collect();
                    self.apply_fn_value(func, args, at, 0, &mut out);
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                let key = self.receiver_type_name(object).map(|ty| Key::Method(ty, method.clone()));
                if let Some(uses) = key.as_ref().and_then(|k| self.parallel_uses.get(k)) {
                    let bind = bind_args(uses, Some(object), args);
                    self.instantiate(uses, &bind, at, 0, &mut out);
                }
            }
            _ => {}
        }
        out
    }

    /// A summary with its parameters bound to the expressions passed for them.
    fn instantiate<'e>(
        &self,
        uses: &ParallelUse,
        bind: &HashMap<String, Vec<&'e Expr>>,
        at: &Site<'_, 'e>,
        depth: u32,
        out: &mut Effects<'e>,
    ) {
        let none = Vec::new();
        let get = |p: &str| bind.get(p).unwrap_or(&none);
        for (p, q) in &uses.pairs {
            for a in get(p) {
                for b in get(q) {
                    out.pairs.push((*a, *b));
                }
            }
        }
        for (via, args) in &uses.calls {
            let args: Vec<Vec<&'e Expr>> =
                args.iter().map(|set| set.iter().flat_map(|p| get(p).iter().copied()).collect()).collect();
            for f in get(via) {
                self.apply_fn_value(f, args.clone(), at, depth + 1, out);
            }
        }
    }

    /// A call through the function value `f` with these arguments: what the
    /// function `f` turns out to be decides what runs at once.
    fn apply_fn_value<'e>(
        &self,
        f: &'e Expr,
        args: Vec<Vec<&'e Expr>>,
        at: &Site<'_, 'e>,
        depth: u32,
        out: &mut Effects<'e>,
    ) {
        if depth <= MAX_DEPTH {
            // A declared function, or a name bound to one: its own summary.
            let named = match &f.kind {
                ExprKind::Ident(name) => at.fns.get(name).cloned(),
                _ => None,
            };
            if let Some(uses) = self.callee_key(f).or(named).and_then(|k| self.parallel_uses.get(&k)) {
                let bind = uses.params.iter().cloned().zip(args).collect();
                self.instantiate(uses, &bind, at, depth, out);
                return;
            }
            // A parameter of the function being summarized or checked: its
            // callers say what it is.
            if self.names_param_at(f, at.spans) {
                out.through.push((f, args));
                return;
            }
            // A closure literal, written here or bound to a local name: its
            // body, summarized over its own parameters.
            let literal = match &f.kind {
                ExprKind::Closure { .. } => Some(f),
                ExprKind::Ident(name) => at.locals.get(name).copied(),
                _ => None,
            };
            if let Some(Expr { kind: ExprKind::Closure { params, body, .. }, .. }) = literal {
                let params: Vec<SummaryParam> = params
                    .iter()
                    .map(|p| SummaryParam {
                        name: p.name.clone(),
                        span: p.name_span,
                        is_fn: matches!(p.ty, Some(rask_ast::ty::TypeExpr::Func { .. })),
                    })
                    .collect();
                let uses = self.summarize(&params, Body::Expr(body));
                let bind = uses.params.iter().cloned().zip(args).collect();
                self.instantiate(&uses, &bind, at, depth, out);
                return;
            }
        }
        // Anything else — a field, an element, a call's result — may be a
        // function that runs every function argument beside every argument.
        let all: Vec<&'e Expr> = args.into_iter().flatten().collect();
        for a in all.iter().filter(|a| matches!(self.node_ty(&a.id), Some(Type::Fn { .. }))) {
            for b in &all {
                out.pairs.push((*a, *b));
            }
        }
    }

    /// An argument as the side it may run as. A closure is its own body;
    /// a closure bound to a name is the literal it was bound to; anything else
    /// is a block that uses it, writing it when it goes to `mutate`.
    fn as_side(&self, arg: &Expr, mode: ArgMode) -> Expr {
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

    /// B4 at one call: each pair the callee may run at once, checked as two
    /// sides. What that finds is reported at the call.
    pub(crate) fn check_parallel_args(&mut self, call: &Expr) {
        let args: &[CallArg] = match &call.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args,
            _ => return,
        };
        let mode_of = |e: &Expr| -> ArgMode {
            args.iter().find(|a| std::ptr::eq(&a.expr, e)).map_or(ArgMode::Default, |a| a.mode)
        };
        let locals = self.closure_literals.clone();
        let locals: HashMap<String, &Expr> =
            locals.iter().filter(|(n, _)| self.bound_once(n)).map(|(n, e)| (n.clone(), e)).collect();
        let fns = HashMap::new();
        let spans = self.current_param_spans.clone();
        let at = Site { spans: &spans, locals: &locals, fns: &fns };
        let effects = self.parallel_effects(call, &at);
        let mut found = effects.pairs;
        // A call through a parameter: what runs at once is the callers'
        // question for the arguments they passed, and this body's own for the
        // rest. An argument that is a local closure exists only here, so the
        // callers can't check it: it's checked here as though the function may
        // run it beside every argument.
        let passed_in = |e: &Expr| self.callee_key(e).is_some() || self.names_any_param(e);
        for (_, args) in &effects.through {
            let all: Vec<&Expr> = args.iter().flatten().copied().collect();
            for a in all.iter().filter(|a| matches!(self.node_ty(&a.id), Some(Type::Fn { .. }))) {
                for b in &all {
                    if !(passed_in(a) && passed_in(b)) {
                        found.push((*a, *b));
                    }
                }
            }
        }
        let pairs: Vec<(Expr, ArgMode, Expr, ArgMode)> = found
            .into_iter()
            // A declared function captures nothing; a parameter's own callers
            // are checked for it.
            .filter(|(a, _)| !passed_in(a))
            .map(|(a, b)| (a.clone(), mode_of(a), b.clone(), mode_of(b)))
            .collect();
        for (a, a_mode, b, b_mode) in pairs {
            let sides = [self.as_side(&a, a_mode), self.as_side(&b, b_mode)];
            let before = self.errors.len();
            self.check_parallel_sides(&sides);
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

    /// B4 for every call in a body, once the body has been walked and its
    /// closures are known.
    pub(crate) fn check_parallel_calls(&mut self, body: &[Stmt]) {
        let mut calls: Vec<Expr> = Vec::new();
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| {
                if matches!(e.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) {
                    calls.push(e.clone());
                }
            });
        }
        for call in &calls {
            self.check_parallel_args(call);
        }
    }
}

/// How deep a chain of function values is followed before the answer is the
/// cautious one.
const MAX_DEPTH: u32 = 8;

/// A parameter a summary is in terms of.
struct SummaryParam {
    name: String,
    span: Span,
    /// Declared with a function type. One used as a function counts too.
    is_fn: bool,
}

/// A function's body, or a closure literal's.
#[derive(Clone, Copy)]
enum Body<'b> {
    Stmts(&'b [Stmt]),
    Expr(&'b Expr),
}

impl<'b> Body<'b> {
    fn each(&self, f: &mut dyn FnMut(&'b Expr)) {
        match self {
            Body::Stmts(stmts) => {
                for st in *stmts {
                    visit::walk_stmt(st, &mut |e| f(e));
                }
            }
            Body::Expr(e) => visit::walk_expr(e, &mut |x| f(x)),
        }
    }
}

/// Where a call is read: the parameters of the function around it, and the
/// closure literals it binds to names.
struct Site<'s, 'e> {
    spans: &'s [Span],
    locals: &'s HashMap<String, &'e Expr>,
    /// Names bound to a declared function.
    fns: &'s HashMap<String, Key>,
}

/// What one call may run at once.
#[derive(Default)]
struct Effects<'e> {
    /// Argument expressions that may run at the same time.
    pairs: Vec<(&'e Expr, &'e Expr)>,
    /// Calls through a parameter of the function around the call, with each
    /// argument's expressions: what runs at once is up to that function's
    /// callers.
    through: Vec<(&'e Expr, Vec<Vec<&'e Expr>>)>,
}

/// The argument expressions passed for each parameter of `uses`.
fn bind_args<'e>(uses: &ParallelUse, receiver: Option<&'e Expr>, args: &'e [CallArg]) -> HashMap<String, Vec<&'e Expr>> {
    let mut bind: HashMap<String, Vec<&'e Expr>> = HashMap::new();
    let mut positional = uses.params.iter().filter(|p| receiver.is_none() || *p != "self");
    if let Some(r) = receiver {
        bind.insert("self".to_string(), vec![r]);
    }
    for a in args {
        let param = match &a.name {
            Some(label) => uses.params.iter().find(|p| *p == label),
            None => positional.next(),
        };
        if let Some(p) = param {
            bind.insert(p.clone(), vec![&a.expr]);
        }
    }
    bind
}

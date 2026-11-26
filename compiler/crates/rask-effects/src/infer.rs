// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Effect inference engine (comp.effects INF1-INF5).
//!
//! Three phases:
//! 1. Collect function declarations, classify direct effects from body
//! 2. Build call graph from function bodies
//! 3. Fixed-point propagation: union callee effects into callers until stable

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, SpawnTarget};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::{Effects, EffectMap, CallTargets, ParamNeeds, Runtimes};
use rask_ast::decl::TypeArgTarget;
use crate::sources;

/// Qualified function name (plain name or "Type.method").
type FuncName = String;

/// Run effect inference on declarations.
pub fn infer(decls: &[Decl], targets: &CallTargets) -> EffectMap {
    infer_with_reach(decls, &[], targets, &Default::default()).0
}

/// Effects, plus the functions that can only ever run with a runtime installed.
///
/// The second set is what CW2 needs. A function reached only from inside a
/// `spawn` closure or a `using Multitasking { }` block is running under the
/// runtime whatever its own body looks like, so telling its author to install
/// one is wrong twice over — the block is already there, and entering a second
/// one segfaults (rask-lang/rask#524).
pub fn infer_with_reach(
    program: &[Decl],
    stdlib: &[Decl],
    targets: &CallTargets,
    c_macros: &crate::CMacros,
) -> (EffectMap, HashSet<FuncName>, ParamNeeds) {
    let mut pass = InferPass::new(targets, c_macros);
    pass.run(program, stdlib);
    let runtime_only = pass.runtime_only();
    (pass.effects, runtime_only, pass.param_needs)
}

/// Type parameters a body names as declared.
fn same_names(params: &[rask_ast::decl::TypeParam]) -> HashMap<String, String> {
    params.iter().map(|p| (p.name.clone(), p.name.clone())).collect()
}

/// The type's parameters as an `extend` header names them, by position: the
/// body of `extend Holder<U>` says `U` for what calls bind as `Holder`'s `T`.
fn extend_params(target: &rask_ast::ty::TypeExpr, decls: &[Decl]) -> HashMap<String, String> {
    let rask_ast::ty::TypeExpr::Named { path, args } = target else { return HashMap::new() };
    let Some(name) = path.last() else { return HashMap::new() };
    let declared = decls.iter().find_map(|d| match &d.kind {
        DeclKind::Struct(s) if s.name == *name => Some(&s.type_params),
        DeclKind::Enum(e) if e.name == *name => Some(&e.type_params),
        _ => None,
    });
    let Some(declared) = declared else { return HashMap::new() };
    args.iter()
        .zip(declared)
        .filter_map(|(arg, p)| match arg {
            rask_ast::ty::TypeExpr::Named { path, args } if path.len() == 1 && args.is_empty() => {
                Some((path[0].clone(), p.name.clone()))
            }
            _ => None,
        })
        .collect()
}

/// A call that binds the callee's type parameters, and the runtimes installed
/// around it.
struct GenericCall {
    callee: FuncName,
    bindings: Vec<(String, TypeArgTarget)>,
    held: Runtimes,
}

/// What a call binds a type parameter to.
fn bound<'b>(bindings: &'b [(String, TypeArgTarget)], param: &str) -> Option<&'b TypeArgTarget> {
    bindings.iter().find(|(p, _)| p == param).map(|(_, t)| t)
}

/// What classifying a body reads besides the body.
struct Cx<'a> {
    targets: &'a CallTargets,
    imports: &'a sources::ImportedNames,
    /// Joins of tasks that borrow their frame (`crate::borrowing_joins`).
    borrowing_joins: &'a HashSet<rask_ast::NodeId>,
}

struct InferPass<'t> {
    /// The checker's name for each method call it resolved.
    targets: &'t CallTargets,
    /// What each C macro's body does, by the checker's reading.
    c_macros: &'t crate::CMacros,
    /// The imports of the unit being collected, for what a name at a call
    /// site means.
    imports: sources::ImportedNames,
    /// Per-function direct effects (before transitive propagation).
    effects: EffectMap,
    /// Call graph: caller → callees.
    call_graph: HashMap<FuncName, HashSet<FuncName>>,
    /// CC2: the runtimes each function's own task blocks need and it doesn't
    /// install.
    direct_needs_runtime: HashMap<FuncName, Runtimes>,
    /// CC2: callee names reached without an enclosing `using Multitasking {}` block.
    unguarded_callees: HashMap<FuncName, HashSet<FuncName>>,
    /// CC2: callee names reached without an enclosing `using ThreadPool {}` block.
    unguarded_pool_callees: HashMap<FuncName, HashSet<FuncName>>,
    /// CW2: callee names reached with no runtime installed.
    plain_callees: HashMap<FuncName, HashSet<FuncName>>,
    /// CW2: callee names reached with a runtime installed — inside that block,
    /// or inside a `spawn` closure.
    guarded_callees: HashMap<FuncName, HashSet<FuncName>>,
    /// CW2: `public` functions. A caller outside this program can reach one
    /// with no runtime installed, so its local call sites don't settle the
    /// question.
    reachable_from_outside: HashSet<FuncName>,
    /// The type parameters in scope in each function, by the name its body
    /// uses, mapped to the name a call binds: the function's own, and for a
    /// method the type's, which `extend Holder<U>` may rename from the
    /// declared `Holder<T>` (generics/GF6), so `U` is bound as `T`.
    type_params: HashMap<FuncName, HashMap<String, String>>,
    /// CC2: each generic function's method calls on a type parameter, direct
    /// at first and then through the generic calls it makes.
    param_needs: ParamNeeds,
    /// CC2: the generic calls each function makes.
    generic_calls: HashMap<FuncName, Vec<GenericCall>>,
}

impl<'t> InferPass<'t> {
    fn new(targets: &'t CallTargets, c_macros: &'t crate::CMacros) -> Self {
        Self {
            targets,
            c_macros,
            imports: sources::ImportedNames::default(),
            effects: HashMap::new(),
            call_graph: HashMap::new(),
            direct_needs_runtime: HashMap::new(),
            unguarded_callees: HashMap::new(),
            unguarded_pool_callees: HashMap::new(),
            plain_callees: HashMap::new(),
            guarded_callees: HashMap::new(),
            reachable_from_outside: HashSet::new(),
            type_params: HashMap::new(),
            param_needs: HashMap::new(),
            generic_calls: HashMap::new(),
        }
    }

    fn run(&mut self, program: &[Decl], stdlib: &[Decl]) {
        // The stdlib first, so a program declaration of the same name is the
        // one its own calls reach. Each unit reads its own imports: a name
        // means what that file's imports say.
        for decls in [stdlib, program] {
            self.imports = sources::ImportedNames::from_decls(decls);
            // Phase 1: Classify direct effects and build call graph
            self.collect(decls);
            // Phase 2: Mark extern functions conservatively (INF5)
            self.mark_externs(decls);
        }

        // Phase 3: Fixed-point propagation (FX2)
        self.propagate();

        // Phase 4: CC2 — propagate needs_runtime via unguarded-call edges,
        // once each generic call has said which bodies its type picks.
        self.resolve_generic_calls();
        let pick = |r: Runtimes| r.multitasking;
        for fname in propagate_needs(&self.direct_needs_runtime, pick, &self.unguarded_callees) {
            if let Some(e) = self.effects.get_mut(&fname) {
                e.needs_runtime.multitasking = true;
            }
        }
        let pick = |r: Runtimes| r.pool;
        for fname in propagate_needs(&self.direct_needs_runtime, pick, &self.unguarded_pool_callees) {
            if let Some(e) = self.effects.get_mut(&fname) {
                e.needs_runtime.pool = true;
            }
        }
    }

    // ── Phase 1: Collect ────────────────────────────────────────────

    fn collect(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    self.collect_fn(&f.name, f, HashMap::new());
                }
                DeclKind::Struct(s) => {
                    let outer = same_names(&s.type_params);
                    for method in &s.methods {
                        let qname = format!("{}.{}", s.name, method.name);
                        self.collect_fn(&qname, method, outer.clone());
                    }
                }
                DeclKind::Enum(e) => {
                    let outer = same_names(&e.type_params);
                    for method in &e.methods {
                        let qname = format!("{}.{}", e.name, method.name);
                        self.collect_fn(&qname, method, outer.clone());
                    }
                }
                DeclKind::Impl(i) => {
                    let outer = extend_params(&i.target_ty, decls);
                    for method in &i.methods {
                        let qname = crate::method_key(&i.target_ty, &method.name);
                        self.collect_fn(&qname, method, outer.clone());
                    }
                }
                DeclKind::Interface(t) => {
                    for method in &t.methods {
                        let qname = format!("{}.{}", t.name, method.name);
                        self.collect_fn(&qname, method, HashMap::new());
                    }
                }
                // A test or benchmark body is a caller like `main`. Left out,
                // a helper only tests call looked uncalled, so it counted as
                // an entry point that could run without a runtime, and CW2
                // warned about the loop in it that every test ran inside
                // `using Multitasking`. The name has a space in it, so no call
                // can resolve to it.
                DeclKind::Test(t) => {
                    self.collect_body(&format!("test {:?}", t.name), &t.body, &t.attrs, false);
                }
                DeclKind::Benchmark(b) => {
                    self.collect_body(&format!("benchmark {:?}", b.name), &b.body, &b.attrs, false);
                }
                _ => {}
            }
        }
    }

    fn collect_fn(&mut self, qname: &str, f: &FnDecl, mut params: HashMap<String, String>) {
        params.extend(same_names(&f.type_params));
        if !params.is_empty() {
            self.type_params.insert(qname.to_string(), params);
        }
        self.collect_body(qname, &f.body, &f.attrs, f.is_pub);
        // A stdlib native has no body to read, so its declaration says what
        // it does (`@blocks`, `@prints`, `@ambient`, `@grows`, `@shrinks`).
        let declared = f.declared_effects();
        if let Some(e) = self.effects.get_mut(qname) {
            e.blocks |= declared.blocks;
            e.prints |= declared.prints;
            e.ambient |= declared.ambient;
            e.grow |= declared.grows;
            e.shrink |= declared.shrinks;
        }
    }

    fn collect_body(&mut self, qname: &str, body: &[Stmt], _attrs: &[rask_ast::attr::Attr], is_pub: bool) {
        // PU2 says a `comptime func` is pure and that "effect inference
        // confirms this". It used to *assert* it: stamp `Effects::default()`
        // on anything wearing the keyword and skip the body, so the map could
        // never disagree with the marking. That made CT60 uncheckable — the one
        // thing that could have caught a `comptime func` doing I/O was reading
        // the answer off the keyword it was meant to verify. The body is
        // classified like any other now, and `comptime_purity::check` reports a
        // marking the body doesn't earn.

        // Classify direct effects from function body
        let mut direct = Effects::default();
        let mut callees = HashSet::new();
        let borrowing_joins: HashSet<rask_ast::NodeId> = rask_ast::tasks::all_windows(body)
            .iter()
            .filter(|w| matches!(w.spawn.kind, ExprKind::Spawn { target: SpawnTarget::Pool, .. }))
            .map(|w| w.join_call.id)
            .collect();
        let cx = Cx { targets: self.targets, imports: &self.imports, borrowing_joins: &borrowing_joins };
        classify_body(body, &mut direct, &mut callees, &cx);

        self.effects.insert(qname.to_string(), direct);

        if !callees.is_empty() {
            self.call_graph.insert(qname.to_string(), callees);
        }

        if is_pub {
            self.reachable_from_outside.insert(qname.to_string());
        }

        // CC2: compute which functions call spawn (or runtime-needing callees) without a guard
        let no_params = HashMap::new();
        let params = self.type_params.get(qname).unwrap_or(&no_params);
        let mut scan = ReachScan::new(self.targets, &self.imports, params);
        let direct_needs_rt = rt_scan_stmts(body, Runtimes::NONE, &mut scan);
        if direct_needs_rt.any() {
            self.direct_needs_runtime.insert(qname.to_string(), direct_needs_rt);
        }
        if !scan.unguarded.is_empty() {
            self.unguarded_callees.insert(qname.to_string(), scan.unguarded);
        }
        if !scan.unguarded_pool.is_empty() {
            self.unguarded_pool_callees.insert(qname.to_string(), scan.unguarded_pool);
        }
        if !scan.plain.is_empty() {
            self.plain_callees.insert(qname.to_string(), scan.plain);
        }
        if !scan.guarded.is_empty() {
            self.guarded_callees.insert(qname.to_string(), scan.guarded);
        }
        if !scan.param_calls.is_empty() {
            self.param_needs.insert(qname.to_string(), scan.param_calls);
        }
        if !scan.generic_calls.is_empty() {
            self.generic_calls.insert(qname.to_string(), scan.generic_calls);
        }
    }

    /// comp.effects/INF1 for CC2: a generic function's effects are those of
    /// the instance a call picks. `run<T: Starter>(x: T) { x.go() }` spawns
    /// when `T` is a type whose `go` spawns, and not otherwise, so the edge to
    /// `Sx.go` belongs to the caller that picked `Sx`.
    ///
    /// First, a type parameter handed on to another generic call carries that
    /// callee's method calls back to this function's own parameter, until
    /// nothing changes. Then each call that names a type gets an edge to that
    /// type's method, outside a block where neither function installs one.
    fn resolve_generic_calls(&mut self) {
        loop {
            let mut changed = false;
            for (caller, calls) in &self.generic_calls {
                let Some(own) = self.type_params.get(caller) else { continue };
                for call in calls {
                    let Some(reached) = self.param_needs.get(&call.callee) else { continue };
                    let mut add = Vec::new();
                    for ((param, method), open) in reached {
                        let open = open.missing_from(call.held);
                        let Some(TypeArgTarget::Param(q)) = bound(&call.bindings, param) else { continue };
                        let Some(q) = own.get(q) else { continue };
                        if open.any() {
                            add.push(((q.clone(), method.clone()), open));
                        }
                    }
                    let mine = self.param_needs.entry(caller.clone()).or_default();
                    for (key, open) in add {
                        let slot = mine.entry(key).or_default();
                        if (*slot | open) != *slot {
                            *slot |= open;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (caller, calls) in &self.generic_calls {
            for call in calls {
                let Some(reached) = self.param_needs.get(&call.callee) else { continue };
                for ((param, method), open) in reached {
                    let open = open.missing_from(call.held);
                    let Some(TypeArgTarget::Named(ty)) = bound(&call.bindings, param) else { continue };
                    let name = format!("{ty}.{method}");
                    if open.multitasking {
                        self.unguarded_callees.entry(caller.clone()).or_default().insert(name.clone());
                    }
                    if open.pool {
                        self.unguarded_pool_callees.entry(caller.clone()).or_default().insert(name);
                    }
                }
            }
        }
    }

    // ── CW2: which functions can only run under a runtime ───────────

    /// Functions every path to which passes through a runtime-installing
    /// construct — a `spawn` closure or a `using Multitasking { }` block.
    ///
    /// Two walks. First, everything reachable from a guarded call: those *can*
    /// run under the runtime. Second, everything reachable from an entry point
    /// through unguarded calls only: those can run without one. A function in
    /// the first set and not the second is only ever reached with a runtime
    /// installed, so CW2 has nothing to warn it about.
    ///
    /// An entry point is a function nothing in this program calls — `main`, a
    /// `test` body, and every `public` function a caller outside the file could
    /// reach. Anything whose callers aren't all visible stays in the second set
    /// and keeps today's behaviour.
    fn runtime_only(&self) -> HashSet<FuncName> {
        // A method is recorded as `broadcast` at a call on a binding and as
        // `Room.broadcast` at one on a type name. Both spellings have to find
        // the declaration, or `Room.broadcast` looks like nothing calls it and
        // becomes an entry point — which is the bug this whole walk exists to
        // avoid.
        let resolve = |name: &str| -> Vec<&str> {
            let suffix = format!(".{}", name);
            self.effects
                .keys()
                .filter(|k| k.as_str() == name || k.ends_with(&suffix))
                .map(|k| k.as_str())
                .collect()
        };

        let mut called: HashSet<&str> = HashSet::new();
        for callees in self.plain_callees.values().chain(self.guarded_callees.values()) {
            for c in callees {
                called.extend(resolve(c));
            }
        }

        let reach = |seeds: Vec<&str>, edges: &HashMap<FuncName, HashSet<FuncName>>| {
            let mut seen: HashSet<String> = HashSet::new();
            let mut work: Vec<String> = seeds.into_iter().map(str::to_string).collect();
            while let Some(f) = work.pop() {
                if !seen.insert(f.clone()) {
                    continue;
                }
                if let Some(callees) = edges.get(&f) {
                    for c in callees {
                        for target in resolve(c) {
                            work.push(target.to_string());
                        }
                    }
                }
            }
            seen
        };

        // Once a runtime is installed it stays installed, so past the first
        // guarded call every edge counts.
        let mut all_edges: HashMap<FuncName, HashSet<FuncName>> = self.plain_callees.clone();
        for (caller, callees) in &self.guarded_callees {
            all_edges.entry(caller.clone()).or_default().extend(callees.iter().cloned());
        }

        let guarded_seeds: Vec<&str> = self
            .guarded_callees
            .values()
            .flat_map(|cs| cs.iter())
            .flat_map(|c| resolve(c))
            .collect();
        let under_runtime = reach(guarded_seeds, &all_edges);

        let entries: Vec<&str> = self
            .effects
            .keys()
            .filter(|k| !called.contains(k.as_str()) || self.reachable_from_outside.contains(*k))
            .map(|k| k.as_str())
            .collect();
        let bare = reach(entries, &self.plain_callees);

        under_runtime.difference(&bare).cloned().collect()
    }

    // ── Phase 2: Extern declarations ────────────────────────────────

    /// INF5: a C function could do anything, so it blocks and touches ambient
    /// state unless its declaration says `@memory_only`. Either way it's C
    /// code. A C macro has its body's effects: arithmetic is pure, since the
    /// compiler computes it; a field read is C code; a call has the effects
    /// of the C function it calls.
    fn mark_externs(&mut self, decls: &[Decl]) {
        for decl in decls {
            if let DeclKind::Extern(e) = &decl.kind {
                // `@ambient`: reads ambient state (the clock, randomness) but
                // never waits, which is a protocol engine's whole shape.
                let says = |name: &str| e.attrs.iter().any(|a| a.is(name));
                let effects = if let Some(body) = self.c_macros.get(&e.name) {
                    if !body.calls.is_empty() {
                        self.call_graph.entry(e.name.clone()).or_default().extend(body.calls.iter().cloned());
                    }
                    Effects { c_code: body.touches_memory, ..Default::default() }
                } else if says("memory_only") {
                    Effects { c_code: true, ..Default::default() }
                } else {
                    Effects { blocks: !says("ambient"), ambient: true, c_code: true, ..Default::default() }
                };
                self.effects.insert(e.name.clone(), effects);
            }
        }
    }

    // ── Phase 3: Fixed-point propagation ────────────────────────────

    fn propagate(&mut self) {
        // Callee → callers, once. A function whose effects grow hands them to
        // its callers, and only those are looked at again; walking the whole
        // graph per round was quadratic once the stdlib's bodies were in it.
        let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
        for (caller, callees) in &self.call_graph {
            for callee in callees {
                callers.entry(callee.as_str()).or_default().push(caller.as_str());
            }
        }
        let mut work: Vec<String> = self
            .effects
            .iter()
            .filter(|(_, e)| !e.is_pure() || e.c_code)
            .map(|(k, _)| k.clone())
            .collect();
        while let Some(callee) = work.pop() {
            let callee_effects = self.effects.get(&callee).copied().unwrap_or_default();
            let Some(cs) = callers.get(callee.as_str()) else { continue };
            for &caller in cs {
                let caller_effects = self.effects.entry(caller.to_string()).or_default();
                let before = *caller_effects;
                caller_effects.union(callee_effects);
                if *caller_effects != before {
                    work.push(caller.to_string());
                }
            }
        }
    }
}

// ── Body classification ──────────────────────────────────────────────

/// Walk a function body, collecting direct effects and callee names.
fn classify_body(stmts: &[Stmt], effects: &mut Effects, callees: &mut HashSet<String>, t: &Cx) {
    for stmt in stmts {
        classify_stmt(stmt, effects, callees, t);
    }
}

fn classify_stmt(stmt: &Stmt, effects: &mut Effects, callees: &mut HashSet<String>, t: &Cx) {
    match &stmt.kind {
        StmtKind::Expr(e) => classify_expr(e, effects, callees, t),
        StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
            classify_expr(init, effects, callees, t);
        }
        StmtKind::MutTuple { init, .. }
        | StmtKind::LetTuple { init, .. }
        | StmtKind::LetStruct { init, .. } => {
            classify_expr(init, effects, callees, t);
        }
        StmtKind::Assign { target, value, .. } => {
            classify_expr(target, effects, callees, t);
            classify_expr(value, effects, callees, t);
        }
        StmtKind::Return(Some(e)) => classify_expr(e, effects, callees, t),
        StmtKind::Return(None) => {}
        StmtKind::Break { value: Some(v), .. } => classify_expr(v, effects, callees, t),
        StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        StmtKind::Discard { .. } => {}
        StmtKind::While { cond, body, .. } => {
            classify_expr(cond, effects, callees, t);
            classify_body(body, effects, callees, t);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            classify_expr(expr, effects, callees, t);
            classify_body(body, effects, callees, t);
        }
        StmtKind::Loop { body, .. } => classify_body(body, effects, callees, t),
        StmtKind::For { iter, body, .. } => {
            classify_expr(iter, effects, callees, t);
            classify_body(body, effects, callees, t);
        }
        StmtKind::Ensure { body, else_handler } => {
            classify_body(body, effects, callees, t);
            if let Some((_, handler)) = else_handler {
                classify_body(handler, effects, callees, t);
            }
        }
        StmtKind::Comptime(body) => classify_body(body, effects, callees, t),
        StmtKind::ComptimeFor { body, .. } => classify_body(body, effects, callees, t),
    }
}

fn classify_expr(expr: &Expr, effects: &mut Effects, callees: &mut HashSet<String>, t: &Cx) {
    if let Some(name) = crate::function_ref(expr, t.targets, t.imports) {
        callees.insert(name);
    }
    match &expr.kind {
        ExprKind::Call { func, args } => {
            if let Some(name) = crate::free_callee(expr.id, func, t.targets, t.imports) {
                callees.insert(name);
            }
            classify_expr(func, effects, callees, t);
            for arg in args {
                classify_expr(&arg.expr, effects, callees, t);
            }
        }

        ExprKind::MethodCall { object, method, args, .. } => {
            let effect = t.targets.methods.get(&expr.id).map(|target| target.effect).unwrap_or_default();
            effects.union(sources::size_effects(effect));
            if !t.borrowing_joins.contains(&expr.id) {
                for name in crate::method_callees(expr.id, object, method, t.targets, t.imports) {
                    callees.insert(name);
                }
            }

            classify_expr(object, effects, callees, t);
            for arg in args {
                classify_expr(&arg.expr, effects, callees, t);
            }
        }

        // IO3: an `unsafe` block is what it calls. A C function in it is
        // conservative on its own (INF5); raw pointer work touches nothing
        // outside the program.
        ExprKind::Unsafe { body } => classify_body(body, effects, callees, t),

        // Spawn is an async source (AS1)

        // Recurse into all other expression kinds
        ExprKind::Binary { left, right, .. } => {
            classify_expr(left, effects, callees, t);
            classify_expr(right, effects, callees, t);
        }
        ExprKind::Unary { operand, .. } => classify_expr(operand, effects, callees, t),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            classify_expr(object, effects, callees, t);
        }
        ExprKind::DynamicField { object, field_expr } => {
            classify_expr(object, effects, callees, t);
            classify_expr(field_expr, effects, callees, t);
        }
        ExprKind::Index { object, index } => {
            classify_expr(object, effects, callees, t);
            classify_expr(index, effects, callees, t);
        }
        ExprKind::Block(stmts) => classify_body(stmts, effects, callees, t),
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            classify_expr(cond, effects, callees, t);
            classify_expr(then_branch, effects, callees, t);
            if let Some(e) = else_branch {
                classify_expr(e, effects, callees, t);
            }
        }
        ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
            classify_expr(expr, effects, callees, t);
            classify_expr(then_branch, effects, callees, t);
            if let Some(e) = else_branch {
                classify_expr(e, effects, callees, t);
            }
        }
        ExprKind::GuardPattern { expr, else_branch, .. } => {
            classify_expr(expr, effects, callees, t);
            classify_expr(else_branch, effects, callees, t);
        }
        ExprKind::IsPattern { expr, .. } => classify_expr(expr, effects, callees, t),
        ExprKind::Match { scrutinee, arms } => {
            classify_expr(scrutinee, effects, callees, t);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    classify_expr(g, effects, callees, t);
                }
                classify_expr(&arm.body, effects, callees, t);
            }
        }
        ExprKind::Try { expr: e } | ExprKind::Take { place: e } => {
            classify_expr(e, effects, callees, t);
        }
        ExprKind::Catch { value, clause } => {
            classify_expr(value, effects, callees, t);
            classify_expr(&clause.body, effects, callees, t);
        }
        ExprKind::IsPresent { expr: e, .. } => {
            classify_expr(e, effects, callees, t);
        }
        ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } | ExprKind::Convert { expr: e, .. } => {
            classify_expr(e, effects, callees, t);
        }
        ExprKind::NullCoalesce { value, default } => {
            classify_expr(value, effects, callees, t);
            classify_expr(default, effects, callees, t);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start { classify_expr(s, effects, callees, t); }
            if let Some(e) = end { classify_expr(e, effects, callees, t); }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                classify_expr(&f.value, effects, callees, t);
            }
            if let Some(s) = spread {
                classify_expr(s, effects, callees, t);
            }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                classify_expr(e, effects, callees, t);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            classify_expr(value, effects, callees, t);
            classify_expr(count, effects, callees, t);
        }
        ExprKind::UsingBlock { args, body, .. } => {
            for arg in args {
                classify_expr(&arg.expr, effects, callees, t);
            }
            classify_body(body, effects, callees, t);
        }
        ExprKind::WithAs { bindings, body } => {
            for binding in bindings {
                classify_expr(&binding.source, effects, callees, t);
            }
            classify_body(body, effects, callees, t);
        }
        ExprKind::Closure { body, .. } => classify_expr(body, effects, callees, t),
        // Starting a task returns at once: concurrency, not a wait (AS1, #1362).
        ExprKind::Spawn { body, .. } => {
            effects.async_ = true;
            classify_expr(body, effects, callees, t);
        }
        ExprKind::Comptime { body }
        | ExprKind::Loop { body, .. } => {
            classify_body(body, effects, callees, t);
        }
        ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
            classify_expr(condition, effects, callees, t);
            if let Some(m) = message {
                classify_expr(m, effects, callees, t);
            }
        }
        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                        classify_expr(channel, effects, callees, t);
                    }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        classify_expr(channel, effects, callees, t);
                        classify_expr(value, effects, callees, t);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                classify_expr(&arm.body, effects, callees, t);
            }
        }
        // Leaves
        ExprKind::Int(_, _)
        | ExprKind::Float(_, _)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::StringInterp(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::None
        | ExprKind::Ident(_)
        | ExprKind::GenericName { .. } => {}
    }
}

// ── CC2: runtime-needs scanning ────────────────────────────────────────
//
/// CC2: the functions that need one runtime open at their call site — those
/// whose own task blocks need it, and those that call one of them outside a
/// block installing it.
fn propagate_needs(
    direct: &HashMap<FuncName, Runtimes>,
    pick: impl Fn(Runtimes) -> bool,
    unguarded: &HashMap<FuncName, HashSet<FuncName>>,
) -> HashSet<FuncName> {
    let mut needs: HashSet<FuncName> =
        direct.iter().filter(|(_, r)| pick(**r)).map(|(f, _)| f.clone()).collect();
    loop {
        let mut changed = false;
        for (caller, callees) in unguarded {
            if !needs.contains(caller) && callees.iter().any(|c| needs.contains(c)) {
                needs.insert(caller.clone());
                changed = true;
            }
        }
        if !changed {
            return needs;
        }
    }
}

// These functions walk the AST tracking which runtimes are installed at each
// point (`held`) and return the ones a task block needs and nothing installs.
// Calls made without a runtime are collected into `unguarded` for transitive
// propagation, one set per runtime.

/// Callees of one function, split by whether a runtime was installed when the
/// call was reached.
///
/// `unguarded` feeds CC2 — a `spawn` under one of these needs the caller to
/// supply the block; `unguarded_pool` is the same for `ThreadPool.spawn`. `plain`/`guarded` feed CW2, which asks the opposite
/// question: can this function run with *no* runtime installed? A call inside
/// `using Multitasking { }`, or inside the closure handed to `spawn`, can't.
///
/// The two are not the same set. CC2 walks plain calls only, and widening it
/// to method calls would change which programs it rejects. CW2 needs the
/// methods — a handler that does its I/O in `room.broadcast()` is exactly the
/// shape #1263 is about.
struct ReachScan<'t> {
    targets: &'t CallTargets,
    imports: &'t sources::ImportedNames,
    /// The scanned function's type parameters (`InferPass::type_params`).
    type_params: &'t HashMap<String, String>,
    unguarded: HashSet<String>,
    unguarded_pool: HashSet<String>,
    plain: HashSet<String>,
    guarded: HashSet<String>,
    /// Method calls on one of `type_params`, with the runtimes not installed
    /// around them.
    param_calls: HashMap<(String, String), Runtimes>,
    generic_calls: Vec<GenericCall>,
}

impl<'t> ReachScan<'t> {
    fn new(targets: &'t CallTargets, imports: &'t sources::ImportedNames, type_params: &'t HashMap<String, String>) -> Self {
        Self {
            targets,
            imports,
            type_params,
            unguarded: HashSet::new(),
            unguarded_pool: HashSet::new(),
            plain: HashSet::new(),
            guarded: HashSet::new(),
            param_calls: HashMap::new(),
            generic_calls: Vec::new(),
        }
    }

    /// A call that binds type parameters, kept for `resolve_generic_calls`.
    fn generic_call(&mut self, call: rask_ast::NodeId, held: Runtimes, callee: String) {
        if let Some(bindings) = self.targets.type_args.get(&call) {
            self.generic_calls.push(GenericCall { callee, bindings: bindings.clone(), held });
        }
    }

    /// A method call on a value of the function's own type parameter.
    fn param_call(&mut self, call: rask_ast::NodeId, held: Runtimes) {
        let Some((param, method)) = self.targets.param_methods.get(&call) else { return };
        let Some(bound_as) = self.type_params.get(param) else { return };
        let open = Runtimes { multitasking: true, pool: true }.missing_from(held);
        if open.any() {
            *self.param_calls.entry((bound_as.clone(), method.clone())).or_default() |= open;
        }
    }

    /// A plain call: counts for both analyses.
    fn record(&mut self, held: Runtimes, name: String) {
        if !held.multitasking {
            self.unguarded.insert(name.clone());
        }
        if !held.pool {
            self.unguarded_pool.insert(name.clone());
        }
        self.reach(held, name);
    }

    /// A call CC2 doesn't follow (a method the checker didn't resolve),
    /// recorded for reachability only.
    fn reach(&mut self, held: Runtimes, name: String) {
        if !held.multitasking {
            self.plain.insert(name);
        } else {
            self.guarded.insert(name);
        }
    }
}

fn rt_scan_stmts(stmts: &[Stmt], held: Runtimes, rs: &mut ReachScan<'_>) -> Runtimes {
    stmts.iter().fold(Runtimes::NONE, |acc, s| acc | rt_scan_stmt(s, held, rs))
}

fn rt_scan_stmt(stmt: &Stmt, held: Runtimes, rs: &mut ReachScan<'_>) -> Runtimes {
    match &stmt.kind {
        StmtKind::Expr(e) => rt_scan_expr(e, held, rs),
        StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => rt_scan_expr(init, held, rs),
        StmtKind::MutTuple { init, .. }
        | StmtKind::LetTuple { init, .. }
        | StmtKind::LetStruct { init, .. } => rt_scan_expr(init, held, rs),
        StmtKind::Assign { target, value, .. } => {
            rt_scan_expr(target, held, rs) | rt_scan_expr(value, held, rs)
        }
        StmtKind::Return(Some(e)) => rt_scan_expr(e, held, rs),
        StmtKind::Return(None) | StmtKind::Break { value: None, .. }
        | StmtKind::Continue(_) | StmtKind::Discard { .. } => Runtimes::NONE,
        StmtKind::Break { value: Some(v), .. } => rt_scan_expr(v, held, rs),
        StmtKind::While { cond, body, .. } => {
            rt_scan_expr(cond, held, rs) | rt_scan_stmts(body, held, rs)
        }
        StmtKind::WhileLet { expr, body, .. } => {
            rt_scan_expr(expr, held, rs) | rt_scan_stmts(body, held, rs)
        }
        StmtKind::Loop { body, .. } | StmtKind::Comptime(body) | StmtKind::ComptimeFor { body, .. } => {
            rt_scan_stmts(body, held, rs)
        }
        StmtKind::For { iter, body, .. } => {
            rt_scan_expr(iter, held, rs) | rt_scan_stmts(body, held, rs)
        }
        StmtKind::Ensure { body, else_handler } => {
            let mut r = rt_scan_stmts(body, held, rs);
            if let Some((_, handler)) = else_handler {
                r |= rt_scan_stmts(handler, held, rs);
            }
            r
        }
    }
}

fn rt_scan_expr(expr: &Expr, held: Runtimes, rs: &mut ReachScan<'_>) -> Runtimes {
    if let Some(name) = crate::function_ref(expr, rs.targets, rs.imports) {
        // A generic function named as a value has its types recorded under
        // the name, so it is an instance the way a call is.
        rs.generic_call(expr.id, held, name.clone());
        rs.record(held, name);
    }
    match &expr.kind {
        ExprKind::Call { func, args } => {
            if let Some(name) = crate::free_callee(expr.id, func, rs.targets, rs.imports) {
                rs.generic_call(expr.id, held, name.clone());
                rs.record(held, name);
            }
            let mut r = rt_scan_expr(func, held, rs);
            for arg in args {
                r |= rt_scan_expr(&arg.expr, held, rs);
            }
            r
        }

        // A task runs on the runtime that accepted it, so that runtime is
        // installed for everything its block reaches — the same thing the
        // `using` block means, arriving by a different route. An OS thread
        // needs no runtime to start, and neither does a pool job that said
        // `requires_pool: false`.
        ExprKind::Spawn { target, args, body, .. } => {
            let needs = match target {
                SpawnTarget::Green => Runtimes::MULTITASKING,
                SpawnTarget::Pool if !says_no_pool(args) => Runtimes::POOL,
                SpawnTarget::Pool | SpawnTarget::Thread => Runtimes::NONE,
            };
            let inner = rt_scan_expr(body, held | needs, rs);
            needs.missing_from(held) | inner
        }

        // A `using` block installs its runtime for the body.
        ExprKind::UsingBlock { name, args, body } => {
            let mut r = Runtimes::NONE;
            for arg in args { r |= rt_scan_expr(&arg.expr, held, rs); }
            r | rt_scan_stmts(body, held | Runtimes::of_block(name), rs)
        }

        // All other expressions: recurse with same held
        // A method the checker resolved is a call like any other: CC2
        // follows it (`http.serve` spawns per connection). One it didn't is
        // only a guess from the spelling, which can name methods the call
        // never reaches, so it counts for reachability alone.
        ExprKind::MethodCall { object, method, args, .. } => {
            rs.param_call(expr.id, held);
            if let Some(name) = crate::resolved_method(expr.id, rs.targets) {
                rs.generic_call(expr.id, held, name.clone());
                rs.record(held, name);
            } else {
                for name in crate::method_callees(expr.id, object, method, rs.targets, rs.imports) {
                    rs.reach(held, name);
                }
            }
            let mut r = rt_scan_expr(object, held, rs);
            for arg in args { r |= rt_scan_expr(&arg.expr, held, rs); }
            r
        }
        ExprKind::Binary { left, right, .. } => {
            rt_scan_expr(left, held, rs) | rt_scan_expr(right, held, rs)
        }
        ExprKind::Unary { operand, .. } => rt_scan_expr(operand, held, rs),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => rt_scan_expr(object, held, rs),
        ExprKind::DynamicField { object, field_expr } => {
            rt_scan_expr(object, held, rs) | rt_scan_expr(field_expr, held, rs)
        }
        ExprKind::Index { object, index } => {
            rt_scan_expr(object, held, rs) | rt_scan_expr(index, held, rs)
        }
        ExprKind::Block(stmts) => rt_scan_stmts(stmts, held, rs),
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            let mut r = rt_scan_expr(cond, held, rs);
            r |= rt_scan_expr(then_branch, held, rs);
            if let Some(e) = else_branch { r |= rt_scan_expr(e, held, rs); }
            r
        }
        ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
            let mut r = rt_scan_expr(expr, held, rs);
            r |= rt_scan_expr(then_branch, held, rs);
            if let Some(e) = else_branch { r |= rt_scan_expr(e, held, rs); }
            r
        }
        ExprKind::GuardPattern { expr, else_branch, .. } => {
            rt_scan_expr(expr, held, rs) | rt_scan_expr(else_branch, held, rs)
        }
        ExprKind::IsPattern { expr, .. } | ExprKind::IsPresent { expr, .. }
        | ExprKind::Unwrap { expr, .. } | ExprKind::Cast { expr, .. } | ExprKind::Convert { expr, .. } => rt_scan_expr(expr, held, rs),
        ExprKind::Match { scrutinee, arms } => {
            let mut r = rt_scan_expr(scrutinee, held, rs);
            for arm in arms {
                if let Some(g) = &arm.guard { r |= rt_scan_expr(g, held, rs); }
                r |= rt_scan_expr(&arm.body, held, rs);
            }
            r
        }
        ExprKind::Try { expr: e } | ExprKind::Take { place: e } => rt_scan_expr(e, held, rs),
        ExprKind::Catch { value, clause } => {
            rt_scan_expr(value, held, rs) | rt_scan_expr(&clause.body, held, rs)
        }
        ExprKind::NullCoalesce { value, default } => {
            rt_scan_expr(value, held, rs) | rt_scan_expr(default, held, rs)
        }
        ExprKind::Range { start, end, .. } => {
            let mut r = Runtimes::NONE;
            if let Some(s) = start { r |= rt_scan_expr(s, held, rs); }
            if let Some(e) = end { r |= rt_scan_expr(e, held, rs); }
            r
        }
        ExprKind::StructLit { fields, spread, .. } => {
            let mut r = Runtimes::NONE;
            for f in fields { r |= rt_scan_expr(&f.value, held, rs); }
            if let Some(s) = spread { r |= rt_scan_expr(s, held, rs); }
            r
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            elems.iter().fold(Runtimes::NONE, |acc, e| acc | rt_scan_expr(e, held, rs))
        }
        ExprKind::ArrayRepeat { value, count } => {
            rt_scan_expr(value, held, rs) | rt_scan_expr(count, held, rs)
        }
        ExprKind::WithAs { bindings, body } => {
            let mut r = Runtimes::NONE;
            for b in bindings { r |= rt_scan_expr(&b.source, held, rs); }
            r |= rt_scan_stmts(body, held, rs);
            r
        }
        ExprKind::Closure { body, .. } => rt_scan_expr(body, held, rs),
        ExprKind::Comptime { body }
        | ExprKind::Loop { body, .. } | ExprKind::Unsafe { body } => rt_scan_stmts(body, held, rs),
        ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
            let mut r = rt_scan_expr(condition, held, rs);
            if let Some(m) = message { r |= rt_scan_expr(m, held, rs); }
            r
        }
        ExprKind::Select { arms, .. } => {
            let mut r = Runtimes::NONE;
            for arm in arms {
                match &arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => { r |= rt_scan_expr(channel, held, rs); }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        r |= rt_scan_expr(channel, held, rs);
                        r |= rt_scan_expr(value, held, rs);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                r |= rt_scan_expr(&arm.body, held, rs);
            }
            r
        }
        ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_)
        | ExprKind::StringInterp(_) | ExprKind::Bytes(_) | ExprKind::Char(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Ident(_)
        | ExprKind::GenericName { .. } => Runtimes::NONE,
    }
}

/// `ThreadPool.spawn(requires_pool: false)`, written as a literal. Any other
/// value is only known at run time, where the spawn checks for itself (CC3).
pub(crate) fn says_no_pool(args: &[rask_ast::expr::CallArg]) -> bool {
    args.first().is_some_and(|a| matches!(a.expr.kind, ExprKind::Bool(false)))
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{Decl, DeclKind, FnDecl, ExternDecl};
    use rask_ast::expr::{CallArg, ArgMode, Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    /// Inference over `decls` as the program, with the real stdlib beside it,
    /// the way the compiler runs it.
    fn infer_std(decls: &[Decl], targets: &CallTargets) -> EffectMap {
        let stdlib = rask_stdlib::StubRegistry::typecheck_decls();
        infer_with_reach(decls, &stdlib, targets, &Default::default()).0
    }

    fn target(name: &str) -> rask_ast::decl::MethodTarget {
        rask_ast::decl::MethodTarget { name: name.to_string(), effect: Default::default() }
    }

    fn sp() -> Span { Span::new(0, 0) }

    fn ident(name: &str) -> Expr {
        Expr { id: NodeId(0), kind: ExprKind::Ident(name.into()), span: sp() }
    }

    fn call(func_name: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(ident(func_name)),
                args: args.into_iter().map(|e| CallArg {
                    name: None,
                    mode: ArgMode::Default,
                    expr: e,
                }).collect(),
            },
            span: sp(),
        }
    }

    fn method_call(obj: &str, method: &str) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::MethodCall {
                object: Box::new(ident(obj)),
                method: method.into(),
                type_args: None,
                args: vec![],
            },
            span: sp(),
        }
    }

    fn field_call(obj: &str, field: &str) -> Expr {
        // Represents Type.method() — Field access as callee
        Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Field {
                        object: Box::new(ident(obj)),
                        field: field.into(),
                    },
                    span: sp(),
                }),
                args: vec![],
            },
            span: sp(),
        }
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt { id: NodeId(0), kind: StmtKind::Return(val), span: sp() }
    }

    /// `spawn { }` — the parser's shape: an empty block in a closure node.
    fn spawn_block() -> Expr {
        let block = Expr { id: NodeId(0), kind: ExprKind::Block(vec![]), span: sp() };
        let body = Expr {
            id: NodeId(0),
            kind: ExprKind::Closure { params: vec![], ret_ty: None, body: Box::new(block) },
            span: sp(),
        };
        Expr {
            id: NodeId(0),
            kind: ExprKind::Spawn { target: SpawnTarget::Green, receiver: None, args: Vec::new(), body: Box::new(body) },
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt { id: NodeId(0), kind: StmtKind::Expr(e), span: sp() }
    }

    fn make_fn(name: &str, body: Vec<Stmt>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                id: rask_ast::NodeId::DUMMY,
                name: name.into(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
                decl_start: sp().start,
            }),
            span: sp(),
        }
    }

    fn make_comptime_fn(name: &str) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                id: rask_ast::NodeId::DUMMY,
                name: name.into(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                body: vec![expr_stmt(call("fs.exists", vec![]))],
                is_pub: false,
                is_private: false,
                is_comptime: true,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
                decl_start: sp().start,
            }),
            span: sp(),
        }
    }

    fn make_extern(name: &str) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Extern(ExternDecl {
                abi: "C".into(),
                name: name.into(),
                params: vec![],
                is_variadic: false,
                ret_ty: None,
                doc: None,
                attrs: vec![],
                block_start: None,
            }),
            span: sp(),
        }
    }

    #[test]
    fn pure_function() {
        let decls = vec![make_fn("add", vec![return_stmt(Some(ident("x")))])];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["add"].is_pure());
    }

    #[test]
    fn direct_io_call() {
        let decls = vec![make_fn("load", vec![
            expr_stmt(call("fs.exists", vec![])),
        ])];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["load"].blocks);
        assert!(!effects["load"].async_);
    }

    #[test]
    fn direct_async_call() {
        let decls = vec![make_fn("run", vec![
            expr_stmt(spawn_block()),
        ])];
        let effects = infer_std(&decls, &Default::default());
        assert!(!effects["run"].blocks, "spawning waits on nothing (#1362)");
        assert!(effects["run"].async_);
    }

    #[test]
    fn field_call_io_source() {
        let decls = vec![make_fn("read", vec![
            expr_stmt(field_call("fs", "open")),
        ])];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["read"].blocks);
    }

    /// A method call is classed by the receiver's type as the checker resolved
    /// it, not by the variable's name (#1418): `t.join()` on a `Handle` waits.
    #[test]
    fn method_call_classed_by_resolved_receiver() {
        let decls = vec![make_fn("wait", vec![expr_stmt(method_call("t", "join"))])];
        let targets = CallTargets { methods: [(NodeId(0), target("Handle.join"))].into(), ..Default::default() };
        let effects = infer_std(&decls, &targets);
        assert!(effects["wait"].blocks);

        // A variable spelled like a source type, holding something else.
        let decls = vec![make_fn("words", vec![expr_stmt(method_call("Handle", "join"))])];
        let targets = CallTargets { methods: [(NodeId(0), target("Vec.join"))].into(), ..Default::default() };
        assert!(infer_std(&decls, &targets)["words"].is_pure());
    }

    #[test]
    fn method_call_io_source() {
        let decls = vec![make_fn("net", vec![
            expr_stmt(method_call("Sender", "send")),
        ])];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["net"].blocks);
    }

    #[test]
    fn transitive_propagation() {
        // load calls fs.exists (IO)
        // process calls load → should inherit IO
        let decls = vec![
            make_fn("load", vec![expr_stmt(call("fs.exists", vec![]))]),
            make_fn("process", vec![expr_stmt(call("load", vec![]))]),
        ];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["load"].blocks);
        assert!(effects["process"].blocks, "Transitive IO via call to load");
    }

    #[test]
    fn deep_transitive() {
        // c calls fs.exists, b calls c, a calls b
        let decls = vec![
            make_fn("c", vec![expr_stmt(call("fs.exists", vec![]))]),
            make_fn("b", vec![expr_stmt(call("c", vec![]))]),
            make_fn("a", vec![expr_stmt(call("b", vec![]))]),
        ];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["a"].blocks);
        assert!(effects["b"].blocks);
        assert!(effects["c"].blocks);
    }

    #[test]
    fn mutual_recursion_terminates() {
        // a calls b, b calls a, a also calls fs.exists
        let decls = vec![
            make_fn("a", vec![
                expr_stmt(call("fs.exists", vec![])),
                expr_stmt(call("b", vec![])),
            ]),
            make_fn("b", vec![expr_stmt(call("a", vec![]))]),
        ];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["a"].blocks);
        assert!(effects["b"].blocks, "b inherits IO from a via mutual recursion");
    }

    #[test]
    /// IO3: an `unsafe` block is what it calls. Raw pointer work touches
    /// nothing outside the program, and a C call is conservative on its own
    /// (INF5). Counting the block itself made every stdlib body that
    /// allocates through one a blocking call.
    fn an_unsafe_block_is_what_it_calls() {
        let decls = vec![make_fn("ffi_call", vec![
            Stmt {
                id: NodeId(0),
                kind: StmtKind::Expr(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Unsafe { body: vec![] },
                    span: sp(),
                }),
                span: sp(),
            },
        ])];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["ffi_call"].is_pure());
    }

    /// PU2 says a `comptime func` is pure and that "effect inference confirms
    /// this". Confirming means looking: this fixture's body is an `fs.exists`, so
    /// what inference reports is I/O, and `comptime_purity::check` is what turns
    /// that disagreement into the CT60 error. Stamping it pure — which is what
    /// this test used to assert — made the marking its own evidence.
    #[test]
    fn a_comptime_func_is_measured_not_assumed() {
        let decls = vec![make_comptime_fn("table_gen")];
        let effects = infer_std(&decls, &Default::default());
        assert!(
            effects["table_gen"].blocks,
            "PU2: inference confirms purity rather than granting it"
        );
        let violations = crate::comptime_purity::check(&decls, &effects, &Default::default());
        assert_eq!(violations.len(), 1, "CT60 rejects the marking the body doesn't earn");
        assert_eq!(violations[0].func, "table_gen");
        assert_eq!(violations[0].effect, "a call that blocks");
    }

    /// The other half: a body that stays inside the subset keeps the promise,
    /// and nothing is reported.
    #[test]
    fn a_pure_comptime_func_passes() {
        let mut decl = make_comptime_fn("adds");
        if let DeclKind::Fn(f) = &mut decl.kind {
            f.body = vec![];
        }
        let decls = vec![decl];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["adds"].is_pure());
        assert!(crate::comptime_purity::check(&decls, &effects, &Default::default()).is_empty());
    }

    #[test]
    fn extern_conservative_io() {
        let decls = vec![make_extern("c_function")];
        let effects = infer_std(&decls, &Default::default());
        assert!(effects["c_function"].blocks, "INF5: a C function could block");
        assert!(effects["c_function"].ambient, "INF5: and could touch anything");
    }


    /// The stdlib's bodies are inferred, so a public function gets its
    /// effects from the native at the bottom: `fs.current_dir` calls the C
    /// `getcwd`, which the name list it replaced didn't know about.
    #[test]
    fn stdlib_bodies_carry_their_natives_effects() {
        let effects = infer_std(&[], &Default::default());
        assert!(effects["fs.current_dir"].blocks);
        assert!(effects["fs.exists"].blocks);
        assert!(effects["Instant.now"].ambient);
        assert!(!effects["Instant.now"].blocks);
        assert!(effects["println"].prints);
        assert!(!effects["println"].blocks, "printing is its own effect, so CW2 can leave it out");
        assert!(effects["println"].can_block());
        assert!(effects["Vec.push"].grow);
        assert!(!effects["Vec.push"].blocks, "allocation isn't blocking");
        assert!(!effects["string.split"].blocks);
    }

    /// The checker resolved `insert` to a method declared `@grows`.
    fn growing(node: u32) -> CallTargets {
        let effect = rask_ast::decl::DeclaredEffects { grows: true, ..Default::default() };
        CallTargets { methods: [(NodeId(node), rask_ast::decl::MethodTarget { name: "Rack.insert".to_string(), effect })].into(), ..Default::default() }
    }

    #[test]
    fn an_extern_says_what_it_does() {
        let marked = |name: &str, attr: &str| {
            let mut decl = make_extern(name);
            if let DeclKind::Extern(e) = &mut decl.kind {
                e.attrs.push(rask_ast::attr::Attr::synthetic(attr, None));
            }
            decl
        };
        let decls = vec![
            make_extern("c_read"),
            marked("ngtcp2_step", "ambient"),
            marked("strlen", "memory_only"),
        ];
        let effects = infer(&decls, &Default::default());
        assert!(effects["c_read"].blocks && effects["c_read"].ambient, "INF5: unmarked is both");
        assert!(!effects["ngtcp2_step"].blocks && effects["ngtcp2_step"].ambient, "INF5: @ambient never waits");
        assert!(effects["strlen"].is_pure(), "INF5: @memory_only is neither");
    }

    #[test]
    fn mutation_effect() {
        let decls = vec![make_fn("grow", vec![
            expr_stmt(method_call("pool", "insert")),
        ])];
        let effects = infer_std(&decls, &growing(0));
        assert!(effects["grow"].mutation());
        assert!(!effects["grow"].blocks, "Mutation is orthogonal to IO");
    }

    /// The method's declaration decides, not its name: a program's own
    /// `insert` that declares nothing is pure (#1560).
    #[test]
    fn an_undeclared_insert_is_pure() {
        let decls = vec![make_fn("own", vec![
            expr_stmt(method_call("thing", "insert")),
        ])];
        let effects = infer_std(&decls, &Default::default());
        assert!(!effects["own"].mutation());
    }

    #[test]
    fn mixed_effects() {
        let decls = vec![make_fn("complex", vec![
            expr_stmt(call("fs.exists", vec![])),
            expr_stmt(spawn_block()),
            expr_stmt(method_call("pool", "insert")),
        ])];
        let effects = infer_std(&decls, &growing(0));
        assert!(effects["complex"].blocks);
        assert!(effects["complex"].async_);
        assert!(effects["complex"].mutation());
    }

    #[test]
    fn unknown_callee_is_pure() {
        let decls = vec![make_fn("caller", vec![
            expr_stmt(call("some_unknown_fn", vec![])),
        ])];
        let effects = infer_std(&decls, &Default::default());
        // Unknown function isn't in our source table, and there's no
        // declaration to propagate from → stays pure
        assert!(effects["caller"].is_pure());
    }
}

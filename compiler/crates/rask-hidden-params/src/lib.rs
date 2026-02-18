// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Hidden parameter compiler pass (comp.hidden-params).
//!
//! Desugars `using` clauses into explicit hidden function parameters.
//! Runs after type checking, before monomorphization.
//!
//! Three operations:
//! 1. Rewrite function signatures — add hidden params for each `using` clause
//! 2. Rewrite call sites — insert hidden arguments resolved from scope
//! 3. Rewrite `using` blocks — context construction + body + teardown

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{ContextClause, Decl, DeclKind, FnDecl, Param};
use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};

// ── Types ───────────────────────────────────────────────────────────────

/// A context requirement derived from a `using` clause.
#[derive(Debug, Clone)]
struct ContextReq {
    /// Hidden parameter name: `__ctx_pool_Player`, `__ctx_runtime`, etc.
    param_name: String,
    /// Type string for the parameter: `&Pool<Player>`, `RuntimeContext`
    param_type: String,
    /// Original clause type string: `Pool<Player>`, `Multitasking`
    clause_type: String,
    /// Is this a runtime context (optional `?` param) vs pool (required)?
    #[allow(dead_code)]
    is_runtime: bool,
    /// Named alias from `using players: Pool<Player>`
    #[allow(dead_code)]
    alias: Option<String>,
}

/// Qualified function name for call graph: "damage" or "Player.take_damage".
type FuncName = String;

// ── Public API ──────────────────────────────────────────────────────────

/// Run the hidden parameter pass on a set of declarations.
///
/// Mutates the AST in place:
/// - Functions with `using` clauses gain hidden `__ctx_*` parameters
/// - Call sites to those functions gain hidden arguments
/// - `using Multitasking { }` blocks become context construction + teardown
pub fn desugar_hidden_params(decls: &mut [Decl]) {
    let mut pass = HiddenParamPass::new();
    pass.run(decls);
}

/// Errors from the hidden parameter pass.
#[derive(Debug, Clone)]
pub struct HiddenParamError {
    pub message: String,
    pub span: Span,
}

// ── Pass Implementation ─────────────────────────────────────────────────

struct HiddenParamPass {
    /// Function name → context requirements (from explicit using clauses).
    func_contexts: HashMap<FuncName, Vec<ContextReq>>,
    /// Call graph: caller → callees (by function name).
    call_graph: HashMap<FuncName, HashSet<FuncName>>,
    /// Functions that are public (context propagation stops here).
    public_funcs: HashSet<FuncName>,
    /// Fresh NodeId counter (high range to avoid parser collisions).
    next_id: u32,
}

impl HiddenParamPass {
    fn new() -> Self {
        Self {
            func_contexts: HashMap::new(),
            call_graph: HashMap::new(),
            public_funcs: HashSet::new(),
            next_id: 2_000_000,
        }
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn run(&mut self, decls: &mut [Decl]) {
        // Phase 1: Collect context requirements from explicit `using` clauses
        self.collect_contexts(decls);

        // Phase 2: Build call graph from function bodies
        self.build_call_graph(decls);

        // Phase 3: Propagate — functions calling context-needing functions
        // also need the context if they can't resolve it locally (HP3, PUB2)
        self.propagate();

        // Phase 4-6: Rewrite signatures, call sites, using blocks
        self.rewrite_decls(decls);
    }

    // ── Phase 1: Collect ────────────────────────────────────────────────

    fn collect_contexts(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    self.collect_fn_context(&f.name, f);
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        let qname = format!("{}.{}", s.name, method.name);
                        self.collect_fn_context(&qname, method);
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        let qname = format!("{}.{}", e.name, method.name);
                        self.collect_fn_context(&qname, method);
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        let qname = format!("{}.{}", i.target_ty, method.name);
                        self.collect_fn_context(&qname, method);
                    }
                }
                DeclKind::Trait(t) => {
                    for method in &t.methods {
                        let qname = format!("{}.{}", t.name, method.name);
                        self.collect_fn_context(&qname, method);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_fn_context(&mut self, qname: &str, f: &FnDecl) {
        if f.is_pub {
            self.public_funcs.insert(qname.to_string());
        }

        if f.context_clauses.is_empty() {
            return;
        }

        let reqs: Vec<ContextReq> = f
            .context_clauses
            .iter()
            .map(|cc| context_clause_to_req(cc))
            .collect();

        self.func_contexts.insert(qname.to_string(), reqs);
    }

    // ── Phase 2: Build Call Graph ───────────────────────────────────────

    fn build_call_graph(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    let callees = collect_callees_from_body(&f.body);
                    if !callees.is_empty() {
                        self.call_graph.insert(f.name.clone(), callees);
                    }
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        let qname = format!("{}.{}", s.name, method.name);
                        let callees = collect_callees_from_body(&method.body);
                        if !callees.is_empty() {
                            self.call_graph.insert(qname, callees);
                        }
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        let qname = format!("{}.{}", e.name, method.name);
                        let callees = collect_callees_from_body(&method.body);
                        if !callees.is_empty() {
                            self.call_graph.insert(qname, callees);
                        }
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        let qname = format!("{}.{}", i.target_ty, method.name);
                        let callees = collect_callees_from_body(&method.body);
                        if !callees.is_empty() {
                            self.call_graph.insert(qname, callees);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ── Phase 3: Propagate ──────────────────────────────────────────────

    fn propagate(&mut self) {
        // Fixed-point iteration: if a function calls a context-needing function
        // and can't resolve the context from its own params/using clauses,
        // it also needs the context.
        loop {
            let mut changed = false;

            // Snapshot current state to avoid borrow conflicts
            let graph_snapshot: Vec<(FuncName, HashSet<FuncName>)> =
                self.call_graph.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

            for (caller, callees) in &graph_snapshot {
                for callee in callees {
                    // Check if callee needs contexts
                    let callee_reqs = match self.func_contexts.get(callee) {
                        Some(r) => r.clone(),
                        None => continue,
                    };

                    for req in &callee_reqs {
                        // Does caller already have this context?
                        let caller_has = self
                            .func_contexts
                            .get(caller)
                            .map(|reqs| reqs.iter().any(|r| r.clause_type == req.clause_type))
                            .unwrap_or(false);

                        if caller_has {
                            continue;
                        }

                        // Public functions must declare contexts explicitly (PUB1)
                        if self.public_funcs.contains(caller) {
                            continue;
                        }

                        // Private function: propagate context requirement (PUB2)
                        let new_req = req.clone();
                        self.func_contexts
                            .entry(caller.clone())
                            .or_default()
                            .push(new_req);
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }
    }

    // ── Phase 4-6: Rewrite ──────────────────────────────────────────────

    fn rewrite_decls(&mut self, decls: &mut [Decl]) {
        for decl in decls.iter_mut() {
            match &mut decl.kind {
                DeclKind::Fn(f) => {
                    self.rewrite_fn(&f.name.clone(), f);
                }
                DeclKind::Struct(s) => {
                    let type_name = s.name.clone();
                    for method in &mut s.methods {
                        let qname = format!("{}.{}", type_name, method.name);
                        self.rewrite_fn(&qname, method);
                    }
                }
                DeclKind::Enum(e) => {
                    let type_name = e.name.clone();
                    for method in &mut e.methods {
                        let qname = format!("{}.{}", type_name, method.name);
                        self.rewrite_fn(&qname, method);
                    }
                }
                DeclKind::Impl(i) => {
                    let type_name = i.target_ty.clone();
                    for method in &mut i.methods {
                        let qname = format!("{}.{}", type_name, method.name);
                        self.rewrite_fn(&qname, method);
                    }
                }
                DeclKind::Trait(t) => {
                    let type_name = t.name.clone();
                    for method in &mut t.methods {
                        let qname = format!("{}.{}", type_name, method.name);
                        self.rewrite_fn(&qname, method);
                    }
                }
                DeclKind::Test(t) => {
                    self.rewrite_stmts(&mut t.body);
                }
                DeclKind::Benchmark(b) => {
                    self.rewrite_stmts(&mut b.body);
                }
                _ => {}
            }
        }
    }

    fn rewrite_fn(&mut self, qname: &str, f: &mut FnDecl) {
        // Phase 4 (SIG1-SIG6): Add hidden params to signature
        if let Some(reqs) = self.func_contexts.get(qname) {
            for req in reqs.clone() {
                // Check idempotency (HP4): skip if param already exists
                if f.params.iter().any(|p| p.name == req.param_name) {
                    continue;
                }

                f.params.push(Param {
                    name: req.param_name.clone(),
                    name_span: Span::new(0, 0),
                    ty: req.param_type.clone(),
                    is_take: false,
                    is_mutate: false,
                    default: None,
                });
            }

            // Clear context clauses — they're now expressed as params
            f.context_clauses.clear();
        }

        // Phase 5-6: Rewrite body (call sites and using blocks)
        self.rewrite_stmts(&mut f.body);
    }

    fn rewrite_stmts(&mut self, stmts: &mut [Stmt]) {
        for stmt in stmts.iter_mut() {
            self.rewrite_stmt(stmt);
        }
    }

    fn rewrite_stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Expr(e) => self.rewrite_expr(e),
            StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
                self.rewrite_expr(init);
            }
            StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.rewrite_expr(init);
            }
            StmtKind::Assign { target, value } => {
                self.rewrite_expr(target);
                self.rewrite_expr(value);
            }
            StmtKind::Return(Some(e)) => self.rewrite_expr(e),
            StmtKind::Return(None) => {}
            StmtKind::Break {
                value: Some(v), ..
            } => self.rewrite_expr(v),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
            StmtKind::While { cond, body } => {
                self.rewrite_expr(cond);
                self.rewrite_stmts(body);
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.rewrite_expr(expr);
                self.rewrite_stmts(body);
            }
            StmtKind::Loop { body, .. } => self.rewrite_stmts(body),
            StmtKind::For { iter, body, .. } => {
                self.rewrite_expr(iter);
                self.rewrite_stmts(body);
            }
            StmtKind::Ensure {
                body,
                else_handler,
            } => {
                self.rewrite_stmts(body);
                if let Some((_, handler)) = else_handler {
                    self.rewrite_stmts(handler);
                }
            }
            StmtKind::Comptime(body) => self.rewrite_stmts(body),
        }
    }

    fn rewrite_expr(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            // Phase 5 (CALL1-CALL6): Insert hidden args at call sites
            ExprKind::Call { func, args } => {
                self.rewrite_expr(func);
                for arg in args.iter_mut() {
                    self.rewrite_expr(&mut arg.expr);
                }

                // Check if callee needs hidden params
                if let Some(callee_name) = extract_callee_name(func) {
                    if let Some(reqs) = self.func_contexts.get(&callee_name).cloned() {
                        for req in &reqs {
                            // Don't add duplicate hidden args
                            let already_has = args.iter().any(|a| {
                                matches!(&a.expr.kind, ExprKind::Ident(name) if name == &req.param_name)
                            });
                            if already_has {
                                continue;
                            }

                            // Resolve: use the hidden param from current scope
                            args.push(CallArg {
                                mode: ArgMode::Default,
                                expr: Expr {
                                    id: self.fresh_id(),
                                    kind: ExprKind::Ident(req.param_name.clone()),
                                    span: expr.span,
                                },
                            });
                        }
                    }
                }
            }

            ExprKind::MethodCall {
                object, args, ..
            } => {
                self.rewrite_expr(object);
                for arg in args.iter_mut() {
                    self.rewrite_expr(&mut arg.expr);
                }
                // Method call context resolution requires type info for the
                // receiver. Deferred — method dispatch doesn't commonly carry
                // context in Phase A patterns.
            }

            // Phase 6 (BLK1-BLK4): Desugar `using` blocks
            ExprKind::UsingBlock { name, args, body } => {
                if name == "Multitasking" || name == "multitasking" {
                    // Keep UsingBlock intact — MIR lowering emits
                    // rask_runtime_init/rask_runtime_shutdown directly.
                    self.rewrite_stmts(
                        match &mut expr.kind {
                            ExprKind::UsingBlock { body, .. } => body,
                            _ => unreachable!(),
                        }
                    );
                } else if name == "ThreadPool" {
                    // ThreadPool blocks keep their structure for now — the
                    // interpreter handles them directly and the compiled path
                    // will use rask-rt's thread pool API.
                    self.rewrite_stmts(
                        match &mut expr.kind {
                            ExprKind::UsingBlock { body, .. } => body,
                            _ => unreachable!(),
                        }
                    );
                } else {
                    // Unknown using block — just recurse
                    for arg in args.iter_mut() {
                        self.rewrite_expr(&mut arg.expr);
                    }
                    self.rewrite_stmts(body);
                }
            }

            // Recurse into all other expression kinds
            ExprKind::Binary { left, right, .. } => {
                self.rewrite_expr(left);
                self.rewrite_expr(right);
            }
            ExprKind::Unary { operand, .. } => self.rewrite_expr(operand),
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.rewrite_expr(object);
            }
            ExprKind::Index { object, index } => {
                self.rewrite_expr(object);
                self.rewrite_expr(index);
            }
            ExprKind::Block(stmts) => self.rewrite_stmts(stmts),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.rewrite_expr(cond);
                self.rewrite_expr(then_branch);
                if let Some(e) = else_branch {
                    self.rewrite_expr(e);
                }
            }
            ExprKind::IfLet {
                expr,
                then_branch,
                else_branch,
                ..
            } => {
                self.rewrite_expr(expr);
                self.rewrite_expr(then_branch);
                if let Some(e) = else_branch {
                    self.rewrite_expr(e);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.rewrite_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &mut arm.guard {
                        self.rewrite_expr(g);
                    }
                    self.rewrite_expr(&mut arm.body);
                }
            }
            ExprKind::Try(e) | ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } => {
                self.rewrite_expr(e);
            }
            ExprKind::GuardPattern {
                expr, else_branch, ..
            } => {
                self.rewrite_expr(expr);
                self.rewrite_expr(else_branch);
            }
            ExprKind::IsPattern { expr, .. } => self.rewrite_expr(expr),
            ExprKind::NullCoalesce { value, default } => {
                self.rewrite_expr(value);
                self.rewrite_expr(default);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.rewrite_expr(s);
                }
                if let Some(e) = end {
                    self.rewrite_expr(e);
                }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.rewrite_expr(&mut f.value);
                }
                if let Some(s) = spread {
                    self.rewrite_expr(s);
                }
            }
            ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
                for e in elems {
                    self.rewrite_expr(e);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.rewrite_expr(value);
                self.rewrite_expr(count);
            }
            ExprKind::WithAs { bindings, body } => {
                for (e, _) in bindings {
                    self.rewrite_expr(e);
                }
                self.rewrite_stmts(body);
            }
            ExprKind::Closure { body, .. } => self.rewrite_expr(body),
            ExprKind::Spawn { body }
            | ExprKind::Unsafe { body }
            | ExprKind::Comptime { body }
            | ExprKind::BlockCall { body, .. } => {
                self.rewrite_stmts(body);
            }
            ExprKind::Assert { condition, message }
            | ExprKind::Check { condition, message } => {
                self.rewrite_expr(condition);
                if let Some(m) = message {
                    self.rewrite_expr(m);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &mut arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.rewrite_expr(channel);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.rewrite_expr(channel);
                            self.rewrite_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.rewrite_expr(&mut arm.body);
                }
            }
            // Leaves
            ExprKind::Int(_, _)
            | ExprKind::Float(_, _)
            | ExprKind::String(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Ident(_) => {}
        }
    }

    // desugar_multitasking_block removed — MIR lowering now emits
    // rask_runtime_init/rask_runtime_shutdown directly.
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Convert a ContextClause into a ContextReq.
fn context_clause_to_req(cc: &ContextClause) -> ContextReq {
    let is_runtime = cc.ty == "Multitasking" || cc.ty == "multitasking";

    let (param_name, param_type) = if is_runtime {
        ("__ctx_runtime".to_string(), "RuntimeContext".to_string())
    } else {
        // Pool<T> → __ctx_pool_T with type &Pool<T>
        let inner = extract_generic_arg(&cc.ty).unwrap_or_default();
        let name = if let Some(alias) = &cc.name {
            format!("__ctx_{}", alias)
        } else {
            format!("__ctx_pool_{}", inner)
        };
        let ty = format!("&{}", cc.ty);
        (name, ty)
    };

    ContextReq {
        param_name,
        param_type,
        clause_type: cc.ty.clone(),
        is_runtime,
        alias: cc.name.clone(),
    }
}

/// Extract T from "Pool<T>" → "T".
fn extract_generic_arg(ty: &str) -> Option<String> {
    let start = ty.find('<')?;
    let end = ty.rfind('>')?;
    Some(ty[start + 1..end].to_string())
}

/// Extract the function name from a Call expression's func field.
fn extract_callee_name(func: &Expr) -> Option<String> {
    match &func.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Field { object, field } => {
            // Type.method style: extract "Type.method"
            if let ExprKind::Ident(obj_name) = &object.kind {
                Some(format!("{}.{}", obj_name, field))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Collect all callee names from a function body (for call graph).
fn collect_callees_from_body(stmts: &[Stmt]) -> HashSet<String> {
    let mut callees = HashSet::new();
    for stmt in stmts {
        collect_callees_from_stmt(stmt, &mut callees);
    }
    callees
}

fn collect_callees_from_stmt(stmt: &Stmt, callees: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Expr(e) => collect_callees_from_expr(e, callees),
        StmtKind::Let { init, .. }
        | StmtKind::Const { init, .. }
        | StmtKind::LetTuple { init, .. }
        | StmtKind::ConstTuple { init, .. } => {
            collect_callees_from_expr(init, callees);
        }
        StmtKind::Assign { target, value } => {
            collect_callees_from_expr(target, callees);
            collect_callees_from_expr(value, callees);
        }
        StmtKind::Return(Some(e)) => collect_callees_from_expr(e, callees),
        StmtKind::Return(None) => {}
        StmtKind::Break {
            value: Some(v), ..
        } => collect_callees_from_expr(v, callees),
        StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        StmtKind::While { cond, body } => {
            collect_callees_from_expr(cond, callees);
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::WhileLet { expr, body, .. } => {
            collect_callees_from_expr(expr, callees);
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::Loop { body, .. } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::For { iter, body, .. } => {
            collect_callees_from_expr(iter, callees);
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::Ensure {
            body,
            else_handler,
        } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
            if let Some((_, handler)) = else_handler {
                for s in handler {
                    collect_callees_from_stmt(s, callees);
                }
            }
        }
        StmtKind::Comptime(body) => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
    }
}

fn collect_callees_from_expr(expr: &Expr, callees: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Call { func, args } => {
            if let Some(name) = extract_callee_name(func) {
                callees.insert(name);
            }
            collect_callees_from_expr(func, callees);
            for arg in args {
                collect_callees_from_expr(&arg.expr, callees);
            }
        }
        ExprKind::MethodCall {
            object, args, method, ..
        } => {
            // Record as "?.method" — without type info we can't fully qualify
            callees.insert(method.clone());
            collect_callees_from_expr(object, callees);
            for arg in args {
                collect_callees_from_expr(&arg.expr, callees);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_callees_from_expr(left, callees);
            collect_callees_from_expr(right, callees);
        }
        ExprKind::Unary { operand, .. } => collect_callees_from_expr(operand, callees),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            collect_callees_from_expr(object, callees);
        }
        ExprKind::Index { object, index } => {
            collect_callees_from_expr(object, callees);
            collect_callees_from_expr(index, callees);
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                collect_callees_from_stmt(s, callees);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_callees_from_expr(cond, callees);
            collect_callees_from_expr(then_branch, callees);
            if let Some(e) = else_branch {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::IfLet {
            expr,
            then_branch,
            else_branch,
            ..
        } => {
            collect_callees_from_expr(expr, callees);
            collect_callees_from_expr(then_branch, callees);
            if let Some(e) = else_branch {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_callees_from_expr(scrutinee, callees);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_callees_from_expr(g, callees);
                }
                collect_callees_from_expr(&arm.body, callees);
            }
        }
        ExprKind::Try(e) | ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } => {
            collect_callees_from_expr(e, callees);
        }
        ExprKind::GuardPattern {
            expr, else_branch, ..
        } => {
            collect_callees_from_expr(expr, callees);
            collect_callees_from_expr(else_branch, callees);
        }
        ExprKind::IsPattern { expr, .. } => collect_callees_from_expr(expr, callees),
        ExprKind::NullCoalesce { value, default } => {
            collect_callees_from_expr(value, callees);
            collect_callees_from_expr(default, callees);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_callees_from_expr(s, callees);
            }
            if let Some(e) = end {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                collect_callees_from_expr(&f.value, callees);
            }
            if let Some(s) = spread {
                collect_callees_from_expr(s, callees);
            }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            collect_callees_from_expr(value, callees);
            collect_callees_from_expr(count, callees);
        }
        ExprKind::WithAs { bindings, body } => {
            for (e, _) in bindings {
                collect_callees_from_expr(e, callees);
            }
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        ExprKind::Closure { body, .. } => collect_callees_from_expr(body, callees),
        ExprKind::Spawn { body }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::BlockCall { body, .. } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        ExprKind::Assert { condition, message }
        | ExprKind::Check { condition, message } => {
            collect_callees_from_expr(condition, callees);
            if let Some(m) = message {
                collect_callees_from_expr(m, callees);
            }
        }
        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                        collect_callees_from_expr(channel, callees);
                    }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        collect_callees_from_expr(channel, callees);
                        collect_callees_from_expr(value, callees);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                collect_callees_from_expr(&arm.body, callees);
            }
        }
        ExprKind::UsingBlock { args, body, .. } => {
            for arg in args {
                collect_callees_from_expr(&arg.expr, callees);
            }
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        // Leaves
        ExprKind::Int(_, _)
        | ExprKind::Float(_, _)
        | ExprKind::String(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Ident(_) => {}
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_generic_arg() {
        assert_eq!(extract_generic_arg("Pool<Player>"), Some("Player".to_string()));
        assert_eq!(extract_generic_arg("Pool<Vec<i32>>"), Some("Vec<i32>".to_string()));
        assert_eq!(extract_generic_arg("Multitasking"), None);
    }

    #[test]
    fn test_context_clause_to_req_pool() {
        let cc = ContextClause {
            name: None,
            ty: "Pool<Player>".to_string(),
            is_frozen: false,
        };
        let req = context_clause_to_req(&cc);
        assert_eq!(req.param_name, "__ctx_pool_Player");
        assert_eq!(req.param_type, "&Pool<Player>");
        assert!(!req.is_runtime);
    }

    #[test]
    fn test_context_clause_to_req_named() {
        let cc = ContextClause {
            name: Some("players".to_string()),
            ty: "Pool<Player>".to_string(),
            is_frozen: false,
        };
        let req = context_clause_to_req(&cc);
        assert_eq!(req.param_name, "__ctx_players");
        assert_eq!(req.param_type, "&Pool<Player>");
    }

    #[test]
    fn test_context_clause_to_req_runtime() {
        let cc = ContextClause {
            name: None,
            ty: "Multitasking".to_string(),
            is_frozen: false,
        };
        let req = context_clause_to_req(&cc);
        assert_eq!(req.param_name, "__ctx_runtime");
        assert_eq!(req.param_type, "RuntimeContext");
        assert!(req.is_runtime);
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Effect inference engine (comp.effects INF1-INF5).
//!
//! Three phases:
//! 1. Collect function declarations, classify direct effects from body
//! 2. Build call graph from function bodies
//! 3. Fixed-point propagation: union callee effects into callers until stable

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::{Effects, EffectMap};
use crate::sources;

/// Qualified function name (plain name or "Type.method").
type FuncName = String;

/// Run effect inference on declarations.
pub fn infer(decls: &[Decl]) -> EffectMap {
    let mut pass = InferPass::new();
    pass.run(decls);
    pass.effects
}

struct InferPass {
    /// Per-function direct effects (before transitive propagation).
    effects: EffectMap,
    /// Call graph: caller → callees.
    call_graph: HashMap<FuncName, HashSet<FuncName>>,
}

impl InferPass {
    fn new() -> Self {
        Self {
            effects: HashMap::new(),
            call_graph: HashMap::new(),
        }
    }

    fn run(&mut self, decls: &[Decl]) {
        // Phase 1: Classify direct effects and build call graph
        self.collect(decls);

        // Phase 2: Mark extern functions conservatively (INF5)
        self.mark_externs(decls);

        // Phase 3: Fixed-point propagation (FX2)
        self.propagate();
    }

    // ── Phase 1: Collect ────────────────────────────────────────────

    fn collect(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    self.collect_fn(&f.name, f);
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        let qname = format!("{}.{}", s.name, method.name);
                        self.collect_fn(&qname, method);
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        let qname = format!("{}.{}", e.name, method.name);
                        self.collect_fn(&qname, method);
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        let qname = format!("{}.{}", i.target_ty, method.name);
                        self.collect_fn(&qname, method);
                    }
                }
                DeclKind::Trait(t) => {
                    for method in &t.methods {
                        let qname = format!("{}.{}", t.name, method.name);
                        self.collect_fn(&qname, method);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_fn(&mut self, qname: &str, f: &FnDecl) {
        // PU2: comptime functions are always pure
        if f.is_comptime {
            self.effects.insert(qname.to_string(), Effects::default());
            return;
        }

        // Classify direct effects from function body
        let mut direct = Effects::default();
        let mut callees = HashSet::new();
        classify_body(&f.body, &mut direct, &mut callees);

        // @no_io suppresses conservative IO marking
        let has_no_io = f.attrs.iter().any(|a| a == "no_io");
        if has_no_io {
            direct.io = false;
        }

        self.effects.insert(qname.to_string(), direct);

        if !callees.is_empty() {
            self.call_graph.insert(qname.to_string(), callees);
        }
    }

    // ── Phase 2: Extern declarations ────────────────────────────────

    /// INF5: extern functions are conservatively IO unless @no_io.
    fn mark_externs(&mut self, decls: &[Decl]) {
        for decl in decls {
            if let DeclKind::Extern(e) = &decl.kind {
                // Skip if already registered (shouldn't happen, but defensive)
                if self.effects.contains_key(&e.name) {
                    continue;
                }
                self.effects.insert(
                    e.name.clone(),
                    Effects { io: true, async_: false, grow: false, shrink: false },
                );
            }
        }
    }

    // ── Phase 3: Fixed-point propagation ────────────────────────────

    fn propagate(&mut self) {
        loop {
            let mut changed = false;

            let graph_snapshot: Vec<(FuncName, HashSet<FuncName>)> =
                self.call_graph.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

            for (caller, callees) in &graph_snapshot {
                for callee in callees {
                    let callee_effects = self.effects.get(callee).copied()
                        .unwrap_or_default();

                    if callee_effects.is_pure() {
                        continue;
                    }

                    let caller_effects = self.effects.entry(caller.clone())
                        .or_default();

                    let before = *caller_effects;
                    caller_effects.union(callee_effects);

                    if *caller_effects != before {
                        changed = true;
                    }
                }
            }

            if !changed {
                break;
            }
        }

        // AS3: Async implies IO — enforce invariant after propagation
        for effects in self.effects.values_mut() {
            if effects.async_ {
                effects.io = true;
            }
        }
    }
}

// ── Body classification ──────────────────────────────────────────────

/// Walk a function body, collecting direct effects and callee names.
fn classify_body(stmts: &[Stmt], effects: &mut Effects, callees: &mut HashSet<String>) {
    for stmt in stmts {
        classify_stmt(stmt, effects, callees);
    }
}

fn classify_stmt(stmt: &Stmt, effects: &mut Effects, callees: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Expr(e) => classify_expr(e, effects, callees),
        StmtKind::Mut { init, .. } | StmtKind::Const { init, .. } => {
            classify_expr(init, effects, callees);
        }
        StmtKind::MutTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
            classify_expr(init, effects, callees);
        }
        StmtKind::Assign { target, value } => {
            classify_expr(target, effects, callees);
            classify_expr(value, effects, callees);
        }
        StmtKind::Return(Some(e)) => classify_expr(e, effects, callees),
        StmtKind::Return(None) => {}
        StmtKind::Break { value: Some(v), .. } => classify_expr(v, effects, callees),
        StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        StmtKind::Discard { .. } => {}
        StmtKind::While { cond, body } => {
            classify_expr(cond, effects, callees);
            classify_body(body, effects, callees);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            classify_expr(expr, effects, callees);
            classify_body(body, effects, callees);
        }
        StmtKind::Loop { body, .. } => classify_body(body, effects, callees),
        StmtKind::For { iter, body, .. } => {
            classify_expr(iter, effects, callees);
            classify_body(body, effects, callees);
        }
        StmtKind::Ensure { body, else_handler } => {
            classify_body(body, effects, callees);
            if let Some((_, handler)) = else_handler {
                classify_body(handler, effects, callees);
            }
        }
        StmtKind::Comptime(body) => classify_body(body, effects, callees),
        StmtKind::ComptimeFor { body, .. } => classify_body(body, effects, callees),
    }
}

fn classify_expr(expr: &Expr, effects: &mut Effects, callees: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Call { func, args } => {
            if let Some(name) = extract_callee_name(func) {
                // Check if this is a known source function
                let direct = sources::classify_call(&name);
                effects.union(direct);
                callees.insert(name);
            }
            classify_expr(func, effects, callees);
            for arg in args {
                classify_expr(&arg.expr, effects, callees);
            }
        }

        ExprKind::MethodCall { object, method, args, .. } => {
            // Method calls: record the method name for call graph.
            // Also check qualified "Type.method" form when we can extract
            // the receiver type name.
            if let ExprKind::Ident(type_name) = &object.kind {
                let qname = format!("{}.{}", type_name, method);
                let direct = sources::classify_call(&qname);
                effects.union(direct);
                callees.insert(qname);
            }
            // Also record bare method name
            let direct = sources::classify_call(method);
            effects.union(direct);
            callees.insert(method.clone());

            classify_expr(object, effects, callees);
            for arg in args {
                classify_expr(&arg.expr, effects, callees);
            }
        }

        // IO3: unsafe blocks conservatively get IO
        ExprKind::Unsafe { body } => {
            effects.io = true;
            classify_body(body, effects, callees);
        }

        // Spawn is an async source (AS1)
        ExprKind::Spawn { body } => {
            effects.io = true;
            effects.async_ = true;
            classify_body(body, effects, callees);
        }

        // Recurse into all other expression kinds
        ExprKind::Binary { left, right, .. } => {
            classify_expr(left, effects, callees);
            classify_expr(right, effects, callees);
        }
        ExprKind::Unary { operand, .. } => classify_expr(operand, effects, callees),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            classify_expr(object, effects, callees);
        }
        ExprKind::DynamicField { object, field_expr } => {
            classify_expr(object, effects, callees);
            classify_expr(field_expr, effects, callees);
        }
        ExprKind::Index { object, index } => {
            classify_expr(object, effects, callees);
            classify_expr(index, effects, callees);
        }
        ExprKind::Block(stmts) => classify_body(stmts, effects, callees),
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            classify_expr(cond, effects, callees);
            classify_expr(then_branch, effects, callees);
            if let Some(e) = else_branch {
                classify_expr(e, effects, callees);
            }
        }
        ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
            classify_expr(expr, effects, callees);
            classify_expr(then_branch, effects, callees);
            if let Some(e) = else_branch {
                classify_expr(e, effects, callees);
            }
        }
        ExprKind::GuardPattern { expr, else_branch, .. } => {
            classify_expr(expr, effects, callees);
            classify_expr(else_branch, effects, callees);
        }
        ExprKind::IsPattern { expr, .. } => classify_expr(expr, effects, callees),
        ExprKind::Match { scrutinee, arms } => {
            classify_expr(scrutinee, effects, callees);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    classify_expr(g, effects, callees);
                }
                classify_expr(&arm.body, effects, callees);
            }
        }
        ExprKind::Try { expr: e, else_clause } => {
            classify_expr(e, effects, callees);
            if let Some(ec) = else_clause {
                classify_expr(&ec.body, effects, callees);
            }
        }
        ExprKind::IsPresent { expr: e, .. } => {
            classify_expr(e, effects, callees);
        }
        ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } => {
            classify_expr(e, effects, callees);
        }
        ExprKind::NullCoalesce { value, default } => {
            classify_expr(value, effects, callees);
            classify_expr(default, effects, callees);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start { classify_expr(s, effects, callees); }
            if let Some(e) = end { classify_expr(e, effects, callees); }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                classify_expr(&f.value, effects, callees);
            }
            if let Some(s) = spread {
                classify_expr(s, effects, callees);
            }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                classify_expr(e, effects, callees);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            classify_expr(value, effects, callees);
            classify_expr(count, effects, callees);
        }
        ExprKind::UsingBlock { args, body, .. } => {
            for arg in args {
                classify_expr(&arg.expr, effects, callees);
            }
            classify_body(body, effects, callees);
        }
        ExprKind::WithAs { bindings, body } => {
            for binding in bindings {
                classify_expr(&binding.source, effects, callees);
            }
            classify_body(body, effects, callees);
        }
        ExprKind::Closure { body, .. } => classify_expr(body, effects, callees),
        ExprKind::Comptime { body } | ExprKind::BlockCall { body, .. }
        | ExprKind::Loop { body, .. } => {
            classify_body(body, effects, callees);
        }
        ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
            classify_expr(condition, effects, callees);
            if let Some(m) = message {
                classify_expr(m, effects, callees);
            }
        }
        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                        classify_expr(channel, effects, callees);
                    }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        classify_expr(channel, effects, callees);
                        classify_expr(value, effects, callees);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                classify_expr(&arm.body, effects, callees);
            }
        }
        // Leaves
        ExprKind::Int(_, _)
        | ExprKind::Float(_, _)
        | ExprKind::String(_)
        | ExprKind::StringInterp(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::None
        | ExprKind::Ident(_) => {}
    }
}

/// Extract callee name from a Call expression's func field.
fn extract_callee_name(func: &Expr) -> Option<String> {
    match &func.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Field { object, field } => {
            if let ExprKind::Ident(obj_name) = &object.kind {
                Some(format!("{}.{}", obj_name, field))
            } else {
                None
            }
        }
        _ => None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{Decl, DeclKind, FnDecl, ExternDecl};
    use rask_ast::expr::{CallArg, ArgMode, Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

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

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt { id: NodeId(0), kind: StmtKind::Expr(e), span: sp() }
    }

    fn make_fn(name: &str, body: Vec<Stmt>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.into(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![],
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
            }),
            span: sp(),
        }
    }

    fn make_comptime_fn(name: &str) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.into(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![],
                body: vec![expr_stmt(call("println", vec![]))],
                is_pub: false,
                is_private: false,
                is_comptime: true,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
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
                ret_ty: None,
                doc: None,
            }),
            span: sp(),
        }
    }

    #[test]
    fn pure_function() {
        let decls = vec![make_fn("add", vec![return_stmt(Some(ident("x")))])];
        let effects = infer(&decls);
        assert!(effects["add"].is_pure());
    }

    #[test]
    fn direct_io_call() {
        let decls = vec![make_fn("load", vec![
            expr_stmt(call("println", vec![])),
        ])];
        let effects = infer(&decls);
        assert!(effects["load"].io);
        assert!(!effects["load"].async_);
    }

    #[test]
    fn direct_async_call() {
        let decls = vec![make_fn("run", vec![
            expr_stmt(call("spawn", vec![])),
        ])];
        let effects = infer(&decls);
        assert!(effects["run"].io, "AS3: Async implies IO");
        assert!(effects["run"].async_);
    }

    #[test]
    fn field_call_io_source() {
        let decls = vec![make_fn("read", vec![
            expr_stmt(field_call("File", "open")),
        ])];
        let effects = infer(&decls);
        assert!(effects["read"].io);
    }

    #[test]
    fn method_call_io_source() {
        let decls = vec![make_fn("net", vec![
            expr_stmt(method_call("Channel", "send")),
        ])];
        let effects = infer(&decls);
        assert!(effects["net"].io);
        assert!(effects["net"].async_);
    }

    #[test]
    fn transitive_propagation() {
        // load calls println (IO)
        // process calls load → should inherit IO
        let decls = vec![
            make_fn("load", vec![expr_stmt(call("println", vec![]))]),
            make_fn("process", vec![expr_stmt(call("load", vec![]))]),
        ];
        let effects = infer(&decls);
        assert!(effects["load"].io);
        assert!(effects["process"].io, "Transitive IO via call to load");
    }

    #[test]
    fn deep_transitive() {
        // c calls println, b calls c, a calls b
        let decls = vec![
            make_fn("c", vec![expr_stmt(call("println", vec![]))]),
            make_fn("b", vec![expr_stmt(call("c", vec![]))]),
            make_fn("a", vec![expr_stmt(call("b", vec![]))]),
        ];
        let effects = infer(&decls);
        assert!(effects["a"].io);
        assert!(effects["b"].io);
        assert!(effects["c"].io);
    }

    #[test]
    fn mutual_recursion_terminates() {
        // a calls b, b calls a, a also calls println
        let decls = vec![
            make_fn("a", vec![
                expr_stmt(call("println", vec![])),
                expr_stmt(call("b", vec![])),
            ]),
            make_fn("b", vec![expr_stmt(call("a", vec![]))]),
        ];
        let effects = infer(&decls);
        assert!(effects["a"].io);
        assert!(effects["b"].io, "b inherits IO from a via mutual recursion");
    }

    #[test]
    fn unsafe_block_conservative_io() {
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
        let effects = infer(&decls);
        assert!(effects["ffi_call"].io, "IO3: unsafe blocks conservative IO");
    }

    #[test]
    fn comptime_always_pure() {
        let decls = vec![make_comptime_fn("table_gen")];
        let effects = infer(&decls);
        assert!(effects["table_gen"].is_pure(), "PU2: comptime is pure");
    }

    #[test]
    fn extern_conservative_io() {
        let decls = vec![make_extern("c_function")];
        let effects = infer(&decls);
        assert!(effects["c_function"].io, "INF5: extern is conservative IO");
    }

    #[test]
    fn spawn_expr_is_async() {
        let decls = vec![make_fn("run", vec![
            Stmt {
                id: NodeId(0),
                kind: StmtKind::Expr(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Spawn { body: vec![] },
                    span: sp(),
                }),
                span: sp(),
            },
        ])];
        let effects = infer(&decls);
        assert!(effects["run"].async_);
        assert!(effects["run"].io, "AS3: Async implies IO");
    }

    #[test]
    fn mutation_effect() {
        let decls = vec![make_fn("grow", vec![
            expr_stmt(method_call("pool", "insert")),
        ])];
        let effects = infer(&decls);
        assert!(effects["grow"].mutation());
        assert!(!effects["grow"].io, "Mutation is orthogonal to IO");
    }

    #[test]
    fn mixed_effects() {
        let decls = vec![make_fn("complex", vec![
            expr_stmt(call("println", vec![])),
            expr_stmt(call("spawn", vec![])),
            expr_stmt(method_call("pool", "insert")),
        ])];
        let effects = infer(&decls);
        assert!(effects["complex"].io);
        assert!(effects["complex"].async_);
        assert!(effects["complex"].mutation());
    }

    #[test]
    fn unknown_callee_is_pure() {
        let decls = vec![make_fn("caller", vec![
            expr_stmt(call("some_unknown_fn", vec![])),
        ])];
        let effects = infer(&decls);
        // Unknown function isn't in our source table, and there's no
        // declaration to propagate from → stays pure
        assert!(effects["caller"].is_pure());
    }
}

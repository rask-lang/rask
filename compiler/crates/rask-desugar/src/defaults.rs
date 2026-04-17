// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Default parameter desugaring and named argument resolution.
//!
//! Runs after operator desugaring but before name resolution.
//! Builds a function lookup table from declarations, then rewrites
//! call sites to fill in default values for missing arguments.

use std::collections::HashMap;
use rask_ast::decl::{Decl, DeclKind, FnDecl, Param};
use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::NodeId;

/// Desugar default arguments and named arguments across all declarations.
///
/// Builds a lookup table of function signatures, then rewrites call sites
/// so that missing arguments with defaults are filled in and named
/// arguments are resolved to positional form.
pub fn desugar_default_args(decls: &mut [Decl]) {
    let lookup = FunctionLookup::build(decls);
    let mut ctx = DefaultDesugarer {
        lookup,
        next_id: 2_000_000,
    };
    for decl in decls {
        ctx.desugar_decl(decl);
    }
}

/// Check whether an expression is a valid default (comptime-evaluable).
///
/// Accepts: literals, negated literals, enum-style paths (Type.Variant),
/// bool literals, null. Rejects everything else.
pub fn is_valid_default_expr(expr: &Expr) -> bool {
    match &expr.kind {
        // Literals
        ExprKind::Int(_, _)
        | ExprKind::Float(_, _)
        | ExprKind::String(_)
        | ExprKind::StringInterp(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null => true,

        // Negated literal: -1, -3.14
        ExprKind::Unary { op: rask_ast::expr::UnaryOp::Neg, operand } => {
            matches!(&operand.kind, ExprKind::Int(_, _) | ExprKind::Float(_, _))
        }

        // Enum-style path: Color.Red, FileMode.Read
        ExprKind::Field { object, .. } => {
            matches!(&object.kind, ExprKind::Ident(_))
        }

        // Dynamic field — not valid as a default
        ExprKind::DynamicField { .. } => false,

        // Array of valid defaults: [1, 2, 3]
        ExprKind::Array(elems) => elems.iter().all(is_valid_default_expr),

        // Tuple of valid defaults: (1, "a")
        ExprKind::Tuple(elems) => elems.iter().all(is_valid_default_expr),

        _ => false,
    }
}

// ---- Function Lookup Table ----

/// Maps function names and (type, method) pairs to parameter lists.
struct FunctionLookup {
    /// Free functions: name → params
    functions: HashMap<String, Vec<Param>>,
    /// Methods: (type_name, method_name) → params (excluding self)
    methods: HashMap<(String, String), Vec<Param>>,
    /// Methods indexed by name only (for instance method fallback)
    methods_by_name: HashMap<String, Vec<Vec<Param>>>,
}

impl FunctionLookup {
    fn build(decls: &[Decl]) -> Self {
        let mut functions = HashMap::new();
        let mut methods: HashMap<(String, String), Vec<Param>> = HashMap::new();
        let mut methods_by_name: HashMap<String, Vec<Vec<Param>>> = HashMap::new();

        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    if f.params.iter().any(|p| p.default.is_some()) {
                        functions.insert(f.name.clone(), f.params.clone());
                    }
                }
                DeclKind::Struct(s) => {
                    for m in &s.methods {
                        Self::register_method(
                            &s.name, m, &mut methods, &mut methods_by_name,
                        );
                    }
                }
                DeclKind::Enum(e) => {
                    for m in &e.methods {
                        Self::register_method(
                            &e.name, m, &mut methods, &mut methods_by_name,
                        );
                    }
                }
                DeclKind::Impl(i) => {
                    for m in &i.methods {
                        Self::register_method(
                            &i.target_ty, m, &mut methods, &mut methods_by_name,
                        );
                    }
                }
                _ => {}
            }
        }

        Self { functions, methods, methods_by_name }
    }

    fn register_method(
        type_name: &str,
        method: &FnDecl,
        methods: &mut HashMap<(String, String), Vec<Param>>,
        methods_by_name: &mut HashMap<String, Vec<Vec<Param>>>,
    ) {
        // Only register methods that have default params
        let non_self_params: Vec<Param> = method.params.iter()
            .filter(|p| p.name != "self")
            .cloned()
            .collect();
        if non_self_params.iter().any(|p| p.default.is_some()) {
            methods.insert(
                (type_name.to_string(), method.name.clone()),
                non_self_params.clone(),
            );
            methods_by_name
                .entry(method.name.clone())
                .or_default()
                .push(non_self_params);
        }
    }

    /// Look up params for a free function call.
    fn lookup_function(&self, name: &str) -> Option<&[Param]> {
        self.functions.get(name).map(|v| v.as_slice())
    }

    /// Look up params for a static method call (Type.method).
    fn lookup_static_method(&self, type_name: &str, method: &str) -> Option<&[Param]> {
        self.methods.get(&(type_name.to_string(), method.to_string()))
            .map(|v| v.as_slice())
    }

    /// Look up params for an instance method by name only (fallback).
    /// Returns Some only if there's exactly one signature for this method name.
    fn lookup_instance_method(&self, method: &str) -> Option<&[Param]> {
        self.methods_by_name.get(method).and_then(|sigs| {
            if sigs.len() == 1 {
                Some(sigs[0].as_slice())
            } else {
                None
            }
        })
    }
}

// ---- Argument Resolution ----

/// Resolve call arguments against function parameters, filling in defaults.
///
/// Returns the rewritten args list with defaults inserted and names stripped,
/// or None if resolution can't be done (error or no changes needed).
fn resolve_call_args(
    params: &[Param],
    args: &[CallArg],
    fresh_id: &mut impl FnMut() -> NodeId,
) -> Option<Vec<CallArg>> {
    // Quick check: if all args are positional and count matches, nothing to do
    let has_named = args.iter().any(|a| a.name.is_some());
    let has_defaults = params.iter().any(|p| p.default.is_some());

    if !has_named && args.len() == params.len() {
        return None; // Already fully resolved
    }
    if !has_named && !has_defaults {
        return None; // No defaults to fill, will error elsewhere
    }
    if args.len() > params.len() {
        return None; // Too many args, let type checker report
    }

    let mut result = Vec::with_capacity(params.len());
    let mut arg_idx = 0;

    for param in params {
        if param.name == "self" {
            // self is never filled by default
            if arg_idx < args.len() {
                result.push(CallArg {
                    name: None,
                    mode: args[arg_idx].mode,
                    expr: args[arg_idx].expr.clone(),
                });
                arg_idx += 1;
            }
            continue;
        }

        if arg_idx < args.len() {
            let arg = &args[arg_idx];

            if let Some(ref name) = arg.name {
                if name == &param.name {
                    // Named arg matches this param — use it
                    result.push(CallArg {
                        name: None,
                        mode: arg.mode,
                        expr: arg.expr.clone(),
                    });
                    arg_idx += 1;
                    continue;
                }
                // Named arg doesn't match — this param must have a default
                if let Some(ref default_expr) = param.default {
                    result.push(CallArg {
                        name: None,
                        mode: ArgMode::Default,
                        expr: clone_expr_with_fresh_ids(default_expr, fresh_id),
                    });
                    continue;
                }
                // Required param skipped — bail out, let type checker report
                return None;
            }

            // Positional arg — use it directly
            result.push(CallArg {
                name: None,
                mode: arg.mode,
                expr: arg.expr.clone(),
            });
            arg_idx += 1;
        } else {
            // No more provided args — fill with default
            if let Some(ref default_expr) = param.default {
                result.push(CallArg {
                    name: None,
                    mode: ArgMode::Default,
                    expr: clone_expr_with_fresh_ids(default_expr, fresh_id),
                });
            } else {
                // Required param missing — bail
                return None;
            }
        }
    }

    // If there are leftover args, bail
    if arg_idx < args.len() {
        return None;
    }

    Some(result)
}

/// Clone an expression, assigning fresh NodeIds to avoid collisions.
fn clone_expr_with_fresh_ids(
    expr: &Expr,
    fresh_id: &mut impl FnMut() -> NodeId,
) -> Expr {
    Expr {
        id: fresh_id(),
        kind: clone_expr_kind(&expr.kind, fresh_id),
        span: expr.span,
    }
}

fn clone_expr_kind(
    kind: &ExprKind,
    fresh_id: &mut impl FnMut() -> NodeId,
) -> ExprKind {
    match kind {
        ExprKind::Int(v, suffix) => ExprKind::Int(*v, suffix.clone()),
        ExprKind::Float(v, suffix) => ExprKind::Float(*v, suffix.clone()),
        ExprKind::String(s) => ExprKind::String(s.clone()),
        ExprKind::Char(c) => ExprKind::Char(*c),
        ExprKind::Bool(b) => ExprKind::Bool(*b),
        ExprKind::Null => ExprKind::Null,
        ExprKind::Ident(n) => ExprKind::Ident(n.clone()),
        ExprKind::Field { object, field } => ExprKind::Field {
            object: Box::new(clone_expr_with_fresh_ids(object, fresh_id)),
            field: field.clone(),
        },
        ExprKind::Unary { op, operand } => ExprKind::Unary {
            op: *op,
            operand: Box::new(clone_expr_with_fresh_ids(operand, fresh_id)),
        },
        ExprKind::Array(elems) => ExprKind::Array(
            elems.iter().map(|e| clone_expr_with_fresh_ids(e, fresh_id)).collect(),
        ),
        ExprKind::Tuple(elems) => ExprKind::Tuple(
            elems.iter().map(|e| clone_expr_with_fresh_ids(e, fresh_id)).collect(),
        ),
        // For any other expression kind in a default value, just clone as-is
        // (the validator should have rejected complex expressions)
        other => other.clone(),
    }
}

// ---- Desugaring Traversal ----

struct DefaultDesugarer {
    lookup: FunctionLookup,
    next_id: u32,
}

impl DefaultDesugarer {
    fn desugar_decl(&mut self, decl: &mut Decl) {
        match &mut decl.kind {
            DeclKind::Fn(f) => self.desugar_fn_body(f),
            DeclKind::Struct(s) => {
                for m in &mut s.methods { self.desugar_fn_body(m); }
            }
            DeclKind::Enum(e) => {
                for m in &mut e.methods { self.desugar_fn_body(m); }
            }
            DeclKind::Impl(i) => {
                for m in &mut i.methods { self.desugar_fn_body(m); }
            }
            DeclKind::Trait(t) => {
                for m in &mut t.methods { self.desugar_fn_body(m); }
            }
            DeclKind::Const(c) => self.desugar_expr(&mut c.init),
            DeclKind::Test(t) => {
                for s in &mut t.body { self.desugar_stmt(s); }
            }
            DeclKind::Benchmark(b) => {
                for s in &mut b.body { self.desugar_stmt(s); }
            }
            DeclKind::Import(_) | DeclKind::Export(_) | DeclKind::Extern(_)
            | DeclKind::Package(_) | DeclKind::Union(_) | DeclKind::TypeAlias(_)
            | DeclKind::CImport(_) => {}
        }
    }

    fn desugar_fn_body(&mut self, f: &mut FnDecl) {
        for stmt in &mut f.body {
            self.desugar_stmt(stmt);
        }
    }

    fn desugar_stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Expr(e) => self.desugar_expr(e),
            StmtKind::Mut { init, .. } | StmtKind::Const { init, .. } => self.desugar_expr(init),
            StmtKind::MutTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.desugar_expr(init);
            }
            StmtKind::Assign { target, value } => {
                self.desugar_expr(target);
                self.desugar_expr(value);
            }
            StmtKind::Return(Some(e)) => self.desugar_expr(e),
            StmtKind::Return(None) => {}
            StmtKind::Break { value: Some(v), .. } => self.desugar_expr(v),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
            StmtKind::While { cond, body } => {
                self.desugar_expr(cond);
                for s in body { self.desugar_stmt(s); }
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.desugar_expr(expr);
                for s in body { self.desugar_stmt(s); }
            }
            StmtKind::Loop { body, .. } => {
                for s in body { self.desugar_stmt(s); }
            }
            StmtKind::For { iter, body, .. } => {
                self.desugar_expr(iter);
                for s in body { self.desugar_stmt(s); }
            }
            StmtKind::Ensure { body, else_handler } => {
                for s in body { self.desugar_stmt(s); }
                if let Some((_, handler)) = else_handler {
                    for s in handler { self.desugar_stmt(s); }
                }
            }
            StmtKind::Comptime(body) => {
                for s in body { self.desugar_stmt(s); }
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.desugar_expr(iter);
                for s in body { self.desugar_stmt(s); }
            }
            StmtKind::Discard { .. } => {}
        }
    }

    fn desugar_expr(&mut self, expr: &mut Expr) {
        // Recurse into child expressions first
        match &mut expr.kind {
            ExprKind::Call { func, args } => {
                self.desugar_expr(func);
                for arg in args.iter_mut() { self.desugar_expr(&mut arg.expr); }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.desugar_expr(object);
                for arg in args.iter_mut() { self.desugar_expr(&mut arg.expr); }
            }
            ExprKind::Binary { left, right, .. } => {
                self.desugar_expr(left);
                self.desugar_expr(right);
            }
            ExprKind::Unary { operand, .. } => self.desugar_expr(operand),
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.desugar_expr(object);
            }
            ExprKind::DynamicField { object, field_expr } => {
                self.desugar_expr(object);
                self.desugar_expr(field_expr);
            }
            ExprKind::Index { object, index } => {
                self.desugar_expr(object);
                self.desugar_expr(index);
            }
            ExprKind::Block(stmts) => {
                for s in stmts { self.desugar_stmt(s); }
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.desugar_expr(cond);
                self.desugar_expr(then_branch);
                if let Some(e) = else_branch { self.desugar_expr(e); }
            }
            ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
                self.desugar_expr(expr);
                self.desugar_expr(then_branch);
                if let Some(e) = else_branch { self.desugar_expr(e); }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.desugar_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &mut arm.guard { self.desugar_expr(g); }
                    self.desugar_expr(&mut arm.body);
                }
            }
            ExprKind::Try { expr: e, ref mut else_clause } => {
                self.desugar_expr(e);
                if let Some(ec) = else_clause {
                    self.desugar_expr(&mut ec.body);
                }
            }
            ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } => {
                self.desugar_expr(e);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.desugar_expr(value);
                self.desugar_expr(default);
            }
            ExprKind::GuardPattern { expr, else_branch, .. } => {
                self.desugar_expr(expr);
                self.desugar_expr(else_branch);
            }
            ExprKind::IsPattern { expr, .. } => self.desugar_expr(expr),
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.desugar_expr(s); }
                if let Some(e) = end { self.desugar_expr(e); }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields { self.desugar_expr(&mut f.value); }
                if let Some(s) = spread { self.desugar_expr(s); }
            }
            ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
                for e in elems { self.desugar_expr(e); }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.desugar_expr(value);
                self.desugar_expr(count);
            }
            ExprKind::Closure { body, .. } => self.desugar_expr(body),
            ExprKind::WithAs { bindings, body } => {
                for b in bindings { self.desugar_expr(&mut b.source); }
                for s in body { self.desugar_stmt(s); }
            }
            ExprKind::Spawn { body } | ExprKind::Unsafe { body }
            | ExprKind::BlockCall { body, .. } | ExprKind::Comptime { body }
            | ExprKind::Loop { body, .. } => {
                for s in body { self.desugar_stmt(s); }
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.desugar_expr(condition);
                if let Some(m) = message { self.desugar_expr(m); }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &mut arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => self.desugar_expr(channel),
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.desugar_expr(channel);
                            self.desugar_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.desugar_expr(&mut arm.body);
                }
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for a in args { self.desugar_expr(&mut a.expr); }
                for s in body { self.desugar_stmt(s); }
            }
            // Terminals
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_)
            | ExprKind::StringInterp(_) | ExprKind::Char(_) | ExprKind::Bool(_)
            | ExprKind::Ident(_) | ExprKind::Null => {}
        }

        // After recursing, try to resolve defaults at this call site
        self.try_resolve_call(expr);
    }

    fn try_resolve_call(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            ExprKind::Call { func, args } => {
                if let ExprKind::Ident(name) = &func.kind {
                    if let Some(params) = self.lookup.lookup_function(name) {
                        let params = params.to_vec();
                        let next_id = &mut self.next_id;
                        let mut id_gen = || {
                            let id = NodeId(*next_id);
                            *next_id += 1;
                            id
                        };
                        if let Some(resolved) = resolve_call_args(&params, args, &mut id_gen) {
                            *args = resolved;
                        }
                    }
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                // Static method: Type.method(...)
                let params = if let ExprKind::Ident(type_name) = &object.kind {
                    self.lookup.lookup_static_method(type_name, method)
                        .map(|p| p.to_vec())
                } else {
                    // Instance method fallback: look up by method name
                    self.lookup.lookup_instance_method(method)
                        .map(|p| p.to_vec())
                };

                if let Some(params) = params {
                    let next_id = &mut self.next_id;
                    let mut id_gen = || {
                        let id = NodeId(*next_id);
                        *next_id += 1;
                        id
                    };
                    if let Some(resolved) = resolve_call_args(&params, args, &mut id_gen) {
                        *args = resolved;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::Span;

    fn sp() -> Span { Span::new(0, 0) }
    fn int_expr(v: i64) -> Expr {
        Expr { id: NodeId(0), kind: ExprKind::Int(v, None), span: sp() }
    }
    fn str_expr(s: &str) -> Expr {
        Expr { id: NodeId(0), kind: ExprKind::String(s.to_string()), span: sp() }
    }

    fn make_param(name: &str, ty: &str, default: Option<Expr>) -> Param {
        Param {
            name: name.to_string(),
            name_span: sp(),
            ty: ty.to_string(),
            is_take: false,
            is_mutate: false,
            default,
        }
    }

    fn make_arg(expr: Expr) -> CallArg {
        CallArg { name: None, mode: ArgMode::Default, expr }
    }

    fn make_named_arg(name: &str, expr: Expr) -> CallArg {
        CallArg { name: Some(name.to_string()), mode: ArgMode::Default, expr }
    }

    #[test]
    fn trailing_defaults_filled() {
        let params = vec![
            make_param("host", "string", None),
            make_param("port", "i32", Some(int_expr(8080))),
            make_param("timeout", "i32", Some(int_expr(30))),
        ];
        let args = vec![make_arg(str_expr("localhost"))];
        let mut next = 100;
        let resolved = resolve_call_args(&params, &args, &mut || {
            next += 1;
            NodeId(next)
        });
        let resolved = resolved.expect("should resolve");
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn named_arg_skips_middle_param() {
        let params = vec![
            make_param("host", "string", None),
            make_param("port", "i32", Some(int_expr(8080))),
            make_param("timeout", "i32", Some(int_expr(30))),
        ];
        // connect("localhost", timeout: 60)
        let args = vec![
            make_arg(str_expr("localhost")),
            make_named_arg("timeout", int_expr(60)),
        ];
        let mut next = 100;
        let resolved = resolve_call_args(&params, &args, &mut || {
            next += 1;
            NodeId(next)
        });
        let resolved = resolved.expect("should resolve");
        assert_eq!(resolved.len(), 3);
        // Second arg should be the default 8080
        match &resolved[1].expr.kind {
            ExprKind::Int(v, _) => assert_eq!(*v, 8080),
            _ => panic!("expected default int 8080"),
        }
        // Third arg should be the provided 60
        match &resolved[2].expr.kind {
            ExprKind::Int(v, _) => assert_eq!(*v, 60),
            _ => panic!("expected provided int 60"),
        }
    }

    #[test]
    fn all_named_args() {
        let params = vec![
            make_param("host", "string", None),
            make_param("port", "i32", Some(int_expr(8080))),
            make_param("timeout", "i32", Some(int_expr(30))),
        ];
        // connect(host: "localhost", timeout: 60)
        let args = vec![
            make_named_arg("host", str_expr("localhost")),
            make_named_arg("timeout", int_expr(60)),
        ];
        let mut next = 100;
        let resolved = resolve_call_args(&params, &args, &mut || {
            next += 1;
            NodeId(next)
        });
        let resolved = resolved.expect("should resolve");
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn no_defaults_no_change() {
        let params = vec![
            make_param("a", "i32", None),
            make_param("b", "i32", None),
        ];
        let args = vec![make_arg(int_expr(1)), make_arg(int_expr(2))];
        let mut next = 100;
        let resolved = resolve_call_args(&params, &args, &mut || {
            next += 1;
            NodeId(next)
        });
        assert!(resolved.is_none(), "fully positional with no defaults should return None");
    }

    #[test]
    fn missing_required_bails() {
        let params = vec![
            make_param("host", "string", None),
            make_param("port", "i32", None),
        ];
        let args = vec![make_arg(str_expr("localhost"))];
        let mut next = 100;
        let resolved = resolve_call_args(&params, &args, &mut || {
            next += 1;
            NodeId(next)
        });
        assert!(resolved.is_none(), "missing required param should bail");
    }

    #[test]
    fn valid_default_exprs() {
        assert!(is_valid_default_expr(&int_expr(42)));
        assert!(is_valid_default_expr(&str_expr("hello")));
        assert!(is_valid_default_expr(&Expr {
            id: NodeId(0),
            kind: ExprKind::Bool(true),
            span: sp(),
        }));
        // Enum path: Color.Red
        assert!(is_valid_default_expr(&Expr {
            id: NodeId(0),
            kind: ExprKind::Field {
                object: Box::new(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Ident("Color".to_string()),
                    span: sp(),
                }),
                field: "Red".to_string(),
            },
            span: sp(),
        }));
    }

    #[test]
    fn invalid_default_rejects_function_call() {
        let call_expr = Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Ident("compute".to_string()),
                    span: sp(),
                }),
                args: vec![],
            },
            span: sp(),
        };
        assert!(!is_valid_default_expr(&call_expr));
    }
}

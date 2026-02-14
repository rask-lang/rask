// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Idiomatic pattern rules.
//!
//! - unwrap-production: Flag .unwrap() outside test blocks
//! - missing-ensure: Flag @resource creation without ensure

use rask_ast::decl::*;
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::types::*;
use crate::util;

/// idiom/unwrap-production: Flag .unwrap() calls outside test/benchmark blocks.
pub fn check_unwrap_production(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Test(_) | DeclKind::Benchmark(_) => continue,
            DeclKind::Fn(f) => walk_stmts_for_unwrap(&f.body, source, &mut diags),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    walk_stmts_for_unwrap(&m.body, source, &mut diags);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    walk_stmts_for_unwrap(&m.body, source, &mut diags);
                }
            }
            DeclKind::Impl(imp) => {
                for m in &imp.methods {
                    walk_stmts_for_unwrap(&m.body, source, &mut diags);
                }
            }
            _ => {}
        }
    }

    diags
}

fn walk_stmts_for_unwrap(stmts: &[Stmt], source: &str, diags: &mut Vec<LintDiagnostic>) {
    for stmt in stmts {
        walk_stmt_for_unwrap(stmt, source, diags);
    }
}

fn walk_stmt_for_unwrap(stmt: &Stmt, source: &str, diags: &mut Vec<LintDiagnostic>) {
    match &stmt.kind {
        StmtKind::Expr(e) => walk_expr_for_unwrap(e, source, diags),
        StmtKind::Let { init, .. }
        | StmtKind::Const { init, .. }
        | StmtKind::Break { value: Some(init), .. } => {
            walk_expr_for_unwrap(init, source, diags);
        }
        StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
            walk_expr_for_unwrap(init, source, diags);
        }
        StmtKind::Return(Some(e)) => walk_expr_for_unwrap(e, source, diags),
        StmtKind::Assign { target, value } => {
            walk_expr_for_unwrap(target, source, diags);
            walk_expr_for_unwrap(value, source, diags);
        }
        StmtKind::While { cond, body } => {
            walk_expr_for_unwrap(cond, source, diags);
            walk_stmts_for_unwrap(body, source, diags);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            walk_expr_for_unwrap(expr, source, diags);
            walk_stmts_for_unwrap(body, source, diags);
        }
        StmtKind::For { iter, body, .. } => {
            walk_expr_for_unwrap(iter, source, diags);
            walk_stmts_for_unwrap(body, source, diags);
        }
        StmtKind::Loop { body, .. } => {
            walk_stmts_for_unwrap(body, source, diags);
        }
        StmtKind::Ensure { body, else_handler } => {
            walk_stmts_for_unwrap(body, source, diags);
            if let Some((_, handler)) = else_handler {
                walk_stmts_for_unwrap(handler, source, diags);
            }
        }
        StmtKind::Comptime(stmts) => walk_stmts_for_unwrap(stmts, source, diags),
        _ => {}
    }
}

fn walk_expr_for_unwrap(expr: &Expr, source: &str, diags: &mut Vec<LintDiagnostic>) {
    match &expr.kind {
        ExprKind::MethodCall {
            object,
            method,
            args,
            ..
        } => {
            if method == "unwrap" {
                let (line, col) = util::line_col(source, expr.span.start);
                let source_line = util::get_source_line(source, line);
                diags.push(LintDiagnostic {
                    rule: "idiom/unwrap-production".to_string(),
                    severity: Severity::Warning,
                    message: "`.unwrap()` in production code — use `try` or `match` instead"
                        .to_string(),
                    location: LintLocation {
                        line,
                        column: col,
                        source_line,
                    },
                    fix: "replace with `try expr` to propagate, or `match` to handle".to_string(),
                });
            }
            walk_expr_for_unwrap(object, source, diags);
            for arg in args {
                walk_expr_for_unwrap(&arg.expr, source, diags);
            }
        }
        ExprKind::Call { func, args } => {
            walk_expr_for_unwrap(func, source, diags);
            for arg in args {
                walk_expr_for_unwrap(&arg.expr, source, diags);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            walk_expr_for_unwrap(left, source, diags);
            walk_expr_for_unwrap(right, source, diags);
        }
        ExprKind::Unary { operand, .. } => {
            walk_expr_for_unwrap(operand, source, diags);
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_expr_for_unwrap(cond, source, diags);
            walk_expr_for_unwrap(then_branch, source, diags);
            if let Some(e) = else_branch {
                walk_expr_for_unwrap(e, source, diags);
            }
        }
        ExprKind::IfLet {
            expr: scrutinee,
            then_branch,
            else_branch,
            ..
        } => {
            walk_expr_for_unwrap(scrutinee, source, diags);
            walk_expr_for_unwrap(then_branch, source, diags);
            if let Some(e) = else_branch {
                walk_expr_for_unwrap(e, source, diags);
            }
        }
        ExprKind::IsPattern { expr, .. } => {
            walk_expr_for_unwrap(expr, source, diags);
        }
        ExprKind::Match { scrutinee, arms } => {
            walk_expr_for_unwrap(scrutinee, source, diags);
            for arm in arms {
                walk_expr_for_unwrap(&arm.body, source, diags);
            }
        }
        ExprKind::Block(stmts)
        | ExprKind::UsingBlock { body: stmts, .. }
        | ExprKind::Spawn { body: stmts }
        | ExprKind::Unsafe { body: stmts }
        | ExprKind::Comptime { body: stmts }
        | ExprKind::BlockCall { body: stmts, .. } => {
            walk_stmts_for_unwrap(stmts, source, diags);
        }
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            walk_expr_for_unwrap(object, source, diags);
        }
        ExprKind::Index { object, index } => {
            walk_expr_for_unwrap(object, source, diags);
            walk_expr_for_unwrap(index, source, diags);
        }
        ExprKind::Try(inner) | ExprKind::Unwrap(inner) | ExprKind::Cast { expr: inner, .. } => {
            walk_expr_for_unwrap(inner, source, diags);
        }
        ExprKind::NullCoalesce { value, default } => {
            walk_expr_for_unwrap(value, source, diags);
            walk_expr_for_unwrap(default, source, diags);
        }
        ExprKind::Closure { body, .. } => {
            walk_expr_for_unwrap(body, source, diags);
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                walk_expr_for_unwrap(item, source, diags);
            }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                walk_expr_for_unwrap(&f.value, source, diags);
            }
            if let Some(s) = spread {
                walk_expr_for_unwrap(s, source, diags);
            }
        }
        _ => {}
    }
}

/// idiom/missing-ensure: Flag @resource struct types created without ensure.
pub fn check_missing_ensure(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut resource_types: Vec<String> = Vec::new();
    for decl in decls {
        if let DeclKind::Struct(s) = &decl.kind {
            if s.attrs.iter().any(|a| a == "resource") {
                resource_types.push(s.name.clone());
            }
        }
    }

    if resource_types.is_empty() {
        return Vec::new();
    }

    let mut diags = Vec::new();

    for decl in decls {
        let body = match &decl.kind {
            DeclKind::Fn(f) => &f.body,
            _ => continue,
        };

        let has_ensure = body
            .iter()
            .any(|s| matches!(&s.kind, StmtKind::Ensure { .. }));

        for stmt in body {
            check_stmt_for_resource(stmt, &resource_types, has_ensure, source, &mut diags);
        }
    }

    diags
}

fn check_stmt_for_resource(
    stmt: &Stmt,
    resource_types: &[String],
    has_ensure: bool,
    source: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    match &stmt.kind {
        StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
            check_expr_for_resource(init, resource_types, has_ensure, source, diags);
        }
        StmtKind::Expr(expr) => {
            check_expr_for_resource(expr, resource_types, has_ensure, source, diags);
        }
        _ => {}
    }
}

fn check_expr_for_resource(
    expr: &Expr,
    resource_types: &[String],
    has_ensure: bool,
    source: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    if let ExprKind::StructLit { name, .. } = &expr.kind {
        if resource_types.contains(name) && !has_ensure {
            let (line, col) = util::line_col(source, expr.span.start);
            let source_line = util::get_source_line(source, line);
            diags.push(LintDiagnostic {
                rule: "idiom/missing-ensure".to_string(),
                severity: Severity::Warning,
                message: format!(
                    "`{}` is a `@resource` type — add `ensure` for cleanup",
                    name
                ),
                location: LintLocation {
                    line,
                    column: col,
                    source_line,
                },
                fix: format!("add `ensure {}.close()` after creation", name.to_lowercase()),
            });
        }
    }
}

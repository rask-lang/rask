// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Idiomatic pattern rules.
//!
//! - unwrap-production: Flag .unwrap() outside test blocks
//! - missing-ensure: Flag @resource creation without ensure
//! - ensure-ordering: Flag ensure registration order that doesn't match acquisition order

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
        ExprKind::Try(inner) | ExprKind::Unwrap { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => {
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

/// idiom/ensure-ordering: Flag ensure registration order that doesn't match
/// variable acquisition order. LIFO execution means misordered ensures clean
/// up resources in the wrong sequence.
///
/// Good (LIFO gives correct cleanup):
///   const a = open("a")
///   ensure a.close()       // registered 1st → runs LAST
///   const b = open("b")
///   ensure b.close()       // registered 2nd → runs FIRST
///
/// Bad (LIFO gives reversed cleanup):
///   const a = open("a")
///   const b = open("b")
///   ensure b.close()       // registered 1st → runs LAST  (b closed after a!)
///   ensure a.close()       // registered 2nd → runs FIRST (a closed before b!)
pub fn check_ensure_ordering(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        let body = match &decl.kind {
            DeclKind::Fn(f) => &f.body,
            _ => continue,
        };
        check_ensure_ordering_in_block(body, source, &mut diags);
    }

    diags
}

fn check_ensure_ordering_in_block(
    stmts: &[Stmt],
    source: &str,
    diags: &mut Vec<LintDiagnostic>,
) {
    // Track binding order: variable name → position index
    let mut binding_order: Vec<String> = Vec::new();
    // Track ensure receivers in registration order
    let mut ensure_receivers: Vec<(String, &Stmt)> = Vec::new();

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Const { name, .. } | StmtKind::Let { name, .. } => {
                binding_order.push(name.clone());
            }
            StmtKind::Ensure { body, .. } => {
                if let Some(receiver) = extract_ensure_receiver(body) {
                    ensure_receivers.push((receiver, stmt));
                }
            }
            // Recurse into nested blocks
            StmtKind::While { body, .. }
            | StmtKind::WhileLet { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::Loop { body, .. } => {
                check_ensure_ordering_in_block(body, source, diags);
            }
            StmtKind::Expr(expr) => {
                recurse_ensure_ordering_in_expr(expr, source, diags);
            }
            _ => {}
        }
    }

    // Check: ensure receivers' binding positions must be non-decreasing.
    // If receiver B (bound later) has its ensure registered before receiver A
    // (bound earlier), LIFO will clean up A first — wrong.
    if ensure_receivers.len() < 2 {
        return;
    }

    let positions: Vec<Option<usize>> = ensure_receivers
        .iter()
        .map(|(name, _)| binding_order.iter().position(|b| b == name))
        .collect();

    let mut prev_pos: Option<usize> = None;
    for (i, pos) in positions.iter().enumerate() {
        let Some(current) = pos else { continue };
        if let Some(prev) = prev_pos {
            if *current < prev {
                // Out of order: this ensure's variable was bound earlier
                // than the previous ensure's variable, but registered later.
                let (prev_name, _) = &ensure_receivers[i - 1];
                let (curr_name, curr_stmt) = &ensure_receivers[i];

                let (line, col) = util::line_col(source, curr_stmt.span.start);
                let source_line = util::get_source_line(source, line);
                diags.push(LintDiagnostic {
                    rule: "idiom/ensure-ordering".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "`ensure {curr_name}...` registered after `ensure {prev_name}...`, \
                         but `{curr_name}` was created first — \
                         LIFO will close `{curr_name}` before `{prev_name}` (ctrl.ensure/EN2)"
                    ),
                    location: LintLocation {
                        line,
                        column: col,
                        source_line,
                    },
                    fix: "reorder ensures to match acquisition order, or interleave: \
                          acquire → ensure → acquire → ensure"
                        .to_string(),
                });
            }
        }
        prev_pos = Some(*current);
    }
}

fn recurse_ensure_ordering_in_expr(expr: &Expr, source: &str, diags: &mut Vec<LintDiagnostic>) {
    match &expr.kind {
        ExprKind::Block(stmts)
        | ExprKind::UsingBlock { body: stmts, .. }
        | ExprKind::Spawn { body: stmts }
        | ExprKind::Unsafe { body: stmts }
        | ExprKind::Comptime { body: stmts }
        | ExprKind::BlockCall { body: stmts, .. } => {
            check_ensure_ordering_in_block(stmts, source, diags);
        }
        _ => {}
    }
}

/// Extract the receiver variable name from an ensure body.
/// Handles `ensure x.method()` and `ensure x.method(args)`.
/// Returns None for complex expressions we can't analyze.
fn extract_ensure_receiver(body: &[Stmt]) -> Option<String> {
    if body.len() != 1 {
        return None;
    }
    let expr = match &body[0].kind {
        StmtKind::Expr(e) => e,
        _ => return None,
    };
    match &expr.kind {
        ExprKind::MethodCall { object, .. } => match &object.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            _ => None,
        },
        ExprKind::Call { func, .. } => match &func.kind {
            ExprKind::Field { object, .. } => match &object.kind {
                ExprKind::Ident(name) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

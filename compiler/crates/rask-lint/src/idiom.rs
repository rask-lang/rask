// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Idiomatic pattern rules.
//!
//! - unwrap-production: Flag .unwrap() outside test blocks
//! - missing-ensure: Flag @resource creation without ensure
//! - ensure-ordering: Flag ensure registration order that doesn't match acquisition order
//! - duck-trait: Flag `duck trait` declarations — sketching tool, nudge to harden

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
        StmtKind::Mut { init, .. }
        | StmtKind::Let { init, .. }
        | StmtKind::Break { value: Some(init), .. } => {
            walk_expr_for_unwrap(init, source, diags);
        }
        StmtKind::MutTuple { init, .. } | StmtKind::LetTuple { init, .. } => {
            walk_expr_for_unwrap(init, source, diags);
        }
        StmtKind::Return(Some(e)) => walk_expr_for_unwrap(e, source, diags),
        StmtKind::Assign { target, value } => {
            walk_expr_for_unwrap(target, source, diags);
            walk_expr_for_unwrap(value, source, diags);
        }
        StmtKind::While { cond, body, .. } => {
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
            ..
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
        | ExprKind::BlockCall { body: stmts, .. }
        | ExprKind::Loop { body: stmts, .. } => {
            walk_stmts_for_unwrap(stmts, source, diags);
        }
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            walk_expr_for_unwrap(object, source, diags);
        }
        ExprKind::DynamicField { object, field_expr } => {
            walk_expr_for_unwrap(object, source, diags);
            walk_expr_for_unwrap(field_expr, source, diags);
        }
        ExprKind::Index { object, index } => {
            walk_expr_for_unwrap(object, source, diags);
            walk_expr_for_unwrap(index, source, diags);
        }
        ExprKind::Try { expr: inner } | ExprKind::Take { place: inner } => {
            walk_expr_for_unwrap(inner, source, diags);
        }
        ExprKind::Catch { value, clause } => {
            walk_expr_for_unwrap(value, source, diags);
            walk_expr_for_unwrap(&clause.body, source, diags);
        }
        ExprKind::IsPresent { expr: inner, .. } => {
            walk_expr_for_unwrap(inner, source, diags);
        }
        ExprKind::Unwrap { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => {
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
        StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
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

/// idiom/ensure-ordering: cleanup order that tears a dependency down too early.
///
/// `ensure` bodies run LIFO, so a resource derived from another has its
/// `ensure` registered *second*:
///
///   let w = make_world()
///   ensure w.destroy()          // registered 1st -> runs LAST
///   let b = make_body(w)
///   ensure b.close(w)           // registered 2nd -> runs FIRST
///
/// Swap the two and `b.close(w)` runs against a destroyed world.
///
/// This used to compare *creation order* as a proxy for derivation, which
/// flagged two unrelated resources whose order genuinely doesn't matter — a
/// world and a log file cleaned up in either sequence is fine, and the lint
/// called it an error. It now shares its evidence with the W10 compiler warning
/// (`rask_effects::ensure_order`, mem.resource-types/EO1) so `rask lint` and
/// `rask check` can't disagree about the same two lines (#584).
pub fn check_ensure_ordering(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    rask_effects::ensure_order::check(decls)
        .into_iter()
        .map(|w| {
            let (line, col) = util::line_col(source, w.span.start);
            let source_line = util::get_source_line(source, line);
            LintDiagnostic {
                rule: "idiom/ensure-ordering".to_string(),
                severity: Severity::Error,
                message: format!(
                    "`{}` is cleaned up before `{}`, which needs it — \
                     `ensure` runs LIFO, so the dependency's ensure comes first \
                     (mem.resource-types/EO1)",
                    w.dependency, w.dependent
                ),
                location: LintLocation { line, column: col, source_line },
                fix: w.fixed_order.replace('\n', " then "),
            }
        })
        .collect()
}

/// idiom/large-unsafe-block: Flag unsafe blocks with too many statements.
/// Big unsafe blocks defeat the purpose — keep them minimal so each unsafe
/// operation is visible and auditable (mem.unsafe/U4).
pub fn check_large_unsafe_blocks(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    const MAX_STMTS: usize = 10;
    let mut diags = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => walk_for_large_unsafe(&f.body, source, MAX_STMTS, &mut diags),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    walk_for_large_unsafe(&m.body, source, MAX_STMTS, &mut diags);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    walk_for_large_unsafe(&m.body, source, MAX_STMTS, &mut diags);
                }
            }
            DeclKind::Impl(imp) => {
                for m in &imp.methods {
                    walk_for_large_unsafe(&m.body, source, MAX_STMTS, &mut diags);
                }
            }
            DeclKind::Test(t) => walk_for_large_unsafe(&t.body, source, MAX_STMTS, &mut diags),
            _ => {}
        }
    }

    diags
}

fn walk_for_large_unsafe(stmts: &[Stmt], source: &str, max: usize, diags: &mut Vec<LintDiagnostic>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expr(e) => check_expr_for_large_unsafe(e, source, max, diags),
            StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
                check_expr_for_large_unsafe(init, source, max, diags);
            }
            StmtKind::While { cond, body, .. } => {
                check_expr_for_large_unsafe(cond, source, max, diags);
                walk_for_large_unsafe(body, source, max, diags);
            }
            StmtKind::For { iter, body, .. } => {
                check_expr_for_large_unsafe(iter, source, max, diags);
                walk_for_large_unsafe(body, source, max, diags);
            }
            StmtKind::Loop { body, .. } => walk_for_large_unsafe(body, source, max, diags),
            _ => {}
        }
    }
}

fn check_expr_for_large_unsafe(expr: &Expr, source: &str, max: usize, diags: &mut Vec<LintDiagnostic>) {
    match &expr.kind {
        ExprKind::Unsafe { body } => {
            if body.len() > max {
                let (line, col) = util::line_col(source, expr.span.start);
                let source_line = util::get_source_line(source, line);
                diags.push(LintDiagnostic {
                    rule: "idiom/large-unsafe-block".to_string(),
                    severity: Severity::Warning,
                    message: format!(
                        "unsafe block has {} statements — keep unsafe blocks minimal (mem.unsafe/U4)",
                        body.len()
                    ),
                    location: LintLocation {
                        line,
                        column: col,
                        source_line,
                    },
                    fix: "extract operations into small safe wrapper functions".to_string(),
                });
            }
            // Still recurse into the body for nested unsafe blocks
            walk_for_large_unsafe(body, source, max, diags);
        }
        ExprKind::Block(stmts)
        | ExprKind::UsingBlock { body: stmts, .. }
        | ExprKind::Spawn { body: stmts }
        | ExprKind::Comptime { body: stmts }
        | ExprKind::BlockCall { body: stmts, .. }
        | ExprKind::Loop { body: stmts, .. } => {
            walk_for_large_unsafe(stmts, source, max, diags);
        }
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            check_expr_for_large_unsafe(cond, source, max, diags);
            check_expr_for_large_unsafe(then_branch, source, max, diags);
            if let Some(e) = else_branch {
                check_expr_for_large_unsafe(e, source, max, diags);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            check_expr_for_large_unsafe(scrutinee, source, max, diags);
            for arm in arms {
                check_expr_for_large_unsafe(&arm.body, source, max, diags);
            }
        }
        ExprKind::Closure { body, .. } => {
            check_expr_for_large_unsafe(body, source, max, diags);
        }
        _ => {}
    }
}

/// idiom/duck-trait: Flag `duck trait` declarations (DT3).
///
/// Shape-matching is for code you're still sketching: nothing states the
/// contract, so a type can start or stop matching silently. A warning, not a
/// gate — DT1 already keeps duck traits out of the public API.
pub fn check_duck_trait(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        let DeclKind::Trait(t) = &decl.kind else { continue };
        if !t.is_duck {
            continue;
        }
        // SU1: an intentionally-kept sketch can opt out
        if t.attrs.iter().any(|a| a == "allow(idiom/duck-trait)") {
            continue;
        }
        let (line, col) = util::line_col(source, decl.span.start);
        let source_line = util::get_source_line(source, line);
        diags.push(LintDiagnostic {
            rule: "idiom/duck-trait".to_string(),
            severity: Severity::Warning,
            message: format!(
                "`{}` is a duck trait — matched by shape, with no conformance declared anywhere",
                t.name
            ),
            location: LintLocation {
                line,
                column: col,
                source_line,
            },
            fix: format!(
                "delete `duck` and declare conformance (`extend Type with {} {{}}`) on each matching type, or `@allow(idiom/duck-trait)` to keep the sketch",
                t.name
            ),
        });
    }

    diags
}

/// I5: `x == none` / `x != none`. Equality on a zero-field type is ordinary,
/// so this type-checks — but it asks a *shape* question with the *value* verb.
/// `is` tests a branch everywhere else in the language (`type.optionals/OPT15`).
pub fn check_equality_absent_check(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for decl in decls {
        for body in decl_bodies(decl) {
            walk_stmts_for_none_eq(body, source, &mut diags);
        }
    }
    diags
}

/// Every statement list a declaration owns.
fn decl_bodies(decl: &Decl) -> Vec<&[Stmt]> {
    match &decl.kind {
        DeclKind::Fn(f) => vec![f.body.as_slice()],
        DeclKind::Test(t) => vec![t.body.as_slice()],
        DeclKind::Benchmark(b) => vec![b.body.as_slice()],
        DeclKind::Struct(s) => s.methods.iter().map(|m| m.body.as_slice()).collect(),
        DeclKind::Enum(e) => e.methods.iter().map(|m| m.body.as_slice()).collect(),
        DeclKind::Impl(i) => i.methods.iter().map(|m| m.body.as_slice()).collect(),
        _ => Vec::new(),
    }
}

fn walk_stmts_for_none_eq(stmts: &[Stmt], source: &str, diags: &mut Vec<LintDiagnostic>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { init, .. } | StmtKind::Mut { init, .. } => {
                check_expr_for_none_eq(init, source, diags)
            }
            StmtKind::LetTuple { init, .. } => check_expr_for_none_eq(init, source, diags),
            StmtKind::Expr(e) => check_expr_for_none_eq(e, source, diags),
            StmtKind::Return(Some(e)) => check_expr_for_none_eq(e, source, diags),
            StmtKind::Assign { target, value } => {
                check_expr_for_none_eq(target, source, diags);
                check_expr_for_none_eq(value, source, diags);
            }
            StmtKind::While { cond, body, .. } => {
                check_expr_for_none_eq(cond, source, diags);
                walk_stmts_for_none_eq(body, source, diags);
            }
            StmtKind::For { iter, body, .. } => {
                check_expr_for_none_eq(iter, source, diags);
                walk_stmts_for_none_eq(body, source, diags);
            }
            StmtKind::Loop { body, .. } => walk_stmts_for_none_eq(body, source, diags),
            StmtKind::Ensure { body, .. } => walk_stmts_for_none_eq(body, source, diags),
            _ => {}
        }
    }
}

fn check_expr_for_none_eq(expr: &Expr, source: &str, diags: &mut Vec<LintDiagnostic>) {
    use rask_ast::expr::BinOp;
    // Descend first, so a comparison nested in a condition is still reached.
    match &expr.kind {
        ExprKind::Binary { left, right, .. } => {
            check_expr_for_none_eq(left, source, diags);
            check_expr_for_none_eq(right, source, diags);
        }
        ExprKind::Unary { operand, .. } => check_expr_for_none_eq(operand, source, diags),
        ExprKind::Block(stmts) => walk_stmts_for_none_eq(stmts, source, diags),
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            check_expr_for_none_eq(cond, source, diags);
            check_expr_for_none_eq(then_branch, source, diags);
            if let Some(e) = else_branch {
                check_expr_for_none_eq(e, source, diags);
            }
        }
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
            for a in args {
                check_expr_for_none_eq(&a.expr, source, diags);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            check_expr_for_none_eq(scrutinee, source, diags);
            for arm in arms {
                check_expr_for_none_eq(&arm.body, source, diags);
            }
        }
        ExprKind::Assert { condition, .. } | ExprKind::Check { condition, .. } => {
            check_expr_for_none_eq(condition, source, diags)
        }
        _ => {}
    }

    let (op, left, right) = match &expr.kind {
        ExprKind::Binary { op, left, right } => (*op, left, right),
        _ => return,
    };
    if !matches!(op, BinOp::Eq | BinOp::Ne) {
        return;
    }
    if !matches!(left.kind, ExprKind::None) && !matches!(right.kind, ExprKind::None) {
        return;
    }
    // `none == none` is a constant, not an absent check.
    if matches!(left.kind, ExprKind::None) && matches!(right.kind, ExprKind::None) {
        return;
    }
    let (line, col) = util::line_col(source, expr.span.start);
    let source_line = util::get_source_line(source, line);
    let (message, fix) = if matches!(op, BinOp::Eq) {
        (
            "`== none` asks a branch question with the equality verb",
            "write `x is none` — the same `is` test the rest of the language uses",
        )
    } else {
        (
            "`!= none` is the presence test spelled the long way",
            "write `x?`",
        )
    };
    diags.push(LintDiagnostic {
        rule: "idiom/equality-absent-check".to_string(),
        severity: Severity::Warning,
        message: message.to_string(),
        location: LintLocation { line, column: col, source_line },
        fix: fix.to_string(),
    });
}

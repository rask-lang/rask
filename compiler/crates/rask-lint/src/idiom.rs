// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Idiomatic pattern rules.
//!
//! - unwrap-production: Flag .unwrap() outside test blocks
//! - missing-ensure: Flag @resource creation without ensure
//! - duck-interface: Flag `duck interface` declarations — sketching tool, nudge to harden

use rask_ast::decl::*;
use rask_ast::expr::{BinOp, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::types::*;
use crate::util;

/// idiom/unwrap-production: Flag .unwrap() calls outside test/benchmark blocks.
pub fn check_unwrap_production(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for decl in decls {
        // `.unwrap()` in a test is the test asserting it can't fail.
        if matches!(decl.kind, DeclKind::Test(_) | DeclKind::Benchmark(_)) {
            continue;
        }
        rask_ast::visit::walk_decl(decl, &mut |expr| {
            if let Some(d) = unwrap_diag(expr, source) {
                diags.push(d);
            }
        });
    }
    diags
}

fn unwrap_diag(expr: &Expr, source: &str) -> Option<LintDiagnostic> {
    let ExprKind::MethodCall { method, .. } = &expr.kind else {
        return None;
    };
    if method != "unwrap" {
        return None;
    }
    let (line, col) = util::line_col(source, expr.span.start);
    let source_line = util::get_source_line(source, line);
    Some(LintDiagnostic {
        rule: "idiom/unwrap-production".to_string(),
        severity: Severity::Warning,
        message: "`.unwrap()` in production code — use `try` or `match` instead".to_string(),
        location: LintLocation { line, column: col, source_line },
        fix: "replace with `try expr` to propagate, or `match` to handle".to_string(),
    })
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

///

/// Every function declaration in a decl — free functions, methods, tests.
fn decl_fns(decl: &Decl) -> Vec<&rask_ast::decl::FnDecl> {
    match &decl.kind {
        DeclKind::Fn(f) => vec![f],
        DeclKind::Struct(s) => s.methods.iter().collect(),
        DeclKind::Enum(e) => e.methods.iter().collect(),
        DeclKind::Impl(i) => i.methods.iter().collect(),
        _ => Vec::new(),
    }
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

/// idiom/duck-interface: Flag `duck interface` declarations (DT3).
///
/// Shape-matching is for code you're still sketching: nothing states the
/// contract, so a type can start or stop matching silently. A warning, not a
/// gate — DT1 already keeps duck interfaces out of the public API.
pub fn check_duck_interface(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        let DeclKind::Interface(t) = &decl.kind else { continue };
        if !t.is_duck {
            continue;
        }
        let (line, col) = util::line_col(source, decl.span.start);
        let source_line = util::get_source_line(source, line);
        diags.push(LintDiagnostic {
            rule: "idiom/duck-interface".to_string(),
            severity: Severity::Warning,
            message: format!(
                "`{}` is a duck interface — matched by shape, with no conformance declared anywhere",
                t.name
            ),
            location: LintLocation {
                line,
                column: col,
                source_line,
            },
            fix: format!(
                "delete `duck` and declare conformance (`Type implements {} {{}}`) on each matching type, or `@allow(idiom/duck-interface)` to keep the sketch",
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
    rask_ast::visit::walk_decls(decls, &mut |expr| {
        if let Some(d) = none_eq_diag(expr, source) {
            diags.push(d);
        }
    });
    diags
}

fn none_eq_diag(expr: &Expr, source: &str) -> Option<LintDiagnostic> {
    let ExprKind::Binary { op, left, right } = &expr.kind else {
        return None;
    };
    if !matches!(op, BinOp::Eq | BinOp::Ne) {
        return None;
    }
    let left_none = matches!(left.kind, ExprKind::None);
    let right_none = matches!(right.kind, ExprKind::None);
    // `none == none` is a constant, not an absent check.
    if left_none == right_none {
        return None;
    }
    let (line, col) = util::line_col(source, expr.span.start);
    let source_line = util::get_source_line(source, line);
    let (message, fix) = if matches!(op, BinOp::Eq) {
        (
            "`== none` asks a branch question with the equality verb",
            "write `x is none` — the same `is` test the rest of the language uses",
        )
    } else {
        ("`!= none` is the presence test spelled the long way", "write `x?`")
    };
    Some(LintDiagnostic {
        rule: "idiom/equality-absent-check".to_string(),
        severity: Severity::Warning,
        message: message.to_string(),
        location: LintLocation { line, column: col, source_line },
        fix: fix.to_string(),
    })
}


/// idiom/mod-for-index: `%` producing an index, where a negative left operand
/// would produce a negative index.
///
/// `%` takes the dividend's sign (type.operators/AR2), so `(i - 1) % n` is `-1`
/// when `i` is 0 — and indexing with it panics rather than wrapping to the end
/// of the buffer, which is what the code meant. `.mod(n)` is the floored answer
/// (AR3), always in range.
///
/// Deliberately narrow: only where the remainder is the *index* of a `[…]`
/// access, and only when the left operand isn't obviously non-negative. A `%`
/// whose result is a value rather than an index is usually exactly what was
/// wanted, and flagging those would drown the case that isn't.
pub fn check_mod_for_index(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    rask_ast::visit::walk_decls(decls, &mut |expr| {
        if let Some(d) = mod_index_diag(expr, source) {
            diags.push(d);
        }
    });
    diags
}

fn mod_index_diag(expr: &Expr, source: &str) -> Option<LintDiagnostic> {
    let ExprKind::Index { object, index } = &expr.kind else {
        return None;
    };
    let ExprKind::Binary { op: BinOp::Mod, left, right } = &index.kind else {
        return None;
    };
    if is_obviously_non_negative(left) {
        return None;
    }
    let (line, col) = util::line_col(source, index.span.start);
    let source_line = util::get_source_line(source, line);
    let container = expr_text(object).unwrap_or_else(|| "…".to_string());
    // A compound left operand keeps its parens in both the message and the
    // fix: `i - 1.mod(n)` parses as `i - (1.mod(n))`, so a fix printed without
    // them is wrong code.
    let lhs = expr_text_grouped(left).unwrap_or_else(|| "i".to_string());
    let rhs = expr_text(right).unwrap_or_else(|| "n".to_string());
    Some(LintDiagnostic {
        rule: "idiom/mod-for-index".to_string(),
        severity: Severity::Warning,
        message: format!(
            "`{lhs} % {rhs}` is negative when `{lhs}` is — `%` takes the dividend's sign, \
             so this indexes out of range instead of wrapping (type.operators/AR2)"
        ),
        location: LintLocation { line, column: col, source_line },
        fix: format!("{container}[{lhs}.mod({rhs})]"),
    })
}


/// Left operands that can't be negative, so `%` on them is already in range.
///
/// A non-negative literal, and a `.len()`/`.count()` call — the two shapes that
/// make up most correct `%`-as-index code. Anything else is left to the warning,
/// which is a suggestion rather than an error.
fn is_obviously_non_negative(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(v, _) => *v >= 0,
        ExprKind::MethodCall { method, .. } => matches!(method.as_str(), "len" | "count"),
        ExprKind::Binary { op: BinOp::Add | BinOp::Mul, left, right } => {
            is_obviously_non_negative(left) && is_obviously_non_negative(right)
        }
        _ => false,
    }
}

/// Like `expr_text`, but parenthesized when it's a compound expression, so it
/// can sit to the left of a `.` or an operator without regrouping.
fn expr_text_grouped(e: &Expr) -> Option<String> {
    let text = expr_text(e)?;
    Some(match &e.kind {
        ExprKind::Binary { .. } => format!("({})", text),
        _ => text,
    })
}

/// An expression as the reader wrote it, for the message and the fix.
fn expr_text(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Int(v, _) => Some(v.to_string()),
        ExprKind::Field { object, field } => Some(format!("{}.{}", expr_text(object)?, field)),
        ExprKind::Binary { op, left, right } => Some(format!(
            "{} {} {}",
            expr_text(left)?,
            binop_text(op)?,
            expr_text(right)?
        )),
        _ => None,
    }
}

fn binop_text(op: &BinOp) -> Option<&'static str> {
    Some(match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        _ => return None,
    })
}

/// idiom/match-on-optional: a two-arm `match` where one arm is `none`.
///
/// `type.optionals/OPT27` — legal, and shorter with the operators. Match on a
/// two-arm union is perfectly safe (exhaustiveness still names the missing
/// `none`), so this is a lint rather than a warning: `tool.warnings/SB3`,
/// "lints enforce how code should look".
///
/// Syntactic, and sound that way: `Pattern::TypePat` with the name `none`
/// parses only from the `none` keyword, so an arm spelled that way means the
/// scrutinee has a `none` branch.
pub fn check_match_on_optional(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    rask_ast::visit::walk_decls(decls, &mut |expr| {
        if let Some(d) = optional_match_diag(expr, source) {
            diags.push(d);
        }
    });
    diags
}

fn is_none_pattern(p: &rask_ast::expr::Pattern) -> bool {
    matches!(
        p,
        rask_ast::expr::Pattern::TypePat { ty_name, binding: None } if ty_name == "none"
    )
}

fn optional_match_diag(expr: &Expr, source: &str) -> Option<LintDiagnostic> {
    let ExprKind::Match { arms, .. } = &expr.kind else {
        return None;
    };
    if arms.len() != 2 || !arms.iter().any(|a| is_none_pattern(&a.pattern)) {
        return None;
    }
    // A guard changes what the arms mean and no operator form carries one.
    if arms.iter().any(|a| a.guard.is_some()) {
        return None;
    }
    let (line, col) = util::line_col(source, expr.span.start);
    let source_line = util::get_source_line(source, line);
    Some(LintDiagnostic {
        rule: "idiom/match-on-optional".to_string(),
        severity: Severity::Warning,
        message: "two-arm `match` with a `none` arm — the `?` operators say this in one line"
            .to_string(),
        location: LintLocation { line, column: col, source_line },
        fix: "`if x? as v { … } else { … }` to branch, `x ?? value` for a fallback, \
              `x ?? return` to leave [type.optionals/OPT27]"
            .to_string(),
    })
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Capability inference from import graph — PM1-PM8.
//!
//! Scans a package's AST for imports, extern declarations,
//! and unsafe blocks to infer required capabilities.
//!
//! Capabilities:
//!   net   — io.net, http.*
//!   read  — io.fs (read operations)
//!   write — io.fs (write operations)
//!   exec  — os.exec, os.process
//!   ffi   — unsafe blocks, extern declarations

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use std::collections::BTreeSet;

/// Known import prefixes and their required capabilities.
const CAPABILITY_MAP: &[(&str, &str)] = &[
    ("io.net", "net"),
    ("http", "net"),
    ("io.fs", "read"),
    ("os.exec", "exec"),
    ("os.process", "exec"),
];

/// Infer capabilities from a list of declarations (one package's AST).
pub fn infer_capabilities(decls: &[Decl]) -> Vec<String> {
    let mut caps = BTreeSet::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Import(import) => {
                let path = import.path.join(".");
                for &(prefix, cap) in CAPABILITY_MAP {
                    if path.starts_with(prefix) {
                        caps.insert(cap.to_string());
                    }
                }
                if path.starts_with("io.fs") {
                    caps.insert("write".to_string());
                }
            }
            DeclKind::Extern(_) => {
                caps.insert("ffi".to_string());
            }
            DeclKind::Fn(f) => {
                if f.is_unsafe {
                    caps.insert("ffi".to_string());
                }
                if has_unsafe_in_stmts(&f.body) {
                    caps.insert("ffi".to_string());
                }
            }
            DeclKind::Impl(impl_decl) => {
                // methods is Vec<FnDecl> directly
                for method in &impl_decl.methods {
                    if method.is_unsafe || has_unsafe_in_stmts(&method.body) {
                        caps.insert("ffi".to_string());
                    }
                }
            }
            _ => {}
        }
    }

    caps.into_iter().collect()
}

/// Check if any statement contains an unsafe block.
fn has_unsafe_in_stmts(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| has_unsafe_in_stmt(stmt))
}

fn has_unsafe_in_stmt(stmt: &Stmt) -> bool {
    match &stmt.kind {
        StmtKind::Expr(expr) => has_unsafe_in_expr(expr),
        StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
            has_unsafe_in_expr(init)
        }
        StmtKind::Assign { target, value } => {
            has_unsafe_in_expr(target) || has_unsafe_in_expr(value)
        }
        StmtKind::Return(Some(expr)) => has_unsafe_in_expr(expr),
        StmtKind::While { cond, body } => {
            has_unsafe_in_expr(cond) || has_unsafe_in_stmts(body)
        }
        StmtKind::For { iter, body, .. } => {
            has_unsafe_in_expr(iter) || has_unsafe_in_stmts(body)
        }
        StmtKind::Loop { body, .. } => has_unsafe_in_stmts(body),
        StmtKind::Ensure { body, .. } => has_unsafe_in_stmts(body),
        StmtKind::Comptime(stmts) => has_unsafe_in_stmts(stmts),
        _ => false,
    }
}

fn has_unsafe_in_expr(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Unsafe { .. } => true,
        ExprKind::Block(stmts) => has_unsafe_in_stmts(stmts),
        ExprKind::Call { func, args } => {
            has_unsafe_in_expr(func) || args.iter().any(|a| has_unsafe_in_expr(&a.expr))
        }
        ExprKind::MethodCall { object, args, .. } => {
            has_unsafe_in_expr(object) || args.iter().any(|a| has_unsafe_in_expr(&a.expr))
        }
        ExprKind::Binary { left, right, .. } => {
            has_unsafe_in_expr(left) || has_unsafe_in_expr(right)
        }
        ExprKind::Unary { operand, .. } => has_unsafe_in_expr(operand),
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            has_unsafe_in_expr(cond)
                || has_unsafe_in_expr(then_branch)
                || else_branch.as_ref().map_or(false, |e| has_unsafe_in_expr(e))
        }
        ExprKind::Match { scrutinee, arms } => {
            has_unsafe_in_expr(scrutinee)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().map_or(false, |g| has_unsafe_in_expr(g))
                        || has_unsafe_in_expr(&arm.body)
                })
        }
        ExprKind::Field { object, .. } => has_unsafe_in_expr(object),
        ExprKind::Index { object, index } => {
            has_unsafe_in_expr(object) || has_unsafe_in_expr(index)
        }
        ExprKind::Closure { body, .. } => has_unsafe_in_expr(body),
        ExprKind::Tuple(exprs) | ExprKind::Array(exprs) => {
            exprs.iter().any(|e| has_unsafe_in_expr(e))
        }
        ExprKind::StructLit { fields, .. } => {
            fields.iter().any(|f| has_unsafe_in_expr(&f.value))
        }
        _ => false,
    }
}

/// Check that a package's inferred capabilities are covered by allowed caps.
pub fn check_capabilities(
    inferred: &[String],
    allowed: &[String],
) -> Vec<String> {
    inferred.iter()
        .filter(|cap| !allowed.contains(cap))
        .cloned()
        .collect()
}

/// Human-readable description of a capability for error messages.
pub fn capability_description(cap: &str) -> &'static str {
    match cap {
        "net" => "network access (io.net, http)",
        "read" => "file system read (io.fs)",
        "write" => "file system write (io.fs)",
        "exec" => "process execution (os.exec)",
        "ffi" => "foreign function interface (unsafe/extern)",
        _ => "unknown capability",
    }
}

/// Severity tier for a capability — determines prompt behavior.
pub fn capability_tier(cap: &str) -> u8 {
    match cap {
        "ffi" => 3,
        "write" | "exec" => 2,
        "net" | "read" => 2,
        _ => 1,
    }
}

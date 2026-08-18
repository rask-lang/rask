// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Cleanup order that inverts a derivation (mem.resource-types, EO1).
//!
//! `ensure` bodies run LIFO, so the *last* one registered runs first. When B is
//! derived from A — a physics body from a world, a statement from a connection —
//! B has to be cleaned up before A, which means A's `ensure` is registered
//! first. Register them the other way round and A is torn down while B is still
//! live; B's cleanup then calls into a destroyed dependency. At an FFI boundary
//! that's UB the language otherwise makes impossible, and nothing said a word
//! either way (#584).
//!
//! Detection is function-local and reads derivation off the calls themselves,
//! which is what keeps it cheap and predictable:
//!
//! - **Constructor evidence.** `let b = make_body(w)` — `w` is an argument to
//!   the call that produced `b`, so `b` depends on `w`.
//! - **Cleanup evidence.** `ensure b.close(w)` — `b`'s cleanup needs `w`, which
//!   says the same thing from the other end and catches the case where the
//!   constructor didn't mention it.
//!
//! Anything cleverer needs a false-positive budget first, and these two cover
//! the shape the stress test found.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;

/// A cleanup registered in an order that tears down a dependency too early.
#[derive(Debug, Clone)]
pub struct EnsureOrderWarning {
    /// The derived resource, cleaned up second.
    pub dependent: String,
    /// What it was derived from, cleaned up first.
    pub dependency: String,
    /// The `ensure` for the dependency — the one that has to move earlier.
    pub span: Span,
    /// The `ensure` for the dependent, for the message.
    pub dependent_span: Span,
    /// The two lines as they should read, for the FIX.
    pub fixed_order: String,
}

/// One `ensure` seen in a function body, in registration order.
struct Registered {
    /// The name whose cleanup this is — the receiver of the call.
    target: String,
    /// Names mentioned anywhere in the cleanup call, receiver excluded.
    mentions: Vec<String>,
    span: Span,
    /// The call as written, so the FIX can show the reordered lines.
    text: String,
}

pub fn check(decls: &[Decl]) -> Vec<EnsureOrderWarning> {
    let mut out = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => check_fn(f, &mut out),
            DeclKind::Struct(s) => for m in &s.methods { check_fn(m, &mut out) },
            DeclKind::Enum(e) => for m in &e.methods { check_fn(m, &mut out) },
            DeclKind::Impl(i) => for m in &i.methods { check_fn(m, &mut out) },
            _ => {}
        }
    }
    out
}

fn check_fn(f: &FnDecl, out: &mut Vec<EnsureOrderWarning>) {
    // name → names its initializer mentioned. `let b = make_body(w)` records
    // b → [w].
    let mut derived_from: HashMap<String, Vec<String>> = HashMap::new();
    let mut registered: Vec<Registered> = Vec::new();
    walk(&f.body, &mut derived_from, &mut registered);

    // LIFO: a later registration runs *first*. So the wrong order is the one
    // where the thing registered later is the dependency — it gets torn down
    // while something registered earlier still needs it.
    //
    // The right order reads backwards from how it runs, which is the whole
    // reason this is easy to get wrong: `ensure world.destroy()` goes *above*
    // `ensure body.close(world)`.
    for (i, earlier) in registered.iter().enumerate() {
        for later in &registered[i + 1..] {
            if !depends_on(&derived_from, &earlier.target, &later.target, earlier) {
                continue;
            }
            out.push(EnsureOrderWarning {
                dependent: earlier.target.clone(),
                dependency: later.target.clone(),
                span: later.span,
                dependent_span: earlier.span,
                fixed_order: format!("ensure {}\nensure {}", later.text, earlier.text),
            });
        }
    }
}

/// Does `dependent` rely on `dependency` still being alive?
fn depends_on(
    derived_from: &HashMap<String, Vec<String>>,
    dependent: &str,
    dependency: &str,
    cleanup: &Registered,
) -> bool {
    if dependent == dependency {
        return false;
    }
    // Constructor evidence.
    if derived_from
        .get(dependent)
        .is_some_and(|from| from.iter().any(|n| n == dependency))
    {
        return true;
    }
    // Cleanup evidence — the dependent's own cleanup takes the dependency.
    cleanup.mentions.iter().any(|n| n == dependency)
}

fn walk(
    stmts: &[Stmt],
    derived_from: &mut HashMap<String, Vec<String>>,
    registered: &mut Vec<Registered>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => {
                let mut names = Vec::new();
                collect_idents(init, &mut names);
                // A binding that just renames another isn't a derivation —
                // `let alias = w` would otherwise make `alias` depend on `w`
                // and warn about a cleanup nobody wrote.
                if !matches!(&init.kind, ExprKind::Ident(_)) {
                    derived_from.insert(name.clone(), names);
                }
            }
            StmtKind::Ensure { body, .. } => {
                // The registration order is what matters, and an `ensure` body
                // is a cleanup call — take the first one in it.
                if let Some(reg) = first_cleanup(body) {
                    registered.push(reg);
                }
            }
            // A nested scope registers its own ensures against its own bindings
            // and unwinds before this one does, so it's checked separately
            // rather than merged into this function's order.
            StmtKind::For { body, .. }
            | StmtKind::While { body, .. }
            | StmtKind::Loop { body, .. }
            | StmtKind::WhileLet { body, .. } => {
                walk(body, &mut derived_from.clone(), &mut Vec::new());
            }
            // `using Multitasking { … }` and the other block expressions are
            // where a server's resources actually live, so their ensures count
            // as this scope's — the block doesn't unwind separately.
            StmtKind::Expr(expr) => walk_expr_blocks(expr, derived_from, registered),
            _ => {}
        }
    }
}

/// Block-shaped expressions carry statements, and their `ensure`s belong to the
/// enclosing scope's order.
fn walk_expr_blocks(
    expr: &Expr,
    derived_from: &mut HashMap<String, Vec<String>>,
    registered: &mut Vec<Registered>,
) {
    match &expr.kind {
        ExprKind::Block(stmts)
        | ExprKind::UsingBlock { body: stmts, .. }
        | ExprKind::Unsafe { body: stmts }
        | ExprKind::Comptime { body: stmts }
        | ExprKind::BlockCall { body: stmts, .. } => walk(stmts, derived_from, registered),
        // A spawned body is a separate task with its own unwind, so its
        // registration order stands alone.
        ExprKind::Spawn { body: stmts } => {
            walk(stmts, &mut derived_from.clone(), &mut Vec::new())
        }
        ExprKind::Loop { body: stmts, .. } => {
            walk(stmts, &mut derived_from.clone(), &mut Vec::new())
        }
        _ => {}
    }
}

/// The first method call in an `ensure` body, as the thing being cleaned up.
fn first_cleanup(body: &[Stmt]) -> Option<Registered> {
    for stmt in body {
        let StmtKind::Expr(expr) = &stmt.kind else { continue };
        let ExprKind::MethodCall { object, method, args, .. } = &expr.kind else { continue };
        let ExprKind::Ident(target) = &object.kind else { continue };
        let mut mentions = Vec::new();
        for arg in args {
            collect_idents(&arg.expr, &mut mentions);
        }
        let arg_text: Vec<String> = args
            .iter()
            .map(|a| ident_text(&a.expr).unwrap_or_else(|| "…".to_string()))
            .collect();
        return Some(Registered {
            target: target.clone(),
            mentions,
            span: stmt.span,
            text: format!("{}.{}({})", target, method, arg_text.join(", ")),
        });
    }
    None
}

fn ident_text(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { object, field } => Some(format!("{}.{}", ident_text(object)?, field)),
        _ => None,
    }
}

/// Every plain name reachable in an expression. Deliberately shallow-ish — it
/// walks calls, fields and operators, which is where a derivation shows up.
fn collect_idents(e: &Expr, out: &mut Vec<String>) {
    match &e.kind {
        ExprKind::Ident(n) => out.push(n.clone()),
        ExprKind::Call { func, args } => {
            collect_idents(func, out);
            for a in args {
                collect_idents(&a.expr, out);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            collect_idents(object, out);
            for a in args {
                collect_idents(&a.expr, out);
            }
        }
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            collect_idents(object, out)
        }
        ExprKind::Binary { left, right, .. } => {
            collect_idents(left, out);
            collect_idents(right, out);
        }
        ExprKind::Unary { operand, .. } => collect_idents(operand, out),
        ExprKind::Try { expr: inner } => collect_idents(inner, out),
        ExprKind::StructLit { fields, .. } => {
            for f in fields {
                collect_idents(&f.value, out);
            }
        }
        _ => {}
    }
}

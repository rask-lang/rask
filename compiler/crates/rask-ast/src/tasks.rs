// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Tasks that borrow their frame (conc.async/S5): which ones they are.
//!
//! A closure that stays in its frame points at what it names, and one that
//! leaves carries it (mem.closures/CM1). A task block is held to the same
//! rule: one whose handle is joined in the block that spawned it, before
//! anything else touches the handle, can't outlive the frame, so it points at
//! what it names instead of taking copies. What runs beside it is the code
//! between the spawn and the join, and that pair is checked the way two
//! blocks that may run at once are.
//!
//! The shape is read off the statement list: `let h = <spawn>` and, later in
//! the same list, `h.join()` as the handle's only use. Anything else (the
//! handle stored, passed, joined in a closure, used twice) leaves the task
//! carrying, as before.

use crate::expr::{Expr, ExprKind};
use crate::stmt::{Stmt, StmtKind};
use crate::visit;

/// One borrowing task and the code that runs beside it.
pub struct Window<'s> {
    /// The `Spawn` expression.
    pub spawn: &'s Expr,
    /// Its body, the closure the parser builds around the block.
    pub body: &'s Expr,
    /// The statements between the spawn and the statement that joins it.
    pub between: &'s [Stmt],
    /// The statement holding the join. It runs beside the task up to the
    /// join; all of it is counted, which only ever adds a conflict.
    pub join: &'s Stmt,
    /// The `h.join()` call itself.
    pub join_call: &'s Expr,
}

/// The borrowing tasks spawned directly in this statement list.
pub fn windows(stmts: &[Stmt]) -> Vec<Window<'_>> {
    let mut out = Vec::new();
    for (i, stmt) in stmts.iter().enumerate() {
        let (StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. }) = &stmt.kind else {
            continue;
        };
        let ExprKind::Spawn { body, .. } = &init.kind else { continue };
        if let Some(j) = joined_at(name, &stmts[i + 1..]) {
            let j = i + 1 + j;
            let Some(join_call) = join_on_every_path(&stmts[j], name) else { continue };
            out.push(Window { spawn: init, body, between: &stmts[i + 1..j], join: &stmts[j], join_call });
        }
    }
    out
}

/// Where `handle`'s one use is, when that use is `handle.join()` outside any
/// closure: the index into `rest`.
fn joined_at(handle: &str, rest: &[Stmt]) -> Option<usize> {
    let mut found: Option<usize> = None;
    for (k, stmt) in rest.iter().enumerate() {
        // A new binding of the name ends the old one's uses.
        if let StmtKind::Let { name, .. } | StmtKind::Mut { name, .. } = &stmt.kind {
            if name == handle && found.is_none() {
                return None;
            }
        }
        let uses = count_uses(stmt, handle);
        if uses.total == 0 {
            continue;
        }
        if found.is_some() || uses.total != 1 || uses.joins != 1 || uses.in_closure != 0 {
            return None;
        }
        found = Some(k);
    }
    found
}

/// The `handle.join()` in this statement, when every run of the statement
/// reaches it: the statement is a binding, an assignment, an expression or a
/// `return`, and the path down to the join goes only through parts that are
/// always evaluated. A join after `&&` or `??`, in a branch, in a loop or in a
/// closure may not run, and a frame that returns without it would leave the
/// task pointing into it.
fn join_on_every_path<'s>(stmt: &'s Stmt, handle: &str) -> Option<&'s Expr> {
    let root = match &stmt.kind {
        StmtKind::Let { init, .. } | StmtKind::Mut { init, .. } => init,
        StmtKind::Assign { value, .. } => value,
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => e,
        _ => return None,
    };
    always_reaches(root, handle)
}

fn always_reaches<'s>(e: &'s Expr, handle: &str) -> Option<&'s Expr> {
    use crate::expr::BinOp;
    if let ExprKind::MethodCall { object, method, args, .. } = &e.kind {
        if method == "join" && args.is_empty() && matches!(&object.kind, ExprKind::Ident(n) if n == handle) {
            return Some(e);
        }
    }
    let first = |kids: &[&'s Expr]| kids.iter().find_map(|k| always_reaches(k, handle));
    match &e.kind {
        ExprKind::Unwrap { expr, .. }
        | ExprKind::Try { expr }
        | ExprKind::Cast { expr, .. }
        | ExprKind::Convert { expr, .. } => always_reaches(expr, handle),
        ExprKind::Catch { value, .. } => always_reaches(value, handle),
        ExprKind::Assert { condition, .. } | ExprKind::Check { condition, .. } => always_reaches(condition, handle),
        ExprKind::NullCoalesce { value, .. } => always_reaches(value, handle),
        ExprKind::Unary { operand, .. } => always_reaches(operand, handle),
        ExprKind::Binary { op, left, right } if !matches!(op, BinOp::And | BinOp::Or) => first(&[left, right]),
        ExprKind::Binary { left, .. } => always_reaches(left, handle),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => always_reaches(object, handle),
        ExprKind::Index { object, index } => first(&[object, index]),
        ExprKind::MethodCall { object, args, .. } => {
            let mut kids: Vec<&Expr> = vec![object];
            kids.extend(args.iter().map(|a| &a.expr));
            first(&kids)
        }
        ExprKind::Call { func, args } => {
            let mut kids: Vec<&Expr> = vec![func];
            kids.extend(args.iter().map(|a| &a.expr));
            first(&kids)
        }
        ExprKind::Tuple(elems) | ExprKind::Array(elems) => elems.iter().find_map(|k| always_reaches(k, handle)),
        ExprKind::StructLit { fields, .. } => fields.iter().find_map(|f| always_reaches(&f.value, handle)),
        _ => None,
    }
}

#[derive(Default)]
struct Uses {
    total: usize,
    /// As the receiver of a `.join()` with no arguments.
    joins: usize,
    /// Inside a closure or task body, which may run anywhere and later.
    in_closure: usize,
}

fn count_uses(stmt: &Stmt, handle: &str) -> Uses {
    let mut uses = Uses::default();
    visit::walk_stmt(stmt, &mut |e| match &e.kind {
        ExprKind::Ident(n) if n == handle => uses.total += 1,
        ExprKind::MethodCall { object, method, args, .. }
            if method == "join" && args.is_empty() && matches!(&object.kind, ExprKind::Ident(n) if n == handle) =>
        {
            uses.joins += 1
        }
        ExprKind::Closure { body, .. } | ExprKind::Spawn { body, .. } => {
            visit::walk_expr(body, &mut |x| {
                if matches!(&x.kind, ExprKind::Ident(n) if n == handle) {
                    uses.in_closure += 1;
                }
            });
        }
        _ => {}
    });
    uses
}

/// Every borrowing task anywhere in these statements, nested blocks and
/// closure bodies included.
pub fn all_windows(stmts: &[Stmt]) -> Vec<Window<'_>> {
    let mut out = Vec::new();
    collect(stmts, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|w| seen.insert(w.spawn.id));
    out
}

fn collect<'s>(stmts: &'s [Stmt], out: &mut Vec<Window<'s>>) {
    out.extend(windows(stmts));
    for st in stmts {
        match &st.kind {
            StmtKind::While { body, .. }
            | StmtKind::WhileLet { body, .. }
            | StmtKind::Loop { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::ComptimeFor { body, .. }
            | StmtKind::Comptime(body) => collect(body, out),
            StmtKind::Ensure { body, else_handler } => {
                collect(body, out);
                if let Some((_, handler)) = else_handler {
                    collect(handler, out);
                }
            }
            _ => {}
        }
        visit::walk_stmt(st, &mut |e| match &e.kind {
            ExprKind::Block(body)
            | ExprKind::Loop { body, .. }
            | ExprKind::UsingBlock { body, .. }
            | ExprKind::WithAs { body, .. }
            | ExprKind::Unsafe { body }
            | ExprKind::Comptime { body } => collect(body, out),
            _ => {}
        });
    }
}

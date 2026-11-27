// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Tasks that borrow their frame (conc.async/S5): the code that runs beside
//! one. Which tasks borrow is read off the syntax (`rask_ast::tasks`).

use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
pub(crate) use rask_ast::tasks::{all_windows, windows, Window};

/// The code beside a task, as a block that may run at the same time as it:
/// a parameterless closure over the statements before the join and the parts
/// of the joining statement that run before the join.
pub(crate) fn beside(window: &Window<'_>, id: rask_ast::NodeId) -> Expr {
    let mut stmts: Vec<Stmt> = window.between.to_vec();
    for e in before_join(window.join, window.join_call) {
        stmts.push(Stmt { id, kind: StmtKind::Expr(e.clone()), span: e.span });
    }
    let span = window.join.span;
    let block = Expr { id, kind: ExprKind::Block(stmts), span };
    Expr { id, kind: ExprKind::Closure { params: Vec::new(), ret_ty: None, body: Box::new(block) }, span }
}

/// What the joining statement evaluates before the join: the subexpressions
/// written to the left of it that don't contain it, each whole. The join is
/// on every path through the statement (`rask_ast::tasks`), so the parts that
/// contain it are calls, operators and unwraps that run after it; a receiver
/// to the join's left runs before it.
fn before_join<'s>(stmt: &'s Stmt, join: &Expr) -> Vec<&'s Expr> {
    let root = match &stmt.kind {
        StmtKind::Let { init, .. } | StmtKind::Mut { init, .. } => init,
        StmtKind::Assign { value, .. } => value,
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => e,
        _ => return Vec::new(),
    };
    let contains = |e: &Expr| e.span.start <= join.span.start && join.span.end <= e.span.end;
    let mut out = Vec::new();
    rask_ast::visit::walk_expr_pruned(root, &mut |e| {
        if std::ptr::eq(e, join) {
            return false;
        }
        if contains(e) {
            return true;
        }
        if e.span.end <= join.span.start {
            out.push(e);
        }
        false
    });
    out
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `comptime if` inside an unrolled `comptime for`.
//!
//! Inside the loop, `item.has<A>()` decides which branch of a `comptime if`
//! exists at all. Native unrolls the loop and drops the other branch; the
//! interpreter walks it instead, so it asks the same question of the body
//! before running it. Both read the shape and the condition here, so they
//! can't disagree about which branch survives (type.annotations/AN6).

use crate::expr::{BinOp, Expr, ExprKind, UnaryOp};
use crate::stmt::{Stmt, StmtKind};

/// `comptime { if cond { … } else { … } }`: the condition and both branches.
/// The parser gives `comptime if` this shape; anything else is `None`.
pub fn parts(stmts: &[Stmt]) -> Option<(&Expr, &Expr, Option<&Expr>)> {
    let [stmt] = stmts else { return None };
    let StmtKind::Expr(inner) = &stmt.kind else { return None };
    let ExprKind::If { cond, then_branch, else_branch, .. } = &inner.kind else {
        return None;
    };
    Some((cond, then_branch, else_branch.as_deref()))
}

/// The statements of whichever branch a decided condition selects. A branch
/// that isn't a block gives `None`, and a false condition with no `else`
/// gives an empty slice: the branch is gone.
pub fn branch<'a>(taken: bool, then_branch: &'a Expr, else_branch: Option<&'a Expr>) -> Option<&'a [Stmt]> {
    let chosen = if taken {
        then_branch
    } else {
        match else_branch {
            Some(e) => e,
            None => return Some(&[]),
        }
    };
    match &chosen.kind {
        ExprKind::Block(stmts) => Some(stmts),
        _ => None,
    }
}

/// A condition built from tests only the loop iteration can answer, combined
/// with `!`, `&&` and `||`. `test` answers one method call (`item.has<A>()`)
/// or gives `None`; any other shape is `None` too.
pub fn decide(cond: &Expr, test: &mut impl FnMut(&Expr) -> Option<bool>) -> Option<bool> {
    match &cond.kind {
        ExprKind::MethodCall { .. } => test(cond),
        ExprKind::Unary { op: UnaryOp::Not, operand } => Some(!decide(operand, test)?),
        ExprKind::Binary { op, left, right } => {
            let (l, r) = (decide(left, test)?, decide(right, test)?);
            match op {
                BinOp::And => Some(l && r),
                BinOp::Or => Some(l || r),
                _ => None,
            }
        }
        _ => None,
    }
}

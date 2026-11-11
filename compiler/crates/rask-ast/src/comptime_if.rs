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

/// The statements of the branch a decided chain keeps: `then` when `cond`
/// holds, else the `else` block, else — for `else if` — the same question of
/// the next link. `None` when any condition on the way can't be decided.
pub fn choose<'a>(
    cond: &Expr,
    then_branch: &'a Expr,
    else_branch: Option<&'a Expr>,
    fact: &mut impl FnMut(&Expr) -> Option<Fact>,
) -> Option<&'a [Stmt]> {
    if decide(cond, fact)? {
        return branch(true, then_branch, else_branch);
    }
    match else_branch.map(|e| &e.kind) {
        Some(ExprKind::If { cond, then_branch, else_branch, .. }) => {
            choose(cond, then_branch, else_branch.as_deref(), fact)
        }
        _ => branch(false, then_branch, else_branch),
    }
}

/// What one fact about the loop iteration folds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Bool(bool),
    Int(i128),
    Str(String),
}

/// A condition built from facts only the loop iteration can answer,
/// combined with `!`, `&&`, `||` and comparisons. `fact` answers one fact
/// about the item — a method call (`item.has<A>()`) or a property
/// (`item.is_skipped`, `item.type_name`) — or gives `None`.
///
/// A comparison reaches here desugared to a method call (`a.eq(b)`,
/// `a.lt(b)`, and `!a.eq(b)` for `!=`). It folds when both sides are facts
/// or literals of the same kind: `item.type_name == "f64"` does,
/// `item.type_name == other` doesn't. Integer `+`, `-`, `*` and unary `-`
/// fold too. Anything else a side names — a module const, a `reflect.*()`
/// call — goes to `fact` as well, which answers what its backend knows.
pub fn decide(cond: &Expr, fact: &mut impl FnMut(&Expr) -> Option<Fact>) -> Option<bool> {
    match value(cond, fact)? {
        Fact::Bool(b) => Some(b),
        _ => None,
    }
}

/// A literal's value.
pub fn literal(expr: &Expr) -> Option<Fact> {
    match &expr.kind {
        ExprKind::Bool(b) => Some(Fact::Bool(*b)),
        ExprKind::Int(n, _) => Some(Fact::Int(*n)),
        ExprKind::String(s) => Some(Fact::Str(s.clone())),
        _ => None,
    }
}

fn value(expr: &Expr, fact: &mut impl FnMut(&Expr) -> Option<Fact>) -> Option<Fact> {
    if let Some(lit) = literal(expr) {
        return Some(lit);
    }
    match &expr.kind {
        ExprKind::MethodCall { object, method, args, .. } if method == "neg" && args.is_empty() => {
            match value(object, fact)? {
                Fact::Int(n) => Some(Fact::Int(n.checked_neg()?)),
                _ => None,
            }
        }
        ExprKind::MethodCall { object, method, args, .. }
            if args.len() == 1 && matches!(method.as_str(), "add" | "sub" | "mul") =>
        {
            let (Fact::Int(a), Fact::Int(b)) = (value(object, fact)?, value(&args[0].expr, fact)?) else {
                return None;
            };
            Some(Fact::Int(match method.as_str() {
                "add" => a.checked_add(b)?,
                "sub" => a.checked_sub(b)?,
                _ => a.checked_mul(b)?,
            }))
        }
        // `!x` after desugaring.
        ExprKind::MethodCall { object, method, args, .. } if method == "not" && args.is_empty() => {
            Some(Fact::Bool(!decide(object, fact)?))
        }
        ExprKind::MethodCall { object, method, args, .. }
            if args.len() == 1 && matches!(method.as_str(), "eq" | "lt" | "le" | "gt" | "ge") =>
        {
            let (l, r) = (value(object, fact)?, value(&args[0].expr, fact)?);
            let ord = match (&l, &r) {
                (Fact::Int(a), Fact::Int(b)) => a.cmp(b),
                (Fact::Str(a), Fact::Str(b)) => a.cmp(b),
                (Fact::Bool(a), Fact::Bool(b)) if method == "eq" => a.cmp(b),
                _ => return None,
            };
            Some(Fact::Bool(match method.as_str() {
                "eq" => ord.is_eq(),
                "lt" => ord.is_lt(),
                "le" => ord.is_le(),
                "gt" => ord.is_gt(),
                _ => ord.is_ge(),
            }))
        }
        ExprKind::MethodCall { .. } | ExprKind::Field { .. } | ExprKind::Ident(_) => fact(expr),
        ExprKind::Unary { op: UnaryOp::Not, operand } => Some(Fact::Bool(!decide(operand, fact)?)),
        ExprKind::Binary { op, left, right } => {
            let (l, r) = (decide(left, fact)?, decide(right, fact)?);
            match op {
                BinOp::And => Some(Fact::Bool(l && r)),
                BinOp::Or => Some(Fact::Bool(l || r)),
                _ => None,
            }
        }
        _ => None,
    }
}

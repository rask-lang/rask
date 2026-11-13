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
#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Bool(bool),
    /// An integer, and the range of its type (`const SMALL: u8 = 200`,
    /// `f.offset` is a `usize`). `None` is a literal or an untyped const,
    /// which is an `i64`. Arithmetic that leaves the range doesn't fold: the
    /// program overflows there, and an answer would be a different program.
    Int(i128, Option<(i128, i128)>),
    Float(f64),
    Str(String),
}

impl Fact {
    /// An integer of no particular declared type: a literal, a reflect fact.
    pub fn int(n: i128) -> Fact {
        Fact::Int(n, None)
    }
}

/// The range of an integer type by its name, for a const's declared type.
pub fn int_range(name: &str) -> Option<(i128, i128)> {
    Some(match name {
        "i8" => (i8::MIN as i128, i8::MAX as i128),
        "i16" => (i16::MIN as i128, i16::MAX as i128),
        "i32" => (i32::MIN as i128, i32::MAX as i128),
        "i64" | "isize" => (i64::MIN as i128, i64::MAX as i128),
        "u8" => (0, u8::MAX as i128),
        "u16" => (0, u16::MAX as i128),
        "u32" => (0, u32::MAX as i128),
        "u64" | "usize" => (0, u64::MAX as i128),
        _ => return None,
    })
}

/// A condition built from facts only the loop iteration can answer,
/// combined with `!`, `&&`, `||` and comparisons. `fact` answers one fact
/// about the item — a method call (`item.has<A>()`) or a property
/// (`item.is_skipped`, `item.type_name`) — or gives `None`.
///
/// A comparison reaches here desugared to a method call (`a.eq(b)`,
/// `a.lt(b)`, and `!a.eq(b)` for `!=`). It folds when both sides are facts
/// or literals of the same kind: `item.type_name == "f64"` does,
/// `item.type_name == other` doesn't. Integer `+`, `-`, `*`, `/`, `%` and unary `-`
/// fold too. Anything else a side names — a module const, a `reflect.*()`
/// call — goes to `fact` as well, which answers what its backend knows.
pub fn decide(cond: &Expr, fact: &mut impl FnMut(&Expr) -> Option<Fact>) -> Option<bool> {
    match value(cond, fact)? {
        Fact::Bool(b) => Some(b),
        _ => None,
    }
}

/// The value of each module const whose initializer folds from literals and
/// earlier consts — `const W = 16`, `const TWICE = W * 2`, `const F = "f64"`
/// — in declaration order. A `comptime if` in a `comptime for` can compare
/// against these; both backends build the table here so they fold the same
/// names.
///
/// `declared` is the type the const was written with, if any: an integer
/// type holds the value and arithmetic on it to its range, and a float type
/// makes an integer literal a float (`const F: f64 = 7`).
pub fn const_facts<'a>(
    consts: impl IntoIterator<Item = (&'a str, &'a Expr, Option<&'a str>)>,
) -> std::collections::HashMap<String, Fact> {
    let mut table = std::collections::HashMap::new();
    for (name, init, declared) in consts {
        let folded = value(init, &mut |e| match &e.kind {
            ExprKind::Ident(n) => table.get(n.as_str()).cloned(),
            _ => None,
        });
        let folded = match (folded, declared) {
            (Some(Fact::Int(n, _)), Some("f32" | "f64")) => Some(Fact::Float(n as f64)),
            (Some(Fact::Int(n, _)), Some(ty)) => match int_range(ty) {
                Some((lo, hi)) if n < lo || n > hi => None,
                Some(r) => Some(Fact::Int(n, Some(r))),
                // A type the fold doesn't know (`i128`, a nominal type):
                // nothing to hold the value to, so it isn't folded.
                None => None,
            },
            (other, _) => other,
        };
        if let Some(fact) = folded {
            table.insert(name.to_string(), fact);
        }
    }
    table
}

/// A literal's value. A negative number is `-n` after desugaring, and is
/// one too.
pub fn literal(expr: &Expr) -> Option<Fact> {
    match &expr.kind {
        ExprKind::Bool(b) => Some(Fact::Bool(*b)),
        ExprKind::Int(n, _) => Some(Fact::int(*n)),
        ExprKind::Float(x, _) => Some(Fact::Float(*x)),
        ExprKind::String(s) => Some(Fact::Str(s.clone())),
        ExprKind::MethodCall { object, method, args, .. } if method == "neg" && args.is_empty() => {
            match literal(object)? {
                Fact::Int(n, r) => Some(Fact::Int(-n, r)),
                Fact::Float(x) => Some(Fact::Float(-x)),
                _ => None,
            }
        }
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
                Fact::Int(n, range) => in_range(n.checked_neg()?, range),
                Fact::Float(x) => Some(Fact::Float(-x)),
                _ => None,
            }
        }
        ExprKind::MethodCall { object, method, args, .. }
            if args.len() == 1 && matches!(method.as_str(), "add" | "sub" | "mul" | "div" | "rem") =>
        {
            let (l, r) = (value(object, fact)?, value(&args[0].expr, fact)?);
            if let (Fact::Float(a), Fact::Float(b)) = (&l, &r) {
                return Some(Fact::Float(match method.as_str() {
                    "add" => a + b,
                    "sub" => a - b,
                    "mul" => a * b,
                    "div" => a / b,
                    _ => a % b,
                }));
            }
            let (Fact::Int(a, ra), Fact::Int(b, rb)) = (l, r) else {
                return None;
            };
            in_range(match method.as_str() {
                "add" => a.checked_add(b)?,
                "sub" => a.checked_sub(b)?,
                "mul" => a.checked_mul(b)?,
                // Truncating, as the language's integer `/` and `%` are; by
                // zero it doesn't fold, and the program says so where it runs.
                "div" => a.checked_div(b)?,
                _ => a.checked_rem(b)?,
            }, ra.or(rb))
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
                (Fact::Int(a, _), Fact::Int(b, _)) => a.cmp(b),
                (Fact::Float(a), Fact::Float(b)) => a.partial_cmp(b)?,
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
        // Short-circuit, as at run time: the right side of `f.has<A>() &&
        // f.get<A>().x > 5` is only a question when the left side holds.
        ExprKind::Binary { op: BinOp::And, left, right } => {
            Some(Fact::Bool(decide(left, fact)? && decide(right, fact)?))
        }
        ExprKind::Binary { op: BinOp::Or, left, right } => {
            Some(Fact::Bool(decide(left, fact)? || decide(right, fact)?))
        }
        _ => None,
    }
}

/// `n` as an integer fact, when it fits the range of its type: the one it
/// carries, else `i64`'s.
fn in_range(n: i128, range: Option<(i128, i128)>) -> Option<Fact> {
    let (lo, hi) = range.unwrap_or((i64::MIN as i128, i64::MAX as i128));
    (lo..=hi).contains(&n).then_some(Fact::Int(n, range))
}

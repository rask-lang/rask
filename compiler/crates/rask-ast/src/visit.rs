// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Walking every expression in a body.
//!
//! Several passes want the same thing — "every X anywhere in this program" —
//! and there was nowhere to ask it. `rask-mono`'s reachability has a walk of
//! its own, private and interleaved with its bookkeeping, so anything else
//! either hand-rolled a partial one or stopped short:
//!
//!   - CT60 wants a `comptime func`'s body checked at its definition, which
//!     means finding what it calls, transitively (#1086).
//!   - `value.(comptime …)` is evaluated at MIR rather than at check time,
//!     because finding the blocks means walking every body (#1090).
//!   - Merging a dependency's declarations renames each declaration and not
//!     the references to it, so a package can't export a function that
//!     constructs its own struct (#1112).
//!
//! The match here is exhaustive on purpose — no `_` arm anywhere. A new
//! `ExprKind` or `StmtKind` variant fails to compile until it says whether it
//! holds expressions, which is the only way a walk like this stays correct as
//! the AST grows. That is worth the length.
//!
//! Pre-order: `f` sees a node before its children, so a visitor that wants to
//! stop at the outermost match of something can answer from `f` alone.
//!
//! The borrow is the caller's: `f` receives references that live as long as the
//! tree, so a visitor can collect the nodes it found rather than having to
//! decide about each one on the spot.
//!
//! Patterns and type strings are not walked. A pattern binds names and holds
//! no expressions, and a type is a string at this stage — the helpers in
//! [`crate::type_str`] are what read those.

use crate::decl::{Decl, DeclKind};
use crate::expr::{Expr, ExprKind, SelectArmKind, StringSegment};
use crate::stmt::{Stmt, StmtKind};

/// Every expression in `expr` and below it, outermost first.
pub fn walk_expr<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    f(expr);
    match &expr.kind {
        // Leaves.
        ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::String(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::None
        | ExprKind::Ident(_) => {}

        ExprKind::StringInterp(segments) => {
            for seg in segments {
                match seg {
                    StringSegment::Literal(_) => {}
                    StringSegment::Expr(e, _) => walk_expr(e, f),
                }
            }
        }

        ExprKind::Binary { left, right, .. } => {
            walk_expr(left, f);
            walk_expr(right, f);
        }
        ExprKind::Unary { operand, .. } => walk_expr(operand, f),

        ExprKind::Call { func, args } => {
            walk_expr(func, f);
            for a in args {
                walk_expr(&a.expr, f);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            walk_expr(object, f);
            for a in args {
                walk_expr(&a.expr, f);
            }
        }

        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            walk_expr(object, f)
        }
        ExprKind::DynamicField { object, field_expr } => {
            walk_expr(object, f);
            walk_expr(field_expr, f);
        }
        ExprKind::Index { object, index } => {
            walk_expr(object, f);
            walk_expr(index, f);
        }

        ExprKind::Block(body)
        | ExprKind::Spawn { body }
        | ExprKind::BlockCall { body, .. }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::Loop { body, .. } => walk_body(body, f),

        ExprKind::If { cond, then_branch, else_branch, .. } => {
            walk_expr(cond, f);
            walk_expr(then_branch, f);
            if let Some(e) = else_branch {
                walk_expr(e, f);
            }
        }
        ExprKind::IfLet { expr: scrutinee, then_branch, else_branch, .. } => {
            walk_expr(scrutinee, f);
            walk_expr(then_branch, f);
            if let Some(e) = else_branch {
                walk_expr(e, f);
            }
        }
        ExprKind::GuardPattern { expr: scrutinee, else_branch, .. } => {
            walk_expr(scrutinee, f);
            walk_expr(else_branch, f);
        }
        ExprKind::IsPattern { expr: scrutinee, .. } => walk_expr(scrutinee, f),

        ExprKind::Match { scrutinee, arms } => {
            walk_expr(scrutinee, f);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_expr(g, f);
                }
                walk_expr(&arm.body, f);
            }
        }

        ExprKind::Try { expr: inner }
        | ExprKind::Take { place: inner }
        | ExprKind::IsPresent { expr: inner, .. }
        | ExprKind::Unwrap { expr: inner, .. }
        | ExprKind::Cast { expr: inner, .. }
        | ExprKind::Convert { expr: inner, .. } => walk_expr(inner, f),

        ExprKind::Catch { value, clause } => {
            walk_expr(value, f);
            walk_expr(&clause.body, f);
        }
        ExprKind::NullCoalesce { value, default } => {
            walk_expr(value, f);
            walk_expr(default, f);
        }

        ExprKind::Range { start, end, .. } => {
            if let Some(e) = start {
                walk_expr(e, f);
            }
            if let Some(e) = end {
                walk_expr(e, f);
            }
        }

        ExprKind::StructLit { fields, spread, .. } => {
            for field in fields {
                walk_expr(&field.value, f);
            }
            if let Some(e) = spread {
                walk_expr(e, f);
            }
        }

        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                walk_expr(e, f);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            walk_expr(value, f);
            walk_expr(count, f);
        }

        ExprKind::UsingBlock { args, body, .. } => {
            for a in args {
                walk_expr(&a.expr, f);
            }
            walk_body(body, f);
        }
        ExprKind::WithAs { bindings, body } => {
            for b in bindings {
                walk_expr(&b.source, f);
            }
            walk_body(body, f);
        }

        ExprKind::Closure { body, .. } => walk_expr(body, f),

        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    SelectArmKind::Recv { channel, .. } => walk_expr(channel, f),
                    SelectArmKind::Send { channel, value } => {
                        walk_expr(channel, f);
                        walk_expr(value, f);
                    }
                    SelectArmKind::Default => {}
                }
                walk_expr(&arm.body, f);
            }
        }

        ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
            walk_expr(condition, f);
            if let Some(m) = message {
                walk_expr(m, f);
            }
        }
    }
}

/// Every expression in `stmt` and below it.
pub fn walk_stmt<'a>(stmt: &'a Stmt, f: &mut impl FnMut(&'a Expr)) {
    match &stmt.kind {
        StmtKind::Expr(e)
        | StmtKind::Mut { init: e, .. }
        | StmtKind::MutTuple { init: e, .. }
        | StmtKind::Let { init: e, .. }
        | StmtKind::LetTuple { init: e, .. }
        | StmtKind::LetStruct { init: e, .. } => walk_expr(e, f),

        StmtKind::Assign { target, value, .. } => {
            walk_expr(target, f);
            walk_expr(value, f);
        }

        StmtKind::Return(value) | StmtKind::Break { value, .. } => {
            if let Some(e) = value {
                walk_expr(e, f);
            }
        }
        StmtKind::Continue(_) | StmtKind::Discard { .. } => {}

        StmtKind::While { cond, body, .. } => {
            walk_expr(cond, f);
            walk_body(body, f);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            walk_expr(expr, f);
            walk_body(body, f);
        }
        StmtKind::Loop { body, .. } | StmtKind::Comptime(body) => walk_body(body, f),
        StmtKind::For { iter, body, .. } | StmtKind::ComptimeFor { iter, body, .. } => {
            walk_expr(iter, f);
            walk_body(body, f);
        }
        StmtKind::Ensure { body, else_handler } => {
            walk_body(body, f);
            if let Some((_, handler)) = else_handler {
                walk_body(handler, f);
            }
        }
    }
}

/// Every expression in a statement list.
pub fn walk_body<'a>(body: &'a [Stmt], f: &mut impl FnMut(&'a Expr)) {
    for stmt in body {
        walk_stmt(stmt, f);
    }
}

/// Every expression in whatever bodies a declaration carries — a function's,
/// the methods on a struct or enum, a trait's defaults, an `extend` block's, a
/// `const`'s initializer, a `test`'s, a `benchmark`'s.
///
/// A declaration that carries none — an import, a type alias, a union — walks
/// nothing, which is the correct answer rather than an omission.
pub fn walk_decl<'a>(decl: &'a Decl, f: &mut impl FnMut(&'a Expr)) {
    match &decl.kind {
        DeclKind::Fn(func) => walk_body(&func.body, f),
        DeclKind::Struct(s) => {
            for m in &s.methods {
                walk_body(&m.body, f);
            }
        }
        DeclKind::Enum(e) => {
            for m in &e.methods {
                walk_body(&m.body, f);
            }
        }
        DeclKind::Trait(t) => {
            for m in &t.methods {
                walk_body(&m.body, f);
            }
        }
        DeclKind::Impl(i) => {
            for m in &i.methods {
                walk_body(&m.body, f);
            }
        }
        DeclKind::Const(c) => walk_expr(&c.init, f),
        DeclKind::Test(t) => walk_body(&t.body, f),
        DeclKind::Benchmark(b) => walk_body(&b.body, f),
        DeclKind::Import(_)
        | DeclKind::Export(_)
        | DeclKind::CImport(_)
        | DeclKind::TypeAlias(_)
        | DeclKind::Union(_)
        | DeclKind::Extern(_)
        | DeclKind::Package(_)
        | DeclKind::Annotation(_) => {}
    }
}

/// Every expression in a whole program.
pub fn walk_decls<'a>(decls: &'a [Decl], f: &mut impl FnMut(&'a Expr)) {
    for decl in decls {
        walk_decl(decl, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{CallArg, ArgMode};
    use crate::{NodeId, Span};

    fn e(kind: ExprKind) -> Expr {
        Expr { id: NodeId::DUMMY, span: Span::new(0, 0), kind }
    }
    fn ident(n: &str) -> Expr {
        e(ExprKind::Ident(n.to_string()))
    }
    fn stmt(kind: StmtKind) -> Stmt {
        Stmt { id: NodeId::DUMMY, kind, span: Span::new(0, 0) }
    }

    /// Every name the walk reaches, outermost first.
    fn names(root: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        walk_expr(root, &mut |x| {
            if let ExprKind::Ident(n) = &x.kind {
                out.push(n.clone());
            }
        });
        out
    }

    #[test]
    fn a_node_comes_before_its_children() {
        let call = e(ExprKind::Call {
            func: Box::new(ident("f")),
            args: vec![CallArg { name: None, mode: ArgMode::Default, expr: ident("a") }],
        });
        let mut kinds = Vec::new();
        walk_expr(&call, &mut |x| {
            kinds.push(match &x.kind {
                ExprKind::Call { .. } => "Call",
                ExprKind::Ident(_) => "Ident",
                _ => "other",
            })
        });
        assert_eq!(kinds, vec!["Call", "Ident", "Ident"]);
    }

    #[test]
    fn it_reaches_into_every_arm_of_a_nested_shape() {
        // if f(a) { g(b) } else { h(c) } — condition, both branches, and the
        // arguments inside each.
        let arg = |n: &str| CallArg { name: None, mode: ArgMode::Default, expr: ident(n) };
        let call = |f: &str, a: &str| {
            e(ExprKind::Call { func: Box::new(ident(f)), args: vec![arg(a)] })
        };
        let cond = e(ExprKind::If {
            cond: Box::new(call("f", "a")),
            then_branch: Box::new(call("g", "b")),
            else_branch: Some(Box::new(call("h", "c"))),
            else_binding: None,
        });
        assert_eq!(names(&cond), vec!["f", "a", "g", "b", "h", "c"]);
    }

    #[test]
    fn a_statement_body_is_walked_through() {
        // A loop holding `let x = f(a)`, reached from the expression side.
        let body = vec![stmt(StmtKind::Let {
            name: "x".to_string(),
            name_span: Span::new(0, 0),
            ty: None,
            init: e(ExprKind::Call {
                func: Box::new(ident("f")),
                args: vec![CallArg { name: None, mode: ArgMode::Default, expr: ident("a") }],
            }),
        })];
        let loop_expr = e(ExprKind::Loop { label: None, body });
        assert_eq!(names(&loop_expr), vec!["f", "a"]);
    }

    #[test]
    fn an_interpolation_holds_expressions() {
        let interp = e(ExprKind::StringInterp(vec![
            StringSegment::Literal("x=".to_string()),
            StringSegment::Expr(Box::new(ident("x")), None),
        ]));
        assert_eq!(names(&interp), vec!["x"]);
    }

    /// The borrow outlives the walk, which is what lets a visitor collect what
    /// it found instead of deciding about each node on the spot.
    #[test]
    fn what_the_visitor_keeps_outlives_the_walk() {
        let tree = e(ExprKind::Unary {
            op: crate::expr::UnaryOp::Not,
            operand: Box::new(ident("flag")),
        });
        let mut found: Vec<&Expr> = Vec::new();
        walk_expr(&tree, &mut |x| {
            if matches!(x.kind, ExprKind::Ident(_)) {
                found.push(x);
            }
        });
        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0].kind, ExprKind::Ident(n) if n == "flag"));
    }
}

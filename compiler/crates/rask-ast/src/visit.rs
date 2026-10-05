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
    walk_expr_pruned(expr, &mut |e| {
        f(e);
        true
    });
}

/// The same walk, with the visitor deciding where to stop.
///
/// `f` returns whether to descend into the node it was given. A visitor that
/// treats some nodes as boundaries — a closure body is its own function, a
/// block's statements are lowered elsewhere — answers `false` there and keeps
/// the rest of the walk.
pub fn walk_expr_pruned<'a>(expr: &'a Expr, f: &mut impl FnMut(&'a Expr) -> bool) {
    visit_expr(expr, &mut ExprsOnly(f));
}

/// A walk that sees statements as well as expressions. `expr` and `stmt`
/// answer whether to descend into what they were given, as the closure of
/// `walk_expr_pruned` does; a visitor that only wants expressions keeps the
/// default `stmt`.
pub trait Visit<'a> {
    fn expr(&mut self, expr: &'a Expr) -> bool;
    fn stmt(&mut self, _stmt: &'a Stmt) -> bool {
        true
    }
}

struct ExprsOnly<F>(F);

impl<'a, F: FnMut(&'a Expr) -> bool> Visit<'a> for ExprsOnly<F> {
    fn expr(&mut self, expr: &'a Expr) -> bool {
        (self.0)(expr)
    }
}

/// Every expression and statement in a statement list, `v` deciding where to
/// stop.
pub fn visit_body<'a>(body: &'a [Stmt], v: &mut impl Visit<'a>) {
    for stmt in body {
        visit_stmt(stmt, v);
    }
}

/// Every expression and statement in a declaration's bodies, as `walk_decl`.
pub fn visit_decl<'a>(decl: &'a Decl, v: &mut impl Visit<'a>) {
    match &decl.kind {
        DeclKind::Fn(func) => visit_body(&func.body, v),
        DeclKind::Struct(s) => {
            for m in &s.methods {
                visit_body(&m.body, v);
            }
        }
        DeclKind::Enum(e) => {
            for m in &e.methods {
                visit_body(&m.body, v);
            }
        }
        DeclKind::Interface(t) => {
            for m in &t.methods {
                visit_body(&m.body, v);
            }
        }
        DeclKind::Impl(i) => {
            for m in &i.methods {
                visit_body(&m.body, v);
            }
        }
        DeclKind::Const(c) => visit_expr(&c.init, v),
        DeclKind::Test(t) => visit_body(&t.body, v),
        DeclKind::Benchmark(b) => visit_body(&b.body, v),
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

pub fn visit_expr<'a>(expr: &'a Expr, v: &mut impl Visit<'a>) {
    if !v.expr(expr) {
        return;
    }
    match &expr.kind {
        // Leaves.
        ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::String(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::None
        | ExprKind::Ident(_)
        | ExprKind::GenericName { .. } => {}

        ExprKind::StringInterp(segments) => {
            for seg in segments {
                match seg {
                    StringSegment::Literal(_) => {}
                    StringSegment::Expr(e, _) => visit_expr(e, v),
                }
            }
        }

        ExprKind::Binary { left, right, .. } => {
            visit_expr(left, v);
            visit_expr(right, v);
        }
        ExprKind::Unary { operand, .. } => visit_expr(operand, v),

        ExprKind::Call { func, args } => {
            visit_expr(func, v);
            for a in args {
                visit_expr(&a.expr, v);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            visit_expr(object, v);
            for a in args {
                visit_expr(&a.expr, v);
            }
        }

        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            visit_expr(object, v)
        }
        ExprKind::DynamicField { object, field_expr } => {
            visit_expr(object, v);
            visit_expr(field_expr, v);
        }
        ExprKind::Index { object, index } => {
            visit_expr(object, v);
            visit_expr(index, v);
        }

        ExprKind::Block(body)
        | ExprKind::BlockCall { body, .. }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::Loop { body, .. } => visit_body(body, v),

        ExprKind::If { cond, then_branch, else_branch, .. } => {
            visit_expr(cond, v);
            visit_expr(then_branch, v);
            if let Some(e) = else_branch {
                visit_expr(e, v);
            }
        }
        ExprKind::IfLet { expr: scrutinee, then_branch, else_branch, .. } => {
            visit_expr(scrutinee, v);
            visit_expr(then_branch, v);
            if let Some(e) = else_branch {
                visit_expr(e, v);
            }
        }
        ExprKind::GuardPattern { expr: scrutinee, else_branch, .. } => {
            visit_expr(scrutinee, v);
            visit_expr(else_branch, v);
        }
        ExprKind::IsPattern { expr: scrutinee, .. } => visit_expr(scrutinee, v),

        ExprKind::Match { scrutinee, arms } => {
            visit_expr(scrutinee, v);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    visit_expr(g, v);
                }
                visit_expr(&arm.body, v);
            }
        }

        ExprKind::Try { expr: inner }
        | ExprKind::Take { place: inner }
        | ExprKind::IsPresent { expr: inner, .. }
        | ExprKind::Unwrap { expr: inner, .. }
        | ExprKind::Cast { expr: inner, .. }
        | ExprKind::Convert { expr: inner, .. } => visit_expr(inner, v),

        ExprKind::Catch { value, clause } => {
            visit_expr(value, v);
            visit_expr(&clause.body, v);
        }
        ExprKind::NullCoalesce { value, default } => {
            visit_expr(value, v);
            visit_expr(default, v);
        }

        ExprKind::Range { start, end, .. } => {
            if let Some(e) = start {
                visit_expr(e, v);
            }
            if let Some(e) = end {
                visit_expr(e, v);
            }
        }

        ExprKind::StructLit { fields, spread, .. } => {
            for field in fields {
                visit_expr(&field.value, v);
            }
            if let Some(e) = spread {
                visit_expr(e, v);
            }
        }

        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                visit_expr(e, v);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            visit_expr(value, v);
            visit_expr(count, v);
        }

        ExprKind::UsingBlock { args, body, .. } => {
            for a in args {
                visit_expr(&a.expr, v);
            }
            visit_body(body, v);
        }
        ExprKind::WithAs { bindings, body } => {
            for b in bindings {
                visit_expr(&b.source, v);
            }
            visit_body(body, v);
        }

        ExprKind::Closure { body, .. } => visit_expr(body, v),

        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    SelectArmKind::Recv { channel, .. } => visit_expr(channel, v),
                    SelectArmKind::Send { channel, value } => {
                        visit_expr(channel, v);
                        visit_expr(value, v);
                    }
                    SelectArmKind::Default => {}
                }
                visit_expr(&arm.body, v);
            }
        }

        ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
            visit_expr(condition, v);
            if let Some(m) = message {
                visit_expr(m, v);
            }
        }
    }
}

/// Every expression in `stmt` and below it.
pub fn walk_stmt<'a>(stmt: &'a Stmt, f: &mut impl FnMut(&'a Expr)) {
    walk_stmt_pruned(stmt, &mut |e| {
        f(e);
        true
    });
}

/// Every expression in `stmt` and below it, the visitor deciding where to stop.
pub fn walk_stmt_pruned<'a>(stmt: &'a Stmt, f: &mut impl FnMut(&'a Expr) -> bool) {
    visit_stmt(stmt, &mut ExprsOnly(f));
}

pub fn visit_stmt<'a>(stmt: &'a Stmt, v: &mut impl Visit<'a>) {
    if !v.stmt(stmt) {
        return;
    }
    match &stmt.kind {
        StmtKind::Expr(e)
        | StmtKind::Mut { init: e, .. }
        | StmtKind::MutTuple { init: e, .. }
        | StmtKind::Let { init: e, .. }
        | StmtKind::LetTuple { init: e, .. }
        | StmtKind::LetStruct { init: e, .. } => visit_expr(e, v),

        StmtKind::Assign { target, value, .. } => {
            visit_expr(target, v);
            visit_expr(value, v);
        }

        StmtKind::Return(value) | StmtKind::Break { value, .. } => {
            if let Some(e) = value {
                visit_expr(e, v);
            }
        }
        StmtKind::Continue(_) | StmtKind::Discard { .. } => {}

        StmtKind::While { cond, body, .. } => {
            visit_expr(cond, v);
            visit_body(body, v);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            visit_expr(expr, v);
            visit_body(body, v);
        }
        StmtKind::Loop { body, .. } | StmtKind::Comptime(body) => visit_body(body, v),
        StmtKind::For { iter, body, .. } | StmtKind::ComptimeFor { iter, body, .. } => {
            visit_expr(iter, v);
            visit_body(body, v);
        }
        StmtKind::Ensure { body, else_handler } => {
            visit_body(body, v);
            if let Some((_, handler)) = else_handler {
                visit_body(handler, v);
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

/// Every expression in a statement list, the visitor deciding where to stop.
pub fn walk_body_pruned<'a>(body: &'a [Stmt], f: &mut impl FnMut(&'a Expr) -> bool) {
    for stmt in body {
        walk_stmt_pruned(stmt, f);
    }
}

/// Every expression in whatever bodies a declaration carries — a function's,
/// the methods on a struct or enum, an interface's defaults, an `extend` block's, a
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
        DeclKind::Interface(t) => {
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

    /// A visitor that stops at a boundary keeps the rest of the walk.
    #[test]
    fn a_pruned_node_keeps_its_siblings() {
        // f(|| { g(a) }, b) — the closure is a boundary, so `g` and `a` are
        // not this walk's, but `b` still is.
        let arg = |e: Expr| CallArg { name: None, mode: ArgMode::Default, expr: e };
        let inner = e(ExprKind::Call {
            func: Box::new(ident("g")),
            args: vec![arg(ident("a"))],
        });
        let closure = e(ExprKind::Closure {
            params: Vec::new(),
            ret_ty: None,
            body: Box::new(inner),
        });
        let call = e(ExprKind::Call {
            func: Box::new(ident("f")),
            args: vec![arg(closure), arg(ident("b"))],
        });
        let mut out = Vec::new();
        walk_expr_pruned(&call, &mut |x| {
            if matches!(x.kind, ExprKind::Closure { .. }) {
                return false;
            }
            if let ExprKind::Ident(n) = &x.kind {
                out.push(n.clone());
            }
            true
        });
        assert_eq!(out, vec!["f", "b"]);
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

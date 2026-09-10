// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Rewriting the names in a declaration.
//!
//! [`crate::visit`] reads; this writes. The two are separate because a reader
//! wants the whole tree with one borrow and a rewriter wants each node on its
//! own, and because a rewriter needs more of the tree: a name in Rask source
//! appears as an expression (`Cat { … }`, `greet(c)`), inside a pattern
//! (`match c { Colour.Red => … }`) and inside a type — and a type at this
//! stage is a `String`, so nothing else can find one.
//!
//! Hence three callbacks rather than one. Merging a dependency's declarations
//! renamed each declaration and not the references to it, which is why a
//! package couldn't export a function that constructs its own struct (#1112);
//! the fix needs every one of the three, and missing one silently leaves half
//! the program talking about a name that no longer exists.
//!
//! The matches are exhaustive on purpose — no `_` arm anywhere, same as the
//! reader. A new variant fails to compile until it says what it holds.
//!
//! Pre-order: the callback sees a node before its children, so a rewriter that
//! replaces a whole subtree can do it and let the walk descend into what it
//! put there.

use crate::decl::{Decl, DeclKind, FnDecl};
use crate::expr::{Expr, ExprKind, Pattern, SelectArmKind, StringSegment};
use crate::stmt::{Stmt, StmtKind};

/// What a rewriter is handed. Implement the parts that matter; the rest are
/// no-ops.
pub trait Rewrite {
    /// Every expression, outermost first.
    fn expr(&mut self, _e: &mut Expr) {}
    /// Every pattern, outermost first.
    fn pattern(&mut self, _p: &mut Pattern) {}
    /// Every type as it was written — `Vec<Cat>`, `Cat?`, `i64 or Cat`.
    fn ty(&mut self, _t: &mut String) {}
}

/// Rewrite a whole program.
pub fn rewrite_decls(decls: &mut [Decl], r: &mut impl Rewrite) {
    for decl in decls {
        rewrite_decl(decl, r);
    }
}

/// Rewrite one declaration: its signature types, its bodies, its patterns.
///
/// The declaration's *own* name is not touched — a rewriter that renames
/// declarations has to decide that per declaration anyway, and doing it here
/// would rename a struct's fields' names along with it.
pub fn rewrite_decl(decl: &mut Decl, r: &mut impl Rewrite) {
    match &mut decl.kind {
        DeclKind::Fn(f) => rewrite_fn(f, r),
        DeclKind::Struct(s) => {
            for field in &mut s.fields {
                r.ty(&mut field.ty);
                if let Some(d) = &mut field.default {
                    rewrite_expr(d, r);
                }
            }
            for m in &mut s.methods {
                rewrite_fn(m, r);
            }
        }
        DeclKind::Enum(e) => {
            for v in &mut e.variants {
                for field in &mut v.fields {
                    r.ty(&mut field.ty);
                    if let Some(d) = &mut field.default {
                        rewrite_expr(d, r);
                    }
                }
            }
            for m in &mut e.methods {
                rewrite_fn(m, r);
            }
        }
        DeclKind::Trait(t) => {
            for s in &mut t.super_traits {
                r.ty(s);
            }
            for m in &mut t.methods {
                rewrite_fn(m, r);
            }
        }
        DeclKind::Impl(i) => {
            for t in &mut i.trait_names {
                r.ty(t);
            }
            r.ty(&mut i.target_ty);
            for b in &mut i.where_bounds {
                for bound in &mut b.bounds {
                    r.ty(bound);
                }
            }
            for m in &mut i.methods {
                rewrite_fn(m, r);
            }
        }
        DeclKind::Const(c) => {
            if let Some(t) = &mut c.ty {
                r.ty(t);
            }
            rewrite_expr(&mut c.init, r);
        }
        DeclKind::Test(t) => rewrite_body(&mut t.body, r),
        DeclKind::Benchmark(b) => rewrite_body(&mut b.body, r),
        DeclKind::Union(u) => {
            for field in &mut u.fields {
                r.ty(&mut field.ty);
            }
        }
        DeclKind::TypeAlias(a) => r.ty(&mut a.target),
        DeclKind::Annotation(a) => {
            for field in &mut a.fields {
                r.ty(&mut field.ty);
                if let Some(d) = &mut field.default {
                    rewrite_expr(d, r);
                }
            }
        }
        DeclKind::Extern(e) => {
            for p in &mut e.params {
                r.ty(&mut p.ty);
            }
            if let Some(t) = &mut e.ret_ty {
                r.ty(t);
            }
        }
        // An import names a package and a member of it, a C import names a
        // header, an export names what it re-exports, a package block is
        // metadata. None of them holds a type or a body.
        DeclKind::Import(_) | DeclKind::Export(_) | DeclKind::CImport(_) | DeclKind::Package(_) => {}
    }
}

fn rewrite_fn(f: &mut FnDecl, r: &mut impl Rewrite) {
    for tp in &mut f.type_params {
        for bound in &mut tp.bounds {
            r.ty(bound);
        }
        if let Some(t) = &mut tp.comptime_type {
            r.ty(t);
        }
    }
    for p in &mut f.params {
        r.ty(&mut p.ty);
        if let Some(d) = &mut p.default {
            rewrite_expr(d, r);
        }
    }
    if let Some(t) = &mut f.ret_ty {
        r.ty(t);
    }
    for c in &mut f.context_clauses {
        r.ty(&mut c.ty);
    }
    rewrite_body(&mut f.body, r);
}

/// Every expression in a statement list.
pub fn rewrite_body(body: &mut [Stmt], r: &mut impl Rewrite) {
    for stmt in body {
        rewrite_stmt(stmt, r);
    }
}

/// Every expression, pattern and type in one statement.
pub fn rewrite_stmt(stmt: &mut Stmt, r: &mut impl Rewrite) {
    match &mut stmt.kind {
        StmtKind::Expr(e) => rewrite_expr(e, r),

        StmtKind::Mut { ty, init, .. } | StmtKind::Let { ty, init, .. } => {
            if let Some(t) = ty {
                r.ty(t);
            }
            rewrite_expr(init, r);
        }
        StmtKind::MutTuple { init, .. } | StmtKind::LetTuple { init, .. } => rewrite_expr(init, r),
        StmtKind::LetStruct { pattern, init, .. } => {
            rewrite_pattern(pattern, r);
            rewrite_expr(init, r);
        }

        StmtKind::Assign { target, value, .. } => {
            rewrite_expr(target, r);
            rewrite_expr(value, r);
        }

        StmtKind::Return(value) | StmtKind::Break { value, .. } => {
            if let Some(e) = value {
                rewrite_expr(e, r);
            }
        }
        StmtKind::Continue(_) | StmtKind::Discard { .. } => {}

        StmtKind::While { cond, body, .. } => {
            rewrite_expr(cond, r);
            rewrite_body(body, r);
        }
        StmtKind::WhileLet { pattern, expr, body, .. } => {
            rewrite_pattern(pattern, r);
            rewrite_expr(expr, r);
            rewrite_body(body, r);
        }
        StmtKind::Loop { body, .. } | StmtKind::Comptime(body) => rewrite_body(body, r),
        StmtKind::For { iter, body, .. } | StmtKind::ComptimeFor { iter, body, .. } => {
            rewrite_expr(iter, r);
            rewrite_body(body, r);
        }
        StmtKind::Ensure { body, else_handler } => {
            rewrite_body(body, r);
            if let Some((_, handler)) = else_handler {
                rewrite_body(handler, r);
            }
        }
    }
}

/// Every expression in `expr` and below it, outermost first.
pub fn rewrite_expr(expr: &mut Expr, r: &mut impl Rewrite) {
    r.expr(expr);
    match &mut expr.kind {
        // Leaves. `Ident` is one for the walk and not for the rewriter — the
        // callback above has already seen it.
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
                    StringSegment::Expr(e, _) => rewrite_expr(e, r),
                }
            }
        }

        ExprKind::Binary { left, right, .. } => {
            rewrite_expr(left, r);
            rewrite_expr(right, r);
        }
        ExprKind::Unary { operand, .. } => rewrite_expr(operand, r),

        ExprKind::Call { func, args } => {
            rewrite_expr(func, r);
            for a in args {
                rewrite_expr(&mut a.expr, r);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            rewrite_expr(object, r);
            for a in args {
                rewrite_expr(&mut a.expr, r);
            }
        }

        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            rewrite_expr(object, r)
        }
        ExprKind::DynamicField { object, field_expr } => {
            rewrite_expr(object, r);
            rewrite_expr(field_expr, r);
        }
        ExprKind::Index { object, index } => {
            rewrite_expr(object, r);
            rewrite_expr(index, r);
        }

        ExprKind::Block(body)
        | ExprKind::Spawn { body }
        | ExprKind::BlockCall { body, .. }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::Loop { body, .. } => rewrite_body(body, r),

        ExprKind::If { cond, then_branch, else_branch, .. } => {
            rewrite_expr(cond, r);
            rewrite_expr(then_branch, r);
            if let Some(e) = else_branch {
                rewrite_expr(e, r);
            }
        }
        ExprKind::IfLet { expr: scrutinee, pattern, then_branch, else_branch, .. } => {
            rewrite_expr(scrutinee, r);
            rewrite_pattern(pattern, r);
            rewrite_expr(then_branch, r);
            if let Some(e) = else_branch {
                rewrite_expr(e, r);
            }
        }
        ExprKind::GuardPattern { expr: scrutinee, pattern, else_branch } => {
            rewrite_expr(scrutinee, r);
            rewrite_pattern(pattern, r);
            rewrite_expr(else_branch, r);
        }
        ExprKind::IsPattern { expr: scrutinee, pattern } => {
            rewrite_expr(scrutinee, r);
            rewrite_pattern(pattern, r);
        }

        ExprKind::Match { scrutinee, arms } => {
            rewrite_expr(scrutinee, r);
            for arm in arms {
                rewrite_pattern(&mut arm.pattern, r);
                if let Some(g) = &mut arm.guard {
                    rewrite_expr(g, r);
                }
                rewrite_expr(&mut arm.body, r);
            }
        }

        ExprKind::Try { expr: inner }
        | ExprKind::Take { place: inner }
        | ExprKind::IsPresent { expr: inner, .. }
        | ExprKind::Unwrap { expr: inner, .. } => rewrite_expr(inner, r),

        ExprKind::Cast { expr: inner, ty } => {
            rewrite_expr(inner, r);
            r.ty(ty);
        }
        ExprKind::Convert { expr: inner, target, .. } => {
            rewrite_expr(inner, r);
            r.ty(target);
        }

        ExprKind::Catch { value, clause } => {
            rewrite_expr(value, r);
            rewrite_expr(&mut clause.body, r);
        }
        ExprKind::NullCoalesce { value, default } => {
            rewrite_expr(value, r);
            rewrite_expr(default, r);
        }

        ExprKind::Range { start, end, .. } => {
            if let Some(e) = start {
                rewrite_expr(e, r);
            }
            if let Some(e) = end {
                rewrite_expr(e, r);
            }
        }

        ExprKind::StructLit { fields, spread, .. } => {
            for field in fields {
                rewrite_expr(&mut field.value, r);
            }
            if let Some(e) = spread {
                rewrite_expr(e, r);
            }
        }

        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                rewrite_expr(e, r);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            rewrite_expr(value, r);
            rewrite_expr(count, r);
        }

        ExprKind::UsingBlock { args, body, .. } => {
            for a in args {
                rewrite_expr(&mut a.expr, r);
            }
            rewrite_body(body, r);
        }
        ExprKind::WithAs { bindings, body } => {
            for b in bindings {
                rewrite_expr(&mut b.source, r);
            }
            rewrite_body(body, r);
        }

        ExprKind::Closure { params, ret_ty, body, .. } => {
            for p in params {
                if let Some(t) = &mut p.ty {
                    r.ty(t);
                }
            }
            if let Some(t) = ret_ty {
                r.ty(t);
            }
            rewrite_expr(body, r);
        }

        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &mut arm.kind {
                    SelectArmKind::Recv { channel, .. } => rewrite_expr(channel, r),
                    SelectArmKind::Send { channel, value } => {
                        rewrite_expr(channel, r);
                        rewrite_expr(value, r);
                    }
                    SelectArmKind::Default => {}
                }
                rewrite_expr(&mut arm.body, r);
            }
        }

        ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
            rewrite_expr(condition, r);
            if let Some(m) = message {
                rewrite_expr(m, r);
            }
        }
    }
}

/// Every pattern in `pattern` and below it, outermost first.
pub fn rewrite_pattern(pattern: &mut Pattern, r: &mut impl Rewrite) {
    r.pattern(pattern);
    match pattern {
        // `Ident` binds a name and `Wildcard` binds nothing; neither names a
        // declaration, and the callback has seen both already.
        Pattern::Wildcard | Pattern::Ident(_) => {}
        Pattern::Literal(e) => rewrite_expr(e, r),
        Pattern::Constructor { fields, .. } => {
            for f in fields {
                rewrite_pattern(f, r);
            }
        }
        Pattern::Struct { fields, .. } => {
            for (_, f) in fields {
                rewrite_pattern(f, r);
            }
        }
        Pattern::Tuple(parts) | Pattern::Or(parts) => {
            for p in parts {
                rewrite_pattern(p, r);
            }
        }
        Pattern::Range { start, end } => {
            rewrite_expr(start, r);
            rewrite_expr(end, r);
        }
        Pattern::TypePat { ty_name, .. } => r.ty(ty_name),
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! type.errors/ER50: an expression statement whose value is a `T or E`.
//!
//! Run over the program once types are settled, since a statement's type can
//! still be open when the statement is checked. The rule is about the
//! statement's type, not the shape of the expression: `save()`,
//! `thread.join()!` (which strips the join error and leaves the closure's own
//! result) and `x catch _ => save()` all end in a result nobody looks at.
//!
//! Which statements are discarded comes from the shared walk in
//! `rask_ast::visit`, which announces every statement list it enters and says
//! whether the last statement is the list's value. A list whose last statement
//! is its value (a block used as an expression, a `using` or `with` body, a
//! closure body) doesn't count that statement; a function's, a loop's or an
//! `ensure`'s `else` body counts all of them, because none has a value of its
//! own. An `ensure` body's last statement is the cleanup: what it fails with is
//! the ensure's to handle.
//!
//! The diagnostic offers `try` only when the function (or closure) around the
//! statement could carry the error out, so each statement is walked with what
//! its enclosing body returns.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{visit_body, visit_expr, Visit};
use rask_types::Type;

use crate::{TypeError, TypedProgram};

/// Where a `try` in the current body would send an error.
#[derive(Clone)]
enum Exit {
    /// The body's declared return type.
    Return(Type),
    /// A `test` block turns any propagated error into the test's failure
    /// (std.testing/T20).
    Test,
    /// Nowhere: an `ensure` handler (ctrl.ensure/ER3), a body with no result.
    Nowhere,
}

/// Every discarded `T or E` in `decls`.
pub fn unused_results(decls: &[Decl], typed: &TypedProgram) -> Vec<TypeError> {
    let mut walk = Walk { typed, found: Vec::new(), exit: Exit::Nowhere };
    for decl in decls {
        for (exit, body) in bodies(decl, typed) {
            walk.exit = exit;
            walk.count_list(body, false);
            visit_body(body, &mut walk);
        }
        if let DeclKind::Const(c) = &decl.kind {
            walk.exit = Exit::Nowhere;
            visit_expr(&c.init, &mut walk);
        }
    }
    walk.found
}

/// The statement lists a declaration runs (function, method and test bodies),
/// each with where a `try` inside it would propagate to.
fn bodies<'d>(decl: &'d Decl, typed: &TypedProgram) -> Vec<(Exit, &'d [Stmt])> {
    let of = |f: &'d FnDecl| {
        let exit = typed
            .fn_sigs
            .get(&f.id)
            .map_or(Exit::Nowhere, |sig| Exit::Return(sig.ret.clone()));
        (exit, f.body.as_slice())
    };
    match &decl.kind {
        DeclKind::Fn(f) => vec![of(f)],
        DeclKind::Struct(s) => s.methods.iter().map(of).collect(),
        DeclKind::Enum(e) => e.methods.iter().map(of).collect(),
        DeclKind::Impl(i) => i.methods.iter().map(of).collect(),
        DeclKind::Interface(t) => t.methods.iter().map(of).collect(),
        DeclKind::Test(t) => vec![(Exit::Test, t.body.as_slice())],
        DeclKind::Benchmark(b) => vec![(Exit::Nowhere, b.body.as_slice())],
        _ => Vec::new(),
    }
}

struct Walk<'t> {
    typed: &'t TypedProgram,
    found: Vec<TypeError>,
    exit: Exit,
}

impl Walk<'_> {
    /// The statements of one list; the last only when the list has no value.
    fn count_list(&mut self, stmts: &[Stmt], last_is_value: bool) {
        let n = if last_is_value { stmts.len().saturating_sub(1) } else { stmts.len() };
        for stmt in &stmts[..n] {
            if let StmtKind::Expr(expr) = &stmt.kind {
                self.discarded(expr);
            }
        }
    }

    /// An expression whose value goes nowhere. An `if`, `match`, block or
    /// `select` written as a statement throws away what its branches end with,
    /// and a `catch` handler's value goes nowhere too (the handler stands in
    /// for the `catch`'s own success value). So those are discards of their
    /// tails: `if verbose { save() }` drops `save`'s error. The other
    /// statements of a block are the block's own list's. Anything else is
    /// judged by its own type.
    fn discarded(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::If { then_branch, else_branch, .. }
            | ExprKind::IfLet { then_branch, else_branch, .. } => {
                self.discarded(then_branch);
                if let Some(e) = else_branch {
                    self.discarded(e);
                }
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    self.discarded(&arm.body);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    self.discarded(&arm.body);
                }
            }
            ExprKind::Catch { clause, .. } => self.discarded(&clause.body),
            ExprKind::Block(stmts)
            | ExprKind::Unsafe { body: stmts, .. }
            | ExprKind::Comptime { body: stmts }
            | ExprKind::UsingBlock { body: stmts, .. }
            | ExprKind::WithAs { body: stmts, .. } => {
                if let Some(Stmt { kind: StmtKind::Expr(last), .. }) = stmts.last() {
                    self.discarded(last);
                }
            }
            _ => self.by_type(expr),
        }
    }

    /// Report `expr` when its value is a `T or E`.
    fn by_type(&mut self, expr: &Expr) {
        let Some(result @ Type::Result { err, .. }) = self.typed.node_types.get(&expr.id) else { return };
        if matches!(**err, Type::None | Type::Never) {
            return;
        }
        let can_try = self.can_propagate(&self.typed.types.resolve_type_names(err));
        self.found.push(TypeError::UnusedResult {
            ty: self.typed.types.resolve_type_names(result).to_string(),
            call: describe(expr),
            can_try,
            span: expr.span,
        });
    }

    /// Can `try` pass an error of type `err` out of the current body? The
    /// answers the checker gives a `try` written there: the error is the
    /// return's error, a member of its union (ER31), boxed into `any I`
    /// (ER32), or wrapped in the one variant of its error enum made for it
    /// (ER31a).
    fn can_propagate(&self, err: &Type) -> bool {
        let types = &self.typed.types;
        let target = match &self.exit {
            Exit::Test => return true,
            Exit::Nowhere => return false,
            Exit::Return(ret) => match types.resolve_type_names(ret) {
                Type::Result { err: target, .. } => *target,
                _ => return false,
            },
        };
        if matches!(target, Type::None | Type::Never | Type::Var(_)) {
            return false;
        }
        let members = |t: &Type| match t {
            Type::Union(ms) => ms.clone(),
            other => vec![other.clone()],
        };
        let wanted = members(&target);
        if members(err).iter().all(|m| wanted.contains(m)) {
            return true;
        }
        if let Type::InterfaceObject { interface_name, decl } = &target {
            return crate::interfaces::implements_interface_object(types, err, interface_name, *decl);
        }
        types.error_wrap_variants(err, &target).len() == 1
    }
}

/// The expression as a fix will write it: the callee with its arguments
/// elided (`save(…)`, `….write_text(…)`), a `!` on top of that, or `(…)` for
/// anything else, which stands for the whole expression.
fn describe(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Call { func, .. } => match &func.kind {
            ExprKind::Ident(name) => format!("{name}(…)"),
            ExprKind::Field { object, field, .. } => match &object.kind {
                ExprKind::Ident(base) => format!("{base}.{field}(…)"),
                _ => format!("….{field}(…)"),
            },
            _ => "(…)".to_string(),
        },
        ExprKind::MethodCall { object, method, .. } => match &object.kind {
            ExprKind::Ident(base) => format!("{base}.{method}(…)"),
            _ => format!("….{method}(…)"),
        },
        ExprKind::Unwrap { expr: inner, .. } => format!("{}!", describe(inner)),
        _ => "(…)".to_string(),
    }
}

impl<'a> Visit<'a> for Walk<'_> {
    fn list(&mut self, stmts: &'a [Stmt], last_is_value: bool) {
        self.count_list(stmts, last_is_value);
    }

    fn expr(&mut self, expr: &'a Expr) -> bool {
        // A closure is its own body: its `try` leaves the closure, so what it
        // returns decides whether `try` is on offer inside it.
        if let ExprKind::Closure { body, .. } = &expr.kind {
            let outer = std::mem::replace(
                &mut self.exit,
                match self.typed.node_types.get(&expr.id) {
                    Some(Type::Fn { ret, .. }) => Exit::Return((**ret).clone()),
                    _ => Exit::Nowhere,
                },
            );
            visit_expr(body, self);
            self.exit = outer;
            return false;
        }
        true
    }

    fn stmt(&mut self, stmt: &'a Stmt) -> bool {
        // An `ensure`'s cleanup is its last statement, whose failure is the
        // ensure's own business: its `else` takes it, and without one it's
        // dropped by design (ensure's error rule). Nothing can propagate out
        // of the body or the handler, since cleanup runs at scope exit.
        if let StmtKind::Ensure { body, else_handler } = &stmt.kind {
            let outer = std::mem::replace(&mut self.exit, Exit::Nowhere);
            self.count_list(body, true);
            visit_body(body, self);
            if let Some((_, handler)) = else_handler {
                self.count_list(handler, false);
                visit_body(handler, self);
            }
            self.exit = outer;
            return false;
        }
        true
    }
}

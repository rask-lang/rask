// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! type.errors/W2: a call whose `T or E` result nobody looks at.
//!
//! Run over the program once types are settled, since a statement's type can
//! still be open when the statement is checked. A statement list whose last
//! statement is its value (a block used as an expression: an `if` arm, a
//! closure body) doesn't count that statement; a function's or a loop's body
//! counts all of them, because neither has a value of its own. An `ensure`'s
//! body is the exception: what its cleanup fails with is the ensure's to
//! handle.

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{visit_body, visit_expr, Visit};
use rask_types::Type;

use crate::{TypeError, TypedProgram};

/// Every discarded `T or E` in `decls`.
pub fn unused_results(decls: &[Decl], typed: &TypedProgram) -> Vec<TypeError> {
    let mut walk = Walk { typed, found: Vec::new() };
    for decl in decls {
        for body in bodies(decl) {
            walk.list(body, true);
            visit_body(body, &mut walk);
        }
        if let DeclKind::Const(c) = &decl.kind {
            visit_expr(&c.init, &mut walk);
        }
    }
    walk.found
}

/// The statement lists a declaration runs: function, method and test bodies.
fn bodies(decl: &Decl) -> Vec<&[Stmt]> {
    match &decl.kind {
        DeclKind::Fn(f) => vec![&f.body],
        DeclKind::Struct(s) => s.methods.iter().map(|m| m.body.as_slice()).collect(),
        DeclKind::Enum(e) => e.methods.iter().map(|m| m.body.as_slice()).collect(),
        DeclKind::Impl(i) => i.methods.iter().map(|m| m.body.as_slice()).collect(),
        DeclKind::Interface(t) => t.methods.iter().map(|m| m.body.as_slice()).collect(),
        DeclKind::Test(t) => vec![&t.body],
        DeclKind::Benchmark(b) => vec![&b.body],
        _ => Vec::new(),
    }
}

struct Walk<'t> {
    typed: &'t TypedProgram,
    found: Vec<TypeError>,
}

impl Walk<'_> {
    /// The statements of one list; the last only when the list has no value.
    fn list(&mut self, stmts: &[Stmt], last_too: bool) {
        let n = if last_too { stmts.len() } else { stmts.len().saturating_sub(1) };
        for stmt in &stmts[..n] {
            if let StmtKind::Expr(expr) = &stmt.kind {
                self.statement(expr);
            }
        }
    }

    fn statement(&mut self, expr: &Expr) {
        if !matches!(expr.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) {
            return;
        }
        let Some(result @ Type::Result { err, .. }) = self.typed.node_types.get(&expr.id) else { return };
        if matches!(**err, Type::None | Type::Never) {
            return;
        }
        self.found.push(TypeError::UnusedResult {
            ty: self.typed.types.resolve_type_names(result).to_string(),
            span: expr.span,
        });
    }
}

impl<'a> Visit<'a> for Walk<'_> {
    fn expr(&mut self, expr: &'a Expr) -> bool {
        if let ExprKind::Block(stmts) = &expr.kind {
            self.list(stmts, false);
        }
        true
    }

    fn stmt(&mut self, stmt: &'a Stmt) -> bool {
        match &stmt.kind {
            StmtKind::While { body, .. }
            | StmtKind::WhileLet { body, .. }
            | StmtKind::Loop { body, .. }
            | StmtKind::For { body, .. }
            | StmtKind::ComptimeFor { body, .. }
            | StmtKind::Comptime(body) => self.list(body, true),
            // An `ensure`'s failure is the ensure's own business: its `else`
            // takes it, and without one it's dropped by design (ensure's
            // error rule), so its body's call isn't a discard.
            StmtKind::Ensure { else_handler, .. } => {
                if let Some((_, handler)) = else_handler {
                    self.list(handler, true);
                }
            }
            _ => {}
        }
        true
    }
}

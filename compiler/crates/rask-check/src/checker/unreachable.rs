// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The unreachable-code warning (tool.warnings/W4): a statement that comes
//! after one control never gets past.
//!
//! Run once the whole program is checked, because whether a call comes back
//! is read off its callee, and a method's callee is only settled once the
//! receiver's type is.

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;

use super::errors::TypeError;
use super::TypeChecker;

/// What stops control in front of an unreachable statement, for the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leaves {
    Return,
    Break,
    Continue,
    /// A call whose callee is `@noreturn`: `panic`, `exit` and the like.
    NoReturnCall,
    /// A `loop` with no `break` in it.
    EndlessLoop,
    /// An `if` or `match` whose every branch leaves.
    EveryBranch,
}

impl TypeChecker {
    /// W4 over the program's bodies. A body whose function (or test block)
    /// says `@allow(unreachable_code)` is skipped.
    pub(super) fn check_unreachable_code(&mut self, decls: &[Decl]) {
        let mut found: Vec<TypeError> = Vec::new();
        let mut scan = |attrs: &[rask_ast::attr::Attr], body: &[Stmt]| {
            let allowed = attrs
                .iter()
                .filter_map(rask_ast::allow_names::allowed_name)
                .any(|a| a == "unreachable_code");
            if !allowed {
                self.scan_unreachable(body, &mut found);
            }
        };
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => scan(&f.attrs, &f.body),
                DeclKind::Impl(i) => {
                    for m in &i.methods {
                        scan(&m.attrs, &m.body);
                    }
                }
                DeclKind::Test(t) => scan(&t.attrs, &t.body),
                DeclKind::Benchmark(b) => scan(&b.attrs, &b.body),
                _ => {}
            }
        }
        // Nested lists are scanned after the list that holds them; report in
        // source order instead.
        found.sort_by_key(|e| match e {
            TypeError::UnreachableCode { span, .. } => (span.file_id, span.start),
            _ => (0, 0),
        });
        self.errors.extend(found);
    }

    /// The body and every statement list nested in it.
    fn scan_unreachable(&self, body: &[Stmt], found: &mut Vec<TypeError>) {
        struct Lists<'a, 'c> {
            checker: &'c TypeChecker,
            found: &'c mut Vec<TypeError>,
            _body: std::marker::PhantomData<&'a ()>,
        }
        impl<'a> rask_ast::visit::Visit<'a> for Lists<'a, '_> {
            fn expr(&mut self, _: &'a Expr) -> bool {
                true
            }
            fn list(&mut self, stmts: &'a [Stmt], _last_is_value: bool) {
                self.checker.unreachable_in_list(stmts, self.found);
            }
        }
        self.unreachable_in_list(body, found);
        let mut v = Lists { checker: self, found, _body: std::marker::PhantomData };
        rask_ast::visit::visit_body(body, &mut v);
    }

    /// One warning per list, at the first statement nothing reaches. The
    /// statements after it are the same dead stretch, not news.
    fn unreachable_in_list(&self, stmts: &[Stmt], found: &mut Vec<TypeError>) {
        for pair in stmts.windows(2) {
            if let Some(leaves) = self.stmt_leaves(&pair[0]) {
                let (first, next) = (&pair[0], &pair[1]);
                // A statement a pass made has no source of its own to point at.
                if next.span == Span::new(0, 0) || next.span.start == next.span.end {
                    return;
                }
                found.push(TypeError::UnreachableCode {
                    span: next.span,
                    after: first.span,
                    leaves,
                });
                return;
            }
        }
    }

    /// Does control never get past this statement, and why?
    fn stmt_leaves(&self, stmt: &Stmt) -> Option<Leaves> {
        match &stmt.kind {
            StmtKind::Return(_) => Some(Leaves::Return),
            StmtKind::Break { .. } => Some(Leaves::Break),
            StmtKind::Continue(_) => Some(Leaves::Continue),
            // A `break` anywhere inside counts as a way out, even one that
            // leaves an inner loop: a warning that is sometimes missed beats
            // one that is sometimes wrong.
            StmtKind::Loop { body, .. } => (!Self::contains_break(body)).then_some(Leaves::EndlessLoop),
            StmtKind::Expr(e)
            | StmtKind::Let { init: e, .. }
            | StmtKind::Mut { init: e, .. }
            | StmtKind::LetTuple { init: e, .. }
            | StmtKind::MutTuple { init: e, .. }
            | StmtKind::LetStruct { init: e, .. } => self.expr_leaves(e),
            _ => None,
        }
    }

    fn expr_leaves(&self, expr: &Expr) -> Option<Leaves> {
        let all = |branches: &mut dyn Iterator<Item = &Expr>| {
            let mut any = false;
            for b in branches {
                self.expr_leaves(b)?;
                any = true;
            }
            any.then_some(Leaves::EveryBranch)
        };
        match &expr.kind {
            ExprKind::Block(stmts) | ExprKind::Unsafe { body: stmts } => {
                stmts.iter().find_map(|s| self.stmt_leaves(s))
            }
            ExprKind::If { then_branch, else_branch: Some(else_br), .. }
            | ExprKind::IfLet { then_branch, else_branch: Some(else_br), .. } => {
                all(&mut [then_branch.as_ref(), else_br.as_ref()].into_iter())
            }
            ExprKind::Match { arms, .. } => all(&mut arms.iter().map(|a| a.body.as_ref())),
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                self.call_never_returns(expr.id).then_some(Leaves::NoReturnCall)
            }
            _ => None,
        }
    }

    fn contains_break(body: &[Stmt]) -> bool {
        struct Finder(bool);
        impl<'a> rask_ast::visit::Visit<'a> for Finder {
            fn expr(&mut self, _: &'a Expr) -> bool {
                !self.0
            }
            fn stmt(&mut self, s: &'a Stmt) -> bool {
                if matches!(s.kind, StmtKind::Break { .. }) {
                    self.0 = true;
                }
                !self.0
            }
        }
        let mut f = Finder(false);
        rask_ast::visit::visit_body(body, &mut f);
        f.0
    }
}

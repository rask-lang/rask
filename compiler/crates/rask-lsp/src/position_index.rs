// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! AST visitor that builds a span-to-node lookup table.

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};

/// Maps source positions to AST nodes for fast lookup.
#[derive(Debug, Clone)]
pub struct PositionIndex {
    /// All expressions with their spans and node IDs
    pub exprs: Vec<(Span, NodeId)>,
    /// Identifiers specifically (for go-to-definition)
    pub idents: Vec<(Span, NodeId, String)>,
}

impl PositionIndex {
    pub fn new() -> Self {
        Self {
            exprs: Vec::new(),
            idents: Vec::new(),
        }
    }

    /// Find the innermost node containing the given byte offset.
    pub fn node_at_position(&self, offset: usize) -> Option<NodeId> {
        self.exprs
            .iter()
            .filter(|(span, _)| span.start <= offset && offset <= span.end)
            .min_by_key(|(span, _)| span.end - span.start) // Smallest span
            .map(|(_, node_id)| *node_id)
    }

    /// Find identifier at the given byte offset.
    pub fn ident_at_position(&self, offset: usize) -> Option<(NodeId, String)> {
        self.idents
            .iter()
            .find(|(span, _, _)| span.start <= offset && offset <= span.end)
            .map(|(_, node_id, name)| (*node_id, name.clone()))
    }

    /// Sort spans for efficient lookup (call after building).
    pub fn finalize(&mut self) {
        self.exprs.sort_by_key(|(span, _)| span.start);
        self.idents.sort_by_key(|(span, _, _)| span.start);
    }
}

/// Build position index by traversing the AST.
pub fn build_position_index(decls: &[Decl]) -> PositionIndex {
    let mut index = PositionIndex::new();
    for decl in decls {
        visit_decl(decl, &mut index);
    }
    index
}

fn visit_decl(decl: &Decl, index: &mut PositionIndex) {
    match &decl.kind {
        DeclKind::Fn(fn_decl) => {
            for stmt in &fn_decl.body {
                visit_stmt(stmt, index);
            }
        }
        DeclKind::Const(const_decl) => {
            visit_expr(&const_decl.init, index);
        }
        DeclKind::Test(test_decl) => {
            for stmt in &test_decl.body {
                visit_stmt(stmt, index);
            }
        }
        DeclKind::Benchmark(bench_decl) => {
            for stmt in &bench_decl.body {
                visit_stmt(stmt, index);
            }
        }
        DeclKind::Impl(impl_decl) => {
            for method in &impl_decl.methods {
                for stmt in &method.body {
                    visit_stmt(stmt, index);
                }
            }
        }
        DeclKind::Trait(trait_decl) => {
            for method in &trait_decl.methods {
                for stmt in &method.body {
                    visit_stmt(stmt, index);
                }
            }
        }
        _ => {}
    }
}

fn visit_stmt(stmt: &Stmt, index: &mut PositionIndex) {
    match &stmt.kind {
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => {
            visit_expr(e, index);
        }
        StmtKind::Break { value: Some(value), .. } => {
            visit_expr(value, index);
        }
        StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
            visit_expr(init, index);
        }
        StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
            visit_expr(init, index);
        }
        StmtKind::Assign { target, value } => {
            visit_expr(target, index);
            visit_expr(value, index);
        }
        StmtKind::While { cond, body } => {
            visit_expr(cond, index);
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::WhileLet { expr, body, .. } => {
            visit_expr(expr, index);
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::For { iter, body, .. } => {
            visit_expr(iter, index);
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::Loop { body, .. } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::Ensure { body, else_handler } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
            if let Some((_, handler)) = else_handler {
                for stmt in handler {
                    visit_stmt(stmt, index);
                }
            }
        }
        StmtKind::Comptime(stmts) => {
            for stmt in stmts {
                visit_stmt(stmt, index);
            }
        }
        _ => {}
    }
}

fn visit_expr(expr: &Expr, index: &mut PositionIndex) {
    // Record this expression
    index.exprs.push((expr.span, expr.id));

    // Record identifiers separately
    if let ExprKind::Ident(name) = &expr.kind {
        index.idents.push((expr.span, expr.id, name.clone()));
    }

    // Recursively visit child expressions
    match &expr.kind {
        ExprKind::Binary { left, right, .. } => {
            visit_expr(left, index);
            visit_expr(right, index);
        }
        ExprKind::Unary { operand, .. } => {
            visit_expr(operand, index);
        }
        ExprKind::Call { func, args } => {
            visit_expr(func, index);
            for arg in args {
                visit_expr(arg, index);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            visit_expr(object, index);
            for arg in args {
                visit_expr(arg, index);
            }
        }
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            visit_expr(object, index);
        }
        ExprKind::Index { object, index: idx } => {
            visit_expr(object, index);
            visit_expr(idx, index);
        }
        ExprKind::Block(stmts) => {
            for stmt in stmts {
                visit_stmt(stmt, index);
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            visit_expr(cond, index);
            visit_expr(then_branch, index);
            if let Some(else_br) = else_branch {
                visit_expr(else_br, index);
            }
        }
        ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
            visit_expr(expr, index);
            visit_expr(then_branch, index);
            if let Some(else_br) = else_branch {
                visit_expr(else_br, index);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            visit_expr(scrutinee, index);
            for arm in arms {
                if let Some(ref guard) = arm.guard {
                    visit_expr(guard, index);
                }
                visit_expr(&arm.body, index);
            }
        }
        ExprKind::Try(e) => {
            visit_expr(e, index);
        }
        ExprKind::Unwrap(e) => {
            visit_expr(e, index);
        }
        ExprKind::NullCoalesce { value, default } => {
            visit_expr(value, index);
            visit_expr(default, index);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                visit_expr(s, index);
            }
            if let Some(e) = end {
                visit_expr(e, index);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for field_init in fields {
                visit_expr(&field_init.value, index);
            }
        }
        ExprKind::Tuple(exprs) => {
            for e in exprs {
                visit_expr(e, index);
            }
        }
        ExprKind::Array(exprs) => {
            for e in exprs {
                visit_expr(e, index);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            visit_expr(value, index);
            visit_expr(count, index);
        }
        ExprKind::Closure { body, .. } => {
            visit_expr(body, index);
        }
        ExprKind::Spawn { body } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        ExprKind::UsingBlock { body, .. } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        _ => {}
    }
}

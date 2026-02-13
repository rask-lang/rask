// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Reachability analysis - walk call graph from main() to find generic instantiations.

use rask_ast::{
    decl::{Decl, DeclKind},
    expr::{Expr, ExprKind},
    stmt::{Stmt, StmtKind},
};
use rask_types::Type;
use std::collections::{HashMap, HashSet, VecDeque};

/// Reachability walker - discovers all reachable function calls
struct ReachabilityWalker {
    /// Functions discovered so far: (name, type_args) -> visited
    discovered: HashMap<(String, Vec<Type>), bool>,
    /// Work queue for breadth-first traversal
    work_queue: VecDeque<(String, Vec<Type>, Decl)>,
}

impl ReachabilityWalker {
    fn new() -> Self {
        Self {
            discovered: HashMap::new(),
            work_queue: VecDeque::new(),
        }
    }

    /// Add entry point to work queue
    fn add_entry(&mut self, entry: &Decl) {
        let fn_decl = match &entry.kind {
            DeclKind::Fn(f) => f,
            _ => return,
        };

        let key = (fn_decl.name.clone(), Vec::new()); // Entry has no type args
        if !self.discovered.contains_key(&key) {
            self.discovered.insert(key.clone(), false);
            self.work_queue
                .push_back((fn_decl.name.clone(), Vec::new(), entry.clone()));
        }
    }

    /// Process work queue until empty
    fn process(&mut self) -> Vec<(String, Vec<Type>)> {
        while let Some((name, type_args, decl)) = self.work_queue.pop_front() {
            let key = (name.clone(), type_args.clone());
            if let Some(visited) = self.discovered.get_mut(&key) {
                if *visited {
                    continue; // Already processed
                }
                *visited = true;
            }

            // Walk function body to find calls
            if let DeclKind::Fn(fn_decl) = &decl.kind {
                for stmt in &fn_decl.body {
                    self.visit_stmt(stmt);
                }
            }
        }

        // Return all discovered functions
        self.discovered.keys().cloned().collect()
    }

    /// Visit statement to find function calls
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.visit_expr(e),
            StmtKind::Let { init, .. } => self.visit_expr(init),
            StmtKind::Const { init, .. } => self.visit_expr(init),
            StmtKind::Assign { target, value } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            StmtKind::Return(Some(e)) => self.visit_expr(e),
            StmtKind::While { cond, body } => {
                self.visit_expr(cond);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Loop { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            _ => {} // TODO: Handle more statement variants
        }
    }

    /// Visit expression to find function calls
    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Call { func, args } => {
                // Check if this is a generic function call
                // TODO: Extract type arguments from call site
                // For now, just recurse
                self.visit_expr(func);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::Unary { operand, .. } => {
                self.visit_expr(operand);
            }
            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    self.visit_stmt(stmt);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(else_br) = else_branch {
                    self.visit_expr(else_br);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_expr(&arm.body);
                }
            }
            _ => {} // TODO: Handle more expression variants
        }
    }
}

/// Collect reachable function instances starting from entry point
///
/// Returns (function_id, concrete_type_args) pairs for all reachable calls
pub fn collect_reachable(entry: &Decl) -> Vec<(String, Vec<Type>)> {
    let mut walker = ReachabilityWalker::new();
    walker.add_entry(entry);
    walker.process()
}

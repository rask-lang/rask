// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization driver - reachability-driven instantiation.
//!
//! Walks the call graph from main(), instantiating generic functions on demand.
//! This is the core loop per spec rules M1-M4:
//!   M1: Walk reachable code from main()
//!   M2: Instantiate each unique (function_id, [type_args])
//!   M3: Compute layouts (done after this pass)
//!   M4: Transitive - new instantiations may discover more calls

use crate::instantiate::instantiate_function;
use crate::MonoFunction;
use rask_ast::{
    decl::{Decl, DeclKind},
    expr::{Expr, ExprKind},
    stmt::{Stmt, StmtKind},
};
use rask_ast::NodeId;
use rask_types::Type;
use std::collections::{HashMap, VecDeque};

/// Monomorphization work item
struct WorkItem {
    name: String,
    type_args: Vec<Type>,
}

/// Drives monomorphization: reachability first, instantiation on demand.
pub struct Monomorphizer<'a> {
    /// Lookup table: function name → original declaration
    fn_table: HashMap<String, &'a Decl>,
    /// Resolved type args per call site (from typechecker)
    call_type_args: &'a HashMap<NodeId, Vec<Type>>,
    /// Already processed (name, type_args) pairs
    seen: HashMap<(String, Vec<Type>), bool>,
    /// BFS work queue
    queue: VecDeque<WorkItem>,
    /// Resulting instantiated functions
    pub results: Vec<MonoFunction>,
}

impl<'a> Monomorphizer<'a> {
    pub fn new(decls: &'a [Decl], call_type_args: &'a HashMap<NodeId, Vec<Type>>) -> Self {
        let mut fn_table = HashMap::new();
        for decl in decls {
            if let DeclKind::Fn(f) = &decl.kind {
                fn_table.insert(f.name.clone(), decl);
            }
        }

        Self {
            fn_table,
            call_type_args,
            seen: HashMap::new(),
            queue: VecDeque::new(),
            results: Vec::new(),
        }
    }

    /// Seed the work queue with main()
    pub fn add_entry(&mut self, name: &str) -> bool {
        if self.fn_table.contains_key(name) {
            self.enqueue(name.to_string(), Vec::new());
            true
        } else {
            false
        }
    }

    /// Run until fixpoint: process queue, instantiate, discover more calls
    pub fn run(&mut self) {
        while let Some(item) = self.queue.pop_front() {
            let key = (item.name.clone(), item.type_args.clone());
            if let Some(visited) = self.seen.get(&key) {
                if *visited {
                    continue;
                }
            }
            self.seen.insert(key, true);

            let original = match self.fn_table.get(&item.name) {
                Some(decl) => *decl,
                None => continue, // External or unknown function
            };

            // Instantiate: if type_args present, clone AST with substitution.
            // Otherwise use original decl directly.
            let concrete = if item.type_args.is_empty() {
                original.clone()
            } else {
                instantiate_function(original, &item.type_args)
            };

            // Walk the concrete body to discover more calls (M4: transitive)
            if let DeclKind::Fn(fn_decl) = &concrete.kind {
                for stmt in &fn_decl.body {
                    self.visit_stmt(stmt);
                }
            }

            self.results.push(MonoFunction {
                name: item.name.clone(),
                type_args: item.type_args,
                body: concrete,
            });
        }
    }

    /// Add a (name, type_args) pair to queue if not already seen
    fn enqueue(&mut self, name: String, type_args: Vec<Type>) {
        let key = (name.clone(), type_args.clone());
        if !self.seen.contains_key(&key) {
            self.seen.insert(key, false);
            self.queue.push_back(WorkItem { name, type_args });
        }
    }

    // --- AST visitors: find calls, enqueue discovered functions ---

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.visit_expr(e),
            StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
                self.visit_expr(init);
            }
            StmtKind::Assign { target, value } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            StmtKind::Return(Some(e)) => self.visit_expr(e),
            StmtKind::Return(None) => {}
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
            StmtKind::Ensure { body, else_handler } => {
                for s in body {
                    self.visit_stmt(s);
                }
                if let Some((_param, handler)) = else_handler {
                    for s in handler {
                        self.visit_stmt(s);
                    }
                }
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.visit_expr(expr);
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::Comptime(body) => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.visit_expr(init);
            }
            StmtKind::Break { value: Some(e), .. } => self.visit_expr(e),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Call { func, args } => {
                if let ExprKind::Ident(name) = &func.kind {
                    let type_args = self.call_type_args
                        .get(&expr.id)
                        .cloned()
                        .unwrap_or_default();
                    self.enqueue(name.clone(), type_args);
                }
                self.visit_expr(func);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.visit_expr(object);
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
            ExprKind::IfLet {
                expr,
                then_branch,
                else_branch,
                ..
            } => {
                self.visit_expr(expr);
                self.visit_expr(then_branch);
                if let Some(else_br) = else_branch {
                    self.visit_expr(else_br);
                }
            }
            ExprKind::GuardPattern {
                expr, else_branch, ..
            } => {
                self.visit_expr(expr);
                self.visit_expr(else_branch);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_expr(&arm.body);
                    if let Some(guard) = &arm.guard {
                        self.visit_expr(guard);
                    }
                }
            }
            ExprKind::Try(e) | ExprKind::Unwrap(e) => self.visit_expr(e),
            ExprKind::NullCoalesce { value, default } => {
                self.visit_expr(value);
                self.visit_expr(default);
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.visit_expr(object);
            }
            ExprKind::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for field in fields {
                    self.visit_expr(&field.value);
                }
                if let Some(s) = spread {
                    self.visit_expr(s);
                }
            }
            ExprKind::Array(elems) => {
                for elem in elems {
                    self.visit_expr(elem);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.visit_expr(value);
                self.visit_expr(count);
            }
            ExprKind::Tuple(elems) => {
                for elem in elems {
                    self.visit_expr(elem);
                }
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            ExprKind::Closure { body, .. } => {
                self.visit_expr(body);
            }
            ExprKind::Cast { expr, .. } => self.visit_expr(expr),
            ExprKind::Spawn { body } | ExprKind::Unsafe { body } | ExprKind::Comptime { body } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.visit_expr(arg);
                }
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::WithAs { bindings, body } => {
                for (expr, _) in bindings {
                    self.visit_expr(expr);
                }
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::BlockCall { body, .. } => {
                for s in body {
                    self.visit_stmt(s);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    self.visit_expr(&arm.body);
                }
            }
            ExprKind::Assert { condition, message }
            | ExprKind::Check { condition, message } => {
                self.visit_expr(condition);
                if let Some(msg) = message {
                    self.visit_expr(msg);
                }
            }
            // Leaves - no sub-expressions
            ExprKind::Int(..)
            | ExprKind::Float(..)
            | ExprKind::String(_)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Ident(_) => {}
        }
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Ownership and borrowing analysis for the Rask language.
//!
//! This crate verifies memory safety by tracking:
//! - Move semantics: detecting use-after-move
//! - Borrow scopes: persistent (block) vs instant (semicolon)
//! - Aliasing rules: shared XOR exclusive access

mod state;
mod error;

pub use state::{BindingState, BorrowMode, BorrowScope, ActiveBorrow};
pub use error::{OwnershipError, OwnershipErrorKind, AccessKind};

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;
use rask_types::{Type, TypedProgram};

/// Result of ownership analysis.
#[derive(Debug)]
pub struct OwnershipResult {
    /// Any errors found during analysis.
    pub errors: Vec<OwnershipError>,
}

impl OwnershipResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Ownership and borrow checker.
pub struct OwnershipChecker<'a> {
    /// The typed program from type checking.
    program: &'a TypedProgram,
    /// State of each binding (owned, moved, borrowed).
    /// Key is the binding name (since we don't have SymbolId in scope).
    bindings: HashMap<String, BindingState>,
    /// Currently active borrows.
    borrows: Vec<ActiveBorrow>,
    /// Current block ID for tracking borrow scopes.
    current_block: u32,
    /// Current statement ID for instant borrows.
    current_stmt: u32,
    /// Errors accumulated during analysis.
    errors: Vec<OwnershipError>,
}

impl<'a> OwnershipChecker<'a> {
    pub fn new(program: &'a TypedProgram) -> Self {
        Self {
            program,
            bindings: HashMap::new(),
            borrows: Vec::new(),
            current_block: 0,
            current_stmt: 0,
            errors: Vec::new(),
        }
    }

    /// Run ownership analysis on all declarations.
    pub fn check(mut self, decls: &[Decl]) -> OwnershipResult {
        for decl in decls {
            self.check_decl(decl);
        }
        OwnershipResult {
            errors: self.errors,
        }
    }

    fn check_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(fn_decl) => self.check_fn(fn_decl),
            DeclKind::Struct(s) => {
                // Check methods
                for method in &s.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Enum(e) => {
                for method in &e.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Trait(_) => {}
            DeclKind::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Import(_) => {}
            DeclKind::Export(_) => {}
            DeclKind::Const(_) => {} // Module-level consts handled differently
            DeclKind::Test(test_decl) => {
                // Check test body like a function
                self.bindings.clear();
                self.borrows.clear();
                self.check_block(&test_decl.body);
            }
            DeclKind::Benchmark(bench_decl) => {
                // Check benchmark body like a function
                self.bindings.clear();
                self.borrows.clear();
                self.check_block(&bench_decl.body);
            }
        }
    }

    fn check_fn(&mut self, fn_decl: &FnDecl) {
        // Reset state for each function (local analysis only)
        self.bindings.clear();
        self.borrows.clear();
        self.current_block = 0;
        self.current_stmt = 0;

        // Register parameters as owned or borrowed bindings
        for param in &fn_decl.params {
            if param.is_take {
                // `take` parameter: owned
                self.bindings.insert(param.name.clone(), BindingState::Owned);
            } else {
                // Default: borrowed (persistent for call duration)
                self.bindings.insert(
                    param.name.clone(),
                    BindingState::Borrowed {
                        mode: BorrowMode::Shared, // Will be upgraded if mutated
                        scope: BorrowScope::Persistent { block_id: 0 },
                    },
                );
            }
        }

        // Check function body
        self.check_block(&fn_decl.body);
    }

    fn check_block(&mut self, stmts: &[Stmt]) {
        let block_id = self.current_block;
        self.current_block += 1;

        for stmt in stmts {
            self.check_stmt(stmt);
            self.current_stmt += 1;

            // Release instant borrows at statement end
            self.release_instant_borrows(self.current_stmt - 1);
        }

        // Release persistent borrows at block end
        self.release_persistent_borrows(block_id);
        self.current_block = block_id;
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, ty: _, init } => {
                // Check the initializer expression
                self.check_expr(init);
                // Handle potential move from initializer
                self.handle_potential_move(init, stmt.span);
                // Register the new binding as owned
                self.bindings.insert(name.clone(), BindingState::Owned);
            }
            StmtKind::LetTuple { names, init } => {
                self.check_expr(init);
                self.handle_potential_move(init, stmt.span);
                for name in names {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                }
            }
            StmtKind::Const { name, ty: _, init } => {
                self.check_expr(init);
                self.handle_potential_move(init, stmt.span);
                self.bindings.insert(name.clone(), BindingState::Owned);
            }
            StmtKind::ConstTuple { names, init } => {
                self.check_expr(init);
                self.handle_potential_move(init, stmt.span);
                for name in names {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                }
            }
            StmtKind::Expr(expr) => {
                self.check_expr(expr);
            }
            StmtKind::Assign { target, value } => {
                self.check_expr(value);
                self.check_expr(target);
                self.handle_potential_move(value, stmt.span);
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.check_expr(expr);
                }
            }
            StmtKind::While { cond, body } => {
                self.check_expr(cond);
                self.check_block(body);
            }
            StmtKind::WhileLet { pattern, expr, body } => {
                self.check_expr(expr);
                self.register_pattern_bindings(pattern);
                self.check_block(body);
            }
            StmtKind::For { label: _, binding, iter, body } => {
                self.check_expr(iter);
                // Register loop binding
                self.bindings.insert(binding.clone(), BindingState::Owned);
                self.check_block(body);
            }
            StmtKind::Loop { label: _, body } => {
                self.check_block(body);
            }
            StmtKind::Break(_) | StmtKind::Continue(_) => {}
            StmtKind::Deliver { label: _, value } => {
                self.check_expr(value);
            }
            StmtKind::Ensure { body, catch } => {
                self.check_block(body);
                if let Some((_name, handler)) = catch {
                    self.check_block(handler);
                }
            }
            StmtKind::Comptime(body) => {
                self.check_block(body);
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                // Check if this identifier is used after move
                if let Some(state) = self.bindings.get(name) {
                    if let BindingState::Moved { at } = state {
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::UseAfterMove {
                                name: name.clone(),
                                moved_at: *at,
                            },
                            span: expr.span,
                        });
                    }
                }
            }
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_)
            | ExprKind::Char(_) | ExprKind::Bool(_) => {}

            ExprKind::Binary { left, op: _, right } => {
                self.check_expr(left);
                self.check_expr(right);
            }
            ExprKind::Unary { op: _, operand } => {
                self.check_expr(operand);
            }
            ExprKind::Call { func, args } => {
                self.check_expr(func);
                for arg in args {
                    self.check_expr(arg);
                }
            }
            ExprKind::MethodCall { object, method: _, type_args: _, args } => {
                self.check_expr(object);
                for arg in args {
                    self.check_expr(arg);
                }
            }
            ExprKind::Field { object, field: _ } => {
                self.check_expr(object);
            }
            ExprKind::OptionalField { object, field: _ } => {
                self.check_expr(object);
            }
            ExprKind::Index { object, index } => {
                self.check_expr(object);
                self.check_expr(index);
                // Index creates an instant borrow for growable types
            }
            ExprKind::StructLit { name: _, fields, spread } => {
                for field in fields {
                    self.check_expr(&field.value);
                }
                if let Some(spread) = spread {
                    self.check_expr(spread);
                }
            }
            ExprKind::Array(elements) => {
                for elem in elements {
                    self.check_expr(elem);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.check_expr(value);
                self.check_expr(count);
            }
            ExprKind::Tuple(elements) => {
                for elem in elements {
                    self.check_expr(elem);
                }
            }
            ExprKind::Range { start, end, inclusive: _ } => {
                if let Some(start) = start {
                    self.check_expr(start);
                }
                if let Some(end) = end {
                    self.check_expr(end);
                }
            }
            ExprKind::Closure { params: _, body } => {
                self.check_expr(body);
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.check_expr(cond);
                self.check_expr(then_branch);
                if let Some(else_branch) = else_branch {
                    self.check_expr(else_branch);
                }
            }
            ExprKind::IfLet { expr: scrutinee, pattern, then_branch, else_branch } => {
                self.check_expr(scrutinee);
                self.register_pattern_bindings(pattern);
                self.check_expr(then_branch);
                if let Some(else_branch) = else_branch {
                    self.check_expr(else_branch);
                }
            }
            ExprKind::Block(stmts) => {
                self.check_block(stmts);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.check_expr(scrutinee);
                for arm in arms {
                    self.register_pattern_bindings(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.check_expr(guard);
                    }
                    self.check_expr(&arm.body);
                }
            }
            ExprKind::Try(inner) => {
                self.check_expr(inner);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.check_expr(value);
                self.check_expr(default);
            }
            ExprKind::Cast { expr: inner, ty: _ } => {
                self.check_expr(inner);
            }
            ExprKind::WithBlock { name: _, args, body } => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.check_block(body);
            }
            ExprKind::Spawn { body } => {
                self.check_block(body);
            }
            ExprKind::BlockCall { name: _, body } => {
                self.check_block(body);
            }
            ExprKind::Unsafe { body } => {
                self.check_block(body);
            }
            ExprKind::Comptime { body } => {
                self.check_block(body);
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.check_expr(condition);
                if let Some(msg) = message {
                    self.check_expr(msg);
                }
            }
        }
    }

    /// Handle potential move of a value.
    fn handle_potential_move(&mut self, expr: &Expr, span: Span) {
        // Get the type of the expression
        if let Some(ty) = self.program.node_types.get(&expr.id) {
            if !self.is_copy(ty) {
                // Non-Copy type: mark as moved
                if let ExprKind::Ident(name) = &expr.kind {
                    self.bindings.insert(name.clone(), BindingState::Moved { at: span });
                }
            }
        }
    }

    /// Check if a type is Copy (implicit copy on assignment).
    fn is_copy(&self, ty: &Type) -> bool {
        match ty {
            // Primitives are always Copy
            Type::Unit | Type::Bool | Type::Char => true,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 => true,
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => true,
            Type::F32 | Type::F64 => true,
            Type::Never => true,

            // String is NOT Copy (owns heap memory)
            Type::String => false,

            // Arrays: Copy if element is Copy and size <= 16 bytes
            Type::Array { elem, len: _ } => {
                self.is_copy(elem) && self.type_size(ty) <= 16
            }

            // Tuples: Copy if all elements are Copy and size <= 16 bytes
            Type::Tuple(elems) => {
                elems.iter().all(|t| self.is_copy(t)) && self.type_size(ty) <= 16
            }

            // Slices are Copy (borrowed view)
            Type::Slice(_) => true,

            // Option: Copy if inner is Copy and size <= 16 bytes
            Type::Option(inner) => {
                self.is_copy(inner) && self.type_size(ty) <= 16
            }

            // Result: NOT Copy (usually contains error info)
            Type::Result { .. } => false,

            // User-defined types: need to check size and fields
            Type::Named(type_id) => {
                if let Some(def) = self.program.types.get(*type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, .. } => {
                            fields.iter().all(|(_, t)| self.is_copy(t))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy(t)))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::Trait { .. } => false,
                    }
                } else {
                    false
                }
            }

            // Generic types: conservative
            Type::Generic { .. } => false,

            // Function types are Copy (just a pointer)
            Type::Fn { .. } => true,

            // Type variables: conservative
            Type::Var(_) => false,

            // Unresolved types: conservative
            Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. } => false,

            // Error: don't report more errors
            Type::Error => true,
        }
    }

    /// Estimate type size in bytes (simplified).
    fn type_size(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit => 0,
            Type::Bool | Type::I8 | Type::U8 => 1,
            Type::I16 | Type::U16 => 2,
            Type::I32 | Type::U32 | Type::F32 | Type::Char => 4,
            Type::I64 | Type::U64 | Type::F64 => 8,
            Type::Tuple(elems) => elems.iter().map(|t| self.type_size(t)).sum(),
            Type::Array { elem, len } => self.type_size(elem) * len,
            Type::Option(inner) => self.type_size(inner) + 1, // tag byte
            Type::Named(type_id) => {
                if let Some(def) = self.program.types.get(*type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, .. } => {
                            fields.iter().map(|(_, t)| self.type_size(t)).sum()
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            let max_variant = variants
                                .iter()
                                .map(|(_, data)| data.iter().map(|t| self.type_size(t)).sum::<usize>())
                                .max()
                                .unwrap_or(0);
                            max_variant + 1
                        }
                        _ => 8,
                    }
                } else {
                    8
                }
            }
            // Pointers/references/slices: fat pointer
            Type::String | Type::Slice(_) | Type::Fn { .. } => 16,
            _ => 8,
        }
    }

    /// Release instant borrows that end at the given statement.
    fn release_instant_borrows(&mut self, stmt_id: u32) {
        self.borrows.retain(|b| {
            !matches!(b.scope, BorrowScope::Instant { stmt_id: id } if id == stmt_id)
        });
    }

    /// Release persistent borrows that end at the given block.
    fn release_persistent_borrows(&mut self, block_id: u32) {
        self.borrows.retain(|b| {
            !matches!(b.scope, BorrowScope::Persistent { block_id: id } if id == block_id)
        });
    }

    /// Register pattern bindings as owned.
    fn register_pattern_bindings(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Ident(name) => {
                self.bindings.insert(name.clone(), BindingState::Owned);
            }
            Pattern::Tuple(pats) => {
                for pat in pats {
                    self.register_pattern_bindings(pat);
                }
            }
            Pattern::Struct { name: _, fields, rest: _ } => {
                for (_, pat) in fields {
                    self.register_pattern_bindings(pat);
                }
            }
            Pattern::Constructor { name: _, fields } => {
                for pat in fields {
                    self.register_pattern_bindings(pat);
                }
            }
            Pattern::Or(pats) => {
                // For or-patterns, all branches should bind the same names
                for pat in pats {
                    self.register_pattern_bindings(pat);
                }
            }
            Pattern::Wildcard | Pattern::Literal(_) => {}
        }
    }
}

/// Run ownership analysis on a typed program.
pub fn check_ownership(program: &TypedProgram, decls: &[Decl]) -> OwnershipResult {
    let checker = OwnershipChecker::new(program);
    checker.check(decls)
}

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

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;
use rask_types::{Type, TypedProgram, extract_projection};

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
    /// Bindings that are @resource types (must be consumed).
    resource_bindings: HashSet<String>,
    /// Resource bindings registered with `ensure` (consumption committed).
    ensure_registered: HashSet<String>,
    /// True when inside an `ensure` body (defer moves).
    in_ensure: bool,
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
            resource_bindings: HashSet::new(),
            ensure_registered: HashSet::new(),
            in_ensure: false,
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
            DeclKind::Extern(_) => {}
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
        self.resource_bindings.clear();
        self.ensure_registered.clear();
        self.current_block = 0;
        self.current_stmt = 0;

        // Register parameters as owned or borrowed bindings
        for param in &fn_decl.params {
            if param.is_take {
                // `take` parameter: owned
                self.bindings.insert(param.name.clone(), BindingState::Owned);
                // Check if it's a resource type
                if self.is_resource_type_name(&param.ty) {
                    self.resource_bindings.insert(param.name.clone());
                }
            } else {
                let mode = if param.is_mutate { BorrowMode::Exclusive } else { BorrowMode::Shared };
                // Default: borrowed (persistent for call duration)
                self.bindings.insert(
                    param.name.clone(),
                    BindingState::Borrowed {
                        mode,
                        scope: BorrowScope::Persistent { block_id: 0 },
                    },
                );
                // Track field projection if parameter has one (e.g., `state: GameState.{entities}`)
                let projection = extract_projection(&param.ty);
                let mut borrow = ActiveBorrow::new(
                    param.name.clone(),
                    mode,
                    BorrowScope::Persistent { block_id: 0 },
                    Span::new(0, 0),
                );
                if let Some(fields) = projection {
                    borrow = borrow.with_projection(fields);
                }
                self.borrows.push(borrow);
            }
        }

        // Check function body
        self.check_block(&fn_decl.body);

        // Check for unconsumed resources at function exit
        self.check_resource_consumption(fn_decl.body.last().map(|s| s.span).unwrap_or(Span::new(0, 0)));
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
            StmtKind::Let { name, name_span: _, ty, init } => {
                self.check_expr(init);
                // let creates mutable binding - moves the value
                self.handle_assignment(init, stmt.span, true);
                self.bindings.insert(name.clone(), BindingState::Owned);
                // Track resource types
                if let Some(ty_str) = ty {
                    if self.is_resource_type_name(ty_str) {
                        self.resource_bindings.insert(name.clone());
                    }
                } else if self.expr_is_resource_type(init) {
                    self.resource_bindings.insert(name.clone());
                }
            }
            StmtKind::LetTuple { names, init } => {
                self.check_expr(init);
                self.handle_assignment(init, stmt.span, true);
                for name in names {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                }
            }
            StmtKind::Const { name, name_span: _, ty, init } => {
                self.check_expr(init);
                // const creates immutable binding - borrows the value
                self.handle_assignment(init, stmt.span, false);
                self.bindings.insert(name.clone(), BindingState::Owned);
                // Track resource types
                if let Some(ty_str) = ty {
                    if self.is_resource_type_name(ty_str) {
                        self.resource_bindings.insert(name.clone());
                    }
                } else if self.expr_is_resource_type(init) {
                    self.resource_bindings.insert(name.clone());
                }
            }
            StmtKind::ConstTuple { names, init } => {
                self.check_expr(init);
                self.handle_assignment(init, stmt.span, false);
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
                // Assignments move the value
                self.handle_assignment(value, stmt.span, true);
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
            StmtKind::Break { value, .. } => {
                if let Some(v) = value {
                    self.check_expr(v);
                }
            }
            StmtKind::Continue(_) => {}
            StmtKind::Ensure { body, else_handler } => {
                // Mark resources referenced in ensure body as consumption-committed
                for s in body {
                    self.mark_ensure_resources(s);
                }
                let prev = self.in_ensure;
                self.in_ensure = true;
                self.check_block(body);
                self.in_ensure = prev;
                if let Some((_name, handler)) = else_handler {
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
            ExprKind::MethodCall { object, method, type_args: _, args } => {
                self.check_expr(object);
                for arg in args {
                    self.check_expr(arg);
                }
                // If this is a `take self` method, mark the object as moved
                // (skip in ensure bodies — ensure defers execution)
                if !self.in_ensure && self.is_take_self_method(object, method) {
                    if let ExprKind::Ident(name) = &object.kind {
                        self.bindings.insert(name.clone(), BindingState::Moved { at: expr.span });
                    }
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
            ExprKind::Closure { params, body } => {
                // Collect names from closure params (these shadow outer bindings)
                let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();

                // Scan body for free variables and create borrows in enclosing scope
                let mut captures = Vec::new();
                self.collect_free_vars(body, &param_names, &mut captures);
                for name in &captures {
                    if self.bindings.contains_key(name) {
                        // Create a shared borrow for each captured variable
                        self.borrows.push(ActiveBorrow::new(
                            name.clone(),
                            BorrowMode::Shared,
                            BorrowScope::Persistent { block_id: self.current_block },
                            expr.span,
                        ));
                    }
                }

                // Check the closure body with its own bindings
                let saved_bindings = self.bindings.clone();
                let saved_borrows = self.borrows.clone();
                for p in params {
                    self.bindings.insert(p.name.clone(), BindingState::Owned);
                }
                self.check_expr(body);
                self.bindings = saved_bindings;
                self.borrows = saved_borrows;
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
            ExprKind::Unwrap(inner) => {
                self.check_expr(inner);
            }
            ExprKind::GuardPattern { expr, pattern: _, else_branch } => {
                self.check_expr(expr);
                self.check_expr(else_branch);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.check_expr(value);
                self.check_expr(default);
            }
            ExprKind::Cast { expr: inner, ty: _ } => {
                self.check_expr(inner);
            }
            ExprKind::UsingBlock { name: _, args, body } => {
                for arg in args {
                    self.check_expr(arg);
                }
                self.check_block(body);
            }
            ExprKind::WithAs { bindings, body } => {
                for (expr, _) in bindings {
                    self.check_expr(expr);
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
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.check_expr(channel);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.check_expr(channel);
                            self.check_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.check_expr(&arm.body);
                }
            }
        }
    }

    /// Handle assignment: borrow or move depending on context.
    ///
    /// For `let` statements: check if borrowed, then move (is_mutable = true)
    /// For `const` statements: create block-scoped borrow (is_mutable = false)
    fn handle_assignment(&mut self, expr: &Expr, span: Span, is_mutable: bool) {
        if let Some(ty) = self.program.node_types.get(&expr.id) {
            if !self.is_copy(ty) {
                if let ExprKind::Ident(source_name) = &expr.kind {
                    if is_mutable {
                        // Mutable binding (let): check not borrowed, then move
                        if let Some(state) = self.bindings.get(source_name) {
                            match state {
                                BindingState::Borrowed { .. } => {
                                    self.errors.push(OwnershipError {
                                        kind: OwnershipErrorKind::MutateWhileBorrowed {
                                            name: source_name.clone(),
                                            borrow_span: span,
                                        },
                                        span,
                                    });
                                    return;
                                }
                                BindingState::Moved { at } => {
                                    self.errors.push(OwnershipError {
                                        kind: OwnershipErrorKind::UseAfterMove {
                                            name: source_name.clone(),
                                            moved_at: *at,
                                        },
                                        span,
                                    });
                                    return;
                                }
                                BindingState::Owned => {}
                            }
                        }
                        self.bindings.insert(source_name.clone(), BindingState::Moved { at: span });
                    } else {
                        // Immutable binding (const): create block-scoped borrow
                        self.create_borrow(source_name.clone(), BorrowMode::Shared, span);
                    }
                }
            }
        }
    }

    /// Create a borrow of a binding, optionally projected to specific fields.
    fn create_borrow(&mut self, source_name: String, mode: BorrowMode, span: Span) {
        self.create_borrow_with_projection(source_name, mode, span, None);
    }

    fn create_borrow_with_projection(
        &mut self,
        source_name: String,
        mode: BorrowMode,
        span: Span,
        projection: Option<Vec<String>>,
    ) {
        if let Some(state) = self.bindings.get(&source_name) {
            match state {
                BindingState::Owned => {
                    self.bindings.insert(
                        source_name.clone(),
                        BindingState::Borrowed {
                            mode,
                            scope: BorrowScope::Persistent { block_id: self.current_block },
                        },
                    );
                    let mut borrow = ActiveBorrow::new(
                        source_name,
                        mode,
                        BorrowScope::Persistent { block_id: self.current_block },
                        span,
                    );
                    if let Some(fields) = projection {
                        borrow = borrow.with_projection(fields);
                    }
                    self.borrows.push(borrow);
                }
                BindingState::Borrowed { mode: existing_mode, .. } => {
                    // Check if there's an actual conflict considering projections.
                    // Non-overlapping projections on the same binding don't conflict.
                    let has_conflict = self.borrows.iter().any(|b| {
                        b.source == source_name && b.overlaps(&projection) && (
                            *existing_mode == BorrowMode::Exclusive
                            || mode == BorrowMode::Exclusive
                        )
                    });

                    if has_conflict {
                        self.errors.push(OwnershipError {
                            kind: OwnershipErrorKind::BorrowConflict {
                                name: source_name,
                                requested: if mode == BorrowMode::Shared { AccessKind::Read } else { AccessKind::Write },
                                existing: if *existing_mode == BorrowMode::Shared { AccessKind::Read } else { AccessKind::Write },
                                existing_span: span,
                            },
                            span,
                        });
                    } else {
                        let mut borrow = ActiveBorrow::new(
                            source_name,
                            mode,
                            BorrowScope::Persistent { block_id: self.current_block },
                            span,
                        );
                        if let Some(fields) = projection {
                            borrow = borrow.with_projection(fields);
                        }
                        self.borrows.push(borrow);
                    }
                }
                BindingState::Moved { at } => {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::UseAfterMove {
                            name: source_name,
                            moved_at: *at,
                        },
                        span,
                    });
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

            // Union: NOT Copy (error union types)
            Type::Union(_) => false,

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
        let mut released_bindings = HashSet::new();

        // Remove borrows for this block
        self.borrows.retain(|b| {
            if matches!(b.scope, BorrowScope::Persistent { block_id: id } if id == block_id) {
                released_bindings.insert(b.source.clone());
                false
            } else {
                true
            }
        });

        // Restore bindings to Owned if no borrows remain
        for binding_name in released_bindings {
            let remaining_borrows = self.borrows.iter().filter(|b| b.source == binding_name).count();

            if remaining_borrows == 0 {
                if let Some(state) = self.bindings.get(&binding_name) {
                    if matches!(state, BindingState::Borrowed { .. }) {
                        self.bindings.insert(binding_name.clone(), BindingState::Owned);
                    }
                }
            }
        }
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

    /// Collect free variables referenced in an expression (excluding local bindings).
    fn collect_free_vars(&self, expr: &Expr, locals: &HashSet<String>, out: &mut Vec<String>) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if !locals.contains(name) && self.bindings.contains_key(name) {
                    if !out.contains(name) {
                        out.push(name.clone());
                    }
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.collect_free_vars(left, locals, out);
                self.collect_free_vars(right, locals, out);
            }
            ExprKind::Unary { operand, .. } => {
                self.collect_free_vars(operand, locals, out);
            }
            ExprKind::Call { func, args } => {
                self.collect_free_vars(func, locals, out);
                for arg in args { self.collect_free_vars(arg, locals, out); }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.collect_free_vars(object, locals, out);
                for arg in args { self.collect_free_vars(arg, locals, out); }
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.collect_free_vars(object, locals, out);
            }
            ExprKind::Index { object, index } => {
                self.collect_free_vars(object, locals, out);
                self.collect_free_vars(index, locals, out);
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.collect_free_vars(cond, locals, out);
                self.collect_free_vars(then_branch, locals, out);
                if let Some(e) = else_branch { self.collect_free_vars(e, locals, out); }
            }
            ExprKind::Block(stmts) => {
                for s in stmts { self.collect_free_vars_stmt(s, locals, out); }
            }
            ExprKind::Closure { params, body } => {
                let mut inner_locals = locals.clone();
                for p in params { inner_locals.insert(p.name.clone()); }
                self.collect_free_vars(body, &inner_locals, out);
            }
            _ => {
                // For other expressions, recurse into sub-expressions
            }
        }
    }

    fn collect_free_vars_stmt(&self, stmt: &Stmt, locals: &HashSet<String>, out: &mut Vec<String>) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.collect_free_vars(e, locals, out),
            StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
                self.collect_free_vars(init, locals, out);
            }
            StmtKind::Return(Some(e)) => self.collect_free_vars(e, locals, out),
            _ => {}
        }
    }

    /// Check if a method call uses `take self`.
    fn is_take_self_method(&self, object: &Expr, method_name: &str) -> bool {
        // Look up the type of the object expression
        if let Some(ty) = self.program.node_types.get(&object.id) {
            let type_id = match ty {
                Type::Named(id) => Some(*id),
                _ => None,
            };
            if let Some(id) = type_id {
                if let Some(def) = self.program.types.get(id) {
                    let methods = match def {
                        rask_types::TypeDef::Struct { methods, .. } => methods,
                        rask_types::TypeDef::Enum { methods, .. } => methods,
                        _ => return false,
                    };
                    for m in methods {
                        if m.name == method_name {
                            return m.self_param == rask_types::SelfParam::Take;
                        }
                    }
                }
            }
        }
        false
    }

    /// Check if a type name refers to a @resource struct.
    fn is_resource_type_name(&self, ty_name: &str) -> bool {
        // Strip generic args: "File<T>" -> "File"
        let base = ty_name.split('<').next().unwrap_or(ty_name);
        self.program.types.is_resource_type(base)
    }

    /// Check if an expression's inferred type is a @resource type.
    fn expr_is_resource_type(&self, expr: &Expr) -> bool {
        if let Some(ty) = self.program.node_types.get(&expr.id) {
            if let Type::Named(type_id) = ty {
                if let Some(def) = self.program.types.get(*type_id) {
                    if let rask_types::TypeDef::Struct { is_resource, .. } = def {
                        return *is_resource;
                    }
                }
            }
        }
        false
    }

    /// Scan ensure body for resource references and mark them.
    fn mark_ensure_resources(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.mark_ensure_expr(expr);
            }
            _ => {}
        }
    }

    /// Extract resource names from ensure expressions (e.g., `file.close()`).
    fn mark_ensure_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::MethodCall { object, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if self.resource_bindings.contains(name) {
                        self.ensure_registered.insert(name.clone());
                    }
                }
            }
            ExprKind::Call { func, args } => {
                // Check args for resource identifiers
                for arg in args {
                    if let ExprKind::Ident(name) = &arg.kind {
                        if self.resource_bindings.contains(name) {
                            self.ensure_registered.insert(name.clone());
                        }
                    }
                }
                self.mark_ensure_expr(func);
            }
            _ => {}
        }
    }

    /// At function exit, emit errors for unconsumed @resource bindings.
    fn check_resource_consumption(&mut self, span: Span) {
        let unconsumed: Vec<String> = self.resource_bindings.iter()
            .filter(|name| {
                // Not moved (consumed) and not registered with ensure
                if self.ensure_registered.contains(*name) {
                    return false;
                }
                match self.bindings.get(*name) {
                    Some(BindingState::Moved { .. }) => false, // consumed
                    _ => true, // still owned = not consumed
                }
            })
            .cloned()
            .collect();

        for name in unconsumed {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ResourceNotConsumed { name },
                span,
            });
        }
    }
}

/// Run ownership analysis on a typed program.
pub fn check_ownership(program: &TypedProgram, decls: &[Decl]) -> OwnershipResult {
    let checker = OwnershipChecker::new(program);
    checker.check(decls)
}

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
pub use error::{OwnershipError, OwnershipErrorKind, AccessKind, MoveReason};

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, Expr, ExprKind, Pattern};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
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

/// W2 tracking: active `with` block binding info.
#[derive(Debug, Clone)]
struct WithBindingInfo {
    /// Collection variable name (e.g. "pool" from `with pool[h] as entity`)
    collection_name: String,
    /// Handle/key variable name (e.g. "h")
    handle_name: String,
    /// Whether the collection is a Pool (relaxed W2 rules) vs Vec/Map/string
    is_pool: bool,
    /// Span of the `with` binding for error messages
    span: Span,
}

/// LP14/LP16: Tracks the collection being iterated in a `for mutate` loop.
#[derive(Debug, Clone)]
struct ForMutateInfo {
    /// Collection variable name (e.g. "items" from `for mutate item in items`)
    collection_name: String,
    /// Binding variable names (e.g. ["item"])
    binding_names: Vec<String>,
    /// Span for error messages
    span: Span,
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
    /// Type of each binding, for generating move-reason diagnostics.
    binding_types: HashMap<String, Type>,
    /// Bindings that are @resource types (must be consumed).
    resource_bindings: HashSet<String>,
    /// Resource bindings registered with `ensure` (consumption committed).
    ensure_registered: HashSet<String>,
    /// True when inside an `ensure` body (defer moves).
    in_ensure: bool,
    /// Pool type names with frozen context (CC3/PF5: no writes, inserts, removes, clears).
    frozen_contexts: HashSet<String>,
    /// Active `with` block bindings for W2 checking.
    active_with_bindings: Vec<WithBindingInfo>,
    /// LP14/LP16: Active `for mutate` loops for structural mutation checking.
    active_for_mutates: Vec<ForMutateInfo>,
    /// Parameter type strings: param name → type annotation (e.g. "Pool<Entity>").
    param_type_strings: HashMap<String, String>,
    /// SL1: Bindings created by `const` from non-copy expressions (block-scoped borrows).
    /// Maps binding name → block_id where the borrow was created.
    borrow_bindings: HashMap<String, u32>,
    /// Block where each binding was declared (first introduced via let/const).
    binding_decl_blocks: HashMap<String, u32>,
    /// SL1: Bindings that hold scope-limited closures.
    /// Maps binding name → (borrow_block, binding_block).
    /// borrow_block: the block where the captured borrow lives.
    /// binding_block: the block where the closure binding was declared.
    /// Escape: binding_block < borrow_block (binding outlives borrow).
    scope_limited_closures: HashMap<String, (u32, u32)>,
    /// Temporary: scope limit from the last closure expression processed.
    /// Picked up by the next Let/Const binding that uses it.
    last_closure_scope_limit: Option<u32>,
    /// Errors accumulated during analysis.
    errors: Vec<OwnershipError>,
}

impl<'a> OwnershipChecker<'a> {
    pub fn new(program: &'a TypedProgram) -> Self {
        Self {
            program,
            bindings: HashMap::new(),
            binding_types: HashMap::new(),
            borrows: Vec::new(),
            current_block: 0,
            current_stmt: 0,
            resource_bindings: HashSet::new(),
            ensure_registered: HashSet::new(),
            in_ensure: false,
            frozen_contexts: HashSet::new(),
            active_with_bindings: Vec::new(),
            active_for_mutates: Vec::new(),
            param_type_strings: HashMap::new(),
            borrow_bindings: HashMap::new(),
            binding_decl_blocks: HashMap::new(),
            scope_limited_closures: HashMap::new(),
            last_closure_scope_limit: None,
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
            DeclKind::Package(_) | DeclKind::CImport(_) => {}
            DeclKind::Union(_) => {}
            DeclKind::TypeAlias(_) => {}
        }
    }

    fn check_fn(&mut self, fn_decl: &FnDecl) {
        // Reset state for each function (local analysis only)
        self.bindings.clear();
        self.borrows.clear();
        self.resource_bindings.clear();
        self.ensure_registered.clear();
        self.frozen_contexts.clear();
        self.current_block = 0;
        self.current_stmt = 0;

        // CC3/PF5: Track frozen pool contexts
        for clause in &fn_decl.context_clauses {
            if clause.is_frozen {
                self.frozen_contexts.insert(clause.ty.clone());
                if let Some(name) = &clause.name {
                    self.frozen_contexts.insert(name.clone());
                }
            }
        }

        // Register parameter type strings for W2 pool detection
        self.param_type_strings.clear();
        for param in &fn_decl.params {
            self.param_type_strings.insert(param.name.clone(), param.ty.clone());
        }

        // Register parameters as owned or borrowed bindings
        for param in &fn_decl.params {
            if param.is_take {
                // `take` parameter: owned
                self.bindings.insert(param.name.clone(), BindingState::Owned);
                // Check if it's a resource type
                if self.is_resource_type_name(&param.ty) {
                    self.resource_bindings.insert(param.name.clone());
                }
            } else if param.is_mutate {
                // Mutate parameters: treat as owned within the body.
                // The caller holds the exclusive borrow; within the function
                // we can freely read and write the parameter.
                self.bindings.insert(param.name.clone(), BindingState::Owned);
            } else {
                // Shared (non-mutate, non-take): borrowed for call duration
                self.bindings.insert(
                    param.name.clone(),
                    BindingState::Borrowed {
                        mode: BorrowMode::Shared,
                        scope: BorrowScope::Persistent { block_id: 0 },
                    },
                );
                let borrow = ActiveBorrow::new(
                    param.name.clone(),
                    BorrowMode::Shared,
                    BorrowScope::Persistent { block_id: 0 },
                    Span::new(0, 0),
                );
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

        // SL2: Check if any scope-limited closures would escape this block.
        // A closure escapes if its borrow_block is inside the block being exited
        // but its binding_block is outside (binding outlives the borrow).
        let block_inner = block_id + 1;
        let escaping: Vec<String> = self.scope_limited_closures.iter()
            .filter(|(_, &(borrow_block, binding_block))| {
                // Borrow was created in this block or deeper, but binding is in outer scope
                borrow_block >= block_inner && binding_block < block_inner
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in &escaping {
            if matches!(self.bindings.get(name), Some(BindingState::Owned)) {
                self.errors.push(OwnershipError {
                    kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                        name: name.clone(),
                    },
                    span: stmts.last().map(|s| s.span).unwrap_or(Span::new(0, 0)),
                });
            }
            self.scope_limited_closures.remove(name);
        }

        // Clean up scope-limited closures whose bindings were in this block
        // (they naturally die with the block — no escape).
        let dying: Vec<String> = self.scope_limited_closures.iter()
            .filter(|(_, &(_, binding_block))| binding_block >= block_inner)
            .map(|(name, _)| name.clone())
            .collect();
        for name in dying {
            self.scope_limited_closures.remove(&name);
        }

        self.current_block = block_id;
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Mut { name, name_span: _, ty, init } => {
                self.check_expr(init);
                // let: Copy types are copied (source stays valid),
                // non-Copy types are moved (source invalidated)
                self.handle_assignment(init, stmt.span, true);
                self.bindings.insert(name.clone(), BindingState::Owned);
                self.binding_decl_blocks.insert(name.clone(), self.current_block);
                if let Some(t) = self.program.node_types.get(&init.id).cloned() {
                    self.binding_types.insert(name.clone(), t);
                }
                // SL1: inherit scope limit from closure expression
                if let Some(borrow_block) = self.last_closure_scope_limit.take() {
                    self.scope_limited_closures.insert(name.clone(), (borrow_block, self.current_block));
                }
                // Track resource types
                if let Some(ty_str) = ty {
                    if self.is_resource_type_name(ty_str) {
                        self.resource_bindings.insert(name.clone());
                    }
                } else if self.expr_is_resource_type(init) {
                    self.resource_bindings.insert(name.clone());
                }
            }
            StmtKind::MutTuple { patterns, init } => {
                self.check_expr(init);
                self.handle_assignment(init, stmt.span, true);
                let names = rask_ast::stmt::tuple_pats_flat_names(patterns);
                let elem_types = match self.program.node_types.get(&init.id) {
                    Some(Type::Tuple(elems)) => Some(elems.clone()),
                    _ => None,
                };
                for (i, name) in names.iter().enumerate() {
                    self.bindings.insert(name.to_string(), BindingState::Owned);
                    if let Some(ref elems) = elem_types {
                        if let Some(elem_ty) = elems.get(i) {
                            self.binding_types.insert(name.to_string(), elem_ty.clone());
                            if self.type_is_resource(elem_ty) {
                                self.resource_bindings.insert(name.to_string());
                            }
                        }
                    }
                }
            }
            StmtKind::Const { name, name_span: _, ty, init } => {
                self.check_expr(init);
                // const: Copy types are copied, non-Copy types create
                // a block-scoped borrow (source stays valid but frozen)
                self.handle_assignment(init, stmt.span, false);
                self.bindings.insert(name.clone(), BindingState::Owned);
                self.binding_decl_blocks.insert(name.clone(), self.current_block);
                if let Some(t) = self.program.node_types.get(&init.id).cloned() {
                    self.binding_types.insert(name.clone(), t.clone());
                    // SL1: If this is a non-copy type, this const is a borrow view.
                    // Track it so closures capturing it are scope-limited.
                    if !self.is_copy(&t) {
                        self.borrow_bindings.insert(name.clone(), self.current_block);
                    }
                }
                // SL1: inherit scope limit from closure expression
                if let Some(borrow_block) = self.last_closure_scope_limit.take() {
                    self.scope_limited_closures.insert(name.clone(), (borrow_block, self.current_block));
                }
                // Track resource types
                if let Some(ty_str) = ty {
                    if self.is_resource_type_name(ty_str) {
                        self.resource_bindings.insert(name.clone());
                    }
                } else if self.expr_is_resource_type(init) {
                    self.resource_bindings.insert(name.clone());
                }
            }
            StmtKind::ConstTuple { patterns, init } => {
                self.check_expr(init);
                self.handle_assignment(init, stmt.span, false);
                let names = rask_ast::stmt::tuple_pats_flat_names(patterns);
                let elem_types = match self.program.node_types.get(&init.id) {
                    Some(Type::Tuple(elems)) => Some(elems.clone()),
                    _ => None,
                };
                for (i, name) in names.iter().enumerate() {
                    self.bindings.insert(name.to_string(), BindingState::Owned);
                    if let Some(ref elems) = elem_types {
                        if let Some(elem_ty) = elems.get(i) {
                            self.binding_types.insert(name.to_string(), elem_ty.clone());
                            if self.type_is_resource(elem_ty) {
                                self.resource_bindings.insert(name.to_string());
                            }
                        }
                    }
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
                // SL2: Check if assigning a scope-limited closure to an outer variable.
                // SL2: Propagate scope limit to target binding.
                // Use the target's *declaration* block, not the current block.
                if let ExprKind::Ident(value_name) = &value.kind {
                    if let Some(&(borrow_block, _)) = self.scope_limited_closures.get(value_name) {
                        if let ExprKind::Ident(target_name) = &target.kind {
                            let decl_block = self.binding_decl_blocks
                                .get(target_name).copied()
                                .unwrap_or(self.current_block);
                            self.scope_limited_closures.insert(
                                target_name.clone(),
                                (borrow_block, decl_block),
                            );
                        }
                    }
                }
                // Also pick up scope limit from a closure literal assigned directly
                if let Some(borrow_block) = self.last_closure_scope_limit.take() {
                    if let ExprKind::Ident(target_name) = &target.kind {
                        let decl_block = self.binding_decl_blocks
                            .get(target_name).copied()
                            .unwrap_or(self.current_block);
                        self.scope_limited_closures.insert(
                            target_name.clone(),
                            (borrow_block, decl_block),
                        );
                    }
                }
            }
            StmtKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.check_expr(expr);
                    // SL2: Check if returning a scope-limited closure
                    if let ExprKind::Ident(name) = &expr.kind {
                        if self.scope_limited_closures.contains_key(name) {
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::ScopeLimitedClosureEscapes {
                                    name: name.clone(),
                                },
                                span: stmt.span,
                            });
                            // Remove to avoid double-reporting at block exit
                            self.scope_limited_closures.remove(name);
                        }
                    }
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
            StmtKind::For { label: _, binding, mutate, iter, body, .. } => {
                self.check_expr(iter);
                let binding_names: Vec<String> = binding.names().iter().map(|s| s.to_string()).collect();
                match binding {
                    ForBinding::Single(name) => {
                        self.bindings.insert(name.clone(), BindingState::Owned);
                    }
                    ForBinding::Tuple(names) => {
                        for name in names {
                            self.bindings.insert(String::clone(name), BindingState::Owned);
                        }
                    }
                }
                // LP14/LP16: track for-mutate context
                if *mutate {
                    let collection_name = Self::extract_iter_collection(iter);
                    if let Some(coll) = collection_name {
                        self.active_for_mutates.push(ForMutateInfo {
                            collection_name: coll,
                            binding_names: binding_names.clone(),
                            span: stmt.span,
                        });
                    }
                }
                self.check_block(body);
                if *mutate {
                    self.active_for_mutates.pop();
                }
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
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.check_expr(iter);
                self.check_block(body);
            }
            StmtKind::Discard { name, .. } => {
                // D3: resource types cannot be discarded
                if self.resource_bindings.contains(name) {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::DiscardResource {
                            name: name.clone(),
                        },
                        span: stmt.span,
                    });
                } else {
                    // Mark the binding as discarded — subsequent uses are errors
                    self.bindings.insert(name.clone(), BindingState::Discarded { at: stmt.span });
                }
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                // Check if this identifier is used after move
                if let Some(state) = self.bindings.get(name) {
                    match state {
                        BindingState::Moved { at } => {
                            let reason = self.program.node_types.get(&expr.id)
                                .map(|ty| self.move_reason(ty))
                                .unwrap_or_else(|| self.move_reason_for(name));
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::UseAfterMove {
                                    name: name.clone(),
                                    moved_at: *at,
                                    reason,
                                },
                                span: expr.span,
                            });
                        }
                        BindingState::Discarded { at } => {
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::UseAfterDiscard {
                                    name: name.clone(),
                                    discarded_at: *at,
                                },
                                span: expr.span,
                            });
                        }
                        _ => {}
                    }
                }
            }
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_)
            | ExprKind::StringInterp(_)
            | ExprKind::Char(_) | ExprKind::Bool(_) | ExprKind::Null => {}

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
                    self.check_expr(&arg.expr);
                    if arg.mode == ArgMode::Own {
                        // LP16: reject passing for-mutate binding to take parameter
                        if let ExprKind::Ident(name) = &arg.expr.kind {
                            if let Some(fm) = self.active_for_mutates.iter().find(|fm| fm.binding_names.contains(name)) {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::ForMutateTakeItem {
                                        item: name.clone(),
                                        collection: fm.collection_name.clone(),
                                        loop_span: fm.span,
                                    },
                                    span: arg.expr.span,
                                });
                            }
                            self.bindings.insert(name.clone(), BindingState::Moved { at: arg.expr.span });
                        }
                    }
                }
            }
            ExprKind::MethodCall { object, method, type_args: _, args } => {
                self.check_expr(object);
                for arg in args {
                    self.check_expr(&arg.expr);
                    if arg.mode == ArgMode::Own {
                        // LP16: reject passing for-mutate binding to take parameter
                        if let ExprKind::Ident(name) = &arg.expr.kind {
                            if let Some(fm) = self.active_for_mutates.iter().find(|fm| fm.binding_names.contains(name)) {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::ForMutateTakeItem {
                                        item: name.clone(),
                                        collection: fm.collection_name.clone(),
                                        loop_span: fm.span,
                                    },
                                    span: arg.expr.span,
                                });
                            }
                            self.bindings.insert(name.clone(), BindingState::Moved { at: arg.expr.span });
                        }
                    }
                }
                // CC3/PF5: Check for mutations on frozen pool contexts
                if matches!(method.as_str(), "insert" | "remove" | "clear") {
                    if let ExprKind::Ident(name) = &object.kind {
                        if self.frozen_contexts.contains(name) {
                            self.errors.push(OwnershipError {
                                kind: OwnershipErrorKind::FrozenContextMutation {
                                    context_ty: name.clone(),
                                    operation: method.clone(),
                                },
                                span: expr.span,
                            });
                        }
                    }
                }
                // W2: Check structural mutations inside `with` blocks
                if matches!(method.as_str(), "insert" | "remove" | "clear" | "push" | "pop") {
                    if let ExprKind::Ident(coll_name) = &object.kind {
                        for wb in &self.active_with_bindings {
                            if wb.collection_name == *coll_name {
                                if wb.is_pool {
                                    // W2d: clear is always forbidden
                                    if method == "clear" {
                                        self.errors.push(OwnershipError {
                                            kind: OwnershipErrorKind::WithBlockClear {
                                                collection: coll_name.clone(),
                                                binding_span: wb.span,
                                            },
                                            span: expr.span,
                                        });
                                    }
                                    // W2c: remove(bound_handle) is forbidden
                                    else if method == "remove" {
                                        if let Some(first_arg) = args.first() {
                                            if let ExprKind::Ident(arg_name) = &first_arg.expr.kind {
                                                if *arg_name == wb.handle_name {
                                                    self.errors.push(OwnershipError {
                                                        kind: OwnershipErrorKind::WithBlockBoundHandleRemoved {
                                                            handle: arg_name.clone(),
                                                            collection: coll_name.clone(),
                                                            binding_span: wb.span,
                                                        },
                                                        span: expr.span,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                    // W2a/W2b: insert and remove(other) are allowed for Pool
                                } else {
                                    // W2: non-pool collections — all structural mutations forbidden
                                    self.errors.push(OwnershipError {
                                        kind: OwnershipErrorKind::WithBlockStructuralMutation {
                                            collection: coll_name.clone(),
                                            operation: method.clone(),
                                            binding_span: wb.span,
                                        },
                                        span: expr.span,
                                    });
                                }
                                break;
                            }
                        }
                    }
                }
                // LP14: Check structural mutations on collection during `for mutate`
                if matches!(method.as_str(), "insert" | "remove" | "clear" | "push" | "pop" | "drain") {
                    if let ExprKind::Ident(coll_name) = &object.kind {
                        for fm in &self.active_for_mutates {
                            if fm.collection_name == *coll_name {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::ForMutateStructuralMutation {
                                        collection: coll_name.clone(),
                                        operation: method.clone(),
                                        loop_span: fm.span,
                                    },
                                    span: expr.span,
                                });
                                break;
                            }
                        }
                    }
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
            ExprKind::DynamicField { object, field_expr } => {
                self.check_expr(object);
                self.check_expr(field_expr);
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
            ExprKind::Closure { params, body, .. } => {
                // Collect names from closure params (these shadow outer bindings)
                let param_names: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();

                // Scan body for free variables with field projection tracking (F4)
                let mut captures = Vec::new();
                let mut capture_projections: HashMap<String, Option<Vec<String>>> = HashMap::new();
                self.collect_free_vars_with_projections(body, &param_names, &mut captures, &mut capture_projections);

                // Separate resource captures from non-resource captures
                let resource_captures: Vec<String> = captures.iter()
                    .filter(|name| self.resource_bindings.contains(*name))
                    .cloned()
                    .collect();

                // Move resource captures in outer scope
                for name in &resource_captures {
                    self.bindings.insert(name.clone(), BindingState::Moved { at: expr.span });
                }

                // SL1: Check if any capture references a borrow binding (a `const`
                // from a non-copy source) — if so, this closure is scope-limited.
                let mut scope_limit: Option<u32> = None;
                for name in &captures {
                    if resource_captures.contains(name) { continue; }
                    // Check if the captured variable is itself a borrow binding
                    if let Some(&block_id) = self.borrow_bindings.get(name) {
                        scope_limit = Some(match scope_limit {
                            None => block_id,
                            Some(existing) => existing.max(block_id),
                        });
                    }
                    // Also check active borrows on the captured variable
                    for borrow in &self.borrows {
                        if borrow.source == *name {
                            if let BorrowScope::Persistent { block_id } = borrow.scope {
                                scope_limit = Some(match scope_limit {
                                    None => block_id,
                                    Some(existing) => existing.max(block_id),
                                });
                            }
                        }
                    }
                }

                // Shared borrow for non-resource captures (F4: with field projections)
                for name in &captures {
                    if !resource_captures.contains(name) {
                        if self.bindings.contains_key(name) {
                            let projection = capture_projections.get(name).cloned().flatten();
                            let mut borrow = ActiveBorrow::new(
                                name.clone(),
                                BorrowMode::Shared,
                                BorrowScope::Persistent { block_id: self.current_block },
                                expr.span,
                            );
                            if let Some(fields) = projection {
                                borrow = borrow.with_projection(fields);
                            }
                            self.borrows.push(borrow);
                        }
                    }
                }

                // Check closure body with isolated state
                let saved_bindings = self.bindings.clone();
                let saved_borrows = self.borrows.clone();
                let saved_resources = self.resource_bindings.clone();
                let saved_ensure = self.ensure_registered.clone();

                self.resource_bindings.clear();
                self.ensure_registered.clear();

                // Register closure params as owned
                for p in params {
                    self.bindings.insert(p.name.clone(), BindingState::Owned);
                }

                // Register resource captures in closure's resource set
                for name in &resource_captures {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                    self.resource_bindings.insert(name.clone());
                }
                // Register non-resource captures as owned
                for name in &captures {
                    if !resource_captures.contains(name) {
                        self.bindings.insert(name.clone(), BindingState::Owned);
                    }
                }

                self.check_expr(body);

                // Check resource consumption at closure exit
                self.check_resource_consumption_in_closure(expr.span, "closure");

                // Restore outer scope
                self.bindings = saved_bindings;
                self.borrows = saved_borrows;
                self.resource_bindings = saved_resources;
                self.ensure_registered = saved_ensure;

                // Remove moved resources from outer tracking
                for name in &resource_captures {
                    self.resource_bindings.remove(name);
                }

                // SL1: Record scope limit for the next binding to pick up
                self.last_closure_scope_limit = scope_limit;
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.check_expr(cond);
                let pre_branch = self.bindings.clone();
                self.check_expr(then_branch);
                let then_terminal = Self::is_terminal_expr(then_branch);
                if let Some(else_branch) = else_branch {
                    let after_then = self.bindings.clone();
                    self.bindings = pre_branch;
                    self.check_expr(else_branch);
                    let else_terminal = Self::is_terminal_expr(else_branch);
                    if then_terminal && !else_terminal {
                        // then returns — only else state survives
                    } else if else_terminal && !then_terminal {
                        // else returns — only then state survives
                        self.bindings = after_then;
                    } else {
                        self.merge_branch_bindings(&after_then);
                    }
                } else if then_terminal {
                    // if-without-else where then returns — restore pre-branch
                    self.bindings = pre_branch;
                }
            }
            ExprKind::IfLet { expr: scrutinee, pattern, then_branch, else_branch } => {
                self.check_expr(scrutinee);
                let pre_branch = self.bindings.clone();
                self.register_pattern_bindings(pattern);
                self.check_expr(then_branch);
                let then_terminal = Self::is_terminal_expr(then_branch);
                if let Some(else_branch) = else_branch {
                    let after_then = self.bindings.clone();
                    self.bindings = pre_branch;
                    self.check_expr(else_branch);
                    let else_terminal = Self::is_terminal_expr(else_branch);
                    if then_terminal && !else_terminal {
                        // then returns — only else state survives
                    } else if else_terminal && !then_terminal {
                        self.bindings = after_then;
                    } else {
                        self.merge_branch_bindings(&after_then);
                    }
                } else if then_terminal {
                    self.bindings = pre_branch;
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
            ExprKind::Try { expr: inner, ref else_clause } => {
                self.check_expr(inner);
                if let Some(ec) = else_clause {
                    let pre_else = self.bindings.clone();
                    self.check_expr(&ec.body);
                    self.bindings = pre_else;
                }
            }
            ExprKind::IsPresent { expr: inner, .. } => {
                self.check_expr(inner);
            }
            ExprKind::Unwrap { expr: inner, .. } => {
                self.check_expr(inner);
            }
            ExprKind::GuardPattern { expr, pattern: _, else_branch } => {
                self.check_expr(expr);
                self.check_expr(else_branch);
            }
            ExprKind::IsPattern { expr, pattern: _ } => {
                self.check_expr(expr);
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
                    self.check_expr(&arg.expr);
                }
                self.check_block(body);
            }
            ExprKind::WithAs { bindings, body } => {
                let prev_count = self.active_with_bindings.len();
                for binding in bindings {
                    self.check_expr(&binding.source);
                    // W2: Track binding info for structural mutation checking
                    if let ExprKind::Index { object, index } = &binding.source.kind {
                        if let ExprKind::Ident(coll_name) = &object.kind {
                            let handle_name = if let ExprKind::Ident(h) = &index.kind {
                                h.clone()
                            } else {
                                String::new()
                            };
                            self.active_with_bindings.push(WithBindingInfo {
                                collection_name: coll_name.clone(),
                                handle_name,
                                is_pool: self.is_pool_type(coll_name),
                                span: binding.source.span,
                            });
                        }
                    }
                }
                self.check_block(body);
                self.active_with_bindings.truncate(prev_count);
            }
            ExprKind::Spawn { body } => {
                // Collect free variables from spawn body
                let mut captures = Vec::new();
                let empty_params = HashSet::new();
                for stmt in body {
                    self.collect_free_vars_stmt(stmt, &empty_params, &mut captures);
                }
                captures.dedup();

                // Separate resource captures
                let resource_captures: Vec<String> = captures.iter()
                    .filter(|name| self.resource_bindings.contains(*name))
                    .cloned()
                    .collect();

                // Move resource captures in outer scope
                for name in &resource_captures {
                    self.bindings.insert(name.clone(), BindingState::Moved { at: expr.span });
                }

                // Shared borrow for non-resource captures
                for name in &captures {
                    if !resource_captures.contains(name) {
                        if self.bindings.contains_key(name) {
                            self.borrows.push(ActiveBorrow::new(
                                name.clone(),
                                BorrowMode::Shared,
                                BorrowScope::Persistent { block_id: self.current_block },
                                expr.span,
                            ));
                        }
                    }
                }

                // Check spawn body with isolated state
                let saved_bindings = self.bindings.clone();
                let saved_borrows = self.borrows.clone();
                let saved_resources = self.resource_bindings.clone();
                let saved_ensure = self.ensure_registered.clone();

                self.resource_bindings.clear();
                self.ensure_registered.clear();

                // Register captures in spawn scope
                for name in &resource_captures {
                    self.bindings.insert(name.clone(), BindingState::Owned);
                    self.resource_bindings.insert(name.clone());
                }
                for name in &captures {
                    if !resource_captures.contains(name) {
                        self.bindings.insert(name.clone(), BindingState::Owned);
                    }
                }

                self.check_block(body);

                // Check resource consumption at spawn exit
                self.check_resource_consumption_in_closure(expr.span, "spawn");

                // Restore outer scope
                self.bindings = saved_bindings;
                self.borrows = saved_borrows;
                self.resource_bindings = saved_resources;
                self.ensure_registered = saved_ensure;

                // Remove moved resources from outer tracking
                for name in &resource_captures {
                    self.resource_bindings.remove(name);
                }
            }
            ExprKind::BlockCall { name: _, body } => {
                self.check_block(body);
            }
            ExprKind::Unsafe { body } => {
                self.check_block(body);
            }
            ExprKind::Comptime { body } | ExprKind::Loop { body, .. } => {
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

    /// Check if an expression is terminal (always returns/breaks/continues).
    /// Used to determine that code after a branch is unreachable from that branch.
    fn is_terminal_expr(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Block(stmts) => Self::is_terminal_block(stmts),
            _ => false,
        }
    }

    fn is_terminal_block(stmts: &[Stmt]) -> bool {
        stmts.last().map_or(false, |s| match &s.kind {
            StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => true,
            StmtKind::Expr(e) => Self::is_terminal_expr(e),
            _ => false,
        })
    }

    /// Merge binding states after if/else branches.
    /// `other` is the state after the then-branch; `self.bindings` is after the else-branch.
    /// A binding is Moved only if moved in both branches.
    fn merge_branch_bindings(&mut self, other: &HashMap<String, BindingState>) {
        for (name, then_state) in other {
            if let BindingState::Moved { at } = then_state {
                if let Some(else_state) = self.bindings.get(name) {
                    if matches!(else_state, BindingState::Moved { .. }) {
                        // Moved in both branches — stays moved
                        continue;
                    }
                }
                // Moved in then but not else — keep else state (not moved)
            } else if let Some(BindingState::Moved { .. }) = self.bindings.get(name) {
                // Moved in else but not then — restore to not-moved
                self.bindings.insert(name.clone(), then_state.clone());
            }
        }
    }

    /// Handle assignment semantics based on Copy status:
    ///
    /// Copy types (VS1/VS2): implicit bitwise copy, source stays valid.
    /// Non-Copy + `let` (is_mutable=true): move, source invalidated.
    /// Non-Copy + `const` (is_mutable=false): block-scoped borrow.
    fn handle_assignment(&mut self, expr: &Expr, span: Span, is_mutable: bool) {
        if let Some(ty) = self.program.node_types.get(&expr.id) {
            // Copy types: both source and target remain valid (VS1/VS2)
            if self.is_copy(ty) {
                return;
            }

            // Non-Copy types: move or borrow depending on binding mutability
            // F1: Extract root binding and optional field projection
            let (root, projection) = Self::extract_root_and_fields(expr);
            if let Some(source_name) = root {
                if is_mutable {
                    // Mutable binding (let): check not borrowed, then move
                    if let Some(state) = self.bindings.get(&source_name) {
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
                                let reason = self.move_reason_for(&source_name);
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::UseAfterMove {
                                        name: source_name.clone(),
                                        moved_at: *at,
                                        reason,
                                    },
                                    span,
                                });
                                return;
                            }
                            BindingState::Discarded { at } => {
                                self.errors.push(OwnershipError {
                                    kind: OwnershipErrorKind::UseAfterDiscard {
                                        name: source_name.clone(),
                                        discarded_at: *at,
                                    },
                                    span,
                                });
                                return;
                            }
                            BindingState::Owned => {}
                        }
                    }
                    if projection.is_some() {
                        // F1: Field-projected borrow — disjoint fields don't conflict
                        self.create_borrow_with_projection(source_name, BorrowMode::Exclusive, span, projection);
                    } else {
                        self.bindings.insert(source_name, BindingState::Moved { at: span });
                    }
                } else {
                    // Immutable binding (const): create block-scoped borrow
                    self.create_borrow_with_projection(source_name, BorrowMode::Shared, span, projection);
                }
            }
        }
    }

    /// F1: Extract root binding name and field projection from a field expression.
    /// `state.health` → (Some("state"), Some(["health"]))
    /// `state` → (Some("state"), None)
    /// Complex expressions → (None, None)
    fn extract_root_and_fields(expr: &Expr) -> (Option<String>, Option<Vec<String>>) {
        match &expr.kind {
            ExprKind::Ident(name) => (Some(name.clone()), None),
            ExprKind::Field { object, field } => {
                let (root, fields) = Self::extract_root_and_fields(object);
                if let Some(root) = root {
                    let mut projection = fields.unwrap_or_default();
                    projection.push(field.clone());
                    (Some(root), Some(projection))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        }
    }

    /// LP14: Extract the collection name from a for-loop iterator expression.
    /// `items` → Some("items"), `items.iter()` → Some("items")
    fn extract_iter_collection(iter: &Expr) -> Option<String> {
        match &iter.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::MethodCall { object, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    Some(name.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
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
                    // Check if there's an actual conflict considering field-level borrows.
                    // Non-overlapping field borrows on the same binding don't conflict.
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
                    let reason = self.move_reason_for(&source_name);
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::UseAfterMove {
                            name: source_name,
                            moved_at: *at,
                            reason,
                        },
                        span,
                    });
                }
                BindingState::Discarded { at } => {
                    self.errors.push(OwnershipError {
                        kind: OwnershipErrorKind::UseAfterDiscard {
                            name: source_name,
                            discarded_at: *at,
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

            // String is Copy (immutable, refcounted, 16 bytes — std.strings/S1)
            Type::String => true,

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
                        rask_types::TypeDef::Struct { fields, is_unique, .. } => {
                            // U1: @unique disables implicit copy regardless of size
                            if *is_unique { return false; }
                            fields.iter().all(|(_, t)| self.is_copy(t))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy(t)))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::Trait { .. } => false,
                        rask_types::TypeDef::Union { fields, .. } => {
                            fields.iter().all(|(_, t)| self.is_copy(t))
                                && self.type_size(ty) <= 16
                        }
                        rask_types::TypeDef::NominalAlias { underlying, .. } => {
                            self.is_copy(underlying)
                        }
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

            // Raw pointers are always Copy (just an address)
            Type::RawPtr(_) => true,

            // SIMD vectors: NOT Copy (large, stack-allocated)
            Type::SimdVector { .. } => false,

            // Unresolved types: conservative
            Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. } => false,

            // Trait objects: never Copy (TR11 — owns heap data)
            Type::TraitObject { .. } => false,

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
            // Pointers/references/slices/trait objects: fat pointer
            Type::String | Type::Slice(_) | Type::Fn { .. } | Type::TraitObject { .. } => 16,
            _ => 8,
        }
    }

    /// Determine why a type is move-only (not Copy).
    fn move_reason(&self, ty: &Type) -> MoveReason {
        let type_name = format!("{}", self.program.types.resolve_type_names(ty));
        match ty {
            // String is Copy (S1) — this branch shouldn't be reached
            Type::String => MoveReason::Unknown,
            Type::Generic { base, .. } => {
                // Check if the base type is a heap-owning collection
                let base_name = if let Some(def) = self.program.types.get(*base) {
                    match def {
                        rask_types::TypeDef::Struct { name, .. }
                        | rask_types::TypeDef::Enum { name, .. } => Some(name.as_str()),
                        _ => None,
                    }
                } else {
                    None
                };
                if matches!(base_name, Some("Vec" | "Map" | "Pool")) {
                    MoveReason::OwnsHeapMemory { type_name }
                } else {
                    MoveReason::Unknown
                }
            }
            Type::Named(type_id) => {
                if let Some(def) = self.program.types.get(*type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, is_unique, .. } => {
                            // U1: @unique types report as Unique, not size/heap
                            if *is_unique {
                                return MoveReason::Unique { type_name };
                            }
                            let all_fields_copy = fields.iter().all(|(_, t)| self.is_copy(t));
                            if all_fields_copy {
                                MoveReason::SizeExceedsThreshold { type_name, size: self.type_size(ty) }
                            } else {
                                MoveReason::OwnsHeapMemory { type_name }
                            }
                        }
                        rask_types::TypeDef::Enum { variants, .. } => {
                            let all_copy = variants.iter().all(|(_, data)| data.iter().all(|t| self.is_copy(t)));
                            if all_copy {
                                MoveReason::SizeExceedsThreshold { type_name, size: self.type_size(ty) }
                            } else {
                                MoveReason::OwnsHeapMemory { type_name }
                            }
                        }
                        _ => MoveReason::Unknown,
                    }
                } else {
                    MoveReason::Unknown
                }
            }
            Type::Result { .. } | Type::Union(_) => MoveReason::OwnsHeapMemory { type_name },
            _ => {
                let size = self.type_size(ty);
                if size > 16 {
                    MoveReason::SizeExceedsThreshold { type_name, size }
                } else {
                    MoveReason::Unknown
                }
            }
        }
    }

    /// Check if a binding's type is Pool (for W2 pool-aware rules).
    fn is_pool_type(&self, name: &str) -> bool {
        // Check resolved types from type checker
        if let Some(ty) = self.binding_types.get(name) {
            if let Type::Generic { base, .. } = ty {
                if let Some(def) = self.program.types.get(*base) {
                    return matches!(def,
                        rask_types::TypeDef::Struct { name, .. }
                        | rask_types::TypeDef::Enum { name, .. }
                        if name == "Pool"
                    );
                }
            }
        }
        // Fallback: check parameter type strings (e.g. "Pool<Entity>")
        if let Some(ty_str) = self.param_type_strings.get(name) {
            return ty_str.starts_with("Pool<") || ty_str == "Pool";
        }
        false
    }

    /// Look up the move reason for a binding by name.
    fn move_reason_for(&self, name: &str) -> MoveReason {
        if let Some(ty) = self.binding_types.get(name) {
            self.move_reason(ty)
        } else {
            MoveReason::Unknown
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
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Range { .. } => {}
        }
    }

    /// Collect free variables referenced in an expression (excluding local bindings).
    /// Also collects field projections for each capture (F4: closure field-level captures).
    fn collect_free_vars(&self, expr: &Expr, locals: &HashSet<String>, out: &mut Vec<String>) {
        self.collect_free_vars_inner(expr, locals, out, &mut HashMap::new());
    }

    /// Collect free variables with field projection tracking.
    /// `projections` maps captured var name → narrowest field projection used in the closure.
    fn collect_free_vars_with_projections(
        &self,
        expr: &Expr,
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        self.collect_free_vars_inner(expr, locals, out, projections);
    }

    fn collect_free_vars_inner(
        &self,
        expr: &Expr,
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        // F4: For field access expressions, try to extract root + projection
        // and record the field-level capture instead of whole-object.
        if let ExprKind::Field { .. } = &expr.kind {
            let (root, fields) = Self::extract_root_and_fields(expr);
            if let Some(ref root_name) = root {
                if !locals.contains(root_name) && self.bindings.contains_key(root_name) {
                    if !out.contains(root_name) {
                        out.push(root_name.clone());
                    }
                    // Record or merge field projection for this capture.
                    // If this capture already has a whole-object access (None), keep it.
                    // If it has a different field, widen to whole-object.
                    let entry = projections.entry(root_name.clone());
                    match entry {
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(fields);
                        }
                        std::collections::hash_map::Entry::Occupied(mut e) => {
                            // Merge: if existing is None (whole-object), keep None.
                            // If existing is Some(fields_a) and new is Some(fields_b),
                            // union the field sets. If new is None, widen to None.
                            match (e.get_mut(), &fields) {
                                (None, _) => {} // already whole-object
                                (existing @ Some(_), None) => { *existing = None; }
                                (Some(ref mut existing_fields), Some(new_fields)) => {
                                    for f in new_fields {
                                        if !existing_fields.contains(f) {
                                            existing_fields.push(f.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return; // Don't recurse into Field — we already handled the root
                }
            }
        }

        match &expr.kind {
            ExprKind::Ident(name) => {
                if !locals.contains(name) && self.bindings.contains_key(name) {
                    if !out.contains(name) {
                        out.push(name.clone());
                        // Whole-object access (no field projection)
                        projections.entry(name.clone()).or_insert(None);
                    } else {
                        // Already captured — widen to whole-object if accessed directly
                        projections.insert(name.clone(), None);
                    }
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.collect_free_vars_inner(left, locals, out, projections);
                self.collect_free_vars_inner(right, locals, out, projections);
            }
            ExprKind::Unary { operand, .. } => {
                self.collect_free_vars_inner(operand, locals, out, projections);
            }
            ExprKind::Call { func, args } => {
                self.collect_free_vars_inner(func, locals, out, projections);
                for arg in args { self.collect_free_vars_inner(&arg.expr, locals, out, projections); }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.collect_free_vars_inner(object, locals, out, projections);
                for arg in args { self.collect_free_vars_inner(&arg.expr, locals, out, projections); }
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                // Field access on non-free-var roots (handled above for free vars)
                self.collect_free_vars_inner(object, locals, out, projections);
            }
            ExprKind::Index { object, index } => {
                self.collect_free_vars_inner(object, locals, out, projections);
                self.collect_free_vars_inner(index, locals, out, projections);
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.collect_free_vars_inner(cond, locals, out, projections);
                self.collect_free_vars_inner(then_branch, locals, out, projections);
                if let Some(e) = else_branch { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::Block(stmts) => {
                for s in stmts { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            ExprKind::Closure { params, body, .. } => {
                let mut inner_locals = locals.clone();
                for p in params { inner_locals.insert(p.name.clone()); }
                self.collect_free_vars_inner(body, &inner_locals, out, projections);
            }
            ExprKind::Match { scrutinee, arms } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
                for arm in arms {
                    self.collect_free_vars_inner(&arm.body, locals, out, projections);
                    if let Some(g) = &arm.guard { self.collect_free_vars_inner(g, locals, out, projections); }
                }
            }
            ExprKind::IfLet { expr: scrutinee, then_branch, else_branch, .. } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
                self.collect_free_vars_inner(then_branch, locals, out, projections);
                if let Some(e) = else_branch { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::GuardPattern { expr: scrutinee, else_branch, .. } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
                self.collect_free_vars_inner(else_branch, locals, out, projections);
            }
            ExprKind::IsPattern { expr: scrutinee, .. } => {
                self.collect_free_vars_inner(scrutinee, locals, out, projections);
            }
            ExprKind::Try { expr: inner, else_clause } => {
                self.collect_free_vars_inner(inner, locals, out, projections);
                if let Some(tc) = else_clause {
                    self.collect_free_vars_inner(&tc.body, locals, out, projections);
                }
            }
            ExprKind::IsPresent { expr: inner, .. } => {
                self.collect_free_vars_inner(inner, locals, out, projections);
            }
            ExprKind::Unwrap { expr: inner, .. } | ExprKind::Cast { expr: inner, .. } => {
                self.collect_free_vars_inner(inner, locals, out, projections);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.collect_free_vars_inner(value, locals, out, projections);
                self.collect_free_vars_inner(default, locals, out, projections);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.collect_free_vars_inner(s, locals, out, projections); }
                if let Some(e) = end { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields { self.collect_free_vars_inner(&f.value, locals, out, projections); }
                if let Some(s) = spread { self.collect_free_vars_inner(s, locals, out, projections); }
            }
            ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
                for e in elems { self.collect_free_vars_inner(e, locals, out, projections); }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.collect_free_vars_inner(value, locals, out, projections);
                self.collect_free_vars_inner(count, locals, out, projections);
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args { self.collect_free_vars_inner(&arg.expr, locals, out, projections); }
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            ExprKind::WithAs { bindings, body } => {
                for b in bindings { self.collect_free_vars_inner(&b.source, locals, out, projections); }
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            ExprKind::Spawn { body } => {
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message, .. } => {
                self.collect_free_vars_inner(condition, locals, out, projections);
                if let Some(m) = message { self.collect_free_vars_inner(m, locals, out, projections); }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    self.collect_free_vars_inner(&arm.body, locals, out, projections);
                }
            }
            ExprKind::Unsafe { body } | ExprKind::Comptime { body } | ExprKind::BlockCall { body, .. } | ExprKind::Loop { body, .. } => {
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            _ => {
                // Literals, string interpolation, etc.
            }
        }
    }

    fn collect_free_vars_stmt(&self, stmt: &Stmt, locals: &HashSet<String>, out: &mut Vec<String>) {
        self.collect_free_vars_stmt_inner(stmt, locals, out, &mut HashMap::new());
    }

    fn collect_free_vars_stmt_inner(
        &self,
        stmt: &Stmt,
        locals: &HashSet<String>,
        out: &mut Vec<String>,
        projections: &mut HashMap<String, Option<Vec<String>>>,
    ) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.collect_free_vars_inner(e, locals, out, projections),
            StmtKind::Mut { init, .. } | StmtKind::Const { init, .. } => {
                self.collect_free_vars_inner(init, locals, out, projections);
            }
            StmtKind::MutTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.collect_free_vars_inner(init, locals, out, projections);
            }
            StmtKind::Assign { target, value } => {
                self.collect_free_vars_inner(target, locals, out, projections);
                self.collect_free_vars_inner(value, locals, out, projections);
            }
            StmtKind::Return(Some(e)) | StmtKind::Break { value: Some(e), .. } => {
                self.collect_free_vars_inner(e, locals, out, projections);
            }
            StmtKind::While { cond, body } => {
                self.collect_free_vars_inner(cond, locals, out, projections);
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.collect_free_vars_inner(expr, locals, out, projections);
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            StmtKind::Loop { body, .. } => {
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            StmtKind::For { iter, body, .. } => {
                self.collect_free_vars_inner(iter, locals, out, projections);
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            StmtKind::Ensure { body, else_handler } => {
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
                if let Some((_, handler_body)) = else_handler {
                    for s in handler_body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
                }
            }
            StmtKind::Comptime(body) => {
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.collect_free_vars_inner(iter, locals, out, projections);
                for s in body { self.collect_free_vars_stmt_inner(s, locals, out, projections); }
            }
            StmtKind::Return(None) | StmtKind::Break { value: None, .. }
            | StmtKind::Continue(_) | StmtKind::Discard { .. } => {}
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

    /// Check if a Type value refers to a @resource struct.
    fn type_is_resource(&self, ty: &Type) -> bool {
        match ty {
            Type::Named(type_id) => {
                if let Some(rask_types::TypeDef::Struct { is_resource, .. }) = self.program.types.get(*type_id) {
                    return *is_resource;
                }
                false
            }
            Type::Generic { base, .. } => {
                if let Some(rask_types::TypeDef::Struct { is_resource, .. }) = self.program.types.get(*base) {
                    return *is_resource;
                }
                false
            }
            Type::UnresolvedNamed(name) => self.is_resource_type_name(name),
            Type::UnresolvedGeneric { name, .. } => self.is_resource_type_name(name),
            _ => false,
        }
    }

    /// Check if an expression's inferred type is a @resource type.
    fn expr_is_resource_type(&self, expr: &Expr) -> bool {
        self.program.node_types.get(&expr.id)
            .map_or(false, |ty| self.type_is_resource(ty))
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
                    if let ExprKind::Ident(name) = &arg.expr.kind {
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

    /// At closure/spawn exit, emit errors for unconsumed @resource captures.
    fn check_resource_consumption_in_closure(&mut self, span: Span, context: &str) {
        let unconsumed: Vec<String> = self.resource_bindings.iter()
            .filter(|name| {
                if self.ensure_registered.contains(*name) {
                    return false;
                }
                match self.bindings.get(*name) {
                    Some(BindingState::Moved { .. }) => false,
                    _ => true,
                }
            })
            .cloned()
            .collect();

        for name in unconsumed {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::ResourceNotConsumedInClosure {
                    name,
                    context: context.to_string(),
                },
                span,
            });
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

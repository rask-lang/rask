// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Borrow checking, scope management, and ESAD (Expression-Scoped Access Discipline).

use std::collections::HashMap;

use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::StmtKind;
use rask_ast::{NodeId, Span};

use crate::types::Type;

use super::errors::TypeError;
use super::type_defs::{TypeDef, SelfParam};
use super::TypeChecker;

// ============================================================================
// Borrow Tracking for Aliasing Detection
// ============================================================================

/// Borrow mode for active borrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BorrowMode {
    Shared,    // Read-only borrow
    Exclusive, // Mutable borrow
}

/// An active borrow tracked during expression evaluation.
#[derive(Debug, Clone)]
pub(crate) struct ActiveBorrow {
    pub(crate) var_name: String,
    pub(crate) mode: BorrowMode,
    pub(crate) span: Span,
}

/// A detected view-creating expression. `root` is the variable whose mutation
/// would invalidate the view; `viewed_id` is the expression the view reads from,
/// which is the one whose *type* decides whether storing it is legal. They
/// differ whenever the source is reached through fields: `self.url[i..]` has
/// root `self` but views a string.
pub(crate) struct ViewCreation {
    pub(crate) root: String,
    /// Path as written, for diagnostics (`self.url`).
    pub(crate) display: String,
    pub(crate) mode: BorrowMode,
    pub(crate) viewed_id: NodeId,
}

/// A view binding whose source type was still a type variable during the walk
/// (a field, a loop variable, an inferred local). Re-checked after solving —
/// otherwise `const q = self.url[i..]` escapes the slice rules entirely.
pub(crate) struct PendingViewBinding {
    pub(crate) binding: String,
    pub(crate) display: String,
    pub(crate) source_ty: Type,
    pub(crate) slice_span: Span,
    pub(crate) store_span: Span,
}

/// A persistent borrow that lasts until block scope exit (ESAD Phase 2).
/// Created when a view is stored from a fixed-size source (string, array, struct).
#[derive(Debug, Clone)]
pub(crate) struct PersistentBorrow {
    /// Variable being borrowed (e.g., "line").
    pub(crate) source_var: String,
    /// Variable holding the view (e.g., "key").
    pub(crate) view_var: String,
    #[allow(dead_code)]
    pub(crate) mode: BorrowMode,
    pub(crate) borrow_span: Span,
    /// Scope depth (local_types.len()) when created — cleared on scope exit.
    pub(crate) scope_depth: usize,
}

/// Whether a borrow source can grow/shrink (determines view duration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceStability {
    /// Vec, Pool, Map — views are instant (released at semicolon).
    Growable,
    /// string, array, struct — views persist until block end.
    Fixed,
    /// Type variable, unknown — skip check (no false positives).
    Unknown,
}

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Scope Management
    // ------------------------------------------------------------------------

    pub(super) fn push_scope(&mut self) {
        self.local_types.push(HashMap::new());
    }

    pub(super) fn pop_scope(&mut self) {
        let depth = self.local_types.len();
        // ESAD Phase 2: Remove persistent borrows created at this scope depth
        self.persistent_borrows.retain(|b| b.scope_depth < depth);
        self.local_types.pop();
    }

    pub(super) fn define_local(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Mut));
        }
    }

    /// Define a default (read-only) parameter. Use `define_local` for `mutate`/`take` params.
    pub(super) fn define_local_param(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Param));
        }
    }

    /// Define a `let` binding. Deep-immutable: rebinding, index/field assign,
    /// and mutating method calls all forbidden.
    pub(super) fn define_local_const(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Let));
        }
    }

    /// Define a read-only with-binding (`with expr as v`). Same deep
    /// immutability as `let`, but errors point at `as mut v` instead.
    pub(super) fn define_local_with_read(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::WithRead));
        }
    }

    pub(super) fn lookup_local(&self, name: &str) -> Option<Type> {
        for scope in self.local_types.iter().rev() {
            if let Some((ty, _)) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    /// Look up the binding kind for a local.
    pub(super) fn lookup_binding_kind(&self, name: &str) -> Option<super::BindingKind> {
        for scope in self.local_types.iter().rev() {
            if let Some((_, kind)) = scope.get(name) {
                return Some(*kind);
            }
        }
        None
    }

    /// Extract the root identifier name from an assignment target expression.
    pub(super) fn root_ident_name(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, .. } => Self::root_ident_name(object),
            ExprKind::Index { object, .. } => Self::root_ident_name(object),
            _ => None,
        }
    }

    // ------------------------------------------------------------------------
    // Borrow Stack Management (ESAD Phase 1)
    // ------------------------------------------------------------------------

    /// Push a borrow onto the stack.
    pub(super) fn push_borrow(&mut self, var_name: String, mode: BorrowMode, span: Span) {
        self.borrow_stack.push(ActiveBorrow { var_name, mode, span });
    }

    /// Pop all borrows from the current expression (called at statement end).
    pub(super) fn clear_expression_borrows(&mut self) {
        self.borrow_stack.clear();
    }

    /// Check if accessing a variable would conflict with active borrows.
    /// Returns the conflicting borrow if found.
    pub(super) fn check_borrow_conflict(&self, var_name: &str, access_mode: BorrowMode) -> Option<&ActiveBorrow> {
        for borrow in self.borrow_stack.iter().rev() {
            if borrow.var_name == var_name {
                // Check conflict rules from ESAD spec
                match (borrow.mode, access_mode) {
                    (BorrowMode::Shared, BorrowMode::Shared) => {
                        // Shared + Shared = OK
                        continue;
                    }
                    (BorrowMode::Shared, BorrowMode::Exclusive) |
                    (BorrowMode::Exclusive, BorrowMode::Shared) |
                    (BorrowMode::Exclusive, BorrowMode::Exclusive) => {
                        // Any combination with Exclusive = ERROR
                        return Some(borrow);
                    }
                }
            }
        }
        None
    }

    /// Scan a closure body for variable accesses and check for conflicts.
    /// This implements ESAD Phase 2.
    pub(super) fn check_closure_aliasing(&mut self, params: &[rask_ast::expr::ClosureParam], body: &Expr) {
        let param_names: std::collections::HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
        self.collect_closure_accesses(body, &param_names);
    }

    /// Recursively collect variable accesses in a closure body.
    /// Skip closure params — they're fresh bindings, not captures.
    pub(super) fn collect_closure_accesses(&mut self, expr: &Expr, skip: &std::collections::HashSet<&str>) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if skip.contains(name.as_str()) { return; }
                if let Some(borrow) = self.check_borrow_conflict(name, BorrowMode::Shared) {
                    self.errors.push(TypeError::AliasingViolation {
                        var: name.clone(),
                        borrow_span: borrow.span,
                        access_span: expr.span,
                    });
                }
            }
            ExprKind::MethodCall { object, method: _, args, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if !skip.contains(name.as_str()) {
                        if let Some(borrow) = self.check_borrow_conflict(name, BorrowMode::Exclusive) {
                            self.errors.push(TypeError::AliasingViolation {
                                var: name.clone(),
                                borrow_span: borrow.span,
                                access_span: object.span,
                            });
                        }
                    }
                }
                for arg in args {
                    self.collect_closure_accesses(&arg.expr, skip);
                }
            }
            ExprKind::Call { func, args } => {
                self.collect_closure_accesses(func, skip);
                for arg in args {
                    self.collect_closure_accesses(&arg.expr, skip);
                }
            }
            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    if let StmtKind::Expr(e) = &stmt.kind {
                        self.collect_closure_accesses(e, skip);
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------
    // Source Classification (ESAD Phase 2)
    // ------------------------------------------------------------------------

    /// Classify a type as growable (Vec/Pool/Map) or fixed (string/array/struct).
    /// Growable sources have instant views (released at semicolon).
    /// Fixed sources have persistent views (released at block end).
    /// Note: string is Fixed here; string slice storage is rejected separately (S2).
    pub(super) fn classify_source(&self, ty: &Type) -> SourceStability {
        let resolved = self.ctx.apply(ty);
        match &resolved {
            Type::String => SourceStability::Fixed,
            Type::Array { .. } | Type::Slice(_) => SourceStability::Fixed,
            Type::Named(id) => {
                let name = self.types.type_name(*id);
                match name.as_str() {
                    "Vec" | "Pool" | "Map" => SourceStability::Growable,
                    _ => SourceStability::Fixed,
                }
            }
            Type::Generic { base, .. } => {
                let name = self.types.type_name(*base);
                match name.as_str() {
                    "Vec" | "Pool" | "Map" => SourceStability::Growable,
                    _ => SourceStability::Fixed,
                }
            }
            Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
                if name.starts_with("Vec") || name.starts_with("Pool") || name.starts_with("Map") {
                    SourceStability::Growable
                } else {
                    SourceStability::Fixed
                }
            }
            Type::Var(_) => SourceStability::Unknown,
            _ => SourceStability::Fixed,
        }
    }

    /// Check if an expression creates a view (borrow) from a source variable.
    /// Returns (source_var_name, borrow_mode) if it does.
    /// Render an lvalue path for diagnostics: `self.url`, `cfg.parts[0]`.
    /// Indices collapse to `[..]` — the message only needs to name the source.
    fn view_source_path(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, field } => {
                Some(format!("{}.{}", Self::view_source_path(object)?, field))
            }
            ExprKind::Index { object, .. } => {
                Some(format!("{}[..]", Self::view_source_path(object)?))
            }
            _ => None,
        }
    }

    pub(super) fn detect_view_creation(expr: &Expr) -> Option<ViewCreation> {
        match &expr.kind {
            // Range indexing: source[start..end]
            ExprKind::Index { object, index } => {
                if matches!(&index.kind, ExprKind::Range { .. }) {
                    return Self::viewed(object, BorrowMode::Shared);
                }
                None
            }
            // String view methods (trim, split, etc.)
            ExprKind::MethodCall { object, method, .. } => {
                let view_methods = [
                    "trim", "trim_start", "trim_end",
                    "split", "split_whitespace", "lines", "chars",
                ];
                if view_methods.contains(&method.as_str()) {
                    return Self::viewed(object, BorrowMode::Shared);
                }
                None
            }
            _ => None,
        }
    }

    fn viewed(object: &Expr, mode: BorrowMode) -> Option<ViewCreation> {
        let root = Self::root_ident_name(object)?;
        Some(ViewCreation {
            display: Self::view_source_path(object).unwrap_or_else(|| root.clone()),
            root,
            mode,
            viewed_id: object.id,
        })
    }

    /// Check if mutating a variable conflicts with active persistent borrows.
    pub(super) fn check_persistent_borrow_conflict(&self, var_name: &str) -> Option<&PersistentBorrow> {
        self.persistent_borrows.iter().rev().find(|b| b.source_var == var_name)
    }

    /// True if the method mutates its receiver (`mutate self`). `take self`
    /// consumes — not mutates — and is allowed on const bindings.
    /// Falls back to the union of mutating method names across stdlib stubs
    /// when the receiver's type isn't resolved yet (catches
    /// `const v = Vec.new(); v.push(1)` before constraint solving runs).
    pub(super) fn method_mutates_self(&self, var_name: &str, method_name: &str) -> bool {
        let Some(ty) = self.lookup_local(var_name) else { return false };
        let resolved = self.resolve_named(&self.ctx.apply(&ty));
        let type_id = match &resolved {
            Type::Named(id) => Some(*id),
            Type::Generic { base, .. } => Some(*base),
            _ => None,
        };

        if let Some(id) = type_id {
            // User-defined types: check the internal method table.
            let methods = match self.types.get(id) {
                Some(TypeDef::Struct { methods, .. }) |
                Some(TypeDef::Enum { methods, .. }) => Some(methods),
                _ => None,
            };
            if let Some(methods) = methods {
                if let Some(sig) = methods.iter().find(|m| m.name == method_name) {
                    return matches!(sig.self_param, SelfParam::Mutate);
                }
            }

            // Builtin types (Vec, Map, string, ...): check the stdlib stubs.
            let type_name = self.types.type_name(id);
            if let Some(stub) = rask_stdlib::lookup_method(&type_name, method_name) {
                return stub.mutate_self;
            }

            return false;
        }

        // Receiver type unresolved: fall back to the set of method names that
        // are `mutate self` across all stdlib stubs. `add`/`mul`/`eq` aren't
        // in the set, so desugared arithmetic/comparison don't false-positive.
        rask_stdlib::any_builtin_method_mutates(method_name)
    }

    /// Determine borrow mode for a method call by looking up the method signature.
    /// Falls back to a name-based heuristic for unresolved types.
    pub(super) fn method_borrow_mode(&self, var_name: &str, method_name: &str) -> BorrowMode {
        // Try to look up the actual method signature from the variable's type
        if let Some(ty) = self.lookup_local(var_name) {
            let resolved = self.resolve_named(&self.ctx.apply(&ty));
            let type_id = match &resolved {
                Type::Named(id) => Some(*id),
                Type::Generic { base, .. } => Some(*base),
                _ => None,
            };
            if let Some(id) = type_id {
                let methods = match self.types.get(id) {
                    Some(TypeDef::Struct { methods, .. }) |
                    Some(TypeDef::Enum { methods, .. }) => Some(methods),
                    _ => None,
                };
                if let Some(methods) = methods {
                    if let Some(sig) = methods.iter().find(|m| m.name == method_name) {
                        return match sig.self_param {
                            SelfParam::Mutate | SelfParam::Take => BorrowMode::Exclusive,
                            SelfParam::Value | SelfParam::None => BorrowMode::Shared,
                        };
                    }
                }
            }
        }
        // Fallback: name-based heuristic for unknown/builtin types
        if method_name.starts_with("get") || matches!(method_name,
            "read" | "len" | "is_empty" | "contains" | "find"
            | "iter" | "values" | "keys" | "handles"
            | "starts_with" | "ends_with" | "to_string" | "to_owned" | "clone"
            | "trim" | "trim_start" | "trim_end"
            | "split" | "split_whitespace" | "lines" | "chars"
        ) {
            BorrowMode::Shared
        } else {
            BorrowMode::Exclusive
        }
    }

    /// At a const/let binding, check if the init creates a view from a source.
    /// Growable sources → error (volatile view stored).
    /// String sources → error (S2: string slices are temporary views).
    /// Fixed sources → register persistent borrow.
    pub(super) fn check_view_at_binding(&mut self, binding_name: &str, init: &Expr, stmt_span: Span) {
        if let Some(view) = Self::detect_view_creation(init) {
            // The viewed expression's own type, not the root variable's: for
            // `self.url[i..]` the root is a struct while the slice source is a
            // string field. Falling back to the root keeps plain `s[i..]` working
            // when inference hasn't recorded the node.
            let viewed_ty = self
                .node_types
                .get(&view.viewed_id)
                .cloned()
                .or_else(|| self.lookup_local(&view.root));
            if let Some(source_ty) = viewed_ty {
                let resolved = self.ctx.apply(&source_ty);
                // Inference hasn't reached this source yet (field, loop variable,
                // inferred local). Re-check once constraints are solved instead of
                // waving it through.
                if matches!(resolved, Type::Var(_)) {
                    self.pending_view_bindings.push(PendingViewBinding {
                        binding: binding_name.to_string(),
                        display: view.display,
                        source_ty: resolved,
                        slice_span: init.span,
                        store_span: stmt_span,
                    });
                    return;
                }
                // S2: string slices are temporary — can't be stored
                if matches!(resolved, Type::String) {
                    self.errors.push(TypeError::StringSliceStored {
                        source_var: view.display,
                        view_var: binding_name.to_string(),
                        slice_span: init.span,
                        store_span: stmt_span,
                    });
                    return;
                }
                match self.classify_source(&resolved) {
                    SourceStability::Fixed => {
                        self.persistent_borrows.push(PersistentBorrow {
                            source_var: view.root,
                            view_var: binding_name.to_string(),
                            mode: view.mode,
                            borrow_span: init.span,
                            scope_depth: self.local_types.len(),
                        });
                    }
                    SourceStability::Growable => {
                        self.errors.push(TypeError::VolatileViewStored {
                            source_var: view.display,
                            view_var: binding_name.to_string(),
                            source_span: init.span,
                            store_span: stmt_span,
                        });
                    }
                    SourceStability::Unknown => {}
                }
            }
        }
    }

    /// Re-check view bindings whose source type was unresolved during the walk.
    /// Only the "can't store this" errors are reported here — a persistent borrow
    /// can't be registered after the fact, since the scope it guarded is gone.
    pub(super) fn validate_pending_view_bindings(&mut self) {
        for pending in std::mem::take(&mut self.pending_view_bindings) {
            let resolved = self.ctx.apply(&pending.source_ty);
            if matches!(resolved, Type::String) {
                self.errors.push(TypeError::StringSliceStored {
                    source_var: pending.display,
                    view_var: pending.binding,
                    slice_span: pending.slice_span,
                    store_span: pending.store_span,
                });
            } else if matches!(self.classify_source(&resolved), SourceStability::Growable) {
                self.errors.push(TypeError::VolatileViewStored {
                    source_var: pending.display,
                    view_var: pending.binding,
                    source_span: pending.slice_span,
                    store_span: pending.store_span,
                });
            }
        }
    }
}

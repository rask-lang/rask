// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type checker implementation.

use std::collections::HashMap;

use rask_ast::decl::Decl;
use rask_ast::NodeId;
use rask_resolve::{ResolvedProgram, SymbolId};

use crate::types::Type;

mod type_defs;
mod builtins;
mod type_table;
mod inference;
mod errors;
mod parse_type;
mod borrow;
mod declarations;
mod check_pattern;
mod check_fn;
mod check_stmt;
mod check_expr;
mod unify;
mod generics;
mod resolve;

pub use type_defs::{TypeDef, MethodSig, SelfParam, ParamMode, TypedProgram};
pub use type_table::TypeTable;
pub use inference::{TypeConstraint, InferenceContext};
pub use errors::TypeError;
pub use parse_type::{parse_type_string, extract_projection};

use borrow::{ActiveBorrow, PersistentBorrow};

pub struct TypeChecker {
    /// Symbol table from resolution.
    pub(super) resolved: ResolvedProgram,
    /// Type registry.
    pub(super) types: TypeTable,
    /// Inference state.
    pub(super) ctx: InferenceContext,
    /// Types assigned to nodes.
    pub(super) node_types: HashMap<NodeId, Type>,
    /// Types assigned to symbols (for bindings without annotations).
    pub(super) symbol_types: HashMap<SymbolId, Type>,
    /// Collected errors.
    pub(super) errors: Vec<TypeError>,
    /// Current function's return type (for checking return statements).
    pub(super) current_return_type: Option<Type>,
    /// Current Self type (inside extend blocks).
    pub(super) current_self_type: Option<Type>,
    /// Scope stack for local variable types (innermost scope last).
    /// Tuple: (type, is_read_only). Default params are read-only; `mutate` params are not.
    pub(super) local_types: Vec<HashMap<String, (Type, bool)>>,
    /// Active borrows for aliasing detection (ESAD Phase 1).
    pub(super) borrow_stack: Vec<ActiveBorrow>,
    /// Persistent borrows across statements within a scope (ESAD Phase 2).
    pub(super) persistent_borrows: Vec<PersistentBorrow>,
    /// Pending generic call sites: (call NodeId, fresh type vars for type params).
    /// Resolved after constraint solving to populate TypedProgram.call_type_args.
    pub(super) pending_call_type_args: Vec<(NodeId, Vec<Type>)>,
    /// SymbolId → type param names for generic functions.
    /// Keyed by SymbolId (not name) to avoid collisions between
    /// same-named functions in different scopes.
    pub(super) fn_type_params: HashMap<SymbolId, Vec<String>>,
    /// Whether we're inside an `unsafe {}` block (for validating pointer ops and extern calls).
    pub(super) in_unsafe: bool,
}

impl TypeChecker {
    /// Create a new type checker.
    pub fn new(resolved: ResolvedProgram) -> Self {
        Self {
            resolved,
            types: TypeTable::new(),
            ctx: InferenceContext::new(),
            node_types: HashMap::new(),
            symbol_types: HashMap::new(),
            errors: Vec::new(),
            current_return_type: None,
            current_self_type: None,
            local_types: Vec::new(),
            borrow_stack: Vec::new(),
            persistent_borrows: Vec::new(),
            pending_call_type_args: Vec::new(),
            fn_type_params: HashMap::new(),
            in_unsafe: false,
        }
    }

    pub fn check(mut self, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
        self.collect_type_declarations(decls);

        // Global scope for module-level bindings (imports, etc.)
        self.push_scope();
        for decl in decls {
            self.check_decl(decl);
        }
        self.pop_scope();

        self.solve_constraints();

        // Default unresolved literal type vars (unsuffixed int → i32, float → f64)
        self.ctx.apply_literal_defaults();

        let node_types: HashMap<_, _> = self
            .node_types
            .iter()
            .map(|(id, ty)| (*id, self.ctx.apply(ty)))
            .collect();

        // Resolve pending generic call type args
        let call_type_args: HashMap<_, _> = self
            .pending_call_type_args
            .iter()
            .map(|(node_id, vars)| {
                let resolved: Vec<Type> = vars.iter().map(|v| self.ctx.apply(v)).collect();
                (*node_id, resolved)
            })
            .collect();

        if self.errors.is_empty() {
            Ok(TypedProgram {
                symbols: self.resolved.symbols,
                resolutions: self.resolved.resolutions,
                types: self.types,
                node_types,
                call_type_args,
            })
        } else {
            let ctx = &self.ctx;
            let types = &self.types;
            let errors: Vec<_> = self.errors.into_iter()
                .map(|e| Self::apply_error_substitutions_with_ctx(e, ctx))
                .map(|e| types.resolve_error_types(e))
                // Filter out cascading errors where both sides resolved to <error>
                .filter(|e| !matches!(e, TypeError::Mismatch { expected: Type::Error, found: Type::Error, .. }))
                .collect();
            if errors.is_empty() {
                Ok(TypedProgram {
                    symbols: self.resolved.symbols,
                    resolutions: self.resolved.resolutions,
                    types: self.types,
                    node_types,
                    call_type_args,
                })
            } else {
                Err(errors)
            }
        }
    }

    fn apply_error_substitutions_with_ctx(error: TypeError, ctx: &InferenceContext) -> TypeError {
        match error {
            TypeError::Mismatch { expected, found, span } => TypeError::Mismatch {
                expected: ctx.apply(&expected),
                found: ctx.apply(&found),
                span,
            },
            TypeError::NotCallable { ty, span } => TypeError::NotCallable {
                ty: ctx.apply(&ty),
                span,
            },
            TypeError::NoSuchField { ty, field, span } => TypeError::NoSuchField {
                ty: ctx.apply(&ty),
                field,
                span,
            },
            TypeError::NoSuchMethod { ty, method, span } => TypeError::NoSuchMethod {
                ty: ctx.apply(&ty),
                method,
                span,
            },
            TypeError::MissingReturn { function_name, expected_type, span } => TypeError::MissingReturn {
                function_name,
                expected_type: ctx.apply(&expected_type),
                span,
            },
            TypeError::TryInNonPropagatingContext { return_ty, span } => TypeError::TryInNonPropagatingContext {
                return_ty: ctx.apply(&return_ty),
                span,
            },
            TypeError::InfiniteType { var, ty, span } => TypeError::InfiniteType {
                var,
                ty: ctx.apply(&ty),
                span,
            },
            TypeError::TryOnNonResult { found, span } => TypeError::TryOnNonResult {
                found: ctx.apply(&found),
                span,
            },
            other => other,
        }
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new(ResolvedProgram::default())
    }
}

// ============================================================================
// Public API
// ============================================================================

pub fn typecheck(resolved: ResolvedProgram, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
    let checker = TypeChecker::new(resolved);
    checker.check(decls)
}

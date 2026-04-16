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
pub use parse_type::parse_type_string;

use borrow::{ActiveBorrow, PersistentBorrow};

/// Classification of unsafe operations for auditing and tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnsafeCategory {
    PointerDeref,
    PointerDerefWrite,
    PointerArithmetic,
    PointerMethod,
    ExternCall,
    UnsafeFuncCall,
    Transmute,
    UnionFieldAccess,
}

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
    /// Collected unsafe operations with their locations (for tooling/auditing).
    pub(super) unsafe_ops: Vec<(rask_ast::Span, UnsafeCategory)>,
    /// Whether we're inferring an assignment target (union field writes are safe per UN3).
    pub(super) in_assign_target: bool,
    /// Whether we're inferring an expression in statement position (value discarded).
    /// Suppresses branch-type agreement for if/else and match.
    pub(super) in_stmt_expr: bool,
    /// GC1/GC2: Pre-created type vars for functions with inferred params/return.
    /// Key is function name, value is (param_type_vars, return_type_var).
    pub(super) inferred_fn_types: HashMap<String, (Vec<(String, Type)>, Type)>,
    /// TR5: implicit trait coercion sites. NodeId of expression → trait name.
    /// MIR lowering uses this to emit TraitBox instructions at coercion sites.
    pub(super) trait_coercions: HashMap<NodeId, String>,
    /// ER20: Collected error types from `try` calls in error-accumulation mode.
    pub(super) inferred_errors: Vec<Type>,
    /// ER20: Whether we're collecting errors instead of unifying them.
    pub(super) accumulate_errors: bool,
    /// Types for binding names and parameters, keyed by (span.start, span.end).
    pub(super) span_types: HashMap<(usize, usize, u16), Type>,
    /// D1: Bindings invalidated by `discard`. Maps name → discard span.
    pub(super) discarded_bindings: HashMap<String, rask_ast::Span>,
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
            unsafe_ops: Vec::new(),
            inferred_fn_types: HashMap::new(),
            in_assign_target: false,
            in_stmt_expr: false,
            trait_coercions: HashMap::new(),
            inferred_errors: Vec::new(),
            span_types: HashMap::new(),
            accumulate_errors: false,
            discarded_bindings: HashMap::new(),
        }
    }

    pub fn check(self, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
        let (program, errors) = self.check_lenient(decls);
        if errors.is_empty() {
            Ok(program)
        } else {
            Err(errors)
        }
    }

    /// Lenient variant: always returns the (partial) TypedProgram plus any errors.
    ///
    /// The TypedProgram is usable even when errors exist — node_types contains
    /// types for every expression that was successfully inferred. Callers can
    /// run ownership/effects analysis on the partial program to collect more
    /// diagnostics in a single pipeline pass.
    pub fn check_lenient(mut self, decls: &[Decl]) -> (TypedProgram, Vec<TypeError>) {
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

        // Build reverse map TypeId → name for normalizing Named types
        let id_to_name: HashMap<crate::TypeId, String> = self.types.type_names
            .iter()
            .map(|(name, id)| (*id, name.clone()))
            .collect();

        // Resolve pending generic call type args, normalizing Named(TypeId)
        // to UnresolvedNamed(name) so monomorphizer can use consistent names.
        let call_type_args: HashMap<_, _> = self
            .pending_call_type_args
            .iter()
            .map(|(node_id, vars)| {
                let resolved: Vec<Type> = vars.iter().map(|v| {
                    let applied = self.ctx.apply(v);
                    Self::normalize_named_types(&applied, &id_to_name)
                }).collect();
                (*node_id, resolved)
            })
            .collect();

        let trait_coercions = self.trait_coercions.clone();

        let unsafe_ops = self.unsafe_ops;

        let span_types: HashMap<_, _> = self
            .span_types
            .iter()
            .map(|(key, ty)| (*key, self.ctx.apply(ty)))
            .collect();

        let errors: Vec<_> = {
            let ctx = &self.ctx;
            let types = &self.types;
            self.errors.into_iter()
                .map(|e| Self::apply_error_substitutions_with_ctx(e, ctx))
                .map(|e| types.resolve_error_types(e))
                // Filter out cascading errors where either side resolved to <error>.
                // These are always consequences of an earlier failure, not root causes.
                .filter(|e| !matches!(e,
                    TypeError::Mismatch { expected: Type::Error, .. }
                    | TypeError::Mismatch { found: Type::Error, .. }
                ))
                .collect()
        };

        let program = TypedProgram {
            symbols: self.resolved.symbols,
            resolutions: self.resolved.resolutions,
            types: self.types,
            node_types,
            call_type_args,
            trait_coercions,
            unsafe_ops,
            span_types,
        };

        (program, errors)
    }

    /// Replace Named(TypeId) with UnresolvedNamed(name) so the monomorphizer
    /// sees consistent string-based type names regardless of resolution order.
    fn normalize_named_types(ty: &Type, id_to_name: &HashMap<crate::TypeId, String>) -> Type {
        match ty {
            Type::Named(id) => {
                if let Some(name) = id_to_name.get(id) {
                    Type::UnresolvedNamed(name.clone())
                } else {
                    ty.clone()
                }
            }
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| match a {
                    crate::GenericArg::Type(inner) => {
                        crate::GenericArg::Type(Box::new(Self::normalize_named_types(inner, id_to_name)))
                    }
                    other => other.clone(),
                }).collect(),
            },
            Type::Option(inner) => Type::Option(Box::new(Self::normalize_named_types(inner, id_to_name))),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::normalize_named_types(ok, id_to_name)),
                err: Box::new(Self::normalize_named_types(err, id_to_name)),
            },
            _ => ty.clone(),
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
            TypeError::NominalMismatch { expected, found, nominal_name, span } => TypeError::NominalMismatch {
                expected: ctx.apply(&expected),
                found: ctx.apply(&found),
                nominal_name,
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

/// Typecheck with stdlib type/method declarations registered but not body-checked.
pub fn typecheck_with_stdlib(
    resolved: ResolvedProgram,
    decls: &[Decl],
    stdlib_decls: &[Decl],
) -> Result<TypedProgram, Vec<TypeError>> {
    let mut checker = TypeChecker::new(resolved);
    checker.collect_type_declarations(stdlib_decls);
    checker.check(decls)
}

/// Lenient typecheck: always returns the (partial) TypedProgram plus errors.
///
/// Enables cross-stage error accumulation — the driver can feed the partial
/// program to ownership/effects analysis even when type errors exist, so
/// users see type errors + ownership errors + effect warnings in one pass
/// instead of fixing them one category at a time.
pub fn typecheck_with_stdlib_lenient(
    resolved: ResolvedProgram,
    decls: &[Decl],
    stdlib_decls: &[Decl],
) -> (TypedProgram, Vec<TypeError>) {
    let mut checker = TypeChecker::new(resolved);
    checker.collect_type_declarations(stdlib_decls);
    checker.check_lenient(decls)
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type inference context and constraint tracking.

use std::collections::HashMap;

use rask_ast::Span;

use crate::types::{GenericArg, Type, TypeVarId};

/// A constraint generated during type inference.
#[derive(Debug, Clone)]
pub enum TypeConstraint {
    /// Two types must be equal.
    Equal(Type, Type, Span),
    /// Type must have a field with given name and type.
    HasField {
        ty: Type,
        field: String,
        expected: Type,
        span: Span,
        /// V5: Self type at constraint creation site (for private field checks)
        self_type: Option<Type>,
    },
    /// Type must have a method with given signature.
    HasMethod {
        ty: Type,
        method: String,
        args: Vec<Type>,
        ret: Type,
        span: Span,
    },
    /// Return value must match function return type, with auto-wrap into a
    /// sum type (T or E or T or none) when applicable. Defers wrapping
    /// decision until the return type is resolved.
    ///
    /// `position` distinguishes spec ER9 (return: any T or E wraps) from
    /// ER11 (assignment / field / argument: only `T or none` widens, the
    /// optional shape; an error union must already have the union type).
    ReturnValue {
        ret_ty: Type,
        expected: Type,
        position: WrapPosition,
        span: Span,
    },
    /// ER27: scrutinee is a `T or E`, and `narrow_ty` must match either `T`
    /// or a component of `E`. Deferred so method-call return types can
    /// resolve before the pattern side is decided.
    TypePatternMatches {
        scrutinee: Type,
        narrow_ty: Type,
        ty_name: String,
        span: Span,
    },
}

/// Kind of unsuffixed literal (for deferred defaulting).
#[derive(Debug, Clone, Copy)]
pub enum LiteralKind {
    Integer,
    Float,
}

/// Position of a value-coercion site, used by `ReturnValue` to gate
/// auto-wrap into `T or E`.
///
/// Per ER9/ER11: `T or E` (where E ≠ none) auto-wraps **only** at `return`.
/// Optionals (`T or none`) widen at any position. Anywhere else for a
/// non-optional sum, the value must already have the union type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapPosition {
    /// Return statement — full ER9 auto-wrap into T or E.
    Return,
    /// Assignment, field initialiser, function argument — ER11 restricts
    /// auto-wrap to optional (`T or none`) only.
    Bind,
}

/// State for type inference and unification.
#[derive(Debug, Default)]
pub struct InferenceContext {
    /// Counter for fresh type variables.
    pub(super) next_var: u32,
    /// Substitutions: TypeVarId -> Type.
    pub(super) substitutions: HashMap<TypeVarId, Type>,
    /// Constraints collected during inference.
    pub(super) constraints: Vec<TypeConstraint>,
    /// Type vars created for unsuffixed literals. Defaults applied after solving.
    pub(super) literal_vars: HashMap<TypeVarId, LiteralKind>,
}

impl InferenceContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a fresh type variable.
    pub fn fresh_var(&mut self) -> Type {
        let id = TypeVarId(self.next_var);
        self.next_var += 1;
        Type::Var(id)
    }

    /// Create a fresh type variable for an unsuffixed literal.
    /// After constraint solving, unresolved literal vars default to i32/f64.
    pub fn fresh_literal_var(&mut self, kind: LiteralKind) -> Type {
        let id = TypeVarId(self.next_var);
        self.next_var += 1;
        self.literal_vars.insert(id, kind);
        Type::Var(id)
    }

    /// Apply defaults for unresolved literal type vars.
    pub fn apply_literal_defaults(&mut self) {
        for (&var_id, &kind) in self.literal_vars.iter() {
            // Only default if not yet resolved
            if !self.substitutions.contains_key(&var_id) {
                let default = match kind {
                    LiteralKind::Integer => Type::I32,
                    LiteralKind::Float => Type::F64,
                };
                self.substitutions.insert(var_id, default);
            }
        }
    }

    /// Add a constraint.
    pub fn add_constraint(&mut self, constraint: TypeConstraint) {
        self.constraints.push(constraint);
    }

    /// Apply all known substitutions to a type.
    pub fn apply(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(id) => {
                if let Some(resolved) = self.substitutions.get(id) {
                    self.apply(resolved)
                } else {
                    ty.clone()
                }
            }
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| self.apply_generic_arg(a)).collect(),
            },
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| self.apply_generic_arg(a)).collect(),
            },
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|t| self.apply(t)).collect(),
                ret: Box::new(self.apply(ret)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|t| self.apply(t)).collect()),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.apply(elem)),
                len: *len,
            },
            Type::Slice(inner) => Type::Slice(Box::new(self.apply(inner))),
            Type::Result { ok, err } if **err == Type::None => Type::option(self.apply(ok)),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.apply(ok)),
                err: Box::new(self.apply(err)),
            },
            _ => ty.clone(),
        }
    }

    fn apply_generic_arg(&self, arg: &GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(Box::new(self.apply(ty))),
            GenericArg::ConstUsize(n) => GenericArg::ConstUsize(*n),
        }
    }

    /// Check if a type variable occurs in a type (prevents infinite types).
    pub(super) fn occurs_in(&self, var: TypeVarId, ty: &Type) -> bool {
        match ty {
            Type::Var(id) => {
                if *id == var {
                    return true;
                }
                if let Some(subst) = self.substitutions.get(id) {
                    return self.occurs_in(var, subst);
                }
                false
            }
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                args.iter().any(|a| self.occurs_in_generic_arg(var, a))
            }
            Type::Fn { params, ret } => {
                params.iter().any(|p| self.occurs_in(var, p)) || self.occurs_in(var, ret)
            }
            Type::Tuple(elems) => elems.iter().any(|e| self.occurs_in(var, e)),
            Type::Array { elem, .. } => self.occurs_in(var, elem),
            Type::Slice(inner) => self.occurs_in(var, inner),
            Type::Result { ok, err } => self.occurs_in(var, ok) || self.occurs_in(var, err),
            _ => false,
        }
    }

    fn occurs_in_generic_arg(&self, var: TypeVarId, arg: &GenericArg) -> bool {
        match arg {
            GenericArg::Type(ty) => self.occurs_in(var, ty),
            GenericArg::ConstUsize(_) => false,
        }
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Validation for `T or E` type formation (ER3, ER4).

use rask_ast::Span;

use super::errors::TypeError;
use super::type_defs::{MethodSig, SelfParam, TypeDef};
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    /// Walk `ty` and validate every `Result { ok, err }` node against ER3 and ER4.
    ///
    /// ER3: T ≠ E (disjointness).
    /// ER4: E (or each component of a union E) implements `ErrorMessage`.
    ///
    /// Unresolved components (`Var`, `Error`) are skipped to avoid false positives
    /// during inference.
    pub(super) fn validate_result_types_in(&mut self, ty: &Type, span: Span) {
        let mut errs = Vec::new();
        collect_result_errors(ty, span, self, &mut errs);
        self.errors.extend(errs);
    }
}

fn collect_result_errors(
    ty: &Type,
    span: Span,
    checker: &TypeChecker,
    errs: &mut Vec<TypeError>,
) {
    match ty {
        Type::Result { ok, err } => {
            validate_single_result(ok, err, span, checker, errs);
            collect_result_errors(ok, span, checker, errs);
            collect_result_errors(err, span, checker, errs);
        }
        Type::Option(inner)
        | Type::Slice(inner)
        | Type::RawPtr(inner) => collect_result_errors(inner, span, checker, errs),
        Type::Array { elem, .. } | Type::SimdVector { elem, .. } => {
            collect_result_errors(elem, span, checker, errs)
        }
        Type::Tuple(elems) | Type::Union(elems) => {
            for e in elems {
                collect_result_errors(e, span, checker, errs);
            }
        }
        Type::Fn { params, ret } => {
            for p in params {
                collect_result_errors(p, span, checker, errs);
            }
            collect_result_errors(ret, span, checker, errs);
        }
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
            for a in args {
                if let crate::types::GenericArg::Type(inner) = a {
                    collect_result_errors(inner, span, checker, errs);
                }
            }
        }
        _ => {}
    }
}

fn validate_single_result(
    ok: &Type,
    err: &Type,
    span: Span,
    checker: &TypeChecker,
    errs: &mut Vec<TypeError>,
) {
    let ok_r = checker.ctx.apply(ok);
    let err_r = checker.ctx.apply(err);

    if is_unresolved(&ok_r) || is_unresolved(&err_r) {
        return;
    }

    // ER3: disjointness — T ≠ E, and T ∉ components of a union E.
    let err_components: Vec<&Type> = match &err_r {
        Type::Union(types) => types.iter().collect(),
        other => vec![other],
    };
    for comp in &err_components {
        if &&ok_r == comp {
            errs.push(TypeError::ResultNotDisjoint {
                ty: ok_r.clone(),
                span,
            });
            break;
        }
    }

    // ER4: E (or each component of a union E) must implement ErrorMessage.
    // `none` is exempt — it's the absent sentinel for `T or none` (the optional
    // shape), not an error type.
    for comp in &err_components {
        if is_unresolved(comp) {
            continue;
        }
        if matches!(comp, Type::None) {
            continue;
        }
        if !implements_error_message(comp, checker) {
            errs.push(TypeError::ErrorMessageMissing {
                ty: (*comp).clone(),
                span,
            });
        }
    }
}

fn is_unresolved(ty: &Type) -> bool {
    matches!(ty, Type::Var(_) | Type::Error | Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. })
}

/// Structural check: does `ty` have `func message(self) -> string`?
///
/// Primitives and builtins without user methods fail this check.
/// For nominal aliases, methods are on the alias's `Named` TypeId (registered
/// via `extend Alias { ... }` → `register_impl_methods`).
fn implements_error_message(ty: &Type, checker: &TypeChecker) -> bool {
    let type_id = match ty {
        Type::Named(id) => *id,
        Type::Generic { base, .. } => *base,
        // Primitives, functions, tuples, arrays, etc. cannot have user methods.
        _ => return false,
    };

    let def = match checker.types.get(type_id) {
        Some(d) => d,
        None => return false,
    };

    let methods: &[MethodSig] = match def {
        TypeDef::Struct { methods, .. } | TypeDef::Enum { methods, .. } => methods,
        // Nominal aliases store methods separately through impl registration;
        // if a method list ever lives on NominalAlias, handle it here.
        _ => return false,
    };

    methods.iter().any(|m| {
        m.name == "message"
            && matches!(m.self_param, SelfParam::Value)
            && m.params.is_empty()
            && matches!(m.ret, Type::String)
    })
}

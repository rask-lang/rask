// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Validation for `T or E` type formation (ER3, ER4).

use rask_ast::Span;

use super::errors::TypeError;
use super::type_defs::{MethodSig, SelfParam, TypeDef};
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    /// Walk `ty` and validate every `Result { ok, err }` node against ER3, ER4,
    /// and the duplicate-variant rule (U5 from union-types.md).
    ///
    /// ER3: T ≠ E (disjointness).
    /// ER4: E (or each component of a union E) implements `ErrorMessage`.
    /// U5:  flattening the nested `or` tree must not yield a repeated variant
    ///      (e.g. `T??` = `(T or none) or none`, `(T or E) or E`).
    ///
    /// Unresolved components (`Var`, `Error`) are skipped to avoid false positives
    /// during inference.
    pub(super) fn validate_result_types_in(&mut self, ty: &Type, span: Span) {
        let mut errs = Vec::new();
        collect_result_errors(ty, span, self, &mut errs);
        check_nested_optional(ty, span, &mut errs);
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
            check_duplicate_sum_variants(ty, ok, err, span, errs);
            collect_result_errors(ok, span, checker, errs);
            collect_result_errors(err, span, checker, errs);
        }
        Type::Slice(inner)
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

/// U5: walk an `or`-tree (nested Result/Option/Union) and report any leaf type
/// that appears more than once. Each unique duplicate is reported once.
///
/// Skips any variant that disjointness (ER3) already flagged at this span — the
/// fix is the same and reporting both is noise.
fn check_duplicate_sum_variants(
    full_ty: &Type,
    ok: &Type,
    err: &Type,
    span: Span,
    errs: &mut Vec<TypeError>,
) {
    let mut leaves = Vec::new();
    collect_or_leaves(ok, &mut leaves);
    collect_or_leaves(err, &mut leaves);

    // Variants already reported by disjointness on this span — skip to avoid
    // double-reporting the same type.
    let already_disjoint: Vec<Type> = errs
        .iter()
        .filter_map(|e| match e {
            TypeError::ResultNotDisjoint { ty, span: s } if *s == span => Some(ty.clone()),
            _ => None,
        })
        .collect();

    let mut seen = Vec::new();
    let mut reported = Vec::new();
    for leaf in &leaves {
        if seen.contains(leaf) {
            if !reported.contains(leaf) && !already_disjoint.iter().any(|t| t == *leaf) {
                errs.push(TypeError::DuplicateSumVariant {
                    ty: full_ty.clone(),
                    variant: (*leaf).clone(),
                    span,
                });
                reported.push(*leaf);
            }
        } else {
            seen.push(*leaf);
        }
    }
}

/// Detects T?? — Option<Option<_>> — outside of any Result wrapper, where the
/// duplicate-variant rule still applies but `validate_single_result` doesn't
/// fire (because there's no surrounding T or E node).
fn check_nested_optional(ty: &Type, span: Span, errs: &mut Vec<TypeError>) {
    walk_for_nested_option(ty, span, errs);
}

fn walk_for_nested_option(ty: &Type, span: Span, errs: &mut Vec<TypeError>) {
    match ty {
        Type::Result { ok: inner, err } if **err == Type::None => {
            if inner.is_option() {
                errs.push(TypeError::DuplicateSumVariant {
                    ty: ty.clone(),
                    variant: Type::None,
                    span,
                });
            }
            walk_for_nested_option(inner, span, errs);
        }
        Type::Result { ok, err } => {
            walk_for_nested_option(ok, span, errs);
            walk_for_nested_option(err, span, errs);
        }
        Type::Slice(inner) | Type::RawPtr(inner) => walk_for_nested_option(inner, span, errs),
        Type::Array { elem, .. } | Type::SimdVector { elem, .. } => {
            walk_for_nested_option(elem, span, errs)
        }
        Type::Tuple(elems) | Type::Union(elems) => {
            for e in elems {
                walk_for_nested_option(e, span, errs);
            }
        }
        Type::Fn { params, ret } => {
            for p in params {
                walk_for_nested_option(p, span, errs);
            }
            walk_for_nested_option(ret, span, errs);
        }
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
            for a in args {
                if let crate::types::GenericArg::Type(inner) = a {
                    walk_for_nested_option(inner, span, errs);
                }
            }
        }
        _ => {}
    }
}

/// Gather the leaf types of an `or`-tree. `Result { ok, err }` recurses both
/// sides (for `T?` = `T or none`, this naturally pushes T then Type::None);
/// `Union` contributes each component. Anything else is a leaf.
fn collect_or_leaves<'a>(ty: &'a Type, out: &mut Vec<&'a Type>) {
    match ty {
        Type::Result { ok, err } => {
            collect_or_leaves(ok, out);
            collect_or_leaves(err, out);
        }
        Type::Union(types) => {
            for t in types {
                collect_or_leaves(t, out);
            }
        }
        other => out.push(other),
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
        // `any ErrorMessage` is the trait itself — no need to check it satisfies itself
        if matches!(comp, Type::TraitObject { trait_name } if trait_name == "ErrorMessage") {
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

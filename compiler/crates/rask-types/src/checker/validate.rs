// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Validation for `T or E` type formation (ER3, ER4).

use rask_ast::Span;

use super::errors::TypeError;
use super::type_defs::{MethodSig, SelfParam, TypeDef};
use super::TypeChecker;

use crate::types::Type;

/// ER3a: "after substitution, `left` and `right` must still be different types."
/// `left`/`right` carry the fresh vars standing in for the call's type args;
/// `param`/`other` are the source spellings, for the message.
pub(super) struct DisjointObligation {
    pub callee: String,
    pub param: String,
    pub left: Type,
    pub right: Type,
    pub other: Type,
    pub span: Span,
}

/// Gather every `T or E` node in a type as an `(ok, err)` pair.
fn collect_result_nodes<'a>(ty: &'a Type, out: &mut Vec<(&'a Type, &'a Type)>) {
    match ty {
        Type::Result { ok, err } => {
            out.push((ok, err));
            collect_result_nodes(ok, out);
            collect_result_nodes(err, out);
        }
        Type::Slice(inner) | Type::RawPtr(inner) => collect_result_nodes(inner, out),
        Type::Array { elem, .. } | Type::SimdVector { elem, .. } => collect_result_nodes(elem, out),
        Type::Tuple(elems) | Type::Union(elems) => {
            for e in elems {
                collect_result_nodes(e, out);
            }
        }
        Type::Fn { params, ret } => {
            for p in params {
                collect_result_nodes(p, out);
            }
            collect_result_nodes(ret, out);
        }
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
            for a in args {
                if let crate::types::GenericArg::Type(inner) = a {
                    collect_result_nodes(inner, out);
                }
            }
        }
        _ => {}
    }
}

impl TypeChecker {
    /// Walk `ty` and validate every `Result { ok, err }` node against ER3, ER4,
    /// and the duplicate-variant rule (U5 from union-types.md).
    ///
    /// ER3: T ≠ E (disjointness).
    /// ER4: E (or each component of a union E) implements `Error`.
    /// U5:  flattening the nested `or` tree must not yield a repeated variant
    ///      (e.g. `(T or E) or E`). `none` is exempt — repeated `none` layers
    ///      are nested optionals, which stay distinct (type.optionals/OPT28).
    ///
    /// Unresolved components (`Var`, `Error`) are skipped to avoid false positives
    /// during inference.
    pub(super) fn validate_result_types_in(&mut self, ty: &Type, span: Span) {
        let mut errs = Vec::new();
        collect_result_errors(ty, span, self, &mut errs);
        self.errors.extend(errs);
    }

    /// #314: verify each generic call's type argument satisfies the trait
    /// bounds declared on the callee. Runs after constraint solving so the
    /// type-arg vars are resolved. Type args that are still generic (a bound
    /// param forwarded to another generic call) or unresolved are skipped —
    /// they're checked at the outermost concrete call site.
    pub(super) fn validate_pending_bound_checks(&mut self) {
        let pending = std::mem::take(&mut self.pending_bound_checks);
        // Dedup identical (type, trait, span) reports.
        let mut reported: Vec<(String, String, Span)> = Vec::new();
        for (var, traits, span) in pending {
            // Resolve `UnresolvedNamed("Foo")` to `Named(id)` so `check_satisfies`
            // can find the type's methods (an unresolved name reports none).
            let ty = self.resolve_named(&self.ctx.apply(&var));
            // Only check concrete, registered types — skip vars, errors, and
            // bare type parameters (unresolved names with no registered type).
            match &ty {
                Type::Var(_) | Type::Error => continue,
                Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. } => continue,
                _ => {}
            }
            let bound = crate::traits::TraitBound::new("_", traits);
            if let Err(errs) = crate::traits::verify_instantiation(&self.types, &ty, std::slice::from_ref(&bound), span) {
                for e in errs {
                    let (ty_name, trait_name) = trait_error_parts(&e);
                    let key = (ty_name.clone(), trait_name.clone(), span);
                    if reported.contains(&key) {
                        continue;
                    }
                    reported.push(key);
                    // An unknown trait has no type to blame, so reporting it as
                    // "`_` does not implement X" pointed at the wrong thing
                    // entirely — the name is the problem (#713).
                    if matches!(e, crate::traits::TraitError::UnknownTrait(_)) {
                        self.errors.push(TypeError::NoSuchTrait {
                            trait_name,
                            known: self.declared_trait_names(),
                            span,
                        });
                        continue;
                    }
                    let err = self.bound_error(&ty, ty_name, trait_name, span);
                    self.errors.push(err);
                }
            }
        }
    }

    /// ER3a: read the disjointness obligations off a generic callee's signature
    /// and record them against this call site.
    ///
    /// `sig` is the callee's signature with type params still spelled as
    /// `UnresolvedNamed`; `subst` maps each param name to the fresh var standing
    /// in for its type argument. For every `T or E` node in the signature, each
    /// success leaf is obliged to differ from each error leaf. The obligation
    /// only bites when a type param is involved — a signature that writes two
    /// concrete colliding types was already rejected at its declaration.
    pub(super) fn note_disjointness_obligations(
        &mut self,
        callee: &str,
        sig: &Type,
        subst: &std::collections::HashMap<&str, Type>,
        span: Span,
    ) {
        let mut nodes = Vec::new();
        collect_result_nodes(sig, &mut nodes);
        for (ok, err) in nodes {
            let mut ok_leaves = Vec::new();
            let mut err_leaves = Vec::new();
            collect_or_leaves(ok, &mut ok_leaves);
            collect_or_leaves(err, &mut err_leaves);
            for l in &ok_leaves {
                for r in &err_leaves {
                    // `none` layers instead of colliding (ER3b).
                    if matches!(l, Type::None) || matches!(r, Type::None) {
                        continue;
                    }
                    // Blame the type param; if both sides are params, the first.
                    // Neither being a param means nothing substitution can
                    // change, and the declaration site already checked it.
                    let param = [l, r].into_iter().find_map(|t| match t {
                        Type::UnresolvedNamed(n) if subst.contains_key(n.as_str()) => Some(n.clone()),
                        _ => None,
                    });
                    let Some(param) = param else { continue };
                    let other = if matches!(l, Type::UnresolvedNamed(n) if *n == param) { r } else { l };
                    self.pending_disjointness.push(DisjointObligation {
                        callee: callee.to_string(),
                        param,
                        left: TypeChecker::substitute_type_params(l, subst),
                        right: TypeChecker::substitute_type_params(r, subst),
                        other: (*other).clone(),
                        span,
                    });
                }
            }
        }
    }

    /// ER3a: report every recorded obligation whose two sides resolved to the
    /// same concrete type. One error per (call site, parameter).
    pub(super) fn validate_pending_disjointness(&mut self) {
        let pending = std::mem::take(&mut self.pending_disjointness);
        let mut reported: Vec<(Span, String)> = Vec::new();
        for ob in pending {
            let left = self.ctx.apply(&ob.left);
            let right = self.ctx.apply(&ob.right);
            if is_unresolved(&left) || is_unresolved(&right) || left != right {
                continue;
            }
            if left == Type::None {
                continue;
            }
            let key = (ob.span, ob.param.clone());
            if reported.contains(&key) {
                continue;
            }
            reported.push(key);
            self.errors.push(TypeError::ResultNotDisjointAtInstantiation {
                callee: ob.callee,
                param: ob.param,
                arg: left,
                other: ob.other,
                span: ob.span,
            });
        }
    }

    /// RC1/RC3: record a site whose type must not be a `Vec`/`Map` of linear
    /// values. Validated after constraint solving (see
    /// `validate_pending_linear_containers`) so inferred element types are
    /// concrete. `ty` may still contain type vars here — they're resolved later.
    pub(super) fn note_linear_container_site(&mut self, span: Span, ty: Type) {
        self.pending_linear_containers.push((span, ty));
    }

    /// RC1/RC3: reject any recorded site whose resolved type embeds a `Vec<T>`
    /// or `Map<K, V>` with a linear element/key. One error per span.
    ///
    /// Also HA4: a float is not Hashable, so it can't be a Map key. Same list of
    /// sites and the same reason for deferring — a `Map.new()` key type isn't
    /// known until the inserts have been seen. `Map<f64, V>` was accepted, and
    /// then a NaN key found its own entry natively and missed on the interpreter
    /// (#306), which is the contract violation HA4 names.
    pub(super) fn validate_pending_linear_containers(&mut self) {
        let pending = std::mem::take(&mut self.pending_linear_containers);
        let mut reported: Vec<Span> = Vec::new();
        let mut float_key_reported: Vec<Span> = Vec::new();
        for (span, ty) in pending {
            let ty = self.ctx.apply(&ty);
            if let Some(key) = self.types.find_float_map_key(&ty) {
                if !float_key_reported.contains(&span) {
                    float_key_reported.push(span);
                    self.errors.push(TypeError::FloatMapKey { key, span });
                }
            }
            if let Some((container, elem)) = self.types.find_linear_container(&ty) {
                if reported.contains(&span) {
                    continue;
                }
                reported.push(span);
                self.errors.push(TypeError::LinearInContainer {
                    container,
                    elem,
                    span,
                });
            }
        }
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
/// `none` is exempt: repeated `none` layers form a nested optional, where the
/// layers are told apart by position rather than by type (type.optionals/OPT28).
/// `collect_or_leaves` therefore never descends into an optional.
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

/// Gather the leaf types of an `or`-tree. `Result { ok, err }` recurses both
/// sides; `Union` contributes each component. Anything else is a leaf.
///
/// An optional (`X or none`) is a leaf, not a node to flatten. Flattening it
/// would make `T??` look like two `none` variants of one sum, when it is really
/// two layers that stay distinct (type.optionals/OPT28).
fn collect_or_leaves<'a>(ty: &'a Type, out: &mut Vec<&'a Type>) {
    match ty {
        Type::Result { err, .. } if **err == Type::None => out.push(ty),
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
    // ER3b: `none` is exempt. `none?` is a two-layer optional, not a collision.
    let err_components: Vec<&Type> = match &err_r {
        Type::Union(types) => types.iter().collect(),
        other => vec![other],
    };
    for comp in err_components.iter().filter(|_| ok_r != Type::None) {
        if &&ok_r == comp {
            errs.push(TypeError::ResultNotDisjoint {
                ty: ok_r.clone(),
                span,
            });
            break;
        }
    }

    // ER4: E (or each component of a union E) must implement Error.
    // `none` is exempt — it's the absent sentinel for `T or none` (the optional
    // shape), not an error type.
    for comp in &err_components {
        if is_unresolved(comp) {
            continue;
        }
        if matches!(comp, Type::None) {
            continue;
        }
        // `any Error` is the trait itself — no need to check it satisfies itself
        if matches!(comp, Type::TraitObject { trait_name } if trait_name == "Error") {
            continue;
        }
        if !implements_error_message(comp, checker) {
            errs.push(TypeError::ErrorTraitMissing {
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
        TypeDef::Struct { methods, .. }
        | TypeDef::Enum { methods, .. }
        | TypeDef::NominalAlias { methods, .. } => methods,
        _ => return false,
    };

    methods.iter().any(|m| {
        m.name == "message"
            && matches!(m.self_param, SelfParam::Value)
            && m.params.is_empty()
            && matches!(m.ret, Type::String)
    })
}

impl TypeChecker {
    /// The right error for a failed bound. `Encode`/`Decode` aren't method sets,
    /// so they get their own shape of message — one that names the field that
    /// blocked it rather than telling you to implement a trait you can't.
    /// Every trait the program declares, for a did-you-mean on a misspelt one.
    pub(super) fn declared_trait_names(&self) -> Vec<String> {
        self.types
            .iter()
            .filter_map(|def| match def {
                crate::TypeDef::Trait { name, .. } => Some(name.clone()),
                _ => None,
            })
            .chain(
                crate::COMPILER_PROVIDED_TRAITS
                    .iter()
                    .chain(["Numeric", "Integer", "Float", "Encode", "Decode"].iter())
                    .map(|s| s.to_string()),
            )
            .collect()
    }

    pub(super) fn bound_error(
        &self,
        ty: &Type,
        ty_name: String,
        trait_name: String,
        span: Span,
    ) -> TypeError {
        if trait_name != "Encode" && trait_name != "Decode" {
            let context = if matches!(trait_name.as_str(), "Numeric" | "Integer" | "Float") {
                super::TraitBoundContext::NumericBound
            } else {
                super::TraitBoundContext::GenericBound
            };
            return TypeError::TraitNotSatisfied { ty: ty_name, trait_name, context, span };
        }
        let verb = if trait_name == "Encode" { "encoded" } else { "decoded" };
        let checker = crate::traits::TraitChecker::new(&self.types);
        let (field, field_ty) = match checker.first_unencodable_field(ty) {
            Some((f, fty)) => (Some(f), Some(fty)),
            None => (None, None),
        };
        TypeError::NotSerializable {
            ty: ty_name,
            trait_name,
            verb: verb.to_string(),
            field,
            field_ty,
            span,
        }
    }
}

/// Best-effort `(type name, trait name)` for reporting a failed bound.
pub(super) fn trait_error_parts(e: &crate::traits::TraitError) -> (String, String) {
    use crate::traits::TraitError::*;
    match e {
        NotSatisfied { ty, trait_name, .. } => (ty.clone(), trait_name.clone()),
        MissingMethod { ty, trait_name, .. } => (ty.clone(), trait_name.clone()),
        SignatureMismatch { ty, method, .. } => (ty.clone(), method.clone()),
        UnknownTrait(name) => (String::from("_"), name.clone()),
        ConflictingMethods { trait1, .. } => (String::from("_"), trait1.clone()),
    }
}

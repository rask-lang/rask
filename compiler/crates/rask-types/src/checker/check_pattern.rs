// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pattern type checking.

use rask_ast::expr::Pattern;
use rask_ast::Span;

use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::type_defs::TypeDef;
use super::type_table::TypeTable;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

/// Recursively resolve `UnresolvedNamed` and `UnresolvedGeneric` to `Named`
/// and `Generic` where the type table knows the name. Matches `resolve_named`
/// but walks into `Option`, `Result`, `Generic`, `Tuple`, `Slice`, `Array`,
/// `Fn`, and `Union` so two types built from different sources compare equal.
pub(super) fn normalize_type(ty: &Type, types: &TypeTable) -> Type {
    match ty {
        Type::UnresolvedNamed(name) => {
            if let Some(id) = types.get_type_id(name) {
                return Type::Named(id);
            }
            // Stub parsers store generic forms ("Vec<string>") as UnresolvedNamed.
            // Re-parse so they compare equal with properly-parsed Generic types.
            if name.contains('<') {
                if let Ok(parsed) = parse_type_string(name, types) {
                    if parsed != *ty {
                        return normalize_type(&parsed, types);
                    }
                }
            }
            ty.clone()
        }
        Type::UnresolvedGeneric { name, args } => {
            let normalized_args: Vec<GenericArg> = args
                .iter()
                .map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(normalize_type(t, types))),
                    other => other.clone(),
                })
                .collect();
            if let Some(id) = types.get_type_id(name) {
                Type::Generic { base: id, args: normalized_args }
            } else {
                Type::UnresolvedGeneric { name: name.clone(), args: normalized_args }
            }
        }
        Type::Result { ok, err } if **err == Type::None => {
            Type::option(normalize_type(ok, types))
        }
        Type::Result { ok, err } => Type::Result {
            ok: Box::new(normalize_type(ok, types)),
            err: Box::new(normalize_type(err, types)),
        },
        Type::Generic { base, args } => Type::Generic {
            base: *base,
            args: args.iter().map(|a| match a {
                GenericArg::Type(t) => GenericArg::Type(Box::new(normalize_type(t, types))),
                other => other.clone(),
            }).collect(),
        },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| normalize_type(e, types)).collect()),
        Type::Slice(elem) => Type::Slice(Box::new(normalize_type(elem, types))),
        Type::Array { elem, len } => Type::Array {
            elem: Box::new(normalize_type(elem, types)),
            len: *len,
        },
        Type::Fn { params, ret } => Type::Fn {
            params: params.iter().map(|p| normalize_type(p, types)).collect(),
            ret: Box::new(normalize_type(ret, types)),
        },
        Type::Union(variants) => Type::Union(variants.iter().map(|v| normalize_type(v, types)).collect()),
        _ => ty.clone(),
    }
}

/// The leaf types a two-branch value can actually hold, normalized and with
/// the outermost error last. `T?` gives `[T, none]`; a flat `T? or E` gives
/// `[T, none, E]`, because the layers stay distinct (OPT30) and each one is
/// separately matchable.
pub(super) fn two_branch_leaves(
    ctx: &mut super::inference::InferenceContext,
    types: &TypeTable,
    ty: &Type,
) -> Vec<Type> {
    let resolved = ctx.apply(ty);
    let Type::Result { ok, err } = &resolved else {
        return vec![normalize_type(&resolved, types)];
    };
    let mut leaves = two_branch_leaves(ctx, types, ok);
    match normalize_type(&ctx.apply(err), types) {
        Type::Union(variants) => leaves.extend(variants),
        other => leaves.push(other),
    }
    leaves
}

/// Resolve a bare type name to a Type.
/// Returns UnresolvedNamed when the name isn't a known primitive or user type.
fn resolve_type_name(name: &str, types: &TypeTable) -> Type {
    parse_type_string(name, types).unwrap_or_else(|_| Type::UnresolvedNamed(name.to_string()))
}

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Pattern Checking
    // ------------------------------------------------------------------------

    /// When `ty_name` is `Enum.Variant` and `Enum` is the scrutinee's error side,
    /// the type that variant's payload binds to.
    ///
    /// One field binds that field's type; several bind a tuple; none binds unit,
    /// which is what a binder on a fieldless variant deserves — there's nothing
    /// behind it to name.
    ///
    /// A union error side is handled too: `is ParseError.Syntax` on a
    /// `T or (ParseError | DivError)` finds the variant in whichever member
    /// declares it.
    pub(super) fn err_variant_payload(&self, resolved: &Type, ty_name: &str) -> Option<Type> {
        let Type::Result { err, .. } = resolved else { return None };
        let (enum_name, variant_name) = ty_name.split_once('.')?;
        let err_applied = self.ctx.apply(err);
        let candidates: Vec<Type> = match err_applied {
            Type::Union(members) => members,
            other => vec![other],
        };
        for candidate in candidates {
            let Type::Named(id) = self.ctx.apply(&candidate) else { continue };
            if self.types.type_name(id) != enum_name {
                continue;
            }
            let Some(TypeDef::Enum { variants, .. }) = self.types.get(id) else { continue };
            let fields = variants
                .iter()
                .find(|(v, _)| v == variant_name)
                .map(|(_, f)| f.clone())?;
            return Some(match fields.len() {
                0 => Type::Unit,
                1 => fields[0].clone(),
                _ => Type::Tuple(fields),
            });
        }
        None
    }

    pub(super) fn check_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Type, span: Span) -> Vec<(String, Type)> {
        match pattern {
            Pattern::Wildcard => vec![],

            Pattern::Ident(name) => {
                // Qualified enum variant (e.g., "Status.Active") — match, don't bind
                if name.contains('.') {
                    return self.check_constructor_pattern(name, &[], scrutinee_ty, span);
                }
                // OPT2/ER2: reject `Ok`/`Err`/`Some`/`None` when the scrutinee
                // is Result/Option. Allow them as user-enum variant names
                // (e.g. `enum GrepResult { Ok(i32), Err(string) }`).
                if matches!(name.as_str(), "Ok" | "Err" | "Some" | "None") {
                    let applied = self.ctx.apply(scrutinee_ty);
                    if matches!(applied, Type::Result { .. }) {
                        self.errors.push(TypeError::LegacyWrapperPattern {
                            name: name.clone(),
                            with_binding: false,
                            span,
                        });
                        return vec![];
                    }
                }
                // ER27: a bare `Type` against a two-branch scrutinee is a type
                // pattern, not a binding. A name that resolves to a real type
                // has to be one of the branches — otherwise the test can never
                // be true, and falling through to a binding hid that: `r is
                // i32` on a `void or DivError` type-checked, then the two
                // backends disagreed about the answer.
                let resolved = self.ctx.apply(scrutinee_ty);
                if let Type::Result { .. } = &resolved {
                    let candidate = resolve_type_name(name, &self.types);
                    if !matches!(candidate, Type::UnresolvedNamed(_)) {
                        let candidate = normalize_type(&candidate, &self.types);
                        let branches =
                            two_branch_leaves(&mut self.ctx, &self.types, &resolved);
                        if branches.contains(&candidate) {
                            return vec![];
                        }
                        // A branch that hasn't resolved yet can't be compared
                        // against; the deferred check settles it later.
                        if branches.iter().any(|b| matches!(b, Type::Var(_))) {
                            self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                                scrutinee: scrutinee_ty.clone(),
                                narrow_ty: candidate,
                                ty_name: name.clone(),
                                span,
                            });
                            return vec![];
                        }
                        self.errors.push(TypeError::TypePatternNotResult {
                            ty_name: name.clone(),
                            found: resolved,
                            span,
                        });
                        return vec![];
                    }
                }
                vec![(name.clone(), scrutinee_ty.clone())]
            }

            Pattern::Literal(expr) => {
                let lit_ty = self.infer_expr(expr);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    lit_ty,
                    span,
                ));
                vec![]
            }

            Pattern::Constructor { name, fields } => {
                // OPT2/ER2: reject `Ok(v)` / `Err(e)` / `Some(v)` / `None(..)`
                // when the scrutinee is Result/Option. User enums with these
                // variant names (e.g. simple_grep.rk's `GrepResult`) are fine.
                if matches!(name.as_str(), "Ok" | "Err" | "Some" | "None") {
                    let applied = self.ctx.apply(scrutinee_ty);
                    if matches!(applied, Type::Result { .. }) {
                        self.errors.push(TypeError::LegacyWrapperPattern {
                            name: name.clone(),
                            with_binding: !fields.is_empty(),
                            span,
                        });
                        return vec![];
                    }
                }
                self.check_constructor_pattern(name, fields, scrutinee_ty, span)
            }

            Pattern::Struct { name, fields, .. } => {
                // Look up the struct type
                if let Some(type_id) = self.types.get_type_id(name) {
                    // Constrain scrutinee to be this struct type
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        scrutinee_ty.clone(),
                        Type::Named(type_id),
                        span,
                    ));
                    // Check each field pattern
                    let struct_fields = self.types.get(type_id).and_then(|def| {
                        if let TypeDef::Struct { fields, .. } = def {
                            Some(fields.clone())
                        } else {
                            None
                        }
                    });
                    let mut bindings = vec![];
                    if let Some(struct_fields) = struct_fields {
                        for (field_name, field_pattern) in fields {
                            let field_ty = struct_fields
                                .iter()
                                .find(|(n, _)| n == field_name)
                                .map(|(_, t)| t.clone())
                                .unwrap_or_else(|| {
                                    self.errors.push(TypeError::NoSuchField {
                                        ty: Type::Named(type_id),
                                        field: field_name.clone(),
                                        span,
                                    });
                                    Type::Error
                                });
                            bindings.extend(self.check_pattern(field_pattern, &field_ty, span));
                        }
                    }
                    bindings
                } else {
                    let mut bindings = vec![];
                    for (_, field_pattern) in fields {
                        let fresh = self.ctx.fresh_var();
                        bindings.extend(self.check_pattern(field_pattern, &fresh, span));
                    }
                    bindings
                }
            }

            Pattern::Tuple(patterns) => {
                let elem_types: Vec<_> = patterns.iter().map(|_| self.ctx.fresh_var()).collect();
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    Type::Tuple(elem_types.clone()),
                    span,
                ));
                let mut bindings = vec![];
                for (pat, elem_ty) in patterns.iter().zip(elem_types.iter()) {
                    bindings.extend(self.check_pattern(pat, elem_ty, span));
                }
                bindings
            }

            Pattern::Range { start, end } => {
                // Both bounds must match the scrutinee type. The parser guarantees
                // they're char or int literals of matching kind, so we just unify.
                let start_ty = self.infer_expr(start);
                let end_ty = self.infer_expr(end);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    start_ty,
                    span,
                ));
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    end_ty,
                    span,
                ));
                vec![]
            }

            // ER23/ER27: `TypeName [as binding]` type pattern.
            // In match arms, matches either the T (ok) or E (err) branch of a
            // Result by type. In `if r is E as e`, typically the err side.
            // Union `E = A | B | ...`: accept if TypeName is a union component.
            Pattern::TypePat { ty_name, binding } => {
                let narrow_ty = normalize_type(&resolve_type_name(ty_name, &self.types), &self.types);
                let resolved = self.ctx.apply(scrutinee_ty);
                // ER23 at variant granularity. `match` already dispatches on one
                // variant of a `T or E` — `error-types.md` shows exactly that with
                // `IoError.NotFound(p)` arms, and ER30 makes covering every
                // variant of E the exhaustiveness rule. So `is MyErr.Worse as w`
                // is the same question, and the binder takes the *variant's*
                // payload rather than the whole error.
                //
                // Without this, `MyErr.Worse` was compared against the scrutinee's
                // own branches (`i64`, `MyErr`), never matched one, and reported
                // "`MyErr.Worse` is not a branch of `i64 or MyErr` — this test can
                // never be true". The bare `is MyErr.Worse` next to it proved that
                // wrong: it parses as a constructor pattern and never reached this
                // check at all (#766).
                if let Some(payload) = self.err_variant_payload(&resolved, ty_name) {
                    return match binding {
                        Some(name) => vec![(name.clone(), payload)],
                        None => vec![],
                    };
                }
                match &resolved {
                    Type::Result { err, .. } => {
                        // Every branch the scrutinee could hold — a flat
                        // `T? or E` offers `T`, `none` and `E` (OPT30).
                        let branches = two_branch_leaves(&mut self.ctx, &self.types, &resolved);
                        // A branch that hasn't resolved yet can't be compared
                        // against. Defer — by then it's either a match or a
                        // real error, and the message can name the type.
                        if !branches.contains(&narrow_ty)
                            && branches.iter().any(|b| matches!(b, Type::Var(_)))
                        {
                            self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                                scrutinee: scrutinee_ty.clone(),
                                narrow_ty: narrow_ty.clone(),
                                ty_name: ty_name.clone(),
                                span,
                            });
                        } else if !branches.contains(&narrow_ty) {
                            let err_applied = normalize_type(&self.ctx.apply(err), &self.types);
                            // A union error side gets the "not in union"
                            // wording, which names the alternatives.
                            if matches!(&err_applied, Type::Union(_)) {
                                self.errors.push(TypeError::TypePatternNotInUnion {
                                    ty_name: ty_name.clone(),
                                    union: err_applied,
                                    span,
                                });
                            } else {
                                self.errors.push(TypeError::TypePatternNotResult {
                                    ty_name: ty_name.clone(),
                                    found: resolved,
                                    span,
                                });
                            }
                        }
                    }
                    Type::Var(_) => {
                        // Defer the ok-vs-err decision until the scrutinee
                        // resolves (e.g. a method-call return type finishes
                        // unifying). Pinning narrow_ty to err here would
                        // wrongly unify ok == narrow_ty when narrow_ty is
                        // actually the ok-branch type.
                        self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                            scrutinee: scrutinee_ty.clone(),
                            narrow_ty: narrow_ty.clone(),
                            ty_name: ty_name.clone(),
                            span,
                        });
                    }
                    _ => {
                        self.errors.push(TypeError::TypePatternNotResult {
                            ty_name: ty_name.clone(),
                            found: resolved,
                            span,
                        });
                    }
                }
                if let Some(name) = binding {
                    vec![(name.clone(), narrow_ty)]
                } else {
                    vec![]
                }
            }

            Pattern::Or(alternatives) => {
                if let Some(first) = alternatives.first() {
                    let bindings = self.check_pattern(first, scrutinee_ty, span);
                    let expected_names: Vec<&str> = bindings.iter()
                        .map(|(n, _)| n.as_str())
                        .collect();
                    for alt in &alternatives[1..] {
                        let alt_bindings = self.check_pattern(alt, scrutinee_ty, span);
                        // Verify all alternatives bind the same names
                        let alt_names: Vec<&str> = alt_bindings.iter()
                            .map(|(n, _)| n.as_str())
                            .collect();
                        if alt_names != expected_names {
                            self.errors.push(TypeError::GenericError(
                                format!(
                                    "or-pattern alternatives must bind the same variables \
                                     (first binds {:?}, alternative binds {:?})",
                                    expected_names, alt_names,
                                ),
                                span,
                            ));
                        }
                        // Unify binding types across alternatives
                        for ((_, first_ty), (_, alt_ty)) in bindings.iter().zip(alt_bindings.iter()) {
                            self.ctx.add_constraint(TypeConstraint::Equal(
                                first_ty.clone(),
                                alt_ty.clone(),
                                span,
                            ));
                        }
                    }
                    bindings
                } else {
                    vec![]
                }
            }
        }
    }

    pub(super) fn check_constructor_pattern(
        &mut self,
        name: &str,
        fields: &[Pattern],
        scrutinee_ty: &Type,
        span: Span,
    ) -> Vec<(String, Type)> {
        let resolved_scrutinee = self.ctx.apply(scrutinee_ty);

        match name {
            "Ok" => {
                match &resolved_scrutinee {
                    Type::Result { ok, .. } => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], ok, span);
                        }
                        // Bare `Ok` (no fields): return inner type for guard unwrapping
                        if fields.is_empty() {
                            return vec![("".to_string(), *ok.clone())];
                        }
                    }
                    Type::Var(_) => {
                        let ok_ty = self.ctx.fresh_var();
                        let err_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Result {
                                ok: Box::new(ok_ty.clone()),
                                err: Box::new(err_ty),
                            },
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &ok_ty, span);
                        }
                        // Bare `Ok`: return fresh inner type for guard unwrapping
                        if fields.is_empty() {
                            return vec![("".to_string(), ok_ty)];
                        }
                    }
                    _ => {}
                }
            }
            "Err" => {
                match &resolved_scrutinee {
                    Type::Result { err, .. } => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], err, span);
                        }
                        if fields.is_empty() {
                            return vec![("".to_string(), *err.clone())];
                        }
                    }
                    Type::Var(_) => {
                        let ok_ty = self.ctx.fresh_var();
                        let err_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Result {
                                ok: Box::new(ok_ty),
                                err: Box::new(err_ty.clone()),
                            },
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &err_ty, span);
                        }
                        if fields.is_empty() {
                            return vec![("".to_string(), err_ty)];
                        }
                    }
                    _ => {}
                }
            }
            "Some" => {
                match resolved_scrutinee.as_option() {
                    Some(inner) => {
                        let inner = inner.clone();
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &inner, span);
                        }
                        if fields.is_empty() {
                            return vec![("".to_string(), inner)];
                        }
                    }
                    None if matches!(resolved_scrutinee, Type::Var(_)) => {
                        let inner_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::option(inner_ty.clone()),
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &inner_ty, span);
                        }
                        if fields.is_empty() {
                            return vec![("".to_string(), inner_ty)];
                        }
                    }
                    _ => {}
                }
            }
            "None" => {
                if fields.is_empty() {
                    // Constrain scrutinee to Option unless already known to be one.
                    // Var types need the constraint too — otherwise a standalone
                    // None arm won't propagate the Option requirement.
                    if !resolved_scrutinee.is_option() {
                        let inner_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::option(inner_ty),
                            span,
                        ));
                    }
                    return vec![];
                }
            }
            _ => {}
        }

        // Qualified `Enum.Variant` patterns: the variant lookup needs the
        // bare variant name, not the enum-qualified path.
        let variant_lookup_name = name.rsplit('.').next().unwrap_or(name);

        match &resolved_scrutinee {
            Type::Named(type_id) => {
                let variant_fields = self.types.get(*type_id).and_then(|def| {
                    if let TypeDef::Enum { variants, .. } = def {
                        variants.iter()
                            .find(|(n, _)| n == variant_lookup_name)
                            .map(|(_, f)| f.clone())
                    } else {
                        None
                    }
                });

                if let Some(variant_field_types) = variant_fields {
                    if fields.len() != variant_field_types.len() {
                        self.errors.push(TypeError::ArityMismatch {
                            expected: variant_field_types.len(),
                            found: fields.len(),
                            span,
                        });
                        return vec![];
                    }
                    let mut bindings = vec![];
                    for (pat, field_ty) in fields.iter().zip(variant_field_types.iter()) {
                        bindings.extend(self.check_pattern(pat, field_ty, span));
                    }
                    return bindings;
                }
            }
            _ => {}
        }

        let mut bindings = vec![];
        for pat in fields {
            let fresh = self.ctx.fresh_var();
            bindings.extend(self.check_pattern(pat, &fresh, span));
        }
        bindings
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pattern type checking.

use std::collections::HashMap;

use rask_ast::expr::Pattern;
use rask_ast::Span;

use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::resolve_type_expr;
use rask_ast::ty::TypeExpr;
use super::type_defs::TypeDef;
use super::type_table::TypeTable;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

/// The name `guard_type_test_as_binding` binds. Never in scope: only the
/// type it gets is read.
const GUARD_BINDING: &str = "<guard>";

/// Recursively resolve `UnresolvedNamed` and `UnresolvedGeneric` to `Named`
/// and `Generic` where the type table knows the name. Matches `resolve_named`
/// but walks into `Option`, `Result`, `Generic`, `Tuple`, `Array`,
/// `Fn`, and `Union` so two types built from different sources compare equal.
pub(super) fn normalize_type(ty: &Type, types: &TypeTable) -> Type {
    match ty {
        Type::UnresolvedNamed(name) => {
            if let Some(id) = types.get_type_id(name) {
                return Type::Named(id);
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

/// Resolve a written type, or UnresolvedNamed when it names nothing known.
fn resolve_type_name(ty: &TypeExpr, types: &TypeTable) -> Type {
    resolve_type_expr(ty, types).unwrap_or_else(|_| Type::UnresolvedNamed(ty.to_string()))
}

impl TypeChecker {
    /// Which of `branches` a written type names, compared as resolved types:
    /// `usize` names a `u64` branch on a 64-bit target. A generic branch
    /// written without its arguments (`CasFailed` for `CasFailed<i64>`) counts
    /// when exactly one branch has that head. `None` when the name isn't a
    /// known type, or names none of them.
    pub(super) fn branch_named(&self, ty: &TypeExpr, branches: &[Type]) -> Option<usize> {
        let named = resolve_type_name(ty, &self.types);
        if let Type::UnresolvedNamed(n) = &named {
            // A type parameter names no type in the table, but it does name
            // a branch: inside `func f<T, E>(v: T or E)` the arm `T as x` is
            // the `T` side. Without this a generic body couldn't match a
            // `T or E` exhaustively at all (#1439).
            let is_param = self.types.is_type_param_in_scope(n) || self.type_params_in_scope.contains(n);
            return if is_param { branches.iter().position(|b| *b == named) } else { None };
        }
        let named = normalize_type(&named, &self.types);
        if let Some(i) = branches.iter().position(|b| *b == named) {
            return Some(i);
        }
        let mut heads = branches
            .iter()
            .enumerate()
            .filter(|(_, b)| self.same_type_head(b, &named))
            .map(|(i, _)| i);
        match (heads.next(), heads.next()) {
            (Some(i), None) => Some(i),
            _ => None,
        }
    }

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
    pub(super) fn err_variant_payload(&self, resolved: &Type, ty: &TypeExpr) -> Option<Type> {
        let Type::Result { err, .. } = resolved else { return None };
        let TypeExpr::Named { path, args } = ty else { return None };
        let ([enum_name, variant_name], true) = (path.as_slice(), args.is_empty()) else {
            return None;
        };
        let err_applied = self.ctx.apply(err);
        let candidates: Vec<Type> = match err_applied {
            Type::Union(members) => members,
            other => vec![other],
        };
        for candidate in candidates {
            let Type::Named(id) = self.ctx.apply(&candidate) else { continue };
            if self.types.type_name(id) != *enum_name {
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

    /// Spell a variant pattern's name as `Enum.Variant`.
    ///
    /// An already-qualified name is left alone. A bare one is looked up in the
    /// scrutinee's own enum, which is the only thing that can say which enum a
    /// bare `Completed { at }` belongs to.
    fn qualify_variant_name(&mut self, name: &str, scrutinee_ty: &Type) -> String {
        if name.contains('.') {
            return name.to_string();
        }
        let resolved = normalize_type(&self.ctx.apply(scrutinee_ty), &self.types);
        let id = match resolved {
            Type::Named(id) => id,
            Type::Generic { base, .. } => base,
            _ => return name.to_string(),
        };
        let Some(TypeDef::Enum { variants, .. }) = self.types.get(id) else {
            return name.to_string();
        };
        if !variants.iter().any(|(v, _)| v == name) {
            return name.to_string();
        }
        format!("{}.{}", self.types.type_name(id), name)
    }

    /// `name` as a variant of the enum the scrutinee already is, qualified,
    /// with how many payload fields it carries. `None` when the scrutinee
    /// isn't known to be that enum: a bare name is then a binding, and a
    /// qualified one is checked as a constructor.
    fn variant_of_scrutinee(&mut self, name: &str, scrutinee_ty: &Type) -> Option<(String, usize)> {
        let resolved = normalize_type(&self.ctx.apply(scrutinee_ty), &self.types);
        let id = match resolved {
            Type::Named(id) => id,
            Type::Generic { base, .. } => base,
            _ => return None,
        };
        let qualified = self.qualify_variant_name(name, scrutinee_ty);
        let (enum_id, _) = self.enum_id_from_pattern_name(&qualified)?;
        if enum_id != id {
            return None;
        }
        let variant = qualified.rsplit('.').next()?;
        let TypeDef::Enum { variants, .. } = self.types.get(id)? else { return None };
        let arity = variants.iter().find(|(v, _)| v == variant)?.1.len();
        Some((qualified, arity))
    }

    /// The type a bare pattern name stands for, when it names one.
    ///
    /// A module's namespace struct (`struct time { }`, which `time.sleep`
    /// hangs off) is not one: nothing has that type, and a binding a program
    /// calls `time` would otherwise turn into a type test against it.
    fn pattern_type_name(&self, name: &str) -> Option<Type> {
        let ty = resolve_type_name(&TypeExpr::named(name), &self.types);
        match &ty {
            Type::UnresolvedNamed(_) => None,
            Type::Named(_) if rask_stdlib::modules::is_module(name) => None,
            _ => Some(ty),
        }
    }

    /// A bare type test in a guard, `let p = x is Point else { … }`, as the
    /// `as` form it means: `x is Point as <guard>`. `None` when the pattern
    /// isn't a type test against a two-branch value, or already binds.
    ///
    /// The guard's value is the narrowed one either way. With nothing bound
    /// it used to be the success payload whatever the pattern named, which on
    /// a flat `T? or E` is the `T?` around the `Point`, and on an err-side
    /// test is the wrong branch entirely (#1455).
    pub(super) fn guard_type_test_as_binding(&mut self, pattern: &Pattern, scrutinee_ty: &Type) -> Option<Pattern> {
        if matches!(self.ctx.apply(scrutinee_ty), Type::Var(_)) {
            self.solve_constraints();
        }
        if !matches!(self.ctx.apply(scrutinee_ty), Type::Result { .. }) {
            return None;
        }
        let ty = match pattern {
            Pattern::TypePat { ty: TypeExpr::NoneType, .. } => return None,
            Pattern::TypePat { ty, binding: None } => ty.clone(),
            Pattern::Ident(name) if !name.contains('.') => {
                if self.variant_of_scrutinee(name, scrutinee_ty).is_some() {
                    return None;
                }
                self.pattern_type_name(name)?;
                TypeExpr::named(name.as_str())
            }
            _ => return None,
        };
        Some(Pattern::TypePat { ty, binding: Some(GUARD_BINDING.to_string()) })
    }

    pub(super) fn check_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Type, span: Span) -> Vec<(String, Type)> {
        // A pattern that may name a variant is read against the scrutinee's
        // enum, and the scrutinee may only be open because a call's result
        // hasn't been settled yet: `m.get(k)? as v` gives `v` the payload of a
        // `get` that resolves with the statement's other constraints. Read
        // against the open type, a bare `Arr(entries)` names no enum, so
        // `entries` got a fresh variable nothing ever tied back, and native
        // couldn't lower a `for` over it (#1427). Settle what's pending first.
        if matches!(pattern, Pattern::Ident(_) | Pattern::Constructor { .. } | Pattern::Struct { .. })
            && matches!(self.ctx.apply(scrutinee_ty), Type::Var(_))
        {
            self.solve_constraints();
        }
        match pattern {
            Pattern::Wildcard => vec![],

            Pattern::Ident(name) => {
                // A variant of the scrutinee's own enum, written bare or
                // qualified, is a tag test whatever the variant carries:
                // `c is Del`, `c is Cmd.Del`. It binds nothing; a payload is
                // named by writing it, `c is Del(at)`. The qualified form was
                // checked as a constructor with no arguments, so a variant
                // with a payload failed "expected 1 argument, found 0", and
                // the bare one bound a variable named after the variant
                // (#1401).
                if let Some((qualified, arity)) = self.variant_of_scrutinee(name, scrutinee_ty) {
                    let fields = vec![Pattern::Wildcard; arity];
                    return self.check_constructor_pattern(&qualified, &fields, scrutinee_ty, span);
                }
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
                    if let Some(candidate) = self.pattern_type_name(name) {
                        self.type_test_patterns.insert((span, name.clone()));
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
                // The same name against a plain value. A type test picks one
                // of a value's branches (ER23), and a plain value has one, so
                // `v is JsonValue` on a `JsonValue` is decided by the source —
                // the `as` form already says so (E0398). As a binding it was
                // true on the interpreter and false natively (#1352).
                //
                // A variant of the scrutinee's own enum is a variant test, even
                // when a type shares its name.
                if !matches!(resolved, Type::Error)
                    && !self.qualify_variant_name(name, scrutinee_ty).contains('.')
                {
                    if let Some(candidate) = self.pattern_type_name(name) {
                        self.type_test_patterns.insert((span, name.clone()));
                        if matches!(resolved, Type::Var(_)) {
                            self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                                scrutinee: scrutinee_ty.clone(),
                                narrow_ty: normalize_type(&candidate, &self.types),
                                ty_name: name.clone(),
                                span,
                            });
                        } else {
                            self.errors.push(TypeError::TypePatternNotResult {
                                ty_name: name.clone(),
                                found: resolved,
                                span,
                            });
                        }
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
                // A struct-shaped *enum variant* — `Outer.Named { code, kind }`.
                // The name isn't a type, so the struct lookup below missed it and
                // every field got a fresh variable: `kind` had no type at all,
                // and `let x: i64 = kind` type-checked (#809).
                //
                // The arm can leave the enum off — `match s { Completed { at } =>
                // … }` — and then there was no name to look up either, so the
                // fields went back to fresh variables and `at` had no type
                // (#1026). The scrutinee says which enum it is; ask it.
                let name = &self.qualify_variant_name(name, scrutinee_ty);
                if let Some(variant_fields) = self.types.struct_variant_fields(name) {
                    // A generic enum's fields are written in its parameters;
                    // `Slot.Pair { left, right }` on a `Slot<i64>` binds `i64`s,
                    // not `T`s (#1473).
                    let subst_args = self.enum_id_from_pattern_name(name).map(|(id, params)| {
                        let args = self.pattern_enum_args(id, params.len(), scrutinee_ty);
                        (params, args)
                    });
                    let mut bindings = vec![];
                    for (field_name, field_pattern) in fields {
                        let field_ty = match variant_fields.iter().find(|(n, _)| n == field_name) {
                            Some((_, ty)) => match &subst_args {
                                Some((params, args)) if !params.is_empty() => {
                                    let subst: HashMap<&str, Type> = params
                                        .iter()
                                        .map(|p| p.as_str())
                                        .zip(args.iter().cloned())
                                        .collect();
                                    Self::substitute_type_params(ty, &subst)
                                }
                                _ => ty.clone(),
                            },
                            None => {
                                self.errors.push(TypeError::NoSuchField {
                                    ty: scrutinee_ty.clone(),
                                    field: field_name.clone(),
                                    span,
                                });
                                Type::Error
                            }
                        };
                        bindings.extend(self.check_pattern(field_pattern, &field_ty, span));
                    }
                    return bindings;
                }
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
                // Use the scrutinee's own element types when it already has
                // them. Fresh variables plus an `Equal` constraint says the same
                // thing, but the constraint isn't solved until later, so the
                // sub-patterns were checked against variables that were still
                // empty — `(Value.Int(x), Value.Int(y))` matched on a known
                // `(Value, Value)` couldn't see either element was a `Value`, and
                // every binding in the arm came out untyped.
                let resolved = self.ctx.apply(scrutinee_ty);
                let elem_types: Vec<_> = match &resolved {
                    Type::Tuple(elems) if elems.len() == patterns.len() => elems.clone(),
                    _ => patterns.iter().map(|_| self.ctx.fresh_var()).collect(),
                };
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
            Pattern::TypePat { ty, binding } => {
                if self.resolve_written(ty, span).is_none() {
                    return binding.iter().map(|name| (name.clone(), Type::Error)).collect();
                }
                let narrow_ty = normalize_type(&resolve_type_name(ty, &self.types), &self.types);
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
                if let Some(payload) = self.err_variant_payload(&resolved, ty) {
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
                        // A generic branch named without its arguments —
                        // `CasFailed` against a `CasFailed<i64>` branch, which
                        // is how mem.atomics writes it. Bind the pattern to
                        // the whole branch so a field read on it resolves.
                        if !branches.contains(&narrow_ty) {
                            let heads: Vec<Type> = branches
                                .iter()
                                .filter(|b| self.same_type_head(b, &narrow_ty))
                                .cloned()
                                .collect();
                            if heads.len() == 1 {
                                return match binding {
                                    Some(name) => vec![(name.clone(), heads[0].clone())],
                                    None => vec![],
                                };
                            }
                        }
                        // A branch that hasn't resolved yet can't be compared
                        // against. Defer — by then it's either a match or a
                        // real error, and the message can name the type.
                        if !branches.contains(&narrow_ty)
                            && branches.iter().any(|b| matches!(b, Type::Var(_)))
                        {
                            self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                                scrutinee: scrutinee_ty.clone(),
                                narrow_ty: narrow_ty.clone(),
                                ty_name: ty.to_string(),
                                span,
                            });
                        } else if !branches.contains(&narrow_ty) {
                            let err_applied = normalize_type(&self.ctx.apply(err), &self.types);
                            // A union error side gets the "not in union"
                            // wording, which names the alternatives.
                            if matches!(&err_applied, Type::Union(_)) {
                                self.errors.push(TypeError::TypePatternNotInUnion {
                                    ty_name: ty.to_string(),
                                    union: err_applied,
                                    span,
                                });
                            } else {
                                self.errors.push(TypeError::TypePatternNotResult {
                                    ty_name: ty.to_string(),
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
                            ty_name: ty.to_string(),
                            span,
                        });
                    }
                    _ => {
                        self.errors.push(TypeError::TypePatternNotResult {
                            ty_name: ty.to_string(),
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

    /// The enum a qualified pattern names, with its declared type parameters.
    ///
    /// `List.Cons(head, rest)` says `List` outright; nothing else has to be
    /// known for that. Returns `None` for a bare variant name or a prefix that
    /// isn't an enum.
    fn enum_id_from_pattern_name(
        &self,
        name: &str,
    ) -> Option<(crate::types::TypeId, Vec<String>)> {
        let (enum_name, variant) = name.rsplit_once('.')?;
        let id = self.types.get_type_id(enum_name)?;
        let TypeDef::Enum { variants, type_params, .. } = self.types.get(id)? else {
            return None;
        };
        if !variants.iter().any(|(v, _)| v == variant) {
            return None;
        }
        Some((id, type_params.clone()))
    }

    /// What enum `id`'s parameters are bound to in `scrutinee_ty`: its own
    /// arguments, or those of the branch that is this enum when the scrutinee
    /// is a `T or E`. A fresh variable per parameter when it says nothing.
    fn pattern_enum_args(&mut self, id: crate::types::TypeId, arity: usize, scrutinee_ty: &Type) -> Vec<Type> {
        let type_args = |args: &[GenericArg]| -> Vec<Type> {
            args.iter()
                .filter_map(|a| match a {
                    GenericArg::Type(t) => Some((**t).clone()),
                    _ => None,
                })
                .collect()
        };
        let resolved = normalize_type(&self.ctx.apply(scrutinee_ty), &self.types);
        let found = match &resolved {
            Type::Generic { base, args } if *base == id => Some(type_args(args)),
            Type::Result { .. } => two_branch_leaves(&mut self.ctx, &self.types, &resolved)
                .iter()
                .find_map(|leaf| match leaf {
                    Type::Generic { base, args } if *base == id => Some(type_args(args)),
                    _ => None,
                }),
            _ => None,
        };
        match found {
            Some(args) if args.len() == arity => args,
            _ => (0..arity).map(|_| self.ctx.fresh_var()).collect(),
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

        // `Holder<i64>` reaches here as `Generic`, not `Named`, so read the base
        // id out of either and remember what its parameters were bound to. Only
        // `Named` used to be handled: matching a generic enum fell through to the
        // fresh-variable path below, and `Holder.Full(v)` bound `v` to a variable
        // nothing ever solved. The value came out right anyway because MIR's
        // fallback guesses a machine word, which is what an `i64` payload is —
        // a `string` or `f64` payload in the same position would not have been.
        // `Heap<List>` arrives spelled, not registered — the declared field type
        // of a recursive enum never went through the type table. Normalizing
        // first turns it into `Generic { base: Heap, args: [Named(List)] }`, so
        // both the lookup below and the box case further down can read it.
        let resolved_scrutinee = normalize_type(&resolved_scrutinee, &self.types);
        let (base_id, type_args) = match &resolved_scrutinee {
            Type::Named(id) => (Some(*id), Vec::new()),
            Type::Generic { base, args } => (
                Some(*base),
                args.iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            // The scrutinee hasn't been worked out yet — a value returned by a
            // channel receive, or bound by an enclosing pattern that was in the
            // same state. A qualified pattern names its own enum, though, so the
            // payload types are readable from that alone, instead of binding
            // every field to a fresh variable nothing ever solves (#1026).
            //
            // The scrutinee is *not* pinned to that enum. It may well be a
            // `void or GrowError<i32>` and the arm matching only its error side,
            // and saying the whole thing is a `GrowError` rejects the match.
            Type::Var(_) => match self.enum_id_from_pattern_name(name) {
                Some((id, params)) => (
                    Some(id),
                    params.iter().map(|_| self.ctx.fresh_var()).collect(),
                ),
                None => (None, Vec::new()),
            },
            // `MyErr.Bad(m)` against an `i64 or MyErr`: the pattern is about
            // one branch, so its payload types come from that branch. It fell
            // to the fresh-variable path, and native couldn't resolve a method
            // call on `m`. A bare `r is MyErr.Worse` binds nothing and is a
            // tag test, whatever the variant carries.
            Type::Result { .. } if !fields.is_empty() => match self.enum_id_from_pattern_name(name) {
                Some((id, params)) => {
                    let leaves = two_branch_leaves(&mut self.ctx, &self.types, &resolved_scrutinee);
                    let args = leaves.iter().find_map(|leaf| match leaf {
                        Type::Named(b) if *b == id => Some(Vec::new()),
                        Type::Generic { base, args } if *base == id => Some(
                            args.iter()
                                .filter_map(|a| match a {
                                    GenericArg::Type(t) => Some((**t).clone()),
                                    _ => None,
                                })
                                .collect(),
                        ),
                        _ => None,
                    });
                    let args = args.unwrap_or_else(|| params.iter().map(|_| self.ctx.fresh_var()).collect());
                    (Some(id), args)
                }
                None => (None, Vec::new()),
            },
            _ => (None, Vec::new()),
        };

        if let Some(type_id) = base_id {
            let found = self.types.get(type_id).and_then(|def| {
                if let TypeDef::Enum { variants, type_params, .. } = def {
                    variants.iter()
                        .find(|(n, _)| n == variant_lookup_name)
                        .map(|(_, f)| (f.clone(), type_params.clone()))
                } else {
                    None
                }
            });

            if let Some((variant_field_types, type_params)) = found {
                if fields.len() != variant_field_types.len() {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: variant_field_types.len(),
                        found: fields.len(),
                        span,
                    });
                    return vec![];
                }
                let subst: HashMap<&str, Type> = type_params
                    .iter()
                    .map(|p| p.as_str())
                    .zip(type_args.iter().cloned())
                    .collect();
                let mut bindings = vec![];
                for (pat, field_ty) in fields.iter().zip(variant_field_types.iter()) {
                    let field_ty = if subst.is_empty() {
                        field_ty.clone()
                    } else {
                        Self::substitute_type_params(field_ty, &subst)
                    };
                    bindings.extend(self.check_pattern(pat, &field_ty, span));
                }
                return bindings;
            }
        }

        // The enum inside a box. `Heap<List>` is a `List` to a pattern —
        // mem.heap/HP5 lets the box stand for what it holds — but the lookup
        // above asked `Heap` for a `Cons` variant, got nothing, and bound every
        // field to a fresh variable instead (#1026). Only the enum the pattern
        // itself names, and only when the box really holds it.
        if let Some((enum_id, type_params)) = self.enum_id_from_pattern_name(name) {
            let held = type_args.iter().any(|a| {
                let a = normalize_type(&self.ctx.apply(a), &self.types);
                matches!(a, Type::Named(id) | Type::Generic { base: id, .. } if id == enum_id)
            });
            if held {
                let variant_field_types = self.types.get(enum_id).and_then(|def| {
                    let TypeDef::Enum { variants, .. } = def else { return None };
                    variants
                        .iter()
                        .find(|(n, _)| n == variant_lookup_name)
                        .map(|(_, f)| f.clone())
                });
                if let Some(variant_field_types) = variant_field_types {
                    if fields.len() != variant_field_types.len() {
                        self.errors.push(TypeError::ArityMismatch {
                            expected: variant_field_types.len(),
                            found: fields.len(),
                            span,
                        });
                        return vec![];
                    }
                    // A generic enum reached through a box has no arguments
                    // here; a fresh variable per parameter is honest about that.
                    let fresh: Vec<Type> =
                        type_params.iter().map(|_| self.ctx.fresh_var()).collect();
                    let subst: HashMap<&str, Type> = type_params
                        .iter()
                        .map(|p| p.as_str())
                        .zip(fresh.into_iter())
                        .collect();
                    let mut bindings = vec![];
                    for (pat, field_ty) in fields.iter().zip(variant_field_types.iter()) {
                        let field_ty = if subst.is_empty() {
                            field_ty.clone()
                        } else {
                            Self::substitute_type_params(field_ty, &subst)
                        };
                        bindings.extend(self.check_pattern(pat, &field_ty, span));
                    }
                    return bindings;
                }
            }
        }

        let mut bindings = vec![];
        for pat in fields {
            let fresh = self.ctx.fresh_var();
            bindings.extend(self.check_pattern(pat, &fresh, span));
        }
        bindings
    }
}

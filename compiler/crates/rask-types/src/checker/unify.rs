// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Constraint solving and type unification.

use rask_ast::Span;

use super::inference::{TypeConstraint, WrapPosition};
use super::errors::TypeError;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

impl TypeChecker {
    pub(super) fn solve_constraints(&mut self) {
        let mut changed = true;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 100;

        while changed && iterations < MAX_ITERATIONS {
            changed = false;
            iterations += 1;

            let constraints = std::mem::take(&mut self.ctx.constraints);
            for constraint in constraints {
                match self.solve_constraint(constraint) {
                    Ok(true) => changed = true,
                    Ok(false) => {}
                    Err(e) => self.errors.push(e),
                }
            }
        }

        // Report leftover constraints that the solver couldn't resolve.
        // These are real errors — silently dropping them lets bad code
        // reach MIR/codegen where it panics or produces wrong results.
        let leftovers = std::mem::take(&mut self.ctx.constraints);
        for constraint in leftovers {
            match constraint {
                TypeConstraint::HasField { ty, field, span, .. } => {
                    let resolved = self.resolve_named(&self.ctx.apply(&ty));
                    if !Self::is_placeholder_type(&resolved) {
                        self.errors.push(TypeError::NoSuchField {
                            ty: resolved,
                            field,
                            span,
                        });
                    }
                }
                TypeConstraint::HasMethod { ty, method, span, .. } => {
                    let resolved = self.resolve_named(&self.ctx.apply(&ty));
                    // Skip operator methods on primitive types — these are
                    // desugared from +, *, etc. and resolved at the MIR level.
                    if !Self::is_placeholder_type(&resolved)
                        && !Self::is_operator_on_primitive(&resolved, &method)
                    {
                        self.errors.push(TypeError::NoSuchMethod {
                            ty: resolved,
                            method,
                            span,
                        });
                    }
                }
                // Leftover Equal/ReturnValue constraints on type vars
                // that never unified — not necessarily errors (can be
                // resolved by literal defaults), so skip for now.
                _ => {}
            }
        }
    }

    /// Types that legitimately stay unresolved (generic params, placeholders).
    fn is_placeholder_type(ty: &Type) -> bool {
        match ty {
            Type::UnresolvedNamed(name) => {
                name == "Self"
                    || name.starts_with('_')
                    || name.starts_with("__module_")
            }
            Type::Var(_) | Type::Error => true,
            _ => false,
        }
    }

    /// Operator methods desugared from +, *, etc. on primitive types.
    /// These are resolved at the MIR level, not in the type checker.
    fn is_operator_on_primitive(ty: &Type, method: &str) -> bool {
        let is_primitive = matches!(
            ty,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            | Type::F32 | Type::F64 | Type::Bool | Type::Char
        );
        if !is_primitive {
            return false;
        }
        matches!(
            method,
            "add" | "sub" | "mul" | "div" | "rem"
            | "eq" | "ne" | "lt" | "gt" | "le" | "ge"
            | "neg" | "not" | "and" | "or"
            | "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr" | "bit_not"
            | "abs" | "min" | "max" | "pow" | "to_float" | "compare"
        )
    }

    pub(super) fn solve_constraint(&mut self, constraint: TypeConstraint) -> Result<bool, TypeError> {
        match constraint {
            TypeConstraint::Equal(t1, t2, span) => self.unify(&t1, &t2, span),
            TypeConstraint::HasField {
                ty,
                field,
                expected,
                span,
                self_type,
            } => {
                if matches!(self.ctx.apply(&ty), Type::Error) { return Ok(false); }
                self.resolve_field(ty, field, expected, span, self_type)
            }
            TypeConstraint::HasMethod {
                ty,
                method,
                args,
                ret,
                span,
            } => {
                if matches!(self.ctx.apply(&ty), Type::Error) { return Ok(false); }
                self.resolve_method(ty, method, args, ret, span)
            }
            TypeConstraint::ReturnValue {
                ret_ty,
                expected,
                position,
                span,
            } => self.resolve_return_value(ret_ty, expected, position, span),
            TypeConstraint::TypePatternMatches {
                scrutinee,
                narrow_ty,
                ty_name,
                span,
            } => self.resolve_type_pattern(scrutinee, narrow_ty, ty_name, span),
        }
    }

    /// ER27: verify that `narrow_ty` matches either the ok or err branch of
    /// the scrutinee's `T or E` type. Defer if scrutinee is still unresolved.
    fn resolve_type_pattern(
        &mut self,
        scrutinee: Type,
        narrow_ty: Type,
        ty_name: String,
        span: Span,
    ) -> Result<bool, TypeError> {
        let resolved = self.ctx.apply(&scrutinee);
        let narrow_applied = super::check_pattern::normalize_type(
            &self.ctx.apply(&narrow_ty),
            &self.types,
        );
        match &resolved {
            Type::Result { ok, err } => {
                let ok_applied = super::check_pattern::normalize_type(
                    &self.ctx.apply(ok),
                    &self.types,
                );
                let err_applied = super::check_pattern::normalize_type(
                    &self.ctx.apply(err),
                    &self.types,
                );
                let matches_ok = ok_applied == narrow_applied;
                let matches_err = match &err_applied {
                    Type::Union(variants) => variants.contains(&narrow_applied),
                    other => other == &narrow_applied,
                };
                if !matches_ok && !matches_err {
                    if matches!(&err_applied, Type::Union(_)) {
                        Err(TypeError::TypePatternNotInUnion {
                            ty_name,
                            union: err_applied,
                            span,
                        })
                    } else {
                        Err(TypeError::TypePatternNotResult {
                            ty_name,
                            found: resolved,
                            span,
                        })
                    }
                } else {
                    Ok(true)
                }
            }
            Type::Var(_) => {
                // Still unresolved — re-queue and try again later.
                self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                    scrutinee,
                    narrow_ty,
                    ty_name,
                    span,
                });
                Ok(false)
            }
            _ => Err(TypeError::TypePatternNotResult {
                ty_name,
                found: resolved,
                span,
            }),
        }
    }

    /// Resolve a return-value / coercion constraint with deferred auto-wrap.
    ///
    /// `T or E`: at return position, bare `T` wraps to ok and bare `E` (or a
    /// component of a union `E`) wraps to err — ER9, disambiguated by type
    /// (ER3 disjointness). At assignment / field / argument position the wrap
    /// is suppressed (ER11): the value must already have the union type, or
    /// `none` may widen because the optional shape is permissive.
    ///
    /// `T?` (= `T or none`): widens at any position.
    ///
    /// If the return expression's type is still unresolved, defer.
    fn resolve_return_value(
        &mut self,
        ret_ty: Type,
        expected: Type,
        position: WrapPosition,
        span: Span,
    ) -> Result<bool, TypeError> {
        let resolved_expected = self.ctx.apply(&expected);

        if let Type::Result { ok, err } = &resolved_expected {
            let resolved_ret = self.ctx.apply(&ret_ty);
            // Optional shape (T or none) is widened freely; non-optional sums
            // wrap only at return.
            let err_is_none = matches!(self.ctx.apply(err), Type::None);
            let allow_wrap = position == WrapPosition::Return || err_is_none;
            match &resolved_ret {
                Type::Result { .. } => self.unify(&expected, &ret_ty, span),
                Type::Var(id) if !allow_wrap && self.ctx.literal_vars.contains_key(id) => {
                    // Bind position with a non-optional sum: a bare literal can
                    // never satisfy the union type. Default the literal var
                    // immediately so unify reports a precise type mismatch
                    // instead of silently dropping a deferred constraint.
                    use super::inference::LiteralKind;
                    let default = match self.ctx.literal_vars[id] {
                        LiteralKind::Integer => Type::I32,
                        LiteralKind::Float => Type::F64,
                    };
                    let id = *id;
                    self.ctx.substitutions.insert(id, default);
                    let resolved_ret = self.ctx.apply(&ret_ty);
                    self.unify(&expected, &resolved_ret, span)
                }
                Type::Var(_) => {
                    self.ctx.add_constraint(TypeConstraint::ReturnValue {
                        ret_ty,
                        expected,
                        position,
                        span,
                    });
                    Ok(false)
                }
                _ if !allow_wrap => self.unify(&expected, &ret_ty, span),
                _ => {
                    // ER9: pick the branch by type. A value whose type equals
                    // (or is in) E goes to the error branch; otherwise it goes
                    // to T. Disjointness (ER3) makes this unambiguous.
                    let resolved_err = self.ctx.apply(err);
                    let is_err_branch = match &resolved_err {
                        Type::Union(variants) => variants.iter().any(|v| v == &resolved_ret),
                        // ER32: `any Trait` error — concrete types implementing the trait go to err
                        Type::TraitObject { trait_name } => {
                            crate::traits::implements_trait(&self.types, &resolved_ret, trait_name)
                        }
                        other => other == &resolved_ret,
                    };
                    let wrapped = if is_err_branch {
                        Type::Result {
                            ok: ok.clone(),
                            err: Box::new(resolved_err),
                        }
                    } else {
                        Type::Result {
                            ok: Box::new(ret_ty),
                            err: err.clone(),
                        }
                    };
                    self.unify(&expected, &wrapped, span)
                }
            }
        } else if let Type::Option(_) = &resolved_expected {
            let resolved_ret = self.ctx.apply(&ret_ty);
            // Named(option_type_id) is Option-shaped (e.g. bare `None` or Option<T> reference).
            let is_option_shaped = matches!(&resolved_ret, Type::Option(_))
                || matches!(&resolved_ret, Type::Named(id) if Some(*id) == self.types.get_option_type_id());
            match &resolved_ret {
                _ if is_option_shaped => self.unify(&expected, &ret_ty, span),
                Type::Var(_) => {
                    self.ctx.add_constraint(TypeConstraint::ReturnValue {
                        ret_ty,
                        expected,
                        position,
                        span,
                    });
                    Ok(false)
                }
                _ => {
                    let wrapped = Type::Option(Box::new(ret_ty));
                    self.unify(&expected, &wrapped, span)
                }
            }
        } else {
            self.unify(&expected, &ret_ty, span)
        }
    }

    pub(super) fn unify(&mut self, t1: &Type, t2: &Type, span: Span) -> Result<bool, TypeError> {
        let t1 = self.ctx.apply(t1);
        let t2 = self.ctx.apply(t2);

        // Poison propagation: if either side is already an error, unify
        // silently to Error. No new diagnostic — the root cause was
        // already reported when the Error was created.
        if matches!((&t1, &t2), (Type::Error, _) | (_, Type::Error)) {
            return Ok(false);
        }

        match (&t1, &t2) {
            (a, b) if a == b => Ok(false),

            // Empty tuple and Unit are equivalent
            (Type::Tuple(elems), Type::Unit) | (Type::Unit, Type::Tuple(elems))
                if elems.is_empty() =>
            {
                Ok(false)
            }

            (Type::Var(id), other) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                // Literal vars cannot implicitly coerce to nominal types
                if self.ctx.literal_vars.contains_key(id) {
                    if let Type::Named(type_id) = other {
                        if let Some(name) = self.types.get_nominal_name(*type_id) {
                            return Err(TypeError::NominalMismatch {
                                expected: other.clone(),
                                found: t1,
                                nominal_name: name,
                                span,
                            });
                        }
                    }
                }
                self.ctx.substitutions.insert(*id, other.clone());
                Ok(true)
            }

            (other, Type::Var(id)) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                // Literal vars cannot implicitly coerce to nominal types
                if self.ctx.literal_vars.contains_key(id) {
                    if let Type::Named(type_id) = other {
                        if let Some(name) = self.types.get_nominal_name(*type_id) {
                            return Err(TypeError::NominalMismatch {
                                expected: other.clone(),
                                found: t2,
                                nominal_name: name,
                                span,
                            });
                        }
                    }
                }
                self.ctx.substitutions.insert(*id, other.clone());
                Ok(true)
            }

            (Type::Generic { base: b1, args: a1 }, Type::Generic { base: b2, args: a2 }) => {
                if b1 != b2 || a1.len() != a2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (arg1, arg2) in a1.iter().zip(a2.iter()) {
                    if self.unify_generic_arg(arg1, arg2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            // Function types
            (
                Type::Fn {
                    params: p1,
                    ret: r1,
                },
                Type::Fn {
                    params: p2,
                    ret: r2,
                },
            ) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (param1, param2) in p1.iter().zip(p2.iter()) {
                    if self.unify(param1, param2, span)? {
                        progress = true;
                    }
                }
                if self.unify(r1, r2, span)? {
                    progress = true;
                }
                Ok(progress)
            }

            (Type::Tuple(e1), Type::Tuple(e2)) => {
                if e1.len() != e2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (elem1, elem2) in e1.iter().zip(e2.iter()) {
                    if self.unify(elem1, elem2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            (Type::Option(inner1), Type::Option(inner2)) => self.unify(inner1, inner2, span),

            (
                Type::Result { ok: o1, err: e1 },
                Type::Result { ok: o2, err: e2 },
            ) => {
                let p1 = self.unify(o1, o2, span)?;
                // Allow subset widening: Result<T, A> ⊆ Result<T, A|B>
                if e1.is_subset_of(e2) {
                    return Ok(p1);
                }
                let p2 = self.unify(e1, e2, span)?;
                Ok(p1 || p2)
            }

            (
                Type::Array {
                    elem: e1,
                    len: l1,
                },
                Type::Array {
                    elem: e2,
                    len: l2,
                },
            ) => {
                // len 0 is a placeholder for comptime-dependent sizes
                if l1 != l2 && *l1 != 0 && *l2 != 0 {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                self.unify(e1, e2, span)
            }

            (Type::Slice(e1), Type::Slice(e2)) => self.unify(e1, e2, span),

            (Type::RawPtr(inner1), Type::RawPtr(inner2)) => self.unify(inner1, inner2, span),

            // Union types: exact match element-wise, or subset widening for try propagation (ER31).
            (Type::Union(types1), Type::Union(types2)) => {
                // ER31: smaller union is compatible with a larger union that contains all its members
                if t1.is_subset_of(&t2) {
                    return Ok(false);
                }
                if types1.len() != types2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (a, b) in types1.iter().zip(types2.iter()) {
                    if self.unify(a, b, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            // Single type is a subset of a union containing it (for try propagation)
            (single, Type::Union(types)) if !matches!(single, Type::Union(_)) => {
                if types.iter().any(|t| t == single) {
                    Ok(false) // compatible
                } else {
                    Err(TypeError::Mismatch {
                        expected: t2,
                        found: t1,
                        span,
                    })
                }
            }

            (Type::Error, _) | (_, Type::Error) => Ok(false),

            (Type::Never, _) => Ok(false),
            (_, Type::Never) => Ok(false),

            (Type::Result { ok: _, err: _ }, Type::Named(id)) | (Type::Named(id), Type::Result { ok: _, err: _ }) => {
                if Some(*id) == self.types.get_result_type_id() {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            (Type::Option(_inner), Type::Named(id)) | (Type::Named(id), Type::Option(_inner)) => {
                if Some(*id) == self.types.get_option_type_id() {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            // Unresolved generics with same name: unify args element-wise
            (
                Type::UnresolvedGeneric { name: n1, args: a1 },
                Type::UnresolvedGeneric { name: n2, args: a2 },
            ) if n1 == n2 && a1.len() == a2.len() => {
                let mut progress = false;
                for (arg1, arg2) in a1.iter().zip(a2.iter()) {
                    if self.unify_generic_arg(arg1, arg2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            (Type::UnresolvedNamed(_), _) | (_, Type::UnresolvedNamed(_)) => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(t1, t2, span));
                Ok(false)
            }

            (Type::UnresolvedGeneric { .. }, _) | (_, Type::UnresolvedGeneric { .. }) => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(t1, t2, span));
                Ok(false)
            }

            // Integer widening coercion: narrower signed → wider signed,
            // narrower unsigned → wider unsigned. No cross-sign coercion.
            (a, b) if Self::is_integer_widening(a, b) || Self::is_integer_widening(b, a) => {
                Ok(false)
            }

            // Trait object coercion: concrete → any Trait (TR5)
            (concrete, Type::TraitObject { ref trait_name })
            | (Type::TraitObject { ref trait_name }, concrete)
                if !matches!(concrete, Type::TraitObject { .. }) =>
            {
                if crate::traits::implements_trait(&self.types, concrete, trait_name) {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            // Nominal type vs non-nominal: produce specific error
            (Type::Named(id), _) if self.types.get_nominal_name(*id).is_some() => {
                let name = self.types.get_nominal_name(*id).unwrap();
                Err(TypeError::NominalMismatch {
                    expected: t1,
                    found: t2,
                    nominal_name: name,
                    span,
                })
            }
            (_, Type::Named(id)) if self.types.get_nominal_name(*id).is_some() => {
                let name = self.types.get_nominal_name(*id).unwrap();
                Err(TypeError::NominalMismatch {
                    expected: t1,
                    found: t2,
                    nominal_name: name,
                    span,
                })
            }

            _ => Err(TypeError::Mismatch {
                expected: t1,
                found: t2,
                span,
            }),
        }
    }

    pub(super) fn unify_generic_arg(&mut self, arg1: &GenericArg, arg2: &GenericArg, span: Span) -> Result<bool, TypeError> {
        match (arg1, arg2) {
            (GenericArg::Type(t1), GenericArg::Type(t2)) => self.unify(t1, t2, span),
            (GenericArg::ConstUsize(n1), GenericArg::ConstUsize(n2)) => {
                if n1 == n2 {
                    Ok(false)
                } else {
                    Err(TypeError::GenericError(
                        format!("const generic mismatch: {} vs {}", n1, n2),
                        span,
                    ))
                }
            }
            (GenericArg::Type(_), GenericArg::ConstUsize(_)) => {
                Err(TypeError::GenericError(
                    "expected type argument, found const argument".to_string(),
                    span,
                ))
            }
            (GenericArg::ConstUsize(_), GenericArg::Type(_)) => {
                Err(TypeError::GenericError(
                    "expected const argument, found type argument".to_string(),
                    span,
                ))
            }
        }
    }

    /// Check if `from` can widen to `to` (same signedness, strictly narrower).
    fn is_integer_widening(from: &Type, to: &Type) -> bool {
        match (from, to) {
            (Type::I8, Type::I16 | Type::I32 | Type::I64 | Type::I128) => true,
            (Type::I16, Type::I32 | Type::I64 | Type::I128) => true,
            (Type::I32, Type::I64 | Type::I128) => true,
            (Type::I64, Type::I128) => true,
            (Type::U8, Type::U16 | Type::U32 | Type::U64 | Type::U128) => true,
            (Type::U16, Type::U32 | Type::U64 | Type::U128) => true,
            (Type::U32, Type::U64 | Type::U128) => true,
            (Type::U64, Type::U128) => true,
            _ => false,
        }
    }
}

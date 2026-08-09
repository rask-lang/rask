// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Constraint solving and type unification.

use rask_ast::Span;

use super::inference::{TypeConstraint, WrapPosition};
use super::errors::TypeError;
use super::check_expr::ContainerElem;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

/// Can a bare literal of this kind stand in for `ty`? An integer literal takes
/// any numeric width (including a float, so `const x: f64 = 1` reads fine); a
/// float literal only a float. Anything still unknown — a plain var, a name not
/// yet registered — defers rather than rejects.
///
/// Without this a literal var bound to whatever it met first, so
/// `func f() -> string { return 1 }` type-checked.
fn literal_fits(kind: super::inference::LiteralKind, ty: &Type) -> bool {
    use super::inference::LiteralKind;
    match ty {
        Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
        | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => {
            kind == LiteralKind::Integer
        }
        Type::F32 | Type::F64 => true,
        Type::Var(_)
        | Type::UnresolvedNamed(_)
        | Type::UnresolvedGeneric { .. }
        | Type::Error
        | Type::Never => true,
        _ => false,
    }
}

/// The type to name in a mismatch when the value is still an unpinned literal.
fn literal_kind_type(kind: super::inference::LiteralKind) -> Type {
    use super::inference::LiteralKind;
    match kind {
        LiteralKind::Integer => Type::I64,
        LiteralKind::Float => Type::F64,
    }
}

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

        // One last attempt at the shape constraints that only ever wait on
        // another type settling — `??`, `!`, indexing. A constraint that defers
        // reports no progress, so if nothing else moved in the same pass the
        // loop above exits and the drain below throws it away, even though the
        // substitution it was waiting for landed earlier in that very pass. That
        // ordering is what left `try t.lookup(key) ?? continue` un-inferrable:
        // the `try`'s ok type was bound after the `??` had already deferred.
        //
        // Bounded to a single retry on purpose — anything still unresolved after
        // it genuinely has nothing to resolve from, and looping until quiet
        // risks never going quiet.
        let deferred = std::mem::take(&mut self.ctx.constraints);
        for constraint in deferred {
            if matches!(
                constraint,
                TypeConstraint::Coalesce { .. }
                    | TypeConstraint::Unwrap { .. }
                    | TypeConstraint::Index { .. }
                    | TypeConstraint::ElementOf { .. }
            ) {
                match self.solve_constraint(constraint) {
                    Ok(_) => {}
                    Err(e) => self.errors.push(e),
                }
            } else {
                self.ctx.constraints.push(constraint);
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
                call_node,
            } => {
                if matches!(self.ctx.apply(&ty), Type::Error) { return Ok(false); }
                self.resolve_method(ty, method, args, ret, span, call_node)
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

            TypeConstraint::Unwrap { value, result, span } => {
                self.resolve_unwrap(value, result, span)
            }

            TypeConstraint::Index { object, index, result, is_range, span } => {
                self.resolve_index(object, index, result, is_range, span)
            }

            TypeConstraint::Coalesce { node, value, default, result, span } => {
                self.resolve_coalesce(node, value, default, result, span)
            }

            TypeConstraint::ElementOf { container, elem, span } => {
                self.resolve_element_of(container, elem, span)
            }

        }
    }

    /// The element type of an iterated container, once the container is known.
    fn resolve_element_of(
        &mut self,
        container: Type,
        elem: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let resolved = self.ctx.apply(&container);
        if matches!(resolved, Type::Var(_)) {
            self.ctx.add_constraint(TypeConstraint::ElementOf { container, elem, span });
            return Ok(false);
        }
        match self.container_elem_type(&resolved) {
            ContainerElem::Known(found) => {
                self.unify(&elem, &found, span)?;
                Ok(true)
            }
            ContainerElem::Deferred => Ok(true),
            ContainerElem::NotIterable => Err(TypeError::NotIterable {
                found: self.nameable(&resolved),
                span,
            }),
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
            Type::Result { err, .. } => {
                // Every branch the scrutinee could hold — a flat `T? or E`
                // offers `T`, `none` and `E` (OPT30).
                let branches = super::check_pattern::two_branch_leaves(
                    &mut self.ctx,
                    &self.types,
                    &resolved,
                );
                let err_applied = super::check_pattern::normalize_type(
                    &self.ctx.apply(err),
                    &self.types,
                );
                // Exactly one unresolved branch and no match: the pattern
                // names it. `"42".parse<i32>() is i32` pins the ok side that
                // way, instead of failing on a type nothing had fixed yet.
                let unresolved: Vec<Type> = branches
                    .iter()
                    .filter(|b| matches!(b, Type::Var(_)))
                    .cloned()
                    .collect();
                if !branches.contains(&narrow_applied) && unresolved.len() == 1 {
                    self.unify(&unresolved[0], &narrow_applied, span)?;
                    return Ok(true);
                }
                if !branches.contains(&narrow_applied) && !unresolved.is_empty() {
                    self.ctx.add_constraint(TypeConstraint::TypePatternMatches {
                        scrutinee,
                        narrow_ty,
                        ty_name,
                        span,
                    });
                    return Ok(false);
                }
                if !branches.contains(&narrow_applied) {
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

    /// ER14a: settle `value ?? default` once the right side's shape is known.
    ///
    /// Three cases, in order: a divergence collapses to the payload; a still-
    /// wrapped right side with the same success type keeps the shape and the
    /// chain carries on; a bare success value collapses. Only the second one
    /// needs the left side, and only to tell a chain from a layer collapse
    /// (`T??` fed a `T?`, OPT30) — unresolved, the chain reading wins.
    /// `x!` yields the success payload of `x`. Both shapes read the same way —
    /// `T?` is a `Result` whose error side is `none` — so the ok side is the
    /// answer for either. Defers while the operand is still a variable.
    fn resolve_unwrap(
        &mut self,
        value: Type,
        result: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let val = self.ctx.apply(&value);
        match &val {
            Type::Result { ok, .. } => {
                self.unify(&result, ok, span)?;
                Ok(true)
            }
            // Not settled yet — come back once the operand resolves.
            Type::Var(_) => {
                self.ctx.add_constraint(TypeConstraint::Unwrap { value, result, span });
                Ok(false)
            }
            // `!` on something that can't fail or be absent. The direct arm in
            // check_expr already reports this when the type is known up front;
            // this catches the case that only resolved later.
            Type::Error => Ok(true),
            other => Err(TypeError::Mismatch {
                expected: Type::option(self.ctx.fresh_var()),
                found: other.clone(),
                span,
            }),
        }
    }

    /// Settle `object[index]` once the container's shape is known. Defers while
    /// the container is still a variable — a Pool behind a struct field only
    /// gets its type when that field's own constraint resolves.
    fn resolve_index(
        &mut self,
        object: Type,
        index: Type,
        result: Type,
        is_range: bool,
        span: Span,
    ) -> Result<bool, TypeError> {
        let obj = self.ctx.apply(&object);
        if matches!(obj, Type::Var(_)) {
            self.ctx.add_constraint(TypeConstraint::Index {
                object,
                index,
                result,
                is_range,
                span,
            });
            return Ok(false);
        }
        let progressed = match self.index_result_type(&obj, is_range) {
            Some(elem) => {
                self.unify(&result, &elem, span)?;
                true
            }
            // A container with no element type to read — an unparameterized
            // generic, say. Nothing to say about the result type.
            None => false,
        };
        // #310 polices the *index* type, but it ran at the index site with the
        // container still a variable, so it classified nothing and a field-reached
        // index went unchecked — `self.rows["nope"]` on a `Vec<Row>` field passed.
        // The container is known now, and `validate_pending_index` runs after the
        // solver, so registering here still lands in time.
        self.check_index_types(&obj, &index, is_range, span);
        Ok(progressed)
    }

    fn resolve_coalesce(
        &mut self,
        node: rask_ast::NodeId,
        value: Type,
        default: Type,
        result: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let def = self.ctx.apply(&default);
        if matches!(def, Type::Var(_)) {
            self.ctx.add_constraint(TypeConstraint::Coalesce { node, value, default, result, span });
            return Ok(false);
        }

        // A diverging right side contributes no type of its own, so the result
        // can only come from the left — and if the left hasn't resolved yet,
        // there's nothing to take. Going ahead anyway bound the left to a shape
        // whose payload was a fresh variable that nothing would ever fill, so
        // `let v = try t.lookup(key) ?? continue` came out un-inferrable however
        // `lookup` later resolved (#620 family).
        if matches!(def, Type::Never) && matches!(self.ctx.apply(&value), Type::Var(_)) {
            self.ctx.add_constraint(TypeConstraint::Coalesce { node, value, default, result, span });
            return Ok(false);
        }

        // What the left side's success branch has to be, and what the whole
        // expression produces.
        let (want_ok, produces) = match &def {
            Type::Never => {
                let payload = self.ctx.fresh_var();
                (payload.clone(), payload)
            }
            Type::Result { ok: def_ok, .. } => {
                let val = self.ctx.apply(&value);
                let collapses = match &val {
                    Type::Result { ok, .. } => {
                        let val_ok = super::check_pattern::normalize_type(&self.ctx.apply(ok), &self.types);
                        let def_norm = super::check_pattern::normalize_type(&def, &self.types);
                        !matches!(val_ok, Type::Var(_)) && val_ok == def_norm
                    }
                    _ => false,
                };
                if collapses {
                    (def.clone(), def.clone())
                } else {
                    self.fallback_keeps_shape.insert(node);
                    ((**def_ok).clone(), def.clone())
                }
            }
            other => (other.clone(), other.clone()),
        };

        // The error side stays free so both shapes fit: `T?` binds it to
        // `none`, and an operand whose shape isn't pinned yet still unifies.
        let free_err = self.ctx.fresh_var();
        let shape = Type::Result { ok: Box::new(want_ok), err: Box::new(free_err) };
        self.unify(&value, &shape, span)?;
        self.unify(&result, &produces, span)?;
        Ok(true)
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

        // CV1a/CV2: this is the one place the *direction* of a coercion is known
        // — `expected` is the position being filled, `ret_ty` is the value going
        // into it — so it's where "does every value fit" can be enforced. The
        // general `Equal` arm can't: it's reached with the two types in either
        // order, so it tests widening both ways round and accepts narrowing as a
        // result. `let small: u8 = big_u64` type-checked, and then the backends
        // disagreed about what it meant (interp kept 300, native truncated to
        // 44). That hole is #649; this closes it for the positions that carry a
        // direction, which are the ones CV1a is about.
        {
            let resolved_ret = self.ctx.apply(&ret_ty);
            if let (Some(_), Some(_)) = (
                Self::int_shape(&resolved_ret),
                Self::int_shape(&resolved_expected),
            ) {
                if resolved_ret != resolved_expected
                    && !Self::is_integer_widening(&resolved_ret, &resolved_expected)
                {
                    return Err(TypeError::NarrowingNeedsPolicy {
                        from: resolved_ret,
                        to: resolved_expected,
                        span,
                    });
                }
            }
        }

        if let Type::Result { ok, err } = &resolved_expected {
            let resolved_ret = self.ctx.apply(&ret_ty);
            // Optional shape (T or none) is widened freely; non-optional sums
            // wrap only at return.
            let err_is_none = matches!(self.ctx.apply(err), Type::None);
            let allow_wrap = position == WrapPosition::Return || err_is_none;
            // OPT29/OPT31: widening adds an optional layer. A value already
            // typed as the target's *inner* optional fills the outer present
            // branch — `const x: T?? = y` where `y: T?` means "the inner one".
            // Anything else keeps the ordinary same-shape unify.
            if err_is_none {
                let inner = self.ctx.apply(ok);
                let ret_now = self.ctx.apply(&ret_ty);
                // A value that already has the inner optional type, or one that
                // isn't optional-shaped at all and so can only reach the target
                // by gaining layers. A bare `none` is neither — its own `ok` is
                // still a var, and it binds to the outermost layer (OPT29).
                let widens = inner.is_option()
                    && ret_now != resolved_expected
                    && !matches!(ret_now, Type::Var(_))
                    && (ret_now == inner || !ret_now.is_option());
                if widens {
                    return self.resolve_return_value(ret_ty, *ok.clone(), position, span);
                }
            }
            match &resolved_ret {
                Type::Result { err: ret_err, .. } => {
                    // ER9: in a `T? or E` function, a `T?` value is the success
                    // branch — not a sum to match against the whole return
                    // type. `return none` and `return some(v)` both arrive as
                    // an option-shaped Result, and matching them whole put
                    // `none` up against `E` (#383).
                    let ok_is_option = self.ctx.apply(ok).is_option();
                    let ret_is_option = matches!(self.ctx.apply(ret_err), Type::None);
                    if allow_wrap
                        && ok_is_option
                        && ret_is_option
                        && !resolved_expected.is_option()
                    {
                        let wrapped = Type::Result {
                            ok: Box::new(ret_ty),
                            err: err.clone(),
                        };
                        return self.unify(&expected, &wrapped, span);
                    }
                    self.unify(&expected, &ret_ty, span)
                }
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
                    let resolved_ok = self.ctx.apply(ok);
                    // ER39: inferred err. If err is unresolved and the return
                    // value doesn't match ok, treat as an err and accumulate.
                    // Don't unify err here — leave it for the function-level
                    // finalization to compute the union.
                    if self.accumulate_errors
                        && matches!(resolved_err, Type::Var(_))
                        && resolved_ret != resolved_ok
                    {
                        self.inferred_errors.push(resolved_ret);
                        return Ok(false);
                    }
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
                    } else if resolved_ok.is_option() && !resolved_ret.is_option() {
                        // `return k` in a `KV? or E` function fills the present
                        // side — two layers to add, not one (#383).
                        Type::Result {
                            ok: Box::new(Type::option(ret_ty)),
                            err: err.clone(),
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
        } else if resolved_expected.is_option() {
            let resolved_ret = self.ctx.apply(&ret_ty);
            // Named(option_type_id) is Option-shaped (e.g. bare `None` or Option<T> reference).
            let is_option_shaped = resolved_ret.is_option()
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
                    let wrapped = Type::option(ret_ty);
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

            // Never is the bottom type: it fits anywhere, so unifying it must
            // never *bind* anything. These arms sit above the Var arms for that
            // reason — below them, `(Never, Var)` matched the var arm and pinned
            // the variable to Never. That's how a closure over an inferred float
            // lost its type: the body `{ return c }` diverges, so its type is
            // Never, and unifying that with the closure's return variable
            // clobbered the literal var the `return` had just bound it to. Once
            // it read Never instead of Var, literal defaulting skipped it, MIR
            // fell back to i64, and `|| { return 2.5 }` printed 2. Integers hid
            // it — i64 was the right guess for them.
            (Type::Never, _) => Ok(false),
            (_, Type::Never) => Ok(false),

            // Two bare vars, one of them a literal's: point the plain var at the
            // literal var, not the other way round. Binding the literal var
            // takes it off the defaulting list, and if nothing else ever pins
            // the pair down both stay unresolved — `"3.5".parse<f64>() ?? -1.0`
            // ended up with an untyped result that printed as an integer (#480).
            (Type::Var(a), Type::Var(b))
                if self.ctx.literal_vars.contains_key(a)
                    && !self.ctx.literal_vars.contains_key(b) =>
            {
                self.ctx.substitutions.insert(*b, Type::Var(*a));
                Ok(true)
            }

            (Type::Var(id), other) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                if let Some(kind) = self.ctx.literal_vars.get(id).copied() {
                    // Literal vars cannot implicitly coerce to nominal types
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
                    if !literal_fits(kind, other) {
                        return Err(TypeError::Mismatch {
                            expected: other.clone(),
                            found: literal_kind_type(kind),
                            span,
                        });
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
                if let Some(kind) = self.ctx.literal_vars.get(id).copied() {
                    // Literal vars cannot implicitly coerce to nominal types
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
                    if !literal_fits(kind, other) {
                        return Err(TypeError::Mismatch {
                            expected: other.clone(),
                            found: literal_kind_type(kind),
                            span,
                        });
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
                // ER31: smaller union is compatible with a larger union that contains all its members.
                // Resolve names first — a propagated `UnresolvedNamed("X")` and a declared
                // `Named(id)` for the same type would otherwise miss on a raw `==`.
                let resolved1: Vec<Type> = types1.iter().map(|t| self.resolve_named(t)).collect();
                let resolved2: Vec<Type> = types2.iter().map(|t| self.resolve_named(t)).collect();
                if resolved1.iter().all(|t| resolved2.contains(t)) {
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

            // Single type is a subset of a union containing it (for try propagation).
            // Resolve names first so a propagated `UnresolvedNamed("X")` matches a
            // declared `Named(id)` member. If the name still can't be resolved this
            // pass, defer instead of rejecting (same as the general UnresolvedNamed arm).
            (single, Type::Union(types)) if !matches!(single, Type::Union(_)) => {
                let single_r = self.resolve_named(single);
                if types.iter().any(|t| self.resolve_named(t) == single_r) {
                    Ok(false) // member of the union — compatible
                } else if matches!(single_r, Type::UnresolvedNamed(_)) {
                    self.ctx.add_constraint(TypeConstraint::Equal(t1, t2, span));
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t2,
                        found: t1,
                        span,
                    })
                }
            }

            (Type::Error, _) | (_, Type::Error) => Ok(false),

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

            // Option-shaped Result (T or none) unified with Named option type id
            (t, Type::Named(id)) | (Type::Named(id), t) if t.is_option() => {
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
    /// Width and signedness of an integer type, or `None` if it isn't one.
    fn int_shape(ty: &Type) -> Option<(u32, bool)> {
        Some(match ty {
            Type::I8 => (8, true),
            Type::I16 => (16, true),
            Type::I32 => (32, true),
            Type::I64 => (64, true),
            Type::I128 => (128, true),
            Type::U8 => (8, false),
            Type::U16 => (16, false),
            Type::U32 => (32, false),
            Type::U64 => (64, false),
            Type::U128 => (128, false),
            _ => return None,
        })
    }

    /// Can every value of `from` be represented in `to`?
    ///
    /// This is the whole test for whether a conversion is implicit
    /// (type.primitives/CV1a). A conversion that cannot fail tells the reader
    /// nothing, and ceremony that informs nobody is a design bug
    /// (NORTH_STAR commitment 5) — so it's implicit. A conversion that *can*
    /// fail has to be written, and there are named verbs for those
    /// (`truncate to`, `saturate to`, `convert to T?`).
    ///
    /// Cross-sign is allowed in exactly one direction and only when the target
    /// is strictly wider: every `u32` fits an `i64`, so rejecting that protected
    /// nobody. `u64` → `i64` stays rejected because a `u64` above `i64::MAX`
    /// doesn't fit, and `i*` → `u*` never coerces because negatives never fit.
    ///
    /// Positions only — assignment, argument, return, field. Not arithmetic:
    /// operators are homogeneous, so `a + b` on mixed types is still an error.
    /// That's the line C's "usual arithmetic conversions" crossed, and it's why
    /// `-1 < 1u` is true there.
    pub(super) fn is_integer_widening(from: &Type, to: &Type) -> bool {
        let (Some((from_bits, from_signed)), Some((to_bits, to_signed))) =
            (Self::int_shape(from), Self::int_shape(to))
        else {
            return false;
        };
        match (from_signed, to_signed) {
            // Same signedness: fits when the target is no narrower.
            (true, true) | (false, false) => from_bits <= to_bits,
            // Unsigned into signed: the target loses a bit to the sign, so it
            // has to be strictly wider. u8 → i16 fits; u8 → i8 does not.
            (false, true) => from_bits < to_bits,
            // Signed into unsigned: negatives have nowhere to go.
            (true, false) => false,
        }
    }
}

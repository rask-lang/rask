// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Constraint solving and type unification.

use rask_ast::Span;

use rask_ast::coercion::CoercionSite;

use super::inference::TypeConstraint;
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
                    | TypeConstraint::TakePlace { .. }
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
                TypeConstraint::HasMethod { ty, method, args, ret, span, call_node } => {
                    let resolved = self.resolve_named(&self.ctx.apply(&ty));
                    // A receiver that's still an inference variable is usually an
                    // unsuffixed literal waiting on `apply_literal_defaults`,
                    // which runs after all solving. Dropping the constraint here
                    // means the call is never resolved against the type the
                    // literal ends up with: `let x = 3.75` then `x.floor()`
                    // recorded no dispatch target, so MIR had to re-derive the
                    // receiver from its own type further down the chain (#425).
                    // Keep it and retry once defaults have landed.
                    if matches!(resolved, Type::Var(_)) {
                        self.deferred_methods.push(TypeConstraint::HasMethod {
                            ty, method, args, ret, span, call_node,
                        });
                        continue;
                    }
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
                // A leftover `Equal` is usually harmless — a type var waiting on
                // literal defaults — but not always. `unify` defers instead of
                // deciding whenever either side is an unresolved name or
                // generic, since the name may still resolve, and every one of
                // those landed here and was dropped. So a genuine mismatch that
                // took the deferred path was recorded and thrown away:
                //
                //     let m = Mutex.new(0)
                //     let probe: string = m      // type-checked fine (#730)
                //
                // Reported only when both sides name something concrete.
                TypeConstraint::Equal(t1, t2, span) => {
                    let a = self.ctx.apply(&t1);
                    let b = self.ctx.apply(&t2);
                    if self.primitive_against_container(&a, &b) {
                        self.errors.push(TypeError::Mismatch {
                            expected: a,
                            found: b,
                            span,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// A primitive on one side and a stdlib container on the other.
    ///
    /// `unify` defers instead of deciding whenever either side is an unresolved
    /// generic, because the name may still resolve — and every deferred `Equal`
    /// that never resolved was dropped here in silence. So this type-checked:
    ///
    /// ```text
    /// let m = Mutex.new(0)
    /// let probe: string = m
    /// ```
    ///
    /// This is deliberately the narrowest pair that can't be anything else. A
    /// primitive and a `Mutex`/`Sender`/`Vec` have no coercion between them in
    /// either direction, so the constraint is a mismatch rather than something
    /// still settling. Two *named* types are left alone — union members, enum
    /// variants, trait objects and nominal aliases all legitimately unify across
    /// names, and judging those from here reported the stdlib's own source as
    /// broken (#730).
    fn primitive_against_container(&self, a: &Type, b: &Type) -> bool {
        let is_primitive = |t: &Type| matches!(
            t,
            Type::Bool | Type::Char | Type::String | Type::Unit
            | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            | Type::F32 | Type::F64
        );
        let is_stdlib_container = |t: &Type| match t {
            Type::UnresolvedGeneric { name, .. } => {
                rask_stdlib::mir_metadata::stdlib_type_names().contains(name)
            }
            _ => false,
        };
        (is_primitive(a) && is_stdlib_container(b))
            || (is_stdlib_container(a) && is_primitive(b))
    }

    /// Re-solve method calls that deferred on an unresolved receiver, after
    /// literal defaults have given that receiver a type.
    ///
    /// Errors are reported. These calls used to be dropped in silence, so a
    /// method that doesn't exist on the type the literal defaulted to was
    /// accepted here and failed later — in MIR lowering, in codegen, or not at
    /// all.
    pub(super) fn retry_deferred_methods(&mut self) {
        // A deferred call can be waiting on another deferred call. The receiver
        // of `"{n.compare(m)}"`'s `to_string` is compare's *result*, which only
        // binds once compare itself resolves — and compare deferred too, since
        // `n` is an unsuffixed literal. One pass took the queue in order, so the
        // `to_string` sitting ahead of its own dependency saw an open receiver,
        // re-deferred, and was dropped: `{n.compare(m)}` type-checked and
        // printed a raw `Ordering` tag (#729). Go round until a pass resolves
        // nothing new.
        //
        // Bounded, and it stops as soon as a round makes no progress — a
        // receiver that is still open then has nothing left to wait for.
        const MAX_ROUNDS: usize = 8;
        for _ in 0..MAX_ROUNDS {
            let deferred = std::mem::take(&mut self.deferred_methods);
            if deferred.is_empty() {
                break;
            }
            let before = deferred.len();
            for constraint in deferred {
                match self.solve_constraint(constraint) {
                    Ok(_) => {}
                    Err(e) => self.errors.push(e),
                }
            }
            if self.deferred_methods.len() >= before {
                break;
            }
        }
        // Anything still deferred has nothing left to wait for. Those were
        // silently dropped before this existed, and reporting them is a
        // separate question from reporting a real no-such-method — leave them.
        self.ctx.constraints.clear();
    }

    /// Is this a type whose identity is settled — something a mismatch can be
    /// judged against? A variable, an error, or a name that isn't a declared or
    /// stdlib type (a type parameter like `T`, `Self`, a `__module_` marker) is
    /// not, and is left alone.
    fn names_a_concrete_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Var(_) | Type::Error => false,
            Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
                self.types.type_names.contains_key(name)
                    || self.types.stdlib_type_names.contains_key(name)
                    || rask_stdlib::mir_metadata::stdlib_type_names().contains(name)
            }
            // A wrapper is only as settled as what it wraps.
            Type::RawPtr(inner) | Type::Slice(inner) => self.names_a_concrete_type(inner),
            Type::Result { ok, err } => {
                self.names_a_concrete_type(ok) && self.names_a_concrete_type(err)
            }
            Type::Tuple(elems) => elems.iter().all(|e| self.names_a_concrete_type(e)),
            _ => true,
        }
    }

    /// Same nominal type, whatever the arguments — `Mutex<i64>` vs `Mutex<_>`.
    /// Their arguments unify (or don't) on their own; the names agreeing is
    /// enough to say this isn't the mismatch this pass is looking for.
    fn same_named_type(a: &Type, b: &Type) -> bool {
        let name_of = |t: &Type| match t {
            Type::UnresolvedNamed(n) => Some(n.clone()),
            Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
            _ => None,
        };
        match (name_of(a), name_of(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
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
            TypeConstraint::Coerce {
                value,
                target,
                site,
                value_node,
                span,
            } => self.resolve_coercion(value, target, site, value_node, span),
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

            TypeConstraint::Coalesce { node, value, default, result, value_span, default_span, span } => {
                self.resolve_coalesce(node, value, default, result, value_span, default_span, span)
            }

            TypeConstraint::TakePlace { place, result, span } => {
                self.resolve_take_place(place, result, span)
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

    /// OPT32: settle `take <place>` once the place's type is known.
    ///
    /// A place that is always there has no absent branch to leave behind, which
    /// is the whole mechanism — so it's rejected, naming the place's real type.
    /// The walk can't do this: the place is usually a field, and its type comes
    /// from a constraint of its own.
    fn resolve_take_place(
        &mut self,
        place: Type,
        result: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let resolved = self.ctx.apply(&place);
        if resolved.is_option() {
            self.unify(&result, &resolved, span)?;
            return Ok(true);
        }
        match &resolved {
            Type::Var(_) => {
                self.ctx.add_constraint(TypeConstraint::TakePlace { place, result, span });
                Ok(false)
            }
            // Already poisoned by whatever produced the place. One diagnostic
            // for one mistake.
            Type::Error => Ok(true),
            other => {
                if let Type::Var(id) = self.ctx.apply(&result) {
                    self.ctx.bind_var(id, Type::Error);
                }
                Err(TypeError::TakeOnNonOptional { found: other.clone(), span })
            }
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

    /// Could the left side of a `??` be absent?
    ///
    /// True for the optional shape, for a `T or E` (ER12 rejects that one at
    /// the `??` itself, with advice about `catch` — this isn't the place to
    /// re-answer it), and for anything not yet known. False only when the value
    /// is a settled type that is always there.
    pub(super) fn coalesce_operand_can_be_absent(&self, ty: &Type) -> bool {
        match ty {
            Type::Result { .. } => true,
            // Nothing pinned it yet, or it's already poisoned by an earlier
            // error. Neither is a reason to add a second diagnostic. A trait
            // object is in the same class: the concrete type behind it is what
            // decides, and it isn't known here.
            Type::Var(_)
            | Type::Error
            | Type::Never
            | Type::UnresolvedNamed(_)
            | Type::UnresolvedGeneric { .. }
            | Type::TraitObject { .. } => true,
            // `Option<T>` written the long way, or a bare `none`.
            Type::None => true,
            Type::Named(id) => Some(*id) == self.types.get_option_type_id(),
            Type::Generic { base, .. } => Some(*base) == self.types.get_option_type_id(),
            _ => false,
        }
    }

    fn resolve_coalesce(
        &mut self,
        node: rask_ast::NodeId,
        value: Type,
        default: Type,
        result: Type,
        value_span: Span,
        default_span: Span,
        span: Span,
    ) -> Result<bool, TypeError> {
        let def = self.ctx.apply(&default);
        // An unsuffixed number on the right (`?? -1`) is a variable that nothing
        // else will ever pin — literal defaulting is the only thing that settles
        // it, and that runs after the last solve, by which point a deferred
        // constraint has been dropped. So `v.first() ?? -1` left its binding with
        // no type at all (#620). Fall through instead: the literal, the operand's
        // success type and the result all tie to one variable, and defaulting
        // lands on it. Any other variable can still be pinned by something later,
        // so those keep deferring.
        let def_is_bare_literal = match def {
            Type::Var(id) => {
                self.ctx.is_integer_literal_var(id) || self.ctx.is_float_literal_var(id)
            }
            _ => false,
        };
        if matches!(def, Type::Var(_)) && !def_is_bare_literal {
            self.ctx.add_constraint(TypeConstraint::Coalesce { node, value, default, result, value_span, default_span, span });
            return Ok(false);
        }

        // A diverging right side contributes no type of its own, so the result
        // can only come from the left — and if the left hasn't resolved yet,
        // there's nothing to take. Going ahead anyway bound the left to a shape
        // whose payload was a fresh variable that nothing would ever fill, so
        // `let v = try t.lookup(key) ?? continue` came out un-inferrable however
        // `lookup` later resolved (#620 family).
        if matches!(def, Type::Never) && matches!(self.ctx.apply(&value), Type::Var(_)) {
            self.ctx.add_constraint(TypeConstraint::Coalesce { node, value, default, result, value_span, default_span, span });
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

        // OPT3/OPT11: `??` supplies the branch a `T?` has and nothing else
        // does. A left side that's always present has no branch to supply, and
        // the unify below can only report that as a shape mismatch — "expected
        // `i64`, found `i32 or _`", which names the compiler's own rewrite and
        // then advises changing the type to what it already is (#645).
        //
        // Checked here rather than at the `??` because the operand's type often
        // isn't known there: `m[key]` waits on a deferred Index constraint.
        // Anything still open is left alone — it can still turn out optional.
        let resolved_value = self.ctx.apply(&value);
        if !self.coalesce_operand_can_be_absent(&resolved_value) {
            self.errors.push(TypeError::CoalesceOnNonOptional {
                found: resolved_value,
                from_index: self.coalesce_index_operands.contains(&node),
                value_span,
                default_span,
                span,
            });
            // Poison the result rather than leaving it open. Returning the
            // error and stopping left the binding with no type, so one wrong
            // `??` also reported "couldn't work out the type of `v`" — a second
            // diagnostic for a problem the first one already explained.
            if let Type::Var(id) = self.ctx.apply(&result) {
                self.ctx.bind_var(id, Type::Error);
            }
            return Ok(true);
        }

        // The error side stays free so both shapes fit: `T?` binds it to
        // `none`, and an operand whose shape isn't pinned yet still unifies.
        let free_err = self.ctx.fresh_var();
        let shape = Type::Result { ok: Box::new(want_ok), err: Box::new(free_err) };
        self.unify(&value, &shape, span)?;
        self.unify(&result, &produces, span)?;
        Ok(true)
    }

    /// ER11 rejection: a bare value at a non-return position, where the target
    /// is a `T or E` whose `E` isn't `none`.
    ///
    /// Only ever built after the ordinary unify has already failed, so this
    /// replaces the message and never the verdict. Type names are filled in
    /// later, by the one pass that does that for every error.
    fn er11_error(&self, value: &Type, target: &Type, span: Span) -> TypeError {
        TypeError::NoAutoWrapOutsideReturn {
            value: value.clone(),
            target: target.clone(),
            span,
        }
    }

    /// An argument landing in a declared parameter.
    ///
    /// Method dispatch runs inside the solver, so this resolves the coercion on
    /// the spot instead of queueing one: a constraint pushed from in here is
    /// dropped without a word if nothing else makes progress in the same round,
    /// and a dropped coercion is an accepted program.
    ///
    /// Method arguments used to plain-unify while function arguments coerced,
    /// which is why `f(2)` and `w.m(2)` disagreed about an `i64?` parameter.
    pub(super) fn coerce_arg(
        &mut self,
        param_ty: &Type,
        arg_ty: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        self.resolve_coercion(
            arg_ty.clone(),
            param_ty.clone(),
            CoercionSite::Argument,
            None,
            span,
        )
    }

    /// The one place that decides whether a value gains wrapper layers.
    ///
    /// `T or E`: at a `return` (or a `catch` arm) a bare `T` wraps to ok and a
    /// bare `E` — or one component of a union `E` — wraps to err. ER9, with the
    /// branch picked by type; ER3 disjointness makes that unambiguous. At every
    /// other position ER11 suppresses the wrap: the value has to arrive already
    /// carrying the union type. `CoercionSite::wraps_error_branch` is what makes
    /// that distinction, and MIR lowering asks the same method, so neither half
    /// can quietly grow its own opinion about a position.
    ///
    /// `T?` (= `T or none`): widens everywhere. `none` carries nothing, so
    /// there's no hidden branch choice to make visible.
    ///
    /// If the value's type is still unresolved, defer — at an argument or a
    /// field it usually is.
    pub(super) fn resolve_coercion(
        &mut self,
        ret_ty: Type,
        expected: Type,
        site: CoercionSite,
        value_node: Option<rask_ast::NodeId>,
        span: Span,
    ) -> Result<bool, TypeError> {
        let resolved_expected = self.ctx.apply(&expected);

        // CV1a/CV2: a position that carries a direction — `expected` is the slot
        // being filled, `ret_ty` is the value going into it — is where "does
        // every value fit" can be enforced. The general `Equal` arm can't: it's
        // reached with the two types in either order, so it tests widening both
        // ways round and accepts narrowing as a result (#649).
        self.check_fits(&ret_ty, &resolved_expected, span)?;

        if let Type::Result { ok, err } = &resolved_expected {
            let resolved_ret = self.ctx.apply(&ret_ty);
            // Optional shape (T or none) is widened freely; non-optional sums
            // wrap only at return.
            let err_is_none = matches!(self.ctx.apply(err), Type::None);
            let allow_wrap = site.wraps_error_branch() || err_is_none;
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
                    return self.resolve_coercion(ret_ty, *ok.clone(), site, value_node, span);
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
                        .map_err(|_| self.er11_error(&resolved_ret, &resolved_expected, span))
                }
                Type::Var(_) => {
                    self.ctx.add_constraint(TypeConstraint::Coerce {
                        value: ret_ty,
                        target: expected,
                        site,
                        value_node,
                        span,
                    });
                    Ok(false)
                }
                // ER11: the value must already have the union type here. Say
                // that rather than letting the generic mismatch answer, whose
                // "change this to type `T or E`" is advice the author has
                // already taken — and there's no Ok constructor to write
                // instead, so it sends them nowhere (#641, #550).
                _ if !allow_wrap => self
                    .unify(&expected, &ret_ty, span)
                    .map_err(|_| self.er11_error(&resolved_ret, &resolved_expected, span)),
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
                    // ER32 again, from the other side: deciding the branch is
                    // only half of it. The value is a concrete error and the
                    // branch it lands on is erased, so it needs a vtable — and
                    // MIR boxes at the value, keyed by its node (TR5).
                    //
                    // Without this the checker said "error", MIR built a Result
                    // whose tag said Ok and whose payload was the bare concrete
                    // error, and `return Boom.Bad("x")` from an
                    // `i64 or any Error` came back as a *success* holding 0 on
                    // native while the interpreter reported the error (#708).
                    if is_err_branch {
                        if let (Type::TraitObject { trait_name }, Some(node)) =
                            (&resolved_err, value_node)
                        {
                            if !matches!(resolved_ret, Type::TraitObject { .. }) {
                                self.trait_coercions.insert(node, trait_name.clone());
                            }
                        }
                    }
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
                    self.ctx.add_constraint(TypeConstraint::Coerce {
                        value: ret_ty,
                        target: expected,
                        site,
                        value_node,
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

    /// CV1a: the one of these types every other one fits into.
    ///
    /// `None` unless they're all resolved integers and one of them is that
    /// type — a join with no such element (mixed signedness, say) is left to
    /// ordinary unification so the mismatch is reported where it happens.
    pub(super) fn widest_integer(&mut self, types: &[Type]) -> Option<Type> {
        let resolved: Vec<Type> = types.iter().map(|t| self.ctx.apply(t)).collect();
        if resolved.iter().any(|t| Self::int_shape(t).is_none()) {
            return None;
        }
        resolved
            .iter()
            .find(|target| {
                resolved
                    .iter()
                    .all(|src| src == *target || Self::is_integer_widening(src, target))
            })
            .cloned()
    }

    /// CV1a/CV2: reject a value that can't fit the slot it's going into.
    ///
    /// Only for positions that know which side is the source — assignment,
    /// field, return, and a call's arguments. Plain `unify` sees the two types
    /// in whichever order it happens to get them, so it can only ask "are these
    /// related by widening", which is true of a narrowing read backwards. That
    /// is how `v.push(big_u64)` on a `Vec<u8>` type-checked and then the
    /// backends disagreed about it — the interpreter kept 300, native truncated
    /// to 44 (#649).
    pub(super) fn check_fits(
        &mut self,
        source: &Type,
        target: &Type,
        span: Span,
    ) -> Result<(), TypeError> {
        let source = self.ctx.apply(source);
        let target = self.ctx.apply(target);
        if Self::int_shape(&source).is_none() || Self::int_shape(&target).is_none() {
            return Ok(());
        }
        if source == target || Self::is_integer_widening(&source, &target) {
            return Ok(());
        }
        Err(TypeError::NarrowingNeedsPolicy { from: source, to: target, span })
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
    /// A scalar coercion that cannot lose information — the set where a tuple
    /// literal may adopt the annotated element type rather than its own.
    ///
    /// Integer widening plus `f32` → `f64`. The float case can't type-check yet
    /// (CV1a doesn't make it implicit), so it changes nothing today; it's here so
    /// that when #624 does make it implicit, the tuple-literal layout bug doesn't
    /// come back for `(f64, f32)` — element-derived offsets of 0 and 4 against a
    /// declared 0 and 8, which reads back as a plausible wrong number rather than
    /// a crash (#660). MIR's `is_sized_scalar` is the other half of the pair.
    pub(super) fn is_lossless_scalar_widening(from: &Type, to: &Type) -> bool {
        if matches!((from, to), (Type::F32, Type::F64) | (Type::F32, Type::F32)
            | (Type::F64, Type::F64))
        {
            return true;
        }
        Self::is_integer_widening(from, to)
    }

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

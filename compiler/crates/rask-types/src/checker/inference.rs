// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type inference context and constraint tracking.

use std::collections::HashMap;

use rask_ast::{coercion::CoercionSite, NodeId, Span};

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
        /// CALL6: the MethodCall expression's NodeId, so the resolved target
        /// can be recorded once dispatch settles. `None` for synthetic
        /// constraints with no originating call node (e.g. union sub-checks).
        call_node: Option<NodeId>,
    },
    /// A value is being put somewhere with a declared type, and may need to
    /// gain `T?` / `T or E` layers on the way in. Deferred, because whether it
    /// needs any depends on the target resolving first.
    ///
    /// `site` says which position this is. It comes from the list MIR lowering
    /// wraps at, so "can a bare `E` go to the error branch here" is one match
    /// on one enum instead of one rule per position (#701).
    Coerce {
        value: Type,
        target: Type,
        site: CoercionSite,
        /// The expression being coerced, when there is one node to point at.
        ///
        /// Only needed where the decision this constraint makes has to reach a
        /// backend: ER32's error branch erases a concrete error into
        /// `any Trait`, and MIR boxes at the *value*, keyed by its node. `None`
        /// for positions that never erase.
        value_node: Option<NodeId>,
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
    /// ER14a: `value ?? default`. Which of the three cases applies depends on
    /// the right side's shape, which often isn't known until a method-call
    /// return type resolves — so the whole decision waits here.
    /// `x!` — the payload of whatever `value` turns out to wrap.
    ///
    /// The shape usually isn't known at the `!`: `v.get(0)!` has to wait for the
    /// method's return type to resolve. Returning a fresh variable and moving on
    /// left the result permanently disconnected from the operand, so it stayed
    /// open forever even once the operand settled.
    Unwrap {
        value: Type,
        result: Type,
        span: Span,
    },
    /// `object[index]` — the element type of whatever `object` turns out to be.
    ///
    /// The container's shape often isn't known at the index: `state.entities[h]`
    /// has to wait for the field's type to resolve. Handing back a fresh
    /// variable left the result disconnected from the container forever, so it
    /// stayed open however the field later settled.
    Index {
        object: Type,
        /// Carried so #310's index-type check can run again once the container
        /// is known — at the index site it had nothing to classify against.
        index: Type,
        result: Type,
        is_range: bool,
        span: Span,
    },
    /// `a?.b` — the chain's result, once `b`'s own type is known.
    ///
    /// A chain unwraps: `a.inner?.v` where `v: string?` is a `string?`, not a
    /// `string??`. Both absences mean the same thing to the caller, so there is
    /// nothing to keep apart (`type.optionals/OPT10`). Deciding that at the
    /// access doesn't work — the field's type comes from a `HasField` that
    /// hasn't been solved yet, so "is it already an option?" was asked of a
    /// bare variable and always answered no. The result then said `T??` while
    /// both runtimes handed back a `T?`, and every position that names the
    /// type — `chain ?? "default"` — refused to compile (#938). Same shape as
    /// `Index` above, and the same fix: carry the question until the input
    /// settles.
    OptionalChain {
        field: Type,
        result: Type,
        span: Span,
    },
    Coalesce {
        /// The `??` expression itself, so the settled case can be recorded
        /// for the backends — a still-wrapped `??` yields the left operand
        /// as-is, a collapsing one yields the payload.
        node: NodeId,
        value: Type,
        default: Type,
        result: Type,
        /// The two operands' own spans. `??` applied to something that can't
        /// be missing has to underline which half is the problem, and the
        /// whole-expression span can't say "this side always has a value,
        /// so that side is dead" (#662).
        value_span: Span,
        default_span: Span,
        span: Span,
    },
    /// OPT32: `take <place>` — the place has to be a `T?`, and the expression
    /// yields that same `T?`.
    ///
    /// Deferred because the place is usually a field, whose type arrives from
    /// its own constraint: `take conn.pending` meets an unresolved place. The
    /// walk used to settle it by unifying the place with a fresh `T?`, which
    /// answered the question instead of asking it — a non-optional place then
    /// failed inside the field's constraint with a message about `_?`.
    TakePlace {
        place: Type,
        result: Type,
        span: Span,
    },
    /// The element type of a container being iterated.
    ///
    /// A field's type arrives as a deferred `HasField`, so `for t in self.tables`
    /// meets an unresolved container at the loop. Handing back a fresh variable
    /// with nothing tying it to the container left the element open however the
    /// field later resolved; this comes back for it once the container settles.
    ElementOf {
        container: Type,
        elem: Type,
        span: Span,
    },
}

/// Kind of unsuffixed literal (for deferred defaulting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Integer,
    Float,
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
    /// What each unsuffixed integer literal said, so the default can widen when
    /// i32 is too narrow to hold it.
    pub(super) literal_int_values: HashMap<TypeVarId, i128>,
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

    /// True if `id` is an unsuffixed integer-literal var not yet resolved to a
    /// concrete type. Used by index checking to let a literal index adapt.
    pub fn is_integer_literal_var(&self, id: TypeVarId) -> bool {
        !self.substitutions.contains_key(&id)
            && matches!(self.literal_vars.get(&id), Some(LiteralKind::Integer))
    }

    /// True if `id` is an unsuffixed float-literal var not yet resolved.
    pub fn is_float_literal_var(&self, id: TypeVarId) -> bool {
        !self.substitutions.contains_key(&id)
            && matches!(self.literal_vars.get(&id), Some(LiteralKind::Float))
    }

    /// Bind a type variable to a concrete type. Only for callers that have
    /// already ensured `id` is unresolved (e.g. a fresh literal var).
    pub fn bind_var(&mut self, id: TypeVarId, ty: Type) {
        self.substitutions.insert(id, ty);
    }

    /// Remember what an unsuffixed integer literal actually said, so defaulting
    /// can tell `3` from `3000000000`.
    pub fn record_literal_int(&mut self, id: TypeVarId, value: i128) {
        self.literal_int_values.insert(id, value);
    }

    /// Apply defaults for unresolved literal type vars.
    ///
    /// A literal var can be bound to another *variable* rather than a type —
    /// `echo(7)` unifies the literal with the fresh variable standing for the
    /// method's `E`, and which of the two ends up pointing at the other depends
    /// on unification order. Defaulting only the literal var itself then leaves
    /// the other one free forever, and a type argument that stays a variable
    /// mangles to `_`. So follow the chain: whatever a literal ultimately
    /// resolves to, if it's still an unbound variable, that's what needs the
    /// default.
    pub fn apply_literal_defaults(&mut self) {
        let pending: Vec<(TypeVarId, LiteralKind)> = self
            .literal_vars
            .iter()
            .map(|(&id, &kind)| (id, kind))
            .collect();
        let mut defaults: HashMap<TypeVarId, Type> = HashMap::new();
        for (var_id, kind) in pending {
            let default = match kind {
                // i32 is the default (type.primitives/L1), but only where the
                // value fits — `const big = 3000000000` used to keep the low 32
                // bits and print -1294967296. Past that the narrowest type that
                // holds it wins, which is how `18446744073709551615` on its own
                // comes out a `u64` rather than an `i128`.
                LiteralKind::Integer => match self.literal_int_values.get(&var_id) {
                    Some(&v) if i32::try_from(v).is_ok() => Type::I32,
                    Some(&v) if i64::try_from(v).is_ok() => Type::I64,
                    Some(&v) if u64::try_from(v).is_ok() => Type::U64,
                    Some(_) => Type::I128,
                    None => Type::I32,
                },
                LiteralKind::Float => Type::F64,
            };
            // Follow the chain. An unresolved literal var applies to itself, so
            // this covers the plain case too; a literal already bound to a
            // concrete type isn't a variable and needs nothing.
            let Type::Var(tail) = self.apply(&Type::Var(var_id)) else { continue };
            // Two literals can land on the same variable — a wide one and a
            // narrow one. Take the wider, so nothing gets truncated.
            let rank = |t: &Type| match t {
                Type::I32 => 0,
                Type::I64 => 1,
                Type::U64 => 2,
                Type::I128 => 3,
                _ => 0,
            };
            match defaults.get(&tail) {
                Some(existing) if rank(existing) >= rank(&default) => {}
                _ => { defaults.insert(tail, default); }
            }
        }
        self.substitutions.extend(defaults);
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

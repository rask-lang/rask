// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! OR1: `a OP b` resolved on the ordered pair `(typeof a, typeof b)`.
//!
//! Desugaring turns `a * b` into `a.mul(b)`, which used to be the end of it —
//! the call was an ordinary method lookup on `a` and the right operand only
//! ever got checked against whatever signature `a` happened to offer. So
//! `Meters * f64` was writable and `f64 * Meters` was not, and the stdlib's own
//! `instant - instant` had to be hand-written into the type checker because the
//! language couldn't say "this pair answers with a Duration".
//!
//! Here the pair picks the conformance: `(A, B)` looks for `A`'s `Op<B>`, and
//! the result type is the `Out` that conformance declared. Both operand types
//! are known when the program is built, so this is a lookup and nothing about
//! it survives into the binary (OR10).

use rask_ast::{NodeId, Span};

use super::type_defs::{MethodSig, TypeDef};
use super::TypeChecker;
use crate::types::{Type, TypeId};

/// OR2/OR9: `Equal` and `Comparable` are absent from the operator table on
/// purpose — comparison stays same-type on both sides, so `eq`/`lt`/… are not
/// resolved on the pair.
pub use rask_ast::operators::{is_unary_operator_trait, operator_trait};

/// True for the two unary operators, which take no `Rhs`.
pub fn is_unary_operator(method: &str) -> bool {
    operator_trait(method).is_some_and(is_unary_operator_trait)
}

/// What one operand contributes to a conformance key — the spelling a header
/// would have written. `None` for a type no conformance can name: a tuple, a
/// closure, an inference variable still settling.
pub fn conformance_spelling(ty: &Type, types: &super::TypeTable) -> Option<String> {
    match ty {
        Type::Named(id) | Type::Generic { base: id, .. } => Some(types.type_name(*id)),
        Type::UnresolvedNamed(name) => Some(name.split('<').next().unwrap_or(name).to_string()),
        Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
        _ => super::type_table::primitive_spelling(ty).map(str::to_string),
    }
}

/// What asking the pair got back.
pub(super) enum PairOutcome {
    /// A conformance covers it.
    Found(OperatorMatch),
    /// Nothing here is an operator conformance's business — no operator trait,
    /// no receiver to key on, or no conformance of this operator at all.
    NotAnOperator,
    /// The receiver carries more than one conformance of this operator and the
    /// right operand hasn't settled yet. Only it can say which.
    Defer,
    /// OR8: the receiver answers this operator, but not for this right operand.
    NoPair,
}

/// What the pair resolved to: the conformance's method, and the type the
/// conformance answers with.
pub(super) struct OperatorMatch {
    pub sig: MethodSig,
    pub out: Type,
    /// The name the conformance's method is filed under (`mul$f64`).
    pub filed: String,
    /// The applied trait as the conformance table holds it (`Mul<Meters>`),
    /// for diagnostics and for the symbol the backends dispatch to.
    pub applied: String,
    pub self_id: TypeId,
}

impl TypeChecker {
    /// OR1: what the ordered pair `(recv, rhs)` resolves to.
    pub(super) fn operator_conformance(
        &self,
        recv: &Type,
        method: &str,
        args: &[Type],
    ) -> PairOutcome {
        let Some(trait_base) = operator_trait(method) else {
            return PairOutcome::NotAnOperator;
        };
        let Some(self_id) = self.types.conformance_target(recv) else {
            return PairOutcome::NotAnOperator;
        };
        let declared = self.types.applied_conformances(self_id, trait_base);
        if declared.is_empty() {
            // No conformance at all: the primitive and stdlib paths below
            // answer `i64 + i64` and everything else that was working before.
            return PairOutcome::NotAnOperator;
        }

        let applied = if is_unary_operator(method) {
            if !args.is_empty() {
                return PairOutcome::NotAnOperator;
            }
            trait_base.to_string()
        } else {
            let [arg] = args else { return PairOutcome::NotAnOperator };
            let rhs = self.resolve_named(&self.ctx.apply(arg));
            if matches!(rhs, Type::Var(_) | Type::Error) {
                // The right operand hasn't settled — an unsuffixed literal,
                // usually. One conformance is still an answer: there is only
                // one pair the receiver takes part in, and typing the call
                // against it is what settles the literal. Two and the argument
                // is the only thing that could tell them apart, so wait for it.
                // On a primitive receiver the language's own pair is always
                // there too, so an unsettled right operand means the ordinary
                // `i64 * i64` reading — `seconds * 1000000000` is that, and
                // taking the one declared conformance would have made the
                // literal a `Duration`.
                if super::type_table::primitive_spelling(recv).is_some() {
                    return PairOutcome::NotAnOperator;
                }
                // An unsuffixed literal still says something: `2` can only be
                // an integer and `2.0` only a float. One conformance of the
                // right kind is the only answer — which is what `duration / 2`
                // needs, against `Div<i64>` and `Div<Duration>`.
                let kind_match: Vec<&String> = declared
                    .iter()
                    .filter(|applied| {
                        self.applied_rhs_type(applied)
                            .is_some_and(|rhs| self.literal_could_be(arg, &rhs))
                    })
                    .collect();
                if let [only] = kind_match.as_slice() {
                    return self.matched(self_id, only, method, args);
                }
                // Not a literal at all — a binding whose type hasn't landed
                // yet. One conformance is the only pair the receiver takes
                // part in, so type the call against it and let that settle the
                // argument.
                return match declared.as_slice() {
                    [only] if kind_match.is_empty() && !self.is_literal_var(arg) => {
                        self.matched(self_id, only, method, args)
                    }
                    _ => PairOutcome::Defer,
                };
            }
            let Some(spelling) = conformance_spelling(&rhs, &self.types) else {
                return PairOutcome::NotAnOperator;
            };
            format!("{}<{}>", trait_base, spelling)
        };

        if !self.types.declares_conformance(self_id, &applied) {
            // Two primitives are the language's own pair, answered below — a
            // conformance someone wrote on `f64` doesn't take `f64 * f64` away
            // from it.
            let rhs = self.resolve_named(&self.ctx.apply(&args[0]));
            if super::type_table::primitive_spelling(recv).is_some()
                && super::type_table::primitive_spelling(&rhs).is_some()
            {
                return PairOutcome::NotAnOperator;
            }
            // OR8: the receiver answers this operator for *some* right operand,
            // just not this one. Reported here rather than as "no method `mul`",
            // which names neither the operator nor the operand that missed.
            return PairOutcome::NoPair;
        }
        self.matched(self_id, &applied, method, args)
    }

    /// OR6/OR1: the primitive an unsuffixed literal on the *left* of an
    /// operator must be, when exactly one primitive forms a pair with the type
    /// on the right.
    ///
    /// `3 * duration` is the case. Nothing ties the literal to anything — the
    /// right operand isn't a number — so it defaulted to `i32` and the pair
    /// `(i32, Duration)` names no conformance, for a line whose only reading is
    /// the `i64` one the stdlib wrote. The candidate set is the fifteen
    /// primitives, so this is a lookup over a fixed list, not a search.
    pub(super) fn literal_receiver_pair(
        &self,
        recv: &Type,
        method: &str,
        args: &[Type],
    ) -> Option<Type> {
        let trait_base = operator_trait(method)?;
        let [arg] = args else { return None };
        let rhs = self.resolve_named(&self.ctx.apply(arg));
        if matches!(rhs, Type::Var(_) | Type::Error)
            || super::type_table::primitive_spelling(&rhs).is_some()
        {
            return None;
        }
        let applied = format!("{}<{}>", trait_base, conformance_spelling(&rhs, &self.types)?);
        let mut found = None;
        for name in super::type_table::PRIMITIVE_CONFORMANCE_TARGETS {
            let Some(id) = self.types.primitive_id(name) else { continue };
            if !self.types.declares_conformance(id, &applied) {
                continue;
            }
            let candidate = super::parse_type_string(name, &self.types).ok()?;
            if !self.literal_could_be(recv, &candidate) {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(candidate);
        }
        found
    }

    /// The type an applied conformance's `Rhs` names: `Mul<f64>` → `f64`.
    fn applied_rhs_type(&self, applied: &str) -> Option<Type> {
        let rhs = rask_ast::operators::method_rhs(&format!(
            "{}${}",
            rask_ast::operators::operator_trait_method(
                applied.split('<').next().unwrap_or(applied)
            )?,
            applied.split_once('<')?.1.trim_end_matches('>').trim(),
        ))?
        .to_string();
        super::parse_type_string(&rhs, &self.types).ok()
    }

    /// Is this operand an unsuffixed literal still waiting for a type?
    fn is_literal_var(&self, ty: &Type) -> bool {
        matches!(self.ctx.apply(ty), Type::Var(id) if self.ctx.literal_vars.contains_key(&id))
    }

    /// Could an unsuffixed literal be this type? An integer literal is any
    /// integer primitive, a float literal any float one, and neither is ever a
    /// struct.
    fn literal_could_be(&self, literal: &Type, candidate: &Type) -> bool {
        let Type::Var(id) = self.ctx.apply(literal) else { return false };
        match self.ctx.literal_vars.get(&id) {
            Some(super::inference::LiteralKind::Integer) => matches!(
                candidate,
                Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            ),
            Some(super::inference::LiteralKind::Float) => {
                matches!(candidate, Type::F32 | Type::F64)
            }
            _ => false,
        }
    }

    /// The conformance's method, once the applied trait is known.
    fn matched(
        &self,
        self_id: TypeId,
        applied: &str,
        method: &str,
        args: &[Type],
    ) -> PairOutcome {
        // OR4: the conformance's method is filed under the applied argument.
        let filed = rask_ast::operators::conformance_method_name(
            &self.types.type_name(self_id),
            std::slice::from_ref(&applied.to_string()),
            method,
        )
        .unwrap_or_else(|| method.to_string());
        let Some(sig) = self.conformance_method(self_id, &filed, args) else {
            // The conformance is declared and its method isn't there: the block
            // is already being reported for the missing method.
            return PairOutcome::NotAnOperator;
        };
        // AT4 fills in a declared default, so a registered conformance always
        // has an `Out`.
        let out = self
            .types
            .assoc_binding(self_id, applied, "Out")
            .cloned()
            .unwrap_or_else(|| sig.ret.clone());
        PairOutcome::Found(OperatorMatch {
            sig,
            out,
            filed,
            applied: applied.to_string(),
            self_id,
        })
    }

    /// The method the conformance supplies.
    ///
    /// The filed name already carries the applied argument, so this is a
    /// lookup. A stdlib type's methods are registered twice — once off the stub
    /// and once off the body — and the two are the same method, so the first
    /// match is the answer.
    fn conformance_method(&self, self_id: TypeId, filed: &str, args: &[Type]) -> Option<MethodSig> {
        let methods = match self.types.get(self_id)? {
            TypeDef::Struct { methods, .. }
            | TypeDef::Enum { methods, .. }
            | TypeDef::NominalAlias { methods, .. }
            | TypeDef::Primitive { methods, .. } => methods,
            _ => return None,
        };
        methods
            .iter()
            .find(|m| m.name == filed && m.params.len() == args.len())
            .cloned()
    }

    /// OR1/OR5: type the call from the conformance the pair names.
    ///
    /// `None` leaves the receiver to the paths that answer for primitives and
    /// stdlib types — which is every pair no conformance covers.
    pub(super) fn resolve_operator_pair(
        &mut self,
        recv: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
        call_node: Option<NodeId>,
    ) -> Option<Result<bool, super::TypeError>> {
        match self.operator_conformance(recv, method, args) {
            PairOutcome::NotAnOperator => None,
            PairOutcome::Found(found) => {
                Some(self.apply_operator_match(found, recv, args, ret, span, call_node))
            }
            PairOutcome::Defer => {
                // After literal defaulting, not with the ordinary constraints:
                // what this is waiting for is an unsuffixed literal getting a
                // type, and that lands at the very end.
                self.deferred_methods.push(super::TypeConstraint::HasMethod {
                    ty: recv.clone(),
                    method: method.to_string(),
                    args: args.to_vec(),
                    ret: ret.clone(),
                    span,
                    call_node,
                });
                Some(Ok(false))
            }
            PairOutcome::NoPair => {
                let right = self.resolve_named(&self.ctx.apply(&args[0]));
                Some(Err(super::TypeError::IncomparableOperands {
                    left: self.nameable(recv),
                    right: self.nameable(&right),
                    op: Self::operator_spelling(method).to_string(),
                    span,
                }))
            }
        }
    }

    fn apply_operator_match(
        &mut self,
        found: OperatorMatch,
        recv: &Type,
        args: &[Type],
        ret: &Type,
        span: Span,
        call_node: Option<NodeId>,
    ) -> Result<bool, super::TypeError> {
        let mut progress = false;
        for ((param, _), arg) in found.sig.params.iter().zip(args.iter()) {
            if self.coerce_arg(param, arg, span)? {
                progress = true;
            }
        }
        // OR5: `Out` is read off the conformance, never solved for.
        if self.unify(&found.out, ret, span)? {
            progress = true;
        }
        if let Some(node) = call_node {
            self.operator_targets.insert(
                node,
                OperatorTarget {
                    recv: self.types.type_name(found.self_id),
                    method: found.filed.clone(),
                    applied: found.applied,
                },
            );
            // CALL6: dispatch keys on this, and what it dispatches to is the
            // conformance's method — the filed name, not the spelling the
            // operator desugared to.
            self.call_targets.insert(
                node,
                crate::Callee::Method { recv: recv.clone(), method: found.filed },
            );
        }
        Ok(progress)
    }
}

/// Where an operator call was resolved to, for the backends.
///
/// MIR reads this instead of deciding for itself whether `a * b` is a machine
/// instruction or a call: on a primitive receiver it is both, depending on the
/// right operand, and only the checker knows which pair it settled on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorTarget {
    /// The receiver type's name, as method symbols are prefixed with it.
    pub recv: String,
    /// The desugared operator method (`mul`).
    pub method: String,
    /// The applied trait the pair resolved to (`Mul<Meters>`).
    pub applied: String,
}

impl OperatorTarget {
    /// The function symbol this call dispatches to.
    pub fn symbol(&self) -> String {
        format!("{}_{}", self.recv, self.method)
    }
}

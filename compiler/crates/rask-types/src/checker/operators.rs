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
pub use rask_ast::operators::{is_unary_operator_interface, operator_interface};

/// True for the two unary operators, which take no `Rhs`.
pub fn is_unary_operator(method: &str) -> bool {
    operator_interface(method).is_some_and(is_unary_operator_interface)
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
    /// Nothing here is an operator conformance's business — no operator interface,
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
    /// The applied interface as the conformance table holds it (`Mul<Meters>`),
    /// for diagnostics and for the symbol the backends dispatch to.
    pub applied: String,
    /// OR12: the conformance has no body — the compiler answers this pair.
    pub builtin: bool,
}

impl TypeChecker {
    /// OR1: what the ordered pair `(recv, rhs)` resolves to.
    pub(super) fn operator_conformance(
        &self,
        recv: &Type,
        method: &str,
        args: &[Type],
        written_as_operator: bool,
    ) -> PairOutcome {
        let Some(interface_base) = operator_interface(method) else {
            return PairOutcome::NotAnOperator;
        };
        let Some(self_id) = self.types.conformance_target(recv) else {
            return PairOutcome::NotAnOperator;
        };
        let declared = self.types.applied_conformances(self_id, interface_base);
        if declared.is_empty() {
            // OR1: an operator answers to a conformance. A primitive receiver
            // is the exception — `i64 + i64` is the language's own pair, and
            // the paths below have it.
            //
            // `a.mul(b)` written out is not an operator and never was: it is an
            // ordinary method call, and a type with a `mul` of its own keeps it.
            if written_as_operator
                && super::type_table::primitive_spelling(recv).is_none()
                && !matches!(recv, Type::Var(_) | Type::Error)
            {
                // Wait for the right operand to have a type before saying what
                // it is. Reported now, `meters * 2.0` reads "no `*` between
                // `Meters` and `_`", which names the operand the reader can
                // already see and not the one they can't.
                if args.iter().any(|a| self.is_unsettled(a)) {
                    return PairOutcome::Defer;
                }
                return PairOutcome::NoPair;
            }
            return PairOutcome::NotAnOperator;
        }

        let applied = if is_unary_operator(method) {
            if !args.is_empty() {
                return PairOutcome::NotAnOperator;
            }
            interface_base.to_string()
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
            format!("{}<{}>", interface_base, spelling)
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

    /// OR8: the pair names no conformance, and the operator has nothing to be.
    fn no_operator_conformance(
        &self,
        recv: &Type,
        method: &str,
        args: &[Type],
        span: Span,
    ) -> super::TypeError {
        let interface_name = operator_interface(method).unwrap_or("Add").to_string();
        let left = self.render_type(recv);
        let right = args
            .first()
            .map(|a| self.render_type(&self.resolve_named(&self.ctx.apply(a))));
        // `Meters implements Mul<f64>` — the argument's own type is the `Rhs`
        // the author wants, and when it's the receiver's the default covers it.
        let header = match &right {
            Some(r) if *r != left => format!("{}<{}>", interface_name, r),
            _ => interface_name.clone(),
        };
        let has_inherent = self
            .types
            .conformance_target(recv)
            .and_then(|id| self.types.get(id))
            .is_some_and(|def| match def {
                TypeDef::Struct { methods, .. }
                | TypeDef::Enum { methods, .. }
                | TypeDef::NominalAlias { methods, .. }
                | TypeDef::Primitive { methods, .. } => methods.iter().any(|m| m.name == method),
                _ => false,
            });
        super::TypeError::NoOperatorConformance {
            left,
            right,
            op: Self::operator_spelling(method).to_string(),
            interface_name,
            header,
            has_inherent,
            span,
        }
    }

    /// OR1: the type an unsuffixed literal on the *left* of an operator must
    /// be, read off the conformance table.
    ///
    /// `3 * duration` is the case. Nothing ties the literal to anything — the
    /// right operand isn't a number — so it defaulted to `i32`, and the pair
    /// `(i32, Duration)` names no conformance for a line whose only reading is
    /// the `i64` one the stdlib wrote. Asking which types conform to
    /// `Mul<Duration>` is one lookup, and a literal narrows the answer further:
    /// `3` can only be an integer and `3.0` only a float.
    ///
    /// `None` when nothing matches or more than one does — the caller reports
    /// the ambiguity, which a suffix on the literal settles.
    pub(super) fn literal_receiver_pair(
        &self,
        recv: &Type,
        method: &str,
        args: &[Type],
    ) -> Result<Option<Type>, Vec<String>> {
        let Some(interface_base) = operator_interface(method) else { return Ok(None) };
        let [arg] = args else { return Ok(None) };
        let rhs = self.resolve_named(&self.ctx.apply(arg));
        // A number on the right settles the literal the ordinary way.
        if matches!(rhs, Type::Var(_) | Type::Error)
            || super::type_table::primitive_spelling(&rhs).is_some()
        {
            return Ok(None);
        }
        let Some(spelling) = conformance_spelling(&rhs, &self.types) else { return Ok(None) };
        let applied = format!("{}<{}>", interface_base, spelling);

        let mut found: Vec<Type> = Vec::new();
        for id in self.types.conformers_of(&applied) {
            let name = self.types.type_name(*id);
            // Only a primitive: a literal is never anything else.
            if !rask_ast::primitives::is_scalar(&name) {
                continue;
            }
            let Ok(candidate) = super::parse_type_string(&name, &self.types) else { continue };
            if self.literal_could_be(recv, &candidate) {
                found.push(candidate);
            }
        }
        match found.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.clone())),
            several => Err(several.iter().map(|t| self.render_type(t)).collect()),
        }
    }

    /// The type an applied conformance's `Rhs` names: `Mul<f64>` → `f64`.
    fn applied_rhs_type(&self, applied: &str) -> Option<Type> {
        let rhs = rask_ast::operators::method_rhs(&format!(
            "{}${}",
            rask_ast::operators::operator_interface_method(
                applied.split('<').next().unwrap_or(applied)
            )?,
            applied.split_once('<')?.1.trim_end_matches('>').trim(),
        ))?
        .to_string();
        super::parse_type_string(&rhs, &self.types).ok()
    }

    /// The receiver's declared type parameters bound to what it actually is:
    /// `Wrapping<u8>` gives `{T: u8}`. Empty for a receiver with none.
    fn receiver_param_bindings(&self, recv: &Type) -> Vec<(String, Type)> {
        let Type::Generic { base, args } = &self.resolve_named(&self.ctx.apply(recv)) else {
            return Vec::new();
        };
        self.declared_type_params(*base)
            .into_iter()
            .zip(args.iter())
            .filter_map(|(p, a)| match a {
                crate::types::GenericArg::Type(t) => Some((p, (**t).clone())),
                _ => None,
            })
            .collect()
    }

    /// Has this operand no type yet?
    fn is_unsettled(&self, ty: &Type) -> bool {
        matches!(self.resolve_named(&self.ctx.apply(ty)), Type::Var(_))
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

    /// The conformance's method, once the applied interface is known.
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
            Some(applied),
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
        let builtin = self.types.is_builtin_method(self_id, &filed);
        PairOutcome::Found(OperatorMatch {
            sig,
            out,
            filed,
            applied: applied.to_string(),
            builtin,
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
        let written_as_operator = call_node.is_some_and(|n| self.operator_calls.contains(&n));
        match self.operator_conformance(recv, method, args, written_as_operator) {
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
            PairOutcome::NoPair => Some(Err(self.no_operator_conformance(
                recv, method, args, span,
            ))),
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
        // AT10: a conditional conformance's `Out` and parameters are written in
        // the receiver's own parameters — `Wrapping<T> implements Add` answers
        // in `Wrapping<T>`. Bind them to what this receiver actually is, or
        // `(a + b).value` comes back as the literal `T` and every use of it is
        // a method call on a type parameter.
        let bindings = self.receiver_param_bindings(recv);
        let subst: std::collections::HashMap<&str, Type> =
            bindings.iter().map(|(p, t)| (p.as_str(), t.clone())).collect();
        let mut progress = false;
        for ((param, _), arg) in found.sig.params.iter().zip(args.iter()) {
            let param = Self::substitute_type_params(param, &subst);
            if self.coerce_arg(&param, arg, span)? {
                progress = true;
            }
        }
        // OR5: `Out` is read off the conformance, never solved for.
        let out = Self::substitute_type_params(&found.out, &subst);
        if self.unify(&out, ret, span)? {
            progress = true;
        }
        if let Some(node) = call_node {
            self.operator_targets.insert(
                node,
                OperatorTarget {
                    recv: recv.clone(),
                    method: found.filed.clone(),
                    applied: found.applied,
                    builtin: found.builtin,
                },
            );
            // CALL6: dispatch keys on this, and what it dispatches to is the
            // conformance's method — the filed name, not the spelling the
            // operator desugared to.
            self.call_targets.insert(
                node,
                // XC5: an operator method is a conformance method, so it takes
                // the calling package like any other — `Doc implements Equal`
                // in two packages puts two `eq`s on one type.
                crate::Callee::Method {
                    recv: recv.clone(),
                    method: found.filed,
                    package: self.conformance_package_for_call(recv, "", span),
                },
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
#[derive(Debug, Clone, PartialEq)]
pub struct OperatorTarget {
    /// The receiver the pair resolved on. A `Type` rather than a name because
    /// inside a generic body it is still the type parameter — monomorphization
    /// substitutes it on the way into each instantiation, the same as it does
    /// for an ordinary dispatch target.
    pub recv: Type,
    /// The conformance method as it is filed (`mul$f64`).
    pub method: String,
    /// The applied interface the pair resolved to (`Mul<Meters>`).
    pub applied: String,
    /// OR12: the conformance declares what the pair answers with and leaves the
    /// arithmetic to the compiler — `instant - instant` is a machine
    /// subtraction. There is no body, so the backends keep their own lowering
    /// instead of looking for one.
    pub builtin: bool,
}

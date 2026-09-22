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

/// OR2: the declared operator traits, by the method name desugaring produces.
///
/// `Equal` and `Comparable` are deliberately absent: OR9 keeps comparison
/// same-type on both sides, so `eq`/`lt`/… are not resolved on the pair.
const OPERATOR_TRAITS: &[(&str, &str)] = &[
    ("add", "Add"),
    ("sub", "Sub"),
    ("mul", "Mul"),
    ("div", "Div"),
    ("rem", "Rem"),
    ("neg", "Neg"),
    ("bit_and", "BitAnd"),
    ("bit_or", "BitOr"),
    ("bit_xor", "BitXor"),
    ("bit_not", "BitNot"),
    ("shl", "Shl"),
    ("shr", "Shr"),
];

/// The operator trait a desugared method name belongs to.
pub fn operator_trait(method: &str) -> Option<&'static str> {
    OPERATOR_TRAITS
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, t)| *t)
}

/// True for the two unary operator traits, which take no `Rhs`.
pub fn is_unary_operator(method: &str) -> bool {
    matches!(method, "neg" | "bit_not")
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

/// What the pair resolved to: the conformance's method, and the type the
/// conformance answers with.
pub(super) struct OperatorMatch {
    pub sig: MethodSig,
    pub out: Type,
    /// The applied trait as the conformance table holds it (`Mul<Meters>`),
    /// for diagnostics and for the symbol the backends dispatch to.
    pub applied: String,
    pub self_id: TypeId,
}

impl TypeChecker {
    /// OR1: the conformance registered for `(recv, rhs)`, or `None` when the
    /// pair names none.
    ///
    /// `None` is not an error here — the caller falls back to the primitive and
    /// builtin paths, which is where `i64 + i64` and the stdlib's own operators
    /// are answered. OR8's diagnostic is raised by whatever fails after that.
    pub(super) fn operator_conformance(
        &self,
        recv: &Type,
        method: &str,
        args: &[Type],
    ) -> Option<OperatorMatch> {
        let trait_base = operator_trait(method)?;
        let self_id = self.types.conformance_target(recv)?;

        let applied = if is_unary_operator(method) {
            if !args.is_empty() {
                return None;
            }
            trait_base.to_string()
        } else {
            let [arg] = args else { return None };
            let rhs = self.resolve_named(&self.ctx.apply(arg));
            // Still settling: come back once inference has an answer, rather
            // than picking a conformance from a variable.
            if matches!(rhs, Type::Var(_) | Type::Error) {
                return None;
            }
            format!("{}<{}>", trait_base, conformance_spelling(&rhs, &self.types)?)
        };

        if !self.types.declares_conformance(self_id, &applied) {
            return None;
        }

        let sig = self.conformance_method(self_id, method, args)?;
        // AT4 fills in a declared default, so a conformance always has an `Out`
        // once it has been registered. A missing one means the block is already
        // being reported for it.
        let out = self
            .types
            .assoc_binding(self_id, &applied, "Out")
            .cloned()
            .unwrap_or_else(|| sig.ret.clone());
        Some(OperatorMatch { sig, out, applied, self_id })
    }

    /// The method a conformance supplies, chosen by the argument's type when
    /// the receiver carries more than one conformance of the same operator.
    fn conformance_method(&self, self_id: TypeId, method: &str, args: &[Type]) -> Option<MethodSig> {
        let methods = match self.types.get(self_id)? {
            TypeDef::Struct { methods, .. }
            | TypeDef::Enum { methods, .. }
            | TypeDef::NominalAlias { methods, .. }
            | TypeDef::Primitive { methods, .. } => methods,
            _ => return None,
        };
        let candidates: Vec<&MethodSig> = methods
            .iter()
            .filter(|m| m.name == method && m.params.len() == args.len())
            .collect();
        if candidates.len() <= 1 {
            return candidates.first().map(|m| (*m).clone());
        }
        // Two conformances of one operator on one type (`Mul<f64>` and
        // `Mul<Meters>`): the parameter is what tells them apart.
        let rhs = self.resolve_named(&self.ctx.apply(args.first()?));
        let want = conformance_spelling(&rhs, &self.types)?;
        candidates
            .into_iter()
            .find(|m| {
                let (param, _) = &m.params[0];
                conformance_spelling(&self.resolve_named(param), &self.types).as_deref()
                    == Some(want.as_str())
            })
            .cloned()
    }

    /// OR1/OR5: type the call from the conformance the pair names.
    ///
    /// Answers `None` when no conformance covers the pair, leaving the receiver
    /// to the paths that answer for primitives and stdlib types.
    pub(super) fn resolve_operator_pair(
        &mut self,
        recv: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
        call_node: Option<NodeId>,
    ) -> Option<Result<bool, super::TypeError>> {
        let found = self.operator_conformance(recv, method, args)?;
        Some(self.apply_operator_match(found, recv, method, args, ret, span, call_node))
    }

    fn apply_operator_match(
        &mut self,
        found: OperatorMatch,
        recv: &Type,
        method: &str,
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
                    method: method.to_string(),
                    applied: found.applied,
                },
            );
            self.call_targets.insert(
                node,
                crate::Callee::Method { recv: recv.clone(), method: method.to_string() },
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

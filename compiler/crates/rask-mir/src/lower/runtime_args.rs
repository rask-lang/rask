// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The C arguments of a stdlib call whose declaration lists them with
//! `@runtime_args(…)`.
//!
//! Some runtime entry points take facts about a type parameter the program
//! never wrote: `Vec.new()` is `rask_vec_new(elem_size, elem_desc)`. Those
//! used to be added here by the method's spelling, one block per constructor,
//! with codegen counting positions to find them again. The declaration now
//! says what goes where, and this is the one place that builds it.

use super::{LoweringError, MirLowerer};
use crate::{operand::MirConst, MirOperand, MirType};
use rask_ast::NodeId;
use rask_stdlib::{FactOf, RuntimeArg, TypeFact};

/// What `runtime_call_args` built.
pub(super) struct RuntimeCall {
    pub args: Vec<MirOperand>,
    /// `hash_eq(K)` found a key with its own `hash` and `eq`, so the call goes
    /// to the constructor that takes them.
    pub keyed: bool,
}

impl MirLowerer<'_> {
    /// The extended type's parameters at `call`: `i64` for the `T` of
    /// `let v: Vec<i64> = Vec.new()`.
    ///
    /// A call on a value has them in its receiver's type (`Receiver<Job>` for
    /// `rx.receive()`). A call on a stdlib type's name records the bare name
    /// as its receiver, so there they are read off the call's own type, by
    /// matching it against what the declaration returns: `-> Vec<T>` against
    /// `Vec<i64>`, `-> (Sender<T>, Receiver<T>)` against the pair.
    pub(super) fn decl_type_args(
        &self,
        call: NodeId,
        meta: &rask_stdlib::mir_metadata::StdlibMethodMeta,
    ) -> Vec<Option<MirType>> {
        use rask_types::{GenericArg, Type};
        let n = meta.extended_type_params;
        let mut bound: Vec<Option<Type>> = vec![None; n];
        if let Some(callee) = self.ctx.call_targets.get(&call) {
            if let rask_check::Callee::Method {
                recv: Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. },
                ..
            } = &*callee
            {
                for (slot, a) in bound.iter_mut().zip(args) {
                    if let GenericArg::Type(t) = a {
                        *slot = Some((**t).clone());
                    }
                }
            }
        }
        if bound.iter().any(|b| b.as_ref().map_or(true, |t| matches!(t, Type::Var(_)))) {
            // A field initialiser the checker recorded no type for
            // (`Headers(entries: Map.new())` in a stdlib body) has the
            // field's declared type instead.
            let actual = self
                .ctx
                .lookup_raw_type(call)
                .map(|t| t.into_owned())
                .or_else(|| self.field_type_hint.clone());
            if let Some(actual) = actual {
                bind_type_params(&meta.ret_ty, &actual, &meta.type_params[..n], &mut bound);
            }
        }
        bound
            .into_iter()
            .map(|t| match t {
                Some(t) if !matches!(t, Type::Var(_)) => Some(self.ctx.type_to_mir(&t)),
                _ => None,
            })
            .collect()
    }

    /// The arguments of the C call, in the order `decl` lists them. The
    /// receiver, if any, goes first; `written` are the program's arguments in
    /// declared order, each with its type. `call` is the call node, for a
    /// key's `hash`/`eq`.
    pub(super) fn runtime_call_args(
        &mut self,
        decl: &[RuntimeArg],
        receiver: Option<MirOperand>,
        mut written: Vec<Option<(MirOperand, MirType)>>,
        tparams: &[Option<MirType>],
        call: Option<NodeId>,
    ) -> Result<RuntimeCall, LoweringError> {
        let mut args: Vec<MirOperand> = receiver.into_iter().collect();
        let mut keyed = false;
        let target = self.ctx.target();
        for arg in decl {
            match *arg {
                RuntimeArg::Param { index, holds } => {
                    let (op, src_ty) = written.get_mut(index).and_then(Option::take).ok_or_else(|| {
                        LoweringError::InvalidConstruct(format!("stdlib call: no argument {index}"))
                    })?;
                    let slot = holds.and_then(|p| tparams.get(p)).and_then(Option::as_ref).cloned();
                    let op = match slot {
                        Some(slot) => {
                            let op = self.coerce_into_wrapper(
                                rask_ast::coercion::CoercionSite::Argument,
                                op,
                                &src_ty,
                                &slot,
                            );
                            self.as_slot_value(op, &slot)
                        }
                        None => op,
                    };
                    args.push(op);
                }
                RuntimeArg::Of(fact, p) => {
                    // The checker rejects a call whose type arguments it
                    // couldn't settle (E0361), so a missing one is a lowering
                    // bug, never an 8-byte guess.
                    let arg = |p: usize| {
                        tparams.get(p).and_then(Option::as_ref).cloned().ok_or_else(|| {
                            LoweringError::InvalidConstruct(format!(
                                "stdlib call at {call:?}: no type for type argument {p}"
                            ))
                        })
                    };
                    let at = match p {
                        FactOf::Param(p) => arg(p)?,
                        FactOf::Pair(a, b) => MirType::Tuple(vec![arg(a)?, arg(b)?]),
                    };
                    let at = &at;
                    let int = |n: i64| MirOperand::Constant(MirConst::Int(n));
                    match fact {
                        TypeFact::Size => args.push(int(rask_layout::size_of(at, target) as i64)),
                        TypeFact::Align => args.push(int(rask_layout::align_of(at, target) as i64)),
                        TypeFact::Desc => args.push(crate::elem_strs::elem(at.clone())),
                        TypeFact::AtomicKind => args.push(int(Self::atomic_kind(at))),
                        TypeFact::ValueOffset => {
                            let MirType::Tuple(fields) = at else {
                                unreachable!("the stub parser only lets `value_offset` name a pair")
                            };
                            args.push(int(rask_layout::tuple_offsets(fields, target)[1] as i64));
                        }
                        TypeFact::HashEq => {
                            let mir = at.clone();
                            if let Some((hash, eq)) = call.and_then(|c| self.map_key_fn_addrs(c, &mir)) {
                                args.push(hash);
                                args.push(eq);
                                keyed = true;
                            }
                        }
                    }
                }
            }
        }
        Ok(RuntimeCall { args, keyed })
    }

    /// A stdlib constructor lowering emits itself (`collect`'s `Vec.new`):
    /// the C callee and its arguments, read off the same declaration a written
    /// call uses. `call` is the expression the map is built for, when there is
    /// one, so a key with its own `hash` and `eq` gets them.
    pub(super) fn internal_ctor(
        &mut self,
        name: &str,
        tparams: &[MirType],
        call: Option<NodeId>,
    ) -> Result<(crate::FunctionRef, Vec<MirOperand>), LoweringError> {
        let decl = rask_stdlib::mir_metadata::lookup(name)
            .and_then(|m| m.runtime_args.clone())
            .ok_or_else(|| {
                LoweringError::InvalidConstruct(format!("`{name}` declares no `@runtime_args`"))
            })?;
        let tparams: Vec<Option<MirType>> =
            tparams.iter().cloned().map(Some).collect();
        let built = self.runtime_call_args(&decl, None, Vec::new(), &tparams, call)?;
        let key = tparams.first().and_then(Option::as_ref);
        Ok((crate::elem_strs::map_ctor_ref(ctor_callee(name, built.keyed, key)), built.args))
    }
}

/// The runtime constructor a stdlib constructor call goes to. A map's depends
/// on its key: one with its own `hash`/`eq` takes them, and a string key
/// hashes by contents.
pub(super) fn ctor_callee(name: &str, keyed: bool, key: Option<&MirType>) -> String {
    let with_cap = match name {
        "Map_new" => false,
        "Map_with_capacity" => true,
        _ => return name.to_string(),
    };
    match (keyed, key) {
        (true, _) if with_cap => "rask_map_new_keyed_cap".to_string(),
        (true, _) => "rask_map_new_keyed".to_string(),
        (false, Some(k)) if with_cap => crate::elem_strs::map_ctor_with_capacity(k).to_string(),
        (false, Some(k)) => crate::elem_strs::map_ctor_for(k).to_string(),
        (false, None) => name.to_string(),
    }
}

/// Fill `out` (one slot per name in `params`) with what `actual` has where
/// `decl` names a parameter. A slot already holding a resolved type keeps it.
fn bind_type_params(
    decl: &rask_ast::ty::TypeExpr,
    actual: &rask_types::Type,
    params: &[String],
    out: &mut [Option<rask_types::Type>],
) {
    use rask_ast::ty::TypeExpr;
    use rask_types::{GenericArg, Type};
    match (decl, actual) {
        (TypeExpr::Named { path, args }, _) if path.len() == 1 && args.is_empty() => {
            if let Some(i) = params.iter().position(|p| *p == path[0]) {
                if out[i].as_ref().map_or(true, |t| matches!(t, Type::Var(_))) {
                    out[i] = Some(actual.clone());
                }
            }
        }
        (
            TypeExpr::Named { args: decl_args, .. },
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. },
        ) => {
            for (d, a) in decl_args.iter().zip(args) {
                if let GenericArg::Type(a) = a {
                    bind_type_params(d, a, params, out);
                }
            }
        }
        (TypeExpr::Tuple(ds), Type::Tuple(actuals)) => {
            for (d, a) in ds.iter().zip(actuals) {
                bind_type_params(d, a, params, out);
            }
        }
        _ => {}
    }
}

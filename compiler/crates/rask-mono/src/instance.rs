// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! One instance of a generic function, the way rustc reads one: the generic
//! body's records, with the instance's type arguments put in at the moment
//! they're read. Nothing is copied or stored per instance; monomorphization and
//! MIR both ask this.

use std::collections::HashMap;

use rask_ast::NodeId;
use rask_check::TypedProgram;
use rask_types::{GenericArg, Type, TypeBinding};

/// A generic function at one set of type arguments.
#[derive(Clone, Copy)]
pub struct Instance<'a> {
    pub type_args: &'a [TypeBinding],
    /// What each `T.Out` the body writes is at this instance, keyed
    /// `("T", "Out")`.
    pub projections: &'a HashMap<(String, String), Type>,
    pub typed: &'a TypedProgram,
}

impl<'a> Instance<'a> {
    pub fn new(
        type_args: &'a [TypeBinding],
        projections: &'a HashMap<(String, String), Type>,
        typed: &'a TypedProgram,
    ) -> Self {
        Instance { type_args, projections, typed }
    }

    fn bindings(&self) -> HashMap<&'a str, &'a Type> {
        self.type_args.iter().map(|b| (b.param.as_str(), &b.ty)).collect()
    }

    fn args(&self) -> Vec<Type> {
        self.type_args.iter().map(|b| b.ty.clone()).collect()
    }

    /// `ty` at this instance, or `None` if it names something the instance
    /// doesn't bind. Projections first, while `T.Out` still names `T`.
    pub fn ty(&self, ty: &Type) -> Option<Type> {
        let projected = if self.projections.is_empty() {
            std::borrow::Cow::Borrowed(ty)
        } else {
            let keyed: HashMap<String, Type> = self
                .projections
                .iter()
                .map(|((param, assoc), t)| (format!("{param}.{assoc}"), t.clone()))
                .collect();
            std::borrow::Cow::Owned(rask_check::substitute_type(ty, &keyed))
        };
        concretize(&projected, &self.args(), &self.bindings())
    }

    /// The type the checker recorded for a node of the body, at this instance.
    pub fn node_type(&self, id: NodeId) -> Option<Type> {
        self.ty(self.typed.node_types.get(&id)?)
    }

    /// The type arguments a call in the body passes, at this instance.
    pub fn call_type_args(&self, id: NodeId) -> Option<Vec<TypeBinding>> {
        let args = self.typed.call_type_args.get(&id)?;
        let (bindings, type_args) = (self.bindings(), self.args());
        let out: Vec<TypeBinding> = args
            .iter()
            .map(|b| TypeBinding::new(b.param.clone(), substitute_param(&b.ty, &bindings, &type_args)))
            .collect();
        (!out.is_empty()).then_some(out)
    }

    /// What a call in the body dispatches to at this instance. An operator two
    /// primitives answer is the plain operator method, the same answer the
    /// checker gives `5 * 5` outside a generic body.
    pub fn call_target(&self, id: NodeId) -> Option<rask_check::Callee> {
        let callee = match self.typed.call_targets.get(&id)? {
            rask_check::Callee::Free(sym) => rask_check::Callee::Free(*sym),
            rask_check::Callee::Method { recv, method, package } => rask_check::Callee::Method {
                recv: self.ty(recv)?,
                method: method.clone(),
                package: package.clone(),
            },
        };
        let Some((method, language_pair)) = self.operator(id) else { return Some(callee) };
        match callee {
            // The checker named the operator's method after the bound
            // (`mul$T`), and reachability enqueues what this says. Left as it
            // was, the operator's dispatch said `Meters_mul$Meters` while
            // nothing generated it, and `i32_mul$i32` for a pair that has no
            // body (#1472).
            rask_check::Callee::Method { recv, package, .. } => {
                let target = self.typed.operator_targets.get(&id)?;
                let method = if language_pair { target.operator.clone() } else { method };
                Some(rask_check::Callee::Method { recv, method, package })
            }
            free => Some(free),
        }
    }

    /// The conformance an operator in the body calls at this instance. `None`
    /// for one two primitives answer themselves.
    pub fn operator_target(&self, id: NodeId) -> Option<rask_check::OperatorTarget> {
        let target = self.typed.operator_targets.get(&id)?;
        let (method, language_pair) = self.operator(id)?;
        if language_pair {
            return None;
        }
        Some(rask_check::OperatorTarget {
            recv: self.ty(&target.recv)?,
            method,
            operator: target.operator.clone(),
            rhs: self.operator_rhs(target).or_else(|| target.rhs.clone()),
            applied: target.applied.clone(),
            builtin: target.builtin,
        })
    }

    /// The operator's method at this instance, and whether the language
    /// answers it (both sides primitive). `None` when the receiver still names
    /// a parameter.
    fn operator(&self, id: NodeId) -> Option<(String, bool)> {
        let target = self.typed.operator_targets.get(&id)?;
        let recv = self.ty(&target.recv)?;
        let bindings = self.bindings();
        // A bare `T: Mul` bound files the method under the parameter's own
        // name (`mul$T`); it means this instance's type.
        let method = match self.operator_rhs(target) {
            Some(name) => format!("{}${}", target.operator, name),
            None => target.method.clone(),
        };
        let rhs_is_primitive = match target.rhs.as_deref() {
            None => true,
            Some(r) => match bindings.get(r) {
                Some(t) => rask_check::primitive_spelling(t).is_some(),
                None => rask_ast::primitives::is_builtin_value_type(r),
            },
        };
        let language_pair = rask_check::primitive_spelling(&recv).is_some() && rhs_is_primitive;
        Some((method, language_pair))
    }

    /// The right-hand side's type at this instance, spelled for a method name.
    fn operator_rhs(&self, target: &rask_check::OperatorTarget) -> Option<String> {
        let bound = self.bindings().get(target.rhs.as_deref()?).copied()?;
        let named = crate::Monomorphizer::nameable_type(bound, &self.typed.types)?;
        type_spelling(&named)
    }

    /// Whether a closure in the body may not reach another task. One that
    /// captured a link or a `Local` box of a concrete type is in every
    /// instance; one whose capture has a parameter's type is decided here:
    /// `keep<T>`'s `spawn { … x … }` over `x: T` is task-bound in
    /// `keep<Link<Node>>` and not in `keep<i64>`.
    pub fn task_bound(&self, id: NodeId) -> bool {
        let typed = self.typed;
        typed.task_bound_closures.contains(&id)
            || typed.generic_closure_captures.get(&id).is_some_and(|captures| {
                typed.types.generic_closure_task_bound(captures, |ty| self.ty(ty))
            })
    }
}

/// A type argument with this instance's parameters filled in. By name first:
/// `pair<A, B>` calling `level1(a)` records `[A]`, and only the name says which
/// of the two that is. The positional fallback covers a single-parameter
/// instance whose name list is unavailable.
pub(crate) fn substitute_param(ty: &Type, bindings: &HashMap<&str, &Type>, type_args: &[Type]) -> Type {
    if let Type::UnresolvedNamed(name) = ty {
        if let Some(bound) = bindings.get(name.as_str()) {
            return (*bound).clone();
        }
    }
    concretize(ty, type_args, bindings).unwrap_or_else(|| ty.clone())
}

/// The name a concrete type is spelled by, for a conformance method's filed
/// name. `None` for anything a conformance header can't name.
pub(crate) fn type_spelling(ty: &Type) -> Option<String> {
    rask_check::primitive_spelling(ty).map(str::to_string).or_else(|| match ty {
        Type::UnresolvedNamed(name) => Some(name.clone()),
        Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
        _ => None,
    })
}

/// A recorded type with the instance's arguments substituted in, or `None` if
/// it still refers to something only the generic body knows.
pub(crate) fn concretize(ty: &Type, type_args: &[Type], bindings: &HashMap<&str, &Type>) -> Option<Type> {
    // A last-resort guess at whether a name is a type parameter, for the
    // instances that reach here with no name list. `bindings` is the real
    // answer and is checked first everywhere below.
    //
    // It is one uppercase letter, and it used to be the *only* test. That made
    // every multi-letter parameter invisible: `Output`, `Item`, `Elem` carried
    // through verbatim, and MIR then resolved the leftover name against real
    // types. `Vec<Output>` inside `map_all<Item, Output>` got the layout of the
    // stdlib struct `os.Output` and freed two of its string fields out of an
    // i32 (#1247).
    fn looks_like_type_param(name: &str) -> bool {
        let mut chars = name.chars();
        matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
    }
    let all = |tys: &[Type]| tys.iter().map(|t| concretize(t, type_args, bindings)).collect::<Option<Vec<_>>>();
    let args_of = |args: &[GenericArg]| {
        args.iter()
            .map(|arg| match arg {
                GenericArg::Type(inner) => concretize(inner, type_args, bindings).map(|t| GenericArg::Type(Box::new(t))),
                other => Some(other.clone()),
            })
            .collect::<Option<Vec<_>>>()
    };
    match ty {
        // The type parameter itself. Bound by name, which is the only thing
        // that says *which* parameter it is (#425).
        Type::UnresolvedNamed(name) => {
            if let Some(bound) = bindings.get(name.as_str()) {
                return Some((*bound).clone());
            }
            if !looks_like_type_param(name) {
                // An ordinary named type, which this instance has nothing to
                // say about.
                return Some(ty.clone());
            }
            // No name list available: a single argument still pins it.
            if type_args.len() == 1 { Some(type_args[0].clone()) } else { None }
        }
        // `Vec<T>` and friends, only when every argument came through concrete
        // (#814).
        Type::UnresolvedGeneric { name, args } => {
            // A parameter used as the *base* of a generic is one this instance
            // can't name a layout for, whatever it's called.
            if bindings.contains_key(name.as_str()) || looks_like_type_param(name) {
                return None;
            }
            Some(Type::UnresolvedGeneric { name: name.clone(), args: args_of(args)? })
        }
        Type::Generic { base, args } => Some(Type::Generic { base: *base, args: args_of(args)? }),
        Type::Tuple(elems) => Some(Type::Tuple(all(elems)?)),
        Type::Union(elems) => Some(Type::Union(all(elems)?)),
        Type::Array { elem, len } => Some(Type::Array { elem: Box::new(concretize(elem, type_args, bindings)?), len: *len }),
        Type::RawPtr(inner) => Some(Type::RawPtr(Box::new(concretize(inner, type_args, bindings)?))),
        Type::Result { ok, err } => Some(Type::Result {
            ok: Box::new(concretize(ok, type_args, bindings)?),
            err: Box::new(concretize(err, type_args, bindings)?),
        }),
        Type::Fn { params, ret } => Some(Type::Fn {
            params: params
                .iter()
                .map(|p| Some(rask_types::FnParam { mode: p.mode, ty: concretize(&p.ty, type_args, bindings)? }))
                .collect::<Option<_>>()?,
            ret: Box::new(concretize(ret, type_args, bindings)?),
        }),
        Type::Var(_) => None,
        other => Some(other.clone()),
    }
}

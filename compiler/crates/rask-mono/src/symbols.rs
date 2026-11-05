// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The symbol a call runs, worked out once. Monomorphization asks it to know
//! what to generate, and MIR asks it to know what to call; neither builds a
//! name of its own.

use std::collections::HashMap;

use rask_ast::ty::TypeExpr;
use rask_ast::NodeId;
use rask_types::{Type, TypeBinding};

use crate::reachability::mangle_name;

/// The generic type a method is declared on.
#[derive(Debug, Clone)]
pub(crate) struct MethodOwner {
    /// Type parameter names in declaration order, bounds stripped. A tuple
    /// argument contributes each of its members, so `Sequence<(K, V)>` says
    /// `["K", "V"]` — the two names a call binds — rather than one name that
    /// happens to be spelled `(K, V)`.
    pub(crate) params: Vec<String>,
    /// The target as written, so the receiver's type can be rebuilt by
    /// substituting into it. Reconstructing it as `base<args…>` is only right
    /// when the parameters sit directly under the angle brackets:
    /// `extend Sequence<(K, V)>` came back as `Sequence<i32>`, dropping `V`,
    /// and the receiver then had a scalar element type where a pair was due.
    pub(crate) template: TypeExpr,
}

/// Every function's symbol, by its declaration.
#[derive(Debug, Clone, Default)]
pub struct Symbols {
    /// Each declaration compiled from Rask → the symbol it's emitted under,
    /// before any instance's type arguments. A native has no entry: the
    /// compiled stdlib leaves its declaration out.
    pub(crate) by_decl: HashMap<NodeId, String>,
    /// A method of a generic type, by symbol → the type it's declared on.
    pub(crate) owners: HashMap<String, MethodOwner>,
    /// The declarations with a body to make a copy of per instance. A stdlib
    /// function generic in its signature but backed by one C entry point
    /// isn't one: mangling a call to it names a symbol nothing emits.
    pub(crate) instantiable: std::collections::HashSet<NodeId>,
}

/// A call that runs a body in Rask, a method's or a free function's.
#[derive(Debug, Clone)]
pub struct ResolvedCall {
    /// The body's symbol before type arguments: what is generated.
    pub qualified: String,
    /// The receiver's type arguments, then the method's own.
    pub type_args: Vec<TypeBinding>,
    /// What the call names: `qualified` with `type_args` mangled in.
    pub symbol: String,
}

impl Symbols {
    /// The symbol a declaration is emitted under. `None` for a native.
    pub fn of_decl(&self, decl: NodeId) -> Option<&str> {
        self.by_decl.get(&decl).map(String::as_str)
    }

    /// What a method call runs: `callee` is the checker's target at the
    /// instance being lowered, `own_args` the type arguments the call passes
    /// the method itself. `None` for a call with no body in Rask behind it: a
    /// native, a primitive's operation, a builtin.
    ///
    /// A method on a generic type gets one body per receiver instantiation, so
    /// `One<Big>.get()` and `One<i64>.get()` don't share a `self` layout (#814).
    pub fn method_call(
        &self,
        callee: &rask_check::Callee,
        own_args: Vec<TypeBinding>,
        types: &rask_check::TypeTable,
    ) -> Option<ResolvedCall> {
        let rask_check::Callee::Method { recv, body: Some(decl), .. } = callee else { return None };
        let qualified = self.by_decl.get(decl)?.clone();
        let own_args: Vec<TypeBinding> = match self.owners.get(&qualified) {
            Some(owner) => own_args.into_iter().filter(|b| !owner.params.contains(&b.param)).collect(),
            None => own_args,
        };
        let type_args: Vec<TypeBinding> =
            self.receiver_bindings(recv, &qualified, types).into_iter().chain(own_args).collect();
        let symbol = if type_args.is_empty() {
            qualified.clone()
        } else {
            mangle_name(&qualified, &type_args, Some(types))
        };
        Some(ResolvedCall { qualified, type_args, symbol })
    }

    /// What a call to a free function runs: `callee` is the checker's target,
    /// `type_args` the arguments the call passes it. `None` for a function
    /// with nothing compiled from Rask behind it.
    pub fn free_call(
        &self,
        callee: &rask_check::Callee,
        type_args: Vec<TypeBinding>,
        typed: &rask_check::TypedProgram,
    ) -> Option<ResolvedCall> {
        let rask_check::Callee::Free(sym) = callee else { return None };
        let decl = match &typed.symbols.get(*sym)?.kind {
            rask_resolve::SymbolKind::Function { decl, .. } => decl,
            rask_resolve::SymbolKind::BuiltinFunction { decl: Some(decl), .. } => decl,
            _ => return None,
        };
        let qualified = self.by_decl.get(decl)?.clone();
        let type_args = if self.instantiable.contains(decl) { type_args } else { Vec::new() };
        let symbol = if type_args.is_empty() {
            qualified.clone()
        } else {
            mangle_name(&qualified, &type_args, Some(&typed.types))
        };
        Some(ResolvedCall { qualified, type_args, symbol })
    }

    /// A function named as a value (`http.serve(addr, handle)`, `let f =
    /// make<i32>`): the symbol of the function the name resolved to, of the
    /// copy for the type arguments the checker recorded at the name. `None`
    /// when the name is a variable, or the function has no body in Rask.
    pub fn function_value(
        &self,
        name_node: NodeId,
        type_args: Vec<TypeBinding>,
        typed: &rask_check::TypedProgram,
    ) -> Option<ResolvedCall> {
        let sym = *typed.resolutions.get(&name_node)?;
        self.free_call(&rask_check::Callee::Free(sym), type_args, typed)
    }

    /// The runtime entry point a native method or module function is called
    /// by: `{type or module}_{name}`, the spelling the C dispatch table files
    /// it under. Only for a call with no body in Rask behind it.
    pub fn native(&self, callee: &rask_check::Callee, types: &rask_check::TypeTable) -> Option<String> {
        let rask_check::Callee::Method { recv, method, .. } = callee else { return None };
        Some(format!("{}_{}", rask_check::receiver_name(recv, types)?, method))
    }

    /// The receiver's own type parameters, bound to what this receiver fixes
    /// them to.
    ///
    /// A method declared in `extend One<A>` takes `A` from the extend header,
    /// not from its own signature, so the checker's per-call record, which is
    /// the *method's* own arguments, is empty for it. The header's parameters
    /// bind against the receiver as written, so a parameter nested in a tuple
    /// binds to the matching member instead of the whole argument (#1046).
    ///
    /// Empty unless every part of the receiver is settled and nameable, and
    /// every parameter the header declares comes out bound. A type parameter
    /// still standing for itself would mangle to `One_get$A`, which is an
    /// instance of nothing.
    pub(crate) fn receiver_bindings(
        &self,
        recv: &Type,
        qualified: &str,
        types: &rask_check::TypeTable,
    ) -> Vec<TypeBinding> {
        let Some(owner) = self.owners.get(qualified) else { return Vec::new() };
        let Type::Generic { args, .. } = recv else { return Vec::new() };

        // Every argument has to be nameable: a nested instantiation has to say
        // which inner type, since `get` on `Wrap<Wrap<i32>>` hands back a
        // `Wrap<i32>` and on `Wrap<i32>` an `i32` (#871).
        let mut actuals = Vec::with_capacity(args.len());
        for arg in args {
            let rask_types::GenericArg::Type(t) = arg else { return Vec::new() };
            if crate::Monomorphizer::nameable_type(t.as_ref(), types).is_none() {
                return Vec::new();
            }
            actuals.push(rask_types::GenericArg::Type(t.clone()));
        }

        let templates = owner.template.args();
        let Some(bound) = rask_check::bind_header_patterns(types, &templates, &actuals) else {
            return Vec::new();
        };
        if bound.len() != owner.params.len() {
            return Vec::new();
        }
        bound.into_iter().map(|(param, ty)| TypeBinding::new(param, ty)).collect()
    }
}

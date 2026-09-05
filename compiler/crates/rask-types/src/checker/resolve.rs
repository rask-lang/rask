// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Field and method resolution, including builtin type methods.

use std::collections::HashMap;

use rask_ast::{NodeId, Span};

use super::type_defs::{Callee, MethodSig, TypeDef};
use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::TypeChecker;

use crate::types::{GenericArg, Type, TypeId, TypeVarId};

/// Which parameters of a stdlib sequence method are positions or counts
/// (`std.collections/V8`, `V9`). Those take **any** integer type: the value is
/// range-checked at the access, so a negative or oversized index panics or
/// answers `none` rather than wrapping.
///
/// Without this the signatures pinned an index to one integer type while
/// `len()` answers `usize`, so `for i in 0..v.len() { v.get(i) }` — the most
/// ordinary loop there is — didn't compile. `v[i]` never had the problem,
/// because indexing was always checked this way; only the method spelling was.
///
/// Positions, not a blanket rule: a `u64` parameter elsewhere in the stdlib
/// (`Duration.from_millis`) still means that type, and still gets checked.
fn sequence_index_params(recv: &str, method: &str) -> &'static [usize] {
    match recv {
        "Vec" | "Slice" | "string" | "Array" => match method {
            // `set`/`insert` are (index, value) — only the index is a position.
            "get" | "get_clone" | "remove" | "remove_at" | "skip" | "take" | "limit"
            | "chunks" | "truncate" | "split_at" | "repeat" | "with_capacity"
            | "set" | "insert" => &[0],
            "swap" | "slice" => &[0, 1],
            _ => &[],
        },
        _ => &[],
    }
}

impl TypeChecker {
    /// The element type `T` a channel end carries — `Receiver<T>`, `Sender<T>`
    /// or `Channel<T>`. `None` for anything else.
    pub(super) fn channel_element_type(&self, ty: &Type) -> Option<Type> {
        let applied = self.ctx.apply(ty);
        let (name, args) = match &applied {
            Type::Generic { base, args } => (self.types.type_name(*base), args.as_slice()),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args.as_slice()),
            _ => return None,
        };
        if !matches!(name.as_str(), "Receiver" | "Sender" | "Channel") {
            return None;
        }
        match args.first() {
            Some(GenericArg::Type(t)) => Some(self.resolve_named(t)),
            _ => None,
        }
    }

    /// Desugared operators that take their argument at the receiver's own type
    /// and hand back that same type.
    ///
    /// Shifts are absent on purpose — the shift amount is its own type — and so
    /// are `pow`, `to_float` and the single-operand forms.
    fn is_homogeneous_arithmetic(method: &str) -> bool {
        matches!(
            method,
            "add" | "sub" | "mul" | "div" | "rem"
            | "bit_and" | "bit_or" | "bit_xor"
        )
    }

    /// Desugared comparisons: argument at the receiver's type, result `bool`.
    fn is_homogeneous_comparison(method: &str) -> bool {
        matches!(method, "eq" | "ne" | "lt" | "gt" | "le" | "ge")
    }

    fn is_homogeneous_operator(method: &str) -> bool {
        Self::is_homogeneous_arithmetic(method) || Self::is_homogeneous_comparison(method)
    }

    /// Zero-argument operators whose result has the receiver's type.
    fn is_result_preserving_unary(method: &str) -> bool {
        matches!(method, "neg" | "abs" | "bit_not")
    }

    /// The shifts. `x << n` keeps `x`'s type and takes `n` at its own, which is
    /// why they're not homogeneous — but the *result* half still holds, and
    /// that's what lets an annotation settle a literal receiver.
    ///
    /// Without it `let a: i64 = 2 << 40` left `2` unconstrained: nothing tied it
    /// to the argument (correctly) and nothing tied it to the result either, so
    /// it defaulted to `i32` before the annotation was consulted and the program
    /// panicked at runtime with "shift amount exceeds i32 bit width" for an
    /// expression whose only written type is `i64` (#833).
    fn is_shift(method: &str) -> bool {
        matches!(method, "shl" | "shr")
    }

    /// A concrete number. Deliberately excludes nominal types with operator
    /// impls: `5 + meters` should still be rejected, not quietly given the
    /// newtype's type.
    fn is_numeric_primitive(ty: &Type) -> bool {
        matches!(
            ty,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            | Type::F32 | Type::F64
        )
    }

    /// The element type `T` of a `Handle<T>`, or `None` for anything else.
    /// `WeakHandle` is excluded — it must be `upgrade()`d before field access.
    pub(super) fn handle_element_type(&self, ty: &Type) -> Option<Type> {
        let (name, args) = match ty {
            Type::Generic { base, args } => (self.types.type_name(*base), args.as_slice()),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args.as_slice()),
            _ => return None,
        };
        if name != "Handle" {
            return None;
        }
        match args.first() {
            Some(GenericArg::Type(t)) => Some(self.resolve_named(t)),
            _ => None,
        }
    }

    /// The node type `T` of a `Link<T>`, or `None` for anything else.
    ///
    /// Unlike a handle, a link needs no `Pool<T>` in scope to be followed — it
    /// names the node directly (analysis.fourth-option), so this is the whole
    /// resolution story rather than the first half of one.
    pub(super) fn link_node_type(&self, ty: &Type) -> Option<Type> {
        let (name, args) = match ty {
            Type::Generic { base, args } => (self.types.type_name(*base), args.as_slice()),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args.as_slice()),
            _ => return None,
        };
        if name != "Link" {
            return None;
        }
        match args.first() {
            Some(GenericArg::Type(t)) => Some(self.resolve_named(t)),
            _ => None,
        }
    }

    /// The node type `T` of a `Rack<T>`, or `None` for anything else.
    pub(super) fn rack_node_type(&self, ty: &Type) -> Option<Type> {
        let (name, args) = match ty {
            Type::Generic { base, args } => (self.types.type_name(*base), args.as_slice()),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args.as_slice()),
            _ => return None,
        };
        if name != "Rack" {
            return None;
        }
        match args.first() {
            Some(GenericArg::Type(t)) => Some(self.resolve_named(t)),
            _ => None,
        }
    }

    /// The element type `T` of a `Pool<T>`, or `None` for anything else.
    pub(super) fn pool_element_type(&self, ty: &Type) -> Option<Type> {
        let (name, args) = match ty {
            Type::Generic { base, args } => (self.types.type_name(*base), args.as_slice()),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args.as_slice()),
            _ => return None,
        };
        if name != "Pool" {
            return None;
        }
        match args.first() {
            Some(GenericArg::Type(t)) => Some(self.resolve_named(t)),
            _ => None,
        }
    }

    pub(super) fn resolve_field(
        &mut self,
        ty: Type,
        field: String,
        expected: Type,
        span: Span,
        self_type: Option<Type>,
    ) -> Result<bool, TypeError> {
        let ty = self.resolve_named(&self.ctx.apply(&ty));

        // mem.context/CC1: `h.field` on a `Handle<T>` auto-resolves through the
        // active `Pool<T>` context — type the access as the element `T`'s field.
        // Lowering (native hidden-param pass) and the interpreter rewrite it into
        // `pool[h].field`. Only strong handles auto-deref; a `WeakHandle` must be
        // `upgrade()`d first.
        if let Some(elem) = self.handle_element_type(&ty) {
            return self.resolve_field(elem, field, expected, span, self_type);
        }

        // `l.health` on a `Link<Entity>` is the node's field. No context clause
        // and no liveness check: a link that exists points at a live node.
        if let Some(node) = self.link_node_type(&ty) {
            return self.resolve_field(node, field, expected, span, self_type);
        }

        match &ty {
            // Source error already reported — suppress cascading field errors
            Type::Error => Ok(false),
            Type::Var(_) => {
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty,
                    field,
                    expected,
                    span,
                    self_type,
                });
                Ok(false)
            }
            Type::Named(type_id) => {
                // V5: check private field access
                if let Some(TypeDef::Struct { private_fields, name, .. }) = self.types.get(*type_id) {
                    if private_fields.contains(&field) {
                        let is_self = self_type.as_ref()
                            .is_some_and(|st| matches!(st, Type::Named(id) if *id == *type_id));
                        if !is_self {
                            return Err(TypeError::PrivateFieldAccess {
                                ty: name.clone(),
                                field,
                                span,
                            });
                        }
                    }
                }

                // A generic enum written without type arguments —
                // `GrowError.Full(item)` — gets a fresh variable per declared
                // parameter, so the payload can bind them. Handing back a bare
                // `GrowError` instead dropped the arguments, and the value then
                // never matched a declared `void or GrowError<Item>`: the error
                // branch of every generic error type was unwritable (#666).
                let enum_params = self.enum_type_params(*type_id);
                let enum_subst: Vec<Type> = enum_params
                    .iter()
                    .map(|_| self.ctx.fresh_var())
                    .collect();
                let enum_self = if enum_params.is_empty() {
                    ty.clone()
                } else {
                    Type::Generic {
                        base: *type_id,
                        args: enum_subst
                            .iter()
                            .cloned()
                            .map(|t| crate::types::GenericArg::Type(Box::new(t)))
                            .collect(),
                    }
                };
                let param_map: std::collections::HashMap<&str, Type> = enum_params
                    .iter()
                    .map(|p| p.as_str())
                    .zip(enum_subst.iter().cloned())
                    .collect();

                let result = self.types.get(*type_id).and_then(|def| {
                    match def {
                        TypeDef::Struct { fields, .. } | TypeDef::Union { fields, .. } => {
                            fields.iter().find(|(n, _)| n == &field).map(|(_, t)| t.clone())
                        }
                        TypeDef::Enum { variants, .. } => {
                            variants.iter().find(|(n, _)| n == &field).map(|(_, fields)| {
                                if fields.is_empty() {
                                    enum_self.clone()
                                } else {
                                    Type::Fn {
                                        params: fields
                                            .iter()
                                            .map(|t| Self::substitute_type_params(t, &param_map))
                                            .collect(),
                                        ret: Box::new(enum_self.clone()),
                                    }
                                }
                            })
                        }
                        TypeDef::NominalAlias { underlying, .. } => {
                            if field == "value" {
                                Some(underlying.clone())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                });

                if let Some(field_ty) = result {
                    self.unify(&expected, &field_ty, span)
                } else {
                    Err(TypeError::NoSuchField {
                        ty,
                        field,
                        span,
                    })
                }
            }
            Type::Tuple(elems) => {
                if let Ok(idx) = field.parse::<usize>() {
                    if idx < elems.len() {
                        self.unify(&expected, &elems[idx], span)
                    } else {
                        Err(TypeError::NoSuchField {
                            ty,
                            field,
                            span,
                        })
                    }
                } else {
                    Err(TypeError::NoSuchField {
                        ty,
                        field,
                        span,
                    })
                }
            }
            Type::Generic { base, args } => {
                let result = self.types.get(*base).and_then(|def| {
                    match def {
                        TypeDef::Struct { type_params, fields, .. } => {
                            let subst = Self::build_type_param_subst(type_params, args);
                            fields.iter().find(|(n, _)| n == &field).map(|(_, t)| {
                                Self::substitute_type_params(t, &subst)
                            })
                        }
                        TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::build_type_param_subst(type_params, args);
                            variants.iter().find(|(n, _)| n == &field).map(|(_, fields)| {
                                if fields.is_empty() {
                                    ty.clone()
                                } else {
                                    Type::Fn {
                                        params: fields.iter()
                                            .map(|t| Self::substitute_type_params(t, &subst))
                                            .collect(),
                                        ret: Box::new(ty.clone()),
                                    }
                                }
                            })
                        }
                        _ => None,
                    }
                });

                if let Some(field_ty) = result {
                    self.unify(&expected, &field_ty, span)
                } else {
                    Err(TypeError::NoSuchField {
                        ty,
                        field,
                        span,
                    })
                }
            }
            // UnresolvedGeneric: resolve element field access through first
            // type arg. Handles vec[i].field where vec type wasn't fully
            // resolved during inference.
            Type::UnresolvedGeneric { args, .. } => {
                if let Some(GenericArg::Type(elem)) = args.first() {
                    let elem_ty = self.resolve_named(elem);
                    self.resolve_field(elem_ty, field, expected, span, self_type)
                } else {
                    Err(TypeError::NoSuchField { ty, field, span })
                }
            }
            // Module namespace and builtin struct field resolution
            Type::UnresolvedNamed(name) => {
                // Module namespace: __module_X.Field → look up Field in type table
                if name.starts_with("__module_") {
                    if let Some(type_id) = self.types.get_type_id(&field) {
                        return self.unify(&expected, &Type::Named(type_id), span);
                    }
                    // Fallback: treat as unresolved named type
                    let resolved_ty = Type::UnresolvedNamed(field.to_string());
                    return self.unify(&expected, &resolved_ty, span);
                }

                // Builtin struct fields for runtime/stdlib types
                let field_ty = match (name.as_str(), field.as_str()) {
                    ("Response", "status") => Some(Type::U16),
                    ("Response", "headers") => Some(Type::UnresolvedNamed("Headers".to_string())),
                    ("Response", "body") => Some(Type::String),
                    ("Request", "method") => Some(Type::UnresolvedNamed("Method".to_string())),
                    ("Request", "url") => Some(Type::String),
                    ("Request", "body") => Some(Type::String),
                    ("Request", "headers") => Some(Type::UnresolvedNamed("Headers".to_string())),
                    _ => None,
                };
                if let Some(ft) = field_ty {
                    self.unify(&expected, &ft, span)
                } else {
                    Err(TypeError::NoSuchField { ty, field, span })
                }
            }
            // Option<T> field access: unwrap and access inner type
            ty if ty.is_option() => {
                let inner = ty.as_option().unwrap().clone();
                self.resolve_field(inner, field, expected, span, self_type)
            }
            _ => Err(TypeError::NoSuchField {
                ty,
                field,
                span,
            }),
        }
    }

    /// Record a generic method's own type arguments against the call site, so
    /// monomorphization instantiates one body per set of them.
    ///
    /// Only the method's parameters — the receiver's are already fixed by the
    /// receiver's type, and mangling on them too would mint a separate copy per
    /// receiver instantiation for no reason.
    fn note_method_type_args(
        &mut self,
        call_node: Option<NodeId>,
        method_sig: &MethodSig,
        span: Span,
        subst: &std::collections::HashMap<&str, Type>,
    ) {
        if method_sig.type_params.is_empty() {
            return;
        }
        // #314: whatever the argument settles on has to satisfy the bound.
        for (name, bounds) in &method_sig.type_params {
            if bounds.is_empty() {
                continue;
            }
            if let Some(var) = subst.get(name.as_str()) {
                self.pending_bound_checks.push((var.clone(), bounds.clone(), span));
            }
        }
        let Some(node) = call_node else { return };
        let args: Vec<Type> = method_sig
            .type_params
            .iter()
            .filter_map(|(name, _)| subst.get(name.as_str()).cloned())
            .collect();
        if args.len() == method_sig.type_params.len() {
            self.pending_call_type_args.push((node, args));
        }
    }

    /// std.fmt/D2–D5: can `{}` render this on its own?
    ///
    /// Primitives can (D2). Structs and enums opt in with `to_string`, or get
    /// it for free from `message()` (D3, D5). An optional or a result can't —
    /// there may be nothing to show, and the caller has to say what happens
    /// then. Anything else keeps rendering the way it does today.
    fn is_displayable(&self, ty: &Type) -> bool {
        match ty {
            Type::Result { .. } => false,
            Type::Named(id) | Type::Generic { base: id, .. } => {
                // A nominal newtype inherits nothing it didn't ask for
                // (type.aliases/T10), so its `with (…)` list is the answer —
                // and an `extend` block that writes `to_string` counts too.
                if let Some(TypeDef::NominalAlias { with_traits, methods, .. }) = self.types.get(*id) {
                    return with_traits.iter().any(|t| t == "Displayable")
                        || methods.iter().any(|m| m.name == "to_string" || m.name == "message");
                }
                let has = |name: &str| {
                    crate::traits::implements_trait(&self.types, ty, name)
                };
                has("Displayable") || has("Error")
            }
            // std.fmt/D3: a container has no `to_string`. `{v}` on one used to
            // pass this gate and then print the buffer's address natively —
            // the interpreter refused at run time with "no method to_string on
            // type Vec", so the two backends disagreed about a program that
            // shouldn't compile. `{v:debug}` renders it and needs nothing.
            Type::Tuple(_) | Type::Array { .. } | Type::Slice(_) => false,
            Type::UnresolvedGeneric { name, .. } => !matches!(
                name.as_str(),
                "Vec" | "Map" | "Set" | "Pool" | "Rack" | "Iterator"
            ),
            _ => true,
        }
    }

    /// The signature `method` gets on a nominal newtype from one of the traits
    /// its `with (…)` clause lists (type.aliases/T11).
    ///
    /// The trait signatures write `Self` as type variable 0, so binding that to
    /// the newtype is the whole of T12's delegation: `Id`'s `eq` takes an `Id`,
    /// and its `clone` gives one back. Positions the trait spells out concretely
    /// — `hash`'s `u64`, `compare`'s `Ordering` — stay as written.
    fn inherited_trait_method(
        &self,
        ty: &Type,
        with_traits: &[String],
        method: &str,
    ) -> Option<MethodSig> {
        let checker = crate::traits::TraitChecker::new(&self.types);
        let self_var = Type::Var(TypeVarId(0));
        for trait_name in with_traits {
            let Some(mut sig) = checker
                .get_trait_methods_public(trait_name)
                .into_iter()
                .find(|m| m.name == method)
            else {
                continue;
            };
            let bind_self = |t: &Type| if *t == self_var { ty.clone() } else { t.clone() };
            sig.params = sig.params.iter().map(|(t, m)| (bind_self(t), *m)).collect();
            sig.ret = bind_self(&sig.ret);
            return Some(sig);
        }
        None
    }

    /// A type as the user wrote it. `Type`'s own Display can't name a
    /// registered type — it prints `<type#7>`.
    fn render_type(&self, ty: &Type) -> String {
        match ty {
            Type::Result { ok, err } if **err == Type::None => {
                format!("{}?", self.render_type(ok))
            }
            Type::Result { ok, err } => {
                format!("{} or {}", self.render_type(ok), self.render_type(err))
            }
            Type::Named(_) | Type::Generic { .. } => super::receiver_name(ty, &self.types)
                .unwrap_or_else(|| format!("{}", ty)),
            _ => format!("{}", ty),
        }
    }

    pub(super) fn resolve_method(
        &mut self,
        ty: Type,
        method: String,
        args: Vec<Type>,
        ret: Type,
        span: Span,
        call_node: Option<NodeId>,
    ) -> Result<bool, TypeError> {
        let ty = self.resolve_named(&self.ctx.apply(&ty));

        // Type arguments the call wrote, if any. They bind the method's own type
        // parameters where a stub signature has them (#1029).
        let written: Vec<Type> = call_node
            .and_then(|n| self.written_method_type_args.get(&n).cloned())
            .unwrap_or_default();

        // CALL6: record the resolved receiver before any arm runs. Every path
        // below dispatches on this exact type, so recording once here covers
        // user types, stdlib types and the builtin methods alike — and it's the
        // applied type, which is what downstream can't reconstruct.
        //
        // `Var` is skipped: it re-enters through the deferred HasMethod
        // constraint once inference settles, and records then.
        if let Some(node) = call_node {
            if !matches!(ty, Type::Var(_) | Type::Error) {
                self.call_targets.insert(
                    node,
                    Callee::Method { recv: ty.clone(), method: method.clone() },
                );
            }
        }

        // A stdlib signature with nothing behind it. Caught here, where every
        // receiver passes, so the user sees it at their call rather than as
        // `Function not found: Vec_reserve` out of codegen or a runtime error
        // part-way through a run.
        if !matches!(ty, Type::Var(_) | Type::Error) {
            if let Some(prefix) = super::receiver_name(&ty, &self.types) {
                if rask_stdlib::mir_metadata::is_unimplemented(&prefix, &method) {
                    return Err(TypeError::UnimplementedStdlibMethod {
                        ty: prefix,
                        method,
                        span,
                    });
                }
            }
        }

        if method == "clone" && args.is_empty() {
            let progress = self.unify(&ty, &ret, span)?;
            // A clone's result is its receiver's type, so unifying settles both.
            // But this arm answers before the `Var` handling further down, which
            // is what normally files a deferred constraint and records the
            // dispatch target once inference settles — so an unresolved receiver
            // got no target and nothing came back to give it one. That's why
            // `.map(|r| r.view.clone())` in a fused iterator chain type-checked
            // and then left MIR with nothing to dispatch on: the closure's
            // parameter type isn't known until the chain's element type is
            // (#425). Record it here when unifying settled the type, hand it to
            // the post-defaulting retry when it didn't.
            if matches!(ty, Type::Var(_)) {
                let settled = self.ctx.apply(&ty);
                if matches!(settled, Type::Var(_) | Type::Error) {
                    self.deferred_methods.push(TypeConstraint::HasMethod {
                        ty,
                        method,
                        args,
                        ret,
                        span,
                        call_node,
                    });
                } else if let Some(node) = call_node {
                    self.call_targets.insert(
                        node,
                        Callee::Method { recv: settled, method: method.clone() },
                    );
                }
            }
            return Ok(progress);
        }

        // std.fmt/D1–D5: `to_string()` comes from Displayable. Primitives have
        // it, aggregates opt in. `{x}` desugars to this call, so both forms are
        // checked in one place.
        if (method == "to_string" && args.is_empty())
            || (method == "__fmt" && args.len() == 5)
        {
            // The answer is a string either way, so pin that now — deferring it
            // as well would leave the interpolation's own type open and produce
            // a second, unrelated error about it.
            let progress = self.unify(&ret, &Type::String, span)?;

            // "Does this have to_string" has no answer until the receiver's type
            // is settled, and `is_displayable` says yes to anything unresolved.
            // An unsuffixed literal (`let n = 3`) stays a variable until literal
            // defaulting runs at the very end, so its `Ordering` arrived after
            // this check had already waved it through: `println("{n.compare(m)}")`
            // compiled and printed the raw enum tag natively, `0` for Less
            // (#729). Come back for it once defaults have landed.
            if matches!(ty, Type::Var(_)) {
                self.deferred_methods.push(TypeConstraint::HasMethod {
                    ty, method, args, ret, span, call_node,
                });
                return Ok(progress);
            }
            // `{:debug}` is the free half of the D3 split: developer-facing
            // output for every type, no opt-in. Only `{}` and a bare
            // `to_string()` need Displayable.
            let is_debug = call_node
                .map(|n| self.debug_fmt_calls.contains(&n))
                .unwrap_or(false);
            if !is_debug && !self.is_displayable(&ty) {
                return Err(TypeError::NotDisplayable {
                    ty: self.render_type(&ty),
                    interpolated: method == "__fmt",
                    span,
                });
            }
            return Ok(progress);
        }

        // `__concat` is what interpolation desugars to (std.strings has no
        // public `concat`). string in, string out — the receiver is already
        // known to be a string wherever the desugarer emits it.
        if method == "__concat" && args.len() == 1 {
            let progress = self.unify(&ret, &Type::String, span)?;
            let _ = self.unify(&args[0], &Type::String, span)?;
            return Ok(progress);
        }

        // ER16: .origin() on any type returns the error origin string.
        // Set by `try` at first propagation (ER15). Returns "<no origin>" if unset.
        if method == "origin" && args.is_empty() {
            return self.unify(&ret, &Type::String, span);
        }

        match &ty {
            // Source error already reported — suppress cascading method errors
            Type::Error => Ok(false),
            Type::Var(id) => {
                // A primitive arithmetic operator takes both operands at the
                // same type, but desugaring rewrote `1000 / n` into
                // `1000.div(n)` and that fact went with the operator. With an
                // unresolved literal on the left there is nothing left tying it
                // to `n`, so the result type stayed open forever and the binding
                // it fed got reported as un-inferrable. Take the type from the
                // argument — that's where the operand's type actually is (#630).
                // A result-preserving operator with no argument — `-1.0` is
                // `(1.0).neg()` after desugaring. There's no argument to take a
                // type from, but the result has the receiver's type by
                // definition, so tying them together lets the *call site* settle
                // both: `let x: f32 = -2.5` makes the literal f32. Without this
                // the literal defaulted to f64 before the annotation was
                // consulted, and every negative float literal in an f32 position
                // was a type error while the positive one was fine.
                if self.ctx.literal_vars.contains_key(id)
                    && args.is_empty()
                    && Self::is_result_preserving_unary(&method)
                {
                    let progress = self.unify(&ret, &ty, span)?;
                    // Tying the two together settles the *type*, but the
                    // receiver is still a literal variable, so there's no
                    // concrete type to record a dispatch target against. Hand it
                    // to the post-defaulting retry, which has one — otherwise
                    // `n.abs()` type-checks and MIR still has to guess its
                    // receiver (#425).
                    self.deferred_methods.push(TypeConstraint::HasMethod {
                        ty, method, args, ret, span, call_node,
                    });
                    return Ok(progress);
                }
                // A shift's result has the receiver's type, whatever the shift
                // amount is. Same shape as the unary case above: tie the two
                // together so the call site settles both, and hand the dispatch
                // record to the post-defaulting retry.
                if self.ctx.literal_vars.contains_key(id) && Self::is_shift(&method) {
                    let progress = self.unify(&ret, &ty, span)?;
                    // A shift amount written as a bare literal has no type of
                    // its own to defend, and ORD4 puts the shifts with
                    // arithmetic — a `u64` receiver and an `i32` amount is a
                    // mixed-signedness error. Defaulting the literal to `i32`
                    // made `let u: u64 = 1 << 63` that error, for a line with no
                    // `i32` in it. A *typed* amount is left alone: `u64 << u8`
                    // is legal and this must not narrow it.
                    if let [arg] = args.as_slice() {
                        if matches!(self.ctx.apply(arg),
                            Type::Var(arg_id) if self.ctx.literal_vars.contains_key(&arg_id))
                        {
                            self.unify(arg, &ty, span)?;
                        }
                    }
                    self.deferred_methods.push(TypeConstraint::HasMethod {
                        ty, method, args, ret, span, call_node,
                    });
                    return Ok(progress);
                }
                // The mirror of the optional's own `eq` rule: `5 == a` with an
                // optional on the right leaves the literal free, and it
                // defaults to `i32` against an `i64?`. The bare side is meant
                // as the payload wherever it's written (#834).
                if self.ctx.literal_vars.contains_key(id)
                    && Self::is_homogeneous_comparison(&method)
                {
                    if let [arg] = args.as_slice() {
                        if let Some(inner) = self.ctx.apply(arg).as_option().cloned() {
                            self.unify(&ty, &inner, span)?;
                            return self.unify(&ret, &Type::Bool, span);
                        }
                    }
                }
                if self.ctx.literal_vars.contains_key(id)
                    && Self::is_homogeneous_operator(&method)
                {
                    if let [arg] = args.as_slice() {
                        let arg_ty = self.ctx.apply(arg);
                        if Self::is_numeric_primitive(&arg_ty) {
                            self.unify(&ty, &arg_ty, span)?;
                            // Settling the receiver isn't enough on its own —
                            // resolve the call now that it has a concrete type,
                            // or the result type is still nothing.
                            return self.resolve_method(
                                arg_ty, method, args, ret, span, call_node,
                            );
                        }
                        // Both sides are still bare literals (`let r = 100`
                        // then `1000 / r`). Tie them together and give the
                        // result its type from the operator, so literal
                        // defaulting settles all of it at once — deferring
                        // again would strand it, since the leftover pass runs
                        // before defaulting and never revisits.
                        if matches!(arg_ty, Type::Var(arg_id) if self.ctx.literal_vars.contains_key(&arg_id))
                        {
                            self.unify(&ty, &arg_ty, span)?;
                            return if Self::is_homogeneous_comparison(&method) {
                                self.unify(&ret, &Type::Bool, span)
                            } else {
                                self.unify(&ret, &ty, span)
                            };
                        }
                    }
                }
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty,
                    method,
                    args,
                    ret,
                    span,
                    call_node,
                });
                Ok(false)
            }
            Type::Named(type_id) => {
                let (methods, type_params) = match self.types.get(*type_id) {
                    Some(TypeDef::Struct { methods, type_params, .. }) => {
                        (methods.clone(), type_params.clone())
                    }
                    Some(TypeDef::Enum { methods, type_params, .. }) => {
                        (methods.clone(), type_params.clone())
                    }
                    Some(TypeDef::NominalAlias { methods, with_traits, .. }) => {
                        // T11/T12: a nominal newtype inherits the traits its
                        // `with (…)` clause lists, and they delegate to the
                        // value underneath. The list was recorded and never
                        // read, so `type Id = u64 with (Equal)` gave `Id` no
                        // `eq` at all and `a == b` didn't compile (#551).
                        let own = methods.iter().any(|m| m.name == method);
                        let inherited = (!own)
                            .then(|| self.inherited_trait_method(&ty, with_traits, &method))
                            .flatten();
                        match inherited {
                            Some(sig) => (vec![sig], Vec::new()),
                            None => (methods.clone(), Vec::new()),
                        }
                    }
                    _ => {
                        return Err(TypeError::NoSuchMethod {
                            ty,
                            method,
                            span,
                        })
                    }
                };

                if let Some(method_sig) = methods.iter().find(|m| m.name == method) {
                    if method_sig.params.len() != args.len() {
                        return Err(TypeError::ArityMismatch {
                            expected: method_sig.params.len(),
                            found: args.len(),
                            span,
                        });
                    }

                    // Instantiate generic type params with fresh vars so a
                    // bare `Vec.new()` produces a Vec carrying a fresh element
                    // var instead of the literal "T" placeholder. Without this
                    // the placeholder leaks into node_types and downstream
                    // unifications (push, get, ...) silently no-op.
                    //
                    // The method's own parameters go in the same map. They
                    // differ from the receiver's in when they're chosen — once
                    // per call rather than once per receiver — but a fresh
                    // variable each time is exactly that.
                    let subst: std::collections::HashMap<&str, Type> = type_params
                        .iter()
                        .map(|p| p.as_str())
                        .chain(method_sig.type_params.iter().map(|(n, _)| n.as_str()))
                        .map(|p| (p, self.ctx.fresh_var()))
                        .collect();
                    self.note_method_type_args(call_node, method_sig, span, &subst);

                    // ER3a: a `T or E` in the method's signature is a
                    // disjointness obligation on the receiver's type args.
                    self.note_disjointness_obligations(&method, &method_sig.ret, &subst, span);
                    for (param_ty, _mode) in &method_sig.params {
                        self.note_disjointness_obligations(&method, param_ty, &subst, span);
                    }

                    let index_params = super::receiver_name(&ty, &self.types)
                        .map(|n| sequence_index_params(&n, &method))
                        .unwrap_or(&[]);

                    let mut progress = false;
                    for (i, ((param_ty, _mode), arg)) in
                        method_sig.params.iter().zip(args.iter()).enumerate()
                    {
                        // V8: a position or count takes any integer type. The
                        // declared width is what the runtime receives, not a
                        // constraint on the caller.
                        if index_params.contains(&i) {
                            self.check_integer_arg(&ty, arg, span);
                            continue;
                        }
                        let substituted = Self::substitute_type_params(param_ty, &subst);
                        // CV1a/CV2 (#649) and the wrapper coercion (#701) are
                        // the same question — which side is the slot — so one
                        // call answers both: `coerce_arg` runs `check_fits`
                        // before deciding whether layers are needed.
                        if self.coerce_arg(&substituted, arg, span)? {
                            progress = true;
                        }
                    }

                    let substituted_ret = Self::substitute_type_params(&method_sig.ret, &subst);
                    if self.unify(&substituted_ret, &ret, span)? {
                        progress = true;
                    }

                    Ok(progress)
                } else {
                    let variant = self.types.get(*type_id).and_then(|def| {
                        if let TypeDef::Enum { variants, .. } = def {
                            variants.iter().find(|(n, _)| n == &method).map(|(_, fields)| fields.clone())
                        } else {
                            None
                        }
                    });

                    if let Some(mut fields) = variant {
                        // The type args come from the payload: `GrowError.Full(item)`
                        // names no arguments, so the variant's declared `T` gets a
                        // fresh variable that the payload then binds. Without this
                        // the constructed value is bare `GrowError`, which never
                        // matches the declared `void or GrowError<Item>` — so the
                        // error branch of every generic error type was unwritable
                        // (#666).
                        let mut constructed = ty.clone();
                        if Some(*type_id) == self.types.get_result_type_id()
                            || Some(*type_id) == self.types.get_option_type_id()
                        {
                            fields = self.instantiate_builtin_enum_variant(*type_id, &method, &fields);
                        } else if !type_params.is_empty() && matches!(ty, Type::Named(_)) {
                            let fresh: Vec<Type> =
                                type_params.iter().map(|_| self.ctx.fresh_var()).collect();
                            let subst: std::collections::HashMap<&str, Type> = type_params
                                .iter()
                                .map(|p| p.as_str())
                                .zip(fresh.iter().cloned())
                                .collect();
                            fields = fields
                                .iter()
                                .map(|f| Self::substitute_type_params(f, &subst))
                                .collect();
                            constructed = Type::Generic {
                                base: *type_id,
                                args: fresh
                                    .into_iter()
                                    .map(|t| crate::types::GenericArg::Type(Box::new(t)))
                                    .collect(),
                            };
                        } else {
                            // User-defined enum: instantiate any TypeVars with fresh vars
                            fields = self.instantiate_type_vars(&fields);
                        }

                        if fields.len() != args.len() {
                            return Err(TypeError::ArityMismatch {
                                expected: fields.len(),
                                found: args.len(),
                                span,
                            });
                        }
                        let mut progress = false;
                        for (field_ty, arg) in fields.iter().zip(args.iter()) {
                            if self.unify(field_ty, arg, span)? {
                                progress = true;
                            }
                        }
                        if self.unify(&constructed, &ret, span)? {
                            progress = true;
                        }
                        Ok(progress)
                    } else if method == "variants" && args.is_empty() {
                        // .variants() on fieldless enums returns Vec of all variant values (E7-E8)
                        let is_fieldless = self.types.get(*type_id).map(|def| {
                            if let TypeDef::Enum { variants, .. } = def {
                                variants.iter().all(|(_, fields)| fields.is_empty())
                            } else {
                                false
                            }
                        }).unwrap_or(false);
                        if is_fieldless {
                            let vec_ty = Type::Slice(Box::new(ty));
                            self.unify(&vec_ty, &ret, span)
                        } else {
                            Err(TypeError::NoSuchMethod {
                                ty,
                                method: "variants (requires fieldless enum)".to_string(),
                                span,
                            })
                        }
                    } else if method == "discriminant" && args.is_empty() {
                        // E9: .discriminant() returns u16 variant index
                        self.unify(&Type::U16, &ret, span)
                    } else if method == "from_value" && args.len() == 1 {
                        // E18: from_value(n) on fieldless enums returns Option<Enum>
                        let is_fieldless = self.types.get(*type_id).map(|def| {
                            if let TypeDef::Enum { variants, .. } = def {
                                variants.iter().all(|(_, fields)| fields.is_empty())
                            } else {
                                false
                            }
                        }).unwrap_or(false);
                        if is_fieldless {
                            // Accept any integer type for the discriminant value.
                            if let Some(arg_ty) = args.first() {
                                let resolved_arg = self.ctx.apply(arg_ty);
                                let is_int = matches!(
                                    resolved_arg,
                                    Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                                    | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
                                );
                                if !is_int {
                                    self.unify(arg_ty, &Type::I64, span)?;
                                }
                            }
                            let opt_ty = Type::option(ty);
                            self.unify(&opt_ty, &ret, span)
                        } else {
                            Err(TypeError::NoSuchMethod {
                                ty,
                                method: "from_value (requires fieldless enum)".to_string(),
                                span,
                            })
                        }
                    } else {
                        // Method not in registered extend blocks. Check if this
                        // Named type corresponds to a builtin with hardcoded
                        // method resolution (Vec, Map, Shared, etc.).
                        let type_name = self.types.type_name(*type_id);
                        self.resolve_builtin_method_by_name(&type_name, &[], &method, &args, &ret, span)
                            .unwrap_or_else(|| Err(TypeError::NoSuchMethod { ty, method, span }))
                    }
                }
            }
            Type::String => self.resolve_string_method(&method, &args, &ret, &written, span),
            // `string` used as a type namespace — `string.from_utf8(bytes)`,
            // and the `string.new()` people reach for out of Rust habit. The
            // receiver is the type name, not a value, so it arrives unresolved
            // and used to miss the resolver above entirely.
            Type::UnresolvedNamed(ref name) if name == "string" => {
                self.resolve_string_method(&method, &args, &ret, &written, span)
            }
            Type::Char => self.resolve_char_method(&method, &args, &ret, span),
            Type::Array { .. } | Type::Slice(_) => {
                self.resolve_array_method(&ty, &method, &args, &ret, span)
            }
            Type::UnresolvedNamed(name) if name == "File" => {
                self.resolve_file_method(&method, &args, &ret, span)
            }
            Type::UnresolvedGeneric { name, args: type_args } if name == "ThreadHandle" => {
                self.resolve_thread_handle_method(&type_args, &method, &args, &ret, span)
            }
            Type::UnresolvedGeneric { name, args: type_args } if name == "TaskHandle" => {
                self.resolve_task_handle_method(&type_args, &method, &args, &ret, span)
            }
            // Pool<T>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Pool" => {
                self.resolve_pool_method(type_args, &method, &args, &ret, span)
            }
            // Rack<T>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Rack" => {
                self.resolve_rack_method(type_args, &method, &args, &ret, span)
            }
            // Link<T> — a reference. `eq`/`ne` compare node identity; anything
            // else falls through to the node's own methods, the same way field
            // access does.
            Type::UnresolvedGeneric { name, args: type_args } if name == "Link" => {
                match method.as_str() {
                    "eq" | "ne" if args.len() == 1 => self.unify(&ret, &Type::Bool, span),
                    _ => {
                        let node_ty = if let Some(GenericArg::Type(t)) = type_args.first() {
                            *t.clone()
                        } else {
                            self.ctx.fresh_var()
                        };
                        self.resolve_method(node_ty, method, args, ret, span, None)
                    }
                }
            }
            // Handle<T> — value type, eq/ne only
            Type::UnresolvedGeneric { name, .. } if name == "Handle" => {
                match method.as_str() {
                    "eq" | "ne" if args.len() == 1 => self.unify(&ret, &Type::Bool, span),
                    _ => Err(TypeError::NoSuchMethod { ty, method, span }),
                }
            }
            // WeakHandle<T> — valid(), upgrade(), eq, ne
            Type::UnresolvedGeneric { name, args: type_args } if name == "WeakHandle" => {
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };
                match method.as_str() {
                    "valid" if args.is_empty() => self.unify(&ret, &Type::Bool, span),
                    "upgrade" if args.is_empty() => {
                        let handle_ty = Type::UnresolvedGeneric {
                            name: "Handle".to_string(),
                            args: vec![GenericArg::Type(Box::new(inner_type))],
                        };
                        let opt_ty = Type::option(handle_ty);
                        self.unify(&ret, &opt_ty, span)
                    }
                    "eq" | "ne" if args.len() == 1 => self.unify(&ret, &Type::Bool, span),
                    _ => Err(TypeError::NoSuchMethod { ty, method, span }),
                }
            }
            // ctrl.ranges — the range adapters. Both return a range so they
            // chain, and `for i in (0..5).rev()` still sees something iterable.
            Type::UnresolvedNamed(name) if name == "Range" => {
                match method.as_str() {
                    "rev" if args.is_empty() => {
                        self.unify(&ret, &Type::UnresolvedNamed("Range".to_string()), span)
                    }
                    "step" if args.len() == 1 => {
                        self.unify(&args[0], &Type::I64, span)?;
                        self.unify(&ret, &Type::UnresolvedNamed("Range".to_string()), span)
                    }
                    _ => Err(TypeError::NoSuchMethod { ty, method, span }),
                }
            }
            // The adapters hand back the same range, element type included, so
            // `for i in (0..5).rev()` still knows what `i` is.
            Type::UnresolvedGeneric { name, .. } if name == "Range" => {
                match method.as_str() {
                    "rev" if args.is_empty() => self.unify(&ret, &ty, span),
                    "step" if args.len() == 1 => {
                        self.unify(&args[0], &Type::I64, span)?;
                        self.unify(&ret, &ty, span)
                    }
                    _ => Err(TypeError::NoSuchMethod { ty, method, span }),
                }
            }
            // Pool (bare, for static constructors like Pool.new())
            Type::UnresolvedNamed(name) if name == "Pool" => {
                self.resolve_pool_static_method(&method, &args, &ret, span)
            }
            // Rack (bare, for Rack.new())
            Type::UnresolvedNamed(name) if name == "Rack" => match method.as_str() {
                "new" if args.is_empty() => {
                    let rack_ty = Type::UnresolvedGeneric {
                        name: "Rack".to_string(),
                        args: vec![GenericArg::Type(Box::new(self.ctx.fresh_var()))],
                    };
                    self.unify(&ret, &rack_ty, span)
                }
                _ => Err(TypeError::NoSuchMethod { ty, method, span }),
            },
            // Vec<T>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Vec" => {
                self.resolve_vec_method(type_args, &method, &args, &ret, span)
            }
            // Vec (bare, for static constructors like Vec.new())
            Type::UnresolvedNamed(name) if name == "Vec" => {
                self.resolve_vec_static_method(&method, &args, &ret, span)
            }
            // Map<K, V>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Map" => {
                self.resolve_map_method(type_args, &method, &args, &ret, span)
            }
            // Map (bare, for static constructors like Map.new())
            Type::UnresolvedNamed(name) if name == "Map" => {
                self.resolve_map_static_method(&method, &args, &ret, span)
            }
            // Random (no type params — static and instance methods)
            Type::UnresolvedNamed(name) if name == "Random" => {
                self.resolve_rng_method(&method, &args, &ret, span)
            }
            // `Atomic<T>` — one type, one spelling (mem.atomics/GA1).
            _ if self.atomic_payload(&ty).is_some() => {
                let payload = self.atomic_payload(&ty).expect("just checked");
                self.resolve_atomic_method(payload, &method, &args, &ret, span)
            }
            // Thread.spawn(closure) → ThreadHandle<T>
            Type::UnresolvedNamed(name) if name == "Thread" || name == "ThreadPool" => {
                if method == "spawn" && args.len() == 1 {
                    // Extract closure return type for ThreadHandle<T>
                    let inner = if let Type::Fn { ret: fn_ret, .. } = &args[0] {
                        *fn_ret.clone()
                    } else {
                        self.ctx.fresh_var()
                    };
                    let handle_ty = Type::UnresolvedGeneric {
                        name: "ThreadHandle".to_string(),
                        args: vec![GenericArg::Type(Box::new(inner))],
                    };
                    self.unify(&ret, &handle_ty, span)
                } else {
                    Err(TypeError::NoSuchMethod {
                        ty,
                        method,
                        span,
                    })
                }
            }
            // SIMD vector types (f32x4, f32x8, i32x4, i32x8, f64x2, f64x4)
            Type::UnresolvedNamed(name) if Self::is_simd_type(name) => {
                self.resolve_simd_method(name, &method, &args, &ret, span)
            }
            // Iterator<T> — adapter chain methods, terminator methods.
            Type::UnresolvedGeneric { name, args: type_args } if name == "Iterator" => {
                self.resolve_iterator_method(&type_args, &method, &args, &ret, span)
            }
            // Shared<T>, Sender<T>, Receiver<T>, Channel<T>
            Type::UnresolvedGeneric { name, args: type_args } if matches!(name.as_str(), "Cell" | "Shared" | "Mutex" | "Sender" | "Receiver" | "Channel") => {
                self.resolve_concurrency_generic_method(name, &type_args, &method, &args, &ret, span)
            }
            // `Channel.buffered(n)` / `.unbuffered()` with no explicit `<T>`.
            // `Channel` is declared in async.rk, so the stub-registry arm below
            // claimed it and routed it to resolve_runtime_method, which has no
            // constructor for it — the call got no return type at all.
            // `mut (tx, rx) = Channel.buffered(4)` therefore left both bindings
            // as free type variables (`let probe: i64 = tx` type-checked), and
            // every `send`/`receive`/`clone` on them had to be resolved by MIR
            // guessing from the method name (#425). The concurrency resolver
            // mints a fresh inner type when it gets no args, which is what's
            // wanted: the element type is pinned by the first `send`.
            Type::UnresolvedNamed(name) if name == "Channel" => {
                self.resolve_concurrency_generic_method("Channel", &[], &method, &args, &ret, span)
            }
            // Builtin runtime types: Instant, Duration, TcpListener, TcpConnection, Shared (bare)
            Type::UnresolvedNamed(name) if matches!(name.as_str(), "Instant" | "Duration" | "TcpListener" | "TcpConnection" | "Response" | "Request" | "Shared" | "Mutex")
                || rask_stdlib::StubRegistry::load().get_type(name).is_some()
            => {
                self.resolve_runtime_method(name, &method, &args, &ret, span)
            }
            Type::Generic { base, args: generic_args } => {
                let (methods, type_params) = match self.types.get(*base) {
                    Some(TypeDef::Struct { methods, type_params, .. }) => {
                        (methods.clone(), type_params.clone())
                    }
                    Some(TypeDef::Enum { methods, type_params, .. }) => {
                        (methods.clone(), type_params.clone())
                    }
                    _ => {
                        return Err(TypeError::NoSuchMethod {
                            ty,
                            method,
                            span,
                        });
                    }
                };

                let mut subst = Self::build_type_param_subst(&type_params, generic_args);

                if let Some(method_sig) = methods.iter().find(|m| m.name == method) {
                    if method_sig.params.len() != args.len() {
                        return Err(TypeError::ArityMismatch {
                            expected: method_sig.params.len(),
                            found: args.len(),
                            span,
                        });
                    }

                    // The receiver's type args are pinned by the call site; the
                    // method's own parameters are still open, one fresh variable
                    // per call.
                    for (name, _) in &method_sig.type_params {
                        subst.insert(name.as_str(), self.ctx.fresh_var());
                    }
                    self.note_method_type_args(call_node, method_sig, span, &subst);

                    // ER3a: same obligation on the explicitly-spelled type args.
                    self.note_disjointness_obligations(&method, &method_sig.ret, &subst, span);
                    for (param_ty, _mode) in &method_sig.params {
                        self.note_disjointness_obligations(&method, param_ty, &subst, span);
                    }

                    // PC3: names the *method* declares, not the type — the `U`
                    // of `Vec<T>.map(f: func(T) -> U) -> Vec<U>`. One fresh var
                    // per name, shared across params and return so they line up.
                    let mut method_params: HashMap<String, Type> = HashMap::new();

                    let index_params = super::receiver_name(&ty, &self.types)
                        .map(|n| sequence_index_params(&n, &method))
                        .unwrap_or(&[]);

                    let mut progress = false;
                    for (i, ((param_ty, _mode), arg)) in
                        method_sig.params.iter().zip(args.iter()).enumerate()
                    {
                        // V8: a position or count takes any integer type.
                        if index_params.contains(&i) {
                            self.check_integer_arg(&ty, arg, span);
                            continue;
                        }
                        let substituted = Self::substitute_type_params(param_ty, &subst);
                        let substituted =
                            self.freshen_free_type_params(&substituted, &mut method_params);
                        // Same direction as above, same one call (#649, #701).
                        if self.coerce_arg(&substituted, arg, span)? {
                            progress = true;
                        }
                    }

                    let ret_substituted = Self::substitute_type_params(&method_sig.ret, &subst);
                    let ret_substituted =
                        self.freshen_free_type_params(&ret_substituted, &mut method_params);
                    if self.unify(&ret_substituted, &ret, span)? {
                        progress = true;
                    }

                    Ok(progress)
                } else {
                    // Check enum variants as constructors
                    let variant = self.types.get(*base).and_then(|def| {
                        if let TypeDef::Enum { type_params: tp, variants, .. } = def {
                            variants.iter().find(|(n, _)| n == &method).map(|(_, fields)| {
                                let subst = Self::build_type_param_subst(tp, generic_args);
                                fields.iter()
                                    .map(|t| Self::substitute_type_params(t, &subst))
                                    .collect::<Vec<_>>()
                            })
                        } else {
                            None
                        }
                    });

                    if let Some(fields) = variant {
                        if fields.len() != args.len() {
                            return Err(TypeError::ArityMismatch {
                                expected: fields.len(),
                                found: args.len(),
                                span,
                            });
                        }
                        let mut progress = false;
                        for (field_ty, arg) in fields.iter().zip(args.iter()) {
                            if self.unify(field_ty, arg, span)? {
                                progress = true;
                            }
                        }
                        if self.unify(&ty, &ret, span)? {
                            progress = true;
                        }
                        Ok(progress)
                    } else {
                        let type_name = self.types.type_name(*base);
                        self.resolve_builtin_method_by_name(&type_name, generic_args, &method, &args, &ret, span)
                            .unwrap_or_else(|| Err(TypeError::NoSuchMethod { ty, method, span }))
                    }
                }
            }
            // Trait object: look up method in trait definition
            Type::TraitObject { ref trait_name } => {
                let trait_name = trait_name.clone();
                let checker = crate::traits::TraitChecker::new(&self.types);
                // TR3: reject generic methods — they can't be monomorphized
                // into a single vtable slot, so they have no dynamic entry.
                // Checked before method lookup: trait method names carry their
                // type params (`convert<T>`) while the call site does not, so
                // an exact-name lookup would miss and report "no such method".
                let is_generic = self.types.get_type_id(&trait_name)
                    .and_then(|id| self.types.get(id))
                    .map_or(false, |def| def.is_generic_trait_method(&method));
                if is_generic {
                    return Err(TypeError::TraitObjectGenericMethod {
                        trait_name,
                        method,
                        span,
                    });
                }

                let trait_methods = checker.get_trait_methods_public(&trait_name);

                if let Some(method_sig) = trait_methods.iter().find(|m| m.name == method) {
                    // TR2: reject methods returning Self
                    if matches!(&method_sig.ret, Type::UnresolvedNamed(n) if n == "Self") {
                        return Err(TypeError::TraitObjectSelfReturn {
                            trait_name,
                            method,
                            span,
                        });
                    }

                    if method_sig.params.len() != args.len() {
                        return Err(TypeError::ArityMismatch {
                            expected: method_sig.params.len(),
                            found: args.len(),
                            span,
                        });
                    }

                    let mut progress = false;
                    for ((param_ty, _mode), arg) in method_sig.params.iter().zip(args.iter()) {
                        if self.coerce_arg(param_ty, arg, span)? {
                            progress = true;
                        }
                    }
                    if self.unify(&method_sig.ret, &ret, span)? {
                        progress = true;
                    }
                    Ok(progress)
                } else {
                    Err(TypeError::NoSuchMethod {
                        ty,
                        method,
                        span,
                    })
                }
            }
            // Primitive integer types — resolve operator methods directly
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => {
                self.resolve_integer_method(&ty, &method, &args, &ret, span)
            }
            // Primitive float types
            Type::F32 | Type::F64 => {
                self.resolve_float_method(&ty, &method, &args, &ret, span)
            }
            // `bool` is Equal and Comparable with `false < true`
            // (type.operators, support table).
            Type::Bool => match method.as_str() {
                "eq" | "ne" | "lt" | "le" | "gt" | "ge" if args.len() == 1 => {
                    self.unify(&args[0], &Type::Bool, span)?;
                    self.unify(&ret, &Type::Bool, span)
                }
                "compare" if args.len() == 1 => {
                    self.unify(&args[0], &Type::Bool, span)?;
                    self.unify(&ret, &self.ordering_type(), span)
                }
                "to_string" if args.is_empty() => self.unify(&ret, &Type::String, span),
                "hash" if args.is_empty() => self.unify(&ret, &Type::U64, span),
                _ => Err(TypeError::NoSuchMethod { ty, method, span }),
            },
            // Raw pointers. The eager path in check_expr catches these when the
            // receiver's type is already known; it isn't when the pointer came
            // out of another call, and without this arm those fell through to
            // "no method `offset` found for type `*u8`" (#696).
            Type::RawPtr(ref inner) => {
                let inner = (**inner).clone();
                self.resolve_raw_ptr_method(&inner, &ty, &method, &args, &ret, span)
            }
            // type.tuples/TU9 — `==`/`!=` on tuples, element by element. The
            // elements have to be comparable themselves, which unifying the
            // two tuple types checks.
            Type::Tuple(_) => match method.as_str() {
                "eq" | "ne" if args.len() == 1 => {
                    self.unify(&args[0], &ty, span)?;
                    self.unify(&ret, &Type::Bool, span)
                }
                _ => Err(TypeError::NoSuchMethod { ty, method, span }),
            },
            ty if ty.is_option() => {
                let inner = ty.as_option().unwrap().clone();
                self.resolve_option_method(&inner, &method, &args, &ret, span)
            }
            Type::Result { ok, err } => {
                let ok = *ok.clone();
                let err = *err.clone();
                self.resolve_result_method(&ok, &err, &method, &args, &ret, span)
            }
            // ER4: A method on a union (`A | B`) dispatches when every variant
            // implements a compatible method. The result type is the unified
            // return — repeated unification of `ret` against each variant's
            // return type enforces compatibility.
            Type::Union(variants) => {
                let variants = variants.clone();
                let any_unresolved = variants.iter().any(|v| {
                    matches!(self.resolve_named(&self.ctx.apply(v)), Type::Var(_))
                });
                if any_unresolved {
                    self.ctx.add_constraint(TypeConstraint::HasMethod {
                        ty: Type::Union(variants),
                        method,
                        args,
                        ret,
                        span,
                        call_node,
                    });
                    return Ok(false);
                }

                let mut missing: Vec<Type> = Vec::new();
                let mut progress = false;
                for variant in &variants {
                    // No single target for a union receiver — don't record.
                    match self.resolve_method(
                        variant.clone(),
                        method.clone(),
                        args.clone(),
                        ret.clone(),
                        span,
                        None,
                    ) {
                        Ok(p) => {
                            if p {
                                progress = true;
                            }
                        }
                        Err(TypeError::NoSuchMethod { .. }) => {
                            missing.push(variant.clone());
                        }
                        Err(e) => return Err(e),
                    }
                }

                if !missing.is_empty() {
                    return Err(TypeError::NoSuchMethod {
                        ty: Type::Union(variants),
                        method,
                        span,
                    });
                }

                Ok(progress)
            }
            // #314: a bounded type param `T where T: Greeter` carries the
            // trait's method set. Resolve statically against the bound; mono
            // later substitutes T with the concrete type and re-resolves.
            Type::UnresolvedNamed(ref name)
                if self.current_type_param_bounds.contains_key(name) =>
            {
                self.resolve_bounded_type_param_method(name.clone(), method, args, ret, span)
            }
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty,
                    method,
                    args,
                    ret,
                    span,
                    call_node,
                });
                Ok(false)
            }
        }
    }

    /// Resolve a method call on a type parameter through its trait bounds (#314).
    /// The bound brings the trait's methods into scope on the parameter; the
    /// `Self` position in each signature is the parameter itself.
    fn resolve_bounded_type_param_method(
        &mut self,
        param: String,
        method: String,
        args: Vec<Type>,
        ret: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let bounds = self
            .current_type_param_bounds
            .get(&param)
            .cloned()
            .unwrap_or_default();

        // Find a bound trait that declares `method`, pulling out its signature.
        let receiver = Type::UnresolvedNamed(param.clone());
        let sig = {
            let checker = crate::traits::TraitChecker::new(&self.types);
            bounds.iter().find_map(|tr| {
                let base = tr.split('<').next().unwrap_or(tr);
                checker
                    .get_trait_methods_public(base)
                    .into_iter()
                    .find(|m| m.name == method)
            })
        };

        let Some(sig) = sig else {
            // Bounded, but no bound provides this method.
            return Err(TypeError::UnboundedTypeParamMethod {
                param,
                method,
                bounds,
                span,
            });
        };

        if sig.params.len() != args.len() {
            return Err(TypeError::ArityMismatch {
                expected: sig.params.len(),
                found: args.len(),
                span,
            });
        }

        let mut progress = false;
        for ((param_ty, _mode), arg) in sig.params.iter().zip(args.iter()) {
            let substituted = Self::substitute_self_placeholder(param_ty, &receiver);
            // Same direction as above, same one call (#649, #701).
            if self.coerce_arg(&substituted, arg, span)? {
                progress = true;
            }
        }
        let substituted_ret = Self::substitute_self_placeholder(&sig.ret, &receiver);
        if self.unify(&substituted_ret, &ret, span)? {
            progress = true;
        }
        Ok(progress)
    }

    /// Replace the `Self` placeholder in a trait-method signature with the
    /// receiver type. User traits spell it `Self`; builtin trait sigs use the
    /// `Var(0)` placeholder (see `get_builtin_trait_methods`).
    fn substitute_self_placeholder(ty: &Type, receiver: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(n) if n == "Self" => receiver.clone(),
            Type::Var(crate::types::TypeVarId(0)) => receiver.clone(),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::substitute_self_placeholder(ok, receiver)),
                err: Box::new(Self::substitute_self_placeholder(err, receiver)),
            },
            Type::Slice(elem) => Type::Slice(Box::new(Self::substitute_self_placeholder(elem, receiver))),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::substitute_self_placeholder(elem, receiver)),
                len: *len,
            },
            Type::RawPtr(elem) => Type::RawPtr(Box::new(Self::substitute_self_placeholder(elem, receiver))),
            Type::Tuple(elems) => Type::Tuple(
                elems.iter().map(|e| Self::substitute_self_placeholder(e, receiver)).collect(),
            ),
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| Self::substitute_self_placeholder(p, receiver)).collect(),
                ret: Box::new(Self::substitute_self_placeholder(ret, receiver)),
            },
            _ => ty.clone(),
        }
    }

    pub(super) fn instantiate_builtin_enum_variant(
        &self,
        type_id: TypeId,
        _variant_name: &str,
        variant_fields: &[Type],
    ) -> Vec<Type> {
        let substitution = if Some(type_id) == self.types.get_result_type_id() {
            if let Some(Type::Result { ok, err }) = &self.current_return_type {
                let mut subst = HashMap::new();
                subst.insert(TypeVarId(0), *ok.clone());
                subst.insert(TypeVarId(1), *err.clone());
                subst
            } else {
                HashMap::new()
            }
        } else if Some(type_id) == self.types.get_option_type_id() {
            if let Some(ret_ty) = &self.current_return_type {
                if let Some(inner) = ret_ty.as_option() {
                    let mut subst = HashMap::new();
                    subst.insert(TypeVarId(0), inner.clone());
                    subst
                } else {
                    HashMap::new()
                }
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        variant_fields
            .iter()
            .map(|ty| self.apply_type_var_substitution(ty, &substitution))
            .collect()
    }

    /// Resolve methods on `Iterator<T>` (lazy iterator type returned by
    /// `string.split`, `Vec.iter`, etc.). The runtime is implemented in
    /// the interpreter; the type checker only needs to ascribe types.
    pub(super) fn resolve_iterator_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let elem = type_args.first().and_then(|a| {
            if let GenericArg::Type(t) = a { Some(*t.clone()) } else { None }
        }).unwrap_or_else(|| self.ctx.fresh_var());
        let self_ty = Type::UnresolvedGeneric {
            name: "Iterator".to_string(),
            args: vec![GenericArg::Type(Box::new(elem.clone()))],
        };
        match method {
            "next" if args.is_empty() => {
                self.unify(ret, &Type::option(elem), span)
            }
            "iter" if args.is_empty() => self.unify(ret, &self_ty, span),
            // SEQ28/SEQ31: the target is named, and `Vec<T>` is the only thing
            // it can be. There is no `collect()`.
            "to_vec" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(elem))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // SEQ30: the third materializing target, on a sequence of strings.
            "join" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::String, span);
                let _ = self.unify(&elem, &Type::String, span);
                self.unify(ret, &Type::String, span)
            }
            // SEQ29: only on a sequence of pairs. A Map needs a key per value,
            // and `to_map` reads it out of the first tuple slot rather than
            // inventing one — so a non-pair element is an error at the call.
            "to_map" if args.is_empty() => {
                let resolved = self.ctx.apply(&elem);
                match &resolved {
                    Type::Tuple(parts) if parts.len() == 2 => {
                        let map_ty = Type::UnresolvedGeneric {
                            name: "Map".to_string(),
                            args: vec![
                                GenericArg::Type(Box::new(parts[0].clone())),
                                GenericArg::Type(Box::new(parts[1].clone())),
                            ],
                        };
                        self.unify(ret, &map_ty, span)
                    }
                    // Still a variable — the element type may settle into a pair
                    // once the chain ahead of it is solved.
                    Type::Var(_) => {
                        self.ctx.add_constraint(TypeConstraint::HasMethod {
                            ty: self_ty.clone(),
                            method: "to_map".to_string(),
                            args: args.to_vec(),
                            ret: ret.clone(),
                            span,
                            call_node: None,
                        });
                        Ok(false)
                    }
                    other => Err(TypeError::ToMapNeedsPairs {
                        elem: other.clone(),
                        span,
                    }),
                }
            }
            "count" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "sum" if args.is_empty() => self.unify(ret, &elem, span),
            "min" | "max" if args.is_empty() => self.unify(ret, &Type::option(elem), span),
            "map" if args.len() == 1 => {
                let out = self.ctx.fresh_var();
                let expected_fn = Type::Fn {
                    params: vec![elem],
                    ret: Box::new(out.clone()),
                };
                self.unify(&args[0], &expected_fn, span)?;
                let iter_out = Type::UnresolvedGeneric {
                    name: "Iterator".to_string(),
                    args: vec![GenericArg::Type(Box::new(out))],
                };
                self.unify(ret, &iter_out, span)
            }
            "filter" if args.len() == 1 => {
                let expected_fn = Type::Fn {
                    params: vec![elem],
                    ret: Box::new(Type::Bool),
                };
                self.unify(&args[0], &expected_fn, span)?;
                self.unify(ret, &self_ty, span)
            }
            "fold" if args.len() == 2 => {
                let acc = args[0].clone();
                let expected_fn = Type::Fn {
                    params: vec![acc.clone(), elem],
                    ret: Box::new(acc.clone()),
                };
                self.unify(&args[1], &expected_fn, span)?;
                self.unify(ret, &acc, span)
            }
            "take" | "skip" if args.len() == 1 => {
                self.unify(&args[0], &Type::U64, span)?;
                self.unify(ret, &self_ty, span)
            }
            "enumerate" if args.is_empty() => {
                let pair = Type::Tuple(vec![Type::U64, elem]);
                let iter_pairs = Type::UnresolvedGeneric {
                    name: "Iterator".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair))],
                };
                self.unify(ret, &iter_pairs, span)
            }
            "any" | "all" if args.len() == 1 => {
                let expected_fn = Type::Fn {
                    params: vec![elem],
                    ret: Box::new(Type::Bool),
                };
                self.unify(&args[0], &expected_fn, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            "find" if args.len() == 1 => {
                let expected_fn = Type::Fn {
                    params: vec![elem.clone()],
                    ret: Box::new(Type::Bool),
                };
                self.unify(&args[0], &expected_fn, span)?;
                self.unify(ret, &Type::option(elem), span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    pub(super) fn resolve_char_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        if let Some(method_def) = rask_stdlib::lookup_method("char", method) {
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            let ret_ty = super::builtins::parse_stub_type(&method_def.ret_ty);
            return self.unify(ret, &ret_ty, span);
        }

        match method {
            // `char` is Comparable in Unicode scalar order (type.operators
            // ORD1 and the support table). Only `eq`/`ne` were wired up, so
            // `'a' < 'b'` reported "no method lt found for type char".
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" if args.len() == 1 => {
                self.unify(&args[0], &Type::Char, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            "compare" if args.len() == 1 => {
                self.unify(&args[0], &Type::Char, span)?;
                self.unify(ret, &self.ordering_type(), span)
            }
            "hash" if args.is_empty() => self.unify(ret, &Type::U64, span),
            // CH3: runtime construction returns `char?` — `none` on invalid scalar.
            "from_u32" if args.len() == 1 => {
                self.unify(&args[0], &Type::U32, span)?;
                self.unify(ret, &Type::option(Type::Char), span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::Char,
                method: method.to_string(),
                span,
            }),
        }
    }

    pub(super) fn resolve_string_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        written: &[Type],
        span: Span,
    ) -> Result<bool, TypeError> {
        if let Some(method_def) = rask_stdlib::lookup_method("string", method) {
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            // `string` is not generic, so a bare single-uppercase name in its
            // stub signature is the method's own type parameter (PC3) — as in
            // `parse<T>(self) -> T or ParseError`. Left as a named type it made
            // every parse yield the literal type `T`, so `const x: f64 =
            // s.parse()` never learned the target and ran the integer parse
            // (#480). A fresh var lets the call site decide.
            // A type argument written at the call binds the method's own
            // parameter; `freshen_free_type_params` only invents a variable for
            // the ones nothing named. `parse_stub_type` has already rewritten a
            // single-uppercase name to `_Any`, so that is the key to seed.
            let mut seen = std::collections::HashMap::new();
            if let Some(first) = written.first() {
                seen.insert("_Any".to_string(), first.clone());
            }
            let ret_ty = self.freshen_free_type_params(
                &super::builtins::parse_stub_type(&method_def.ret_ty),
                &mut seen,
            );
            return self.unify(ret, &ret_ty, span);
        }

        match method {
            "add" => return Err(TypeError::StringAddForbidden { span }),
            // `a == b` desugars to `a.eq(b)` and there's no `eq` in the string
            // stubs, so this fell through as "no progress" and the result type
            // stayed open. In a condition the `Equal(cond, bool)` constraint hid
            // it; bound to a name (`let same = a == b`) there was nothing to pin
            // it and the binding was reported as un-inferrable (#620). Ordering
            // is lexicographic and both backends already do it.
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "contains" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            // std.strings/S7: `string` has no mutation methods. These two were
            // accepted anyway, and they couldn't be made sound — `string` is an
            // immutable 16-byte value that shares its buffer with every copy,
            // so the backends disagreed about whether a push was visible
            // through the other copies (#693). StringBuilder is the mutable
            // one.
            //
            // `concat` used to sit here too. It's gone on main, which matches
            // the spec: interpolation is the one way to combine strings.
            "push" | "push_str" | "push_char" | "push_byte" | "insert" | "clear" | "truncate" => {
                Err(TypeError::StringIsImmutable { method: method.to_string(), span })
            }
            // Almost always the opening line of the same mistake, so it gets
            // its own message rather than a bare "no method `new`".
            "new" if args.is_empty() => Err(TypeError::StringNewRemoved { span }),
            // `string` is Comparable, same as `char`. This had no signature —
            // it only worked because the fallthrough below used to accept any
            // name at all.
            "compare" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &self.ordering_type(), span)
            }
            // A method `string` doesn't have is an error here, the way it
            // already was for `char`. Answering `Ok(false)` accepted any name
            // at all and left the return type open, so `s.frobnicate()`
            // type-checked and MIR gave the temp `i64` — which is how
            // `part.to_owned()` compiled and then segfaulted instead of
            // saying the method doesn't exist.
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::String,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Methods that change how many elements a sequence holds.
    ///
    /// A `Vec` has them all; a fixed array has none of them, because its length
    /// is written in its type. Sharing the `Vec` method table below is what let
    /// `[i32; 3].push(4)` type-check (#901), so the array receiver is filtered
    /// against this list before the shared lookup runs.
    fn changes_length(method: &str) -> bool {
        matches!(
            method,
            "push" | "pop" | "push_all" | "insert" | "insert_at" | "remove" | "remove_at"
            | "remove_where" | "take_where" | "clear" | "truncate" | "resize"
            | "reserve" | "shrink_to_fit" | "with_capacity" | "try_insert" | "try_push"
        )
    }

    pub(super) fn resolve_array_method(
        &mut self,
        array_ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // A slice is a view and has no growth surface either, but it reaches
        // this function as `Type::Slice` and never had the `Vec` fallback wired
        // up, so only the fixed array needs rejecting here.
        if matches!(array_ty, Type::Array { .. }) && Self::changes_length(method) {
            return Err(TypeError::FixedArrayGrowth {
                method: method.to_string(),
                array: array_ty.clone(),
                span,
            });
        }

        if let Some(method_def) = rask_stdlib::lookup_method("Vec", method) {
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            let ret_ty = super::builtins::parse_stub_type(&method_def.ret_ty);
            return self.unify(ret, &ret_ty, span);
        }

        match method {
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "push" => self.unify(ret, &Type::Unit, span),
            "pop" => {
                let elem_ty = self.ctx.fresh_var();
                self.unify(ret, &Type::option(elem_ty), span)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn resolve_file_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // File methods return Result types (T or IoError)
        let io_error_ty = Type::UnresolvedNamed("IoError".to_string());

        match method {
            "read_text" if args.is_empty() => {
                // Returns string or IoError
                let result_type = Type::Result {
                    ok: Box::new(Type::String),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "read_bytes" if args.is_empty() => {
                // Returns Vec<u8> or IoError
                let bytes_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(Type::U8))],
                };
                let result_type = Type::Result {
                    ok: Box::new(bytes_ty),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "close" if args.is_empty() => {
                // Returns () or IoError (takes self)
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write_text" if args.len() == 1 => {
                // write_text(data: string) -> () or IoError
                self.unify(&args[0], &Type::String, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write_bytes" if args.len() == 1 => {
                // write_bytes(data: Vec<u8>) -> () or IoError
                let bytes_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(Type::U8))],
                };
                self.unify(&args[0], &bytes_ty, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write" if args.len() == 1 => {
                // write(data: string) -> usize or IoError
                self.unify(&args[0], &Type::String, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::U64),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write_line" if args.len() == 1 => {
                // write_line(data: string) -> () or IoError
                self.unify(&args[0], &Type::String, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "lines" if args.is_empty() => {
                // Returns Vec<string> or IoError
                let vec_string = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(Type::String))],
                };
                let result_type = Type::Result {
                    ok: Box::new(vec_string),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::UnresolvedNamed("File".to_string()),
                method: method.to_string(),
                span,
            }),
        }
    }

    pub(super) fn resolve_thread_handle_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // ThreadHandle<T> has two methods:
        // - join(self) -> T or JoinError
        // - detach(self) -> ()

        match method {
            "join" if args.is_empty() => {
                // Extract the T type parameter
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };

                // join returns Result<T, JoinError>
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::UnresolvedNamed("JoinError".to_string())),
                };

                self.unify(ret, &result_type, span)
            }
            "detach" if args.is_empty() => {
                // detach returns ()
                self.unify(ret, &Type::Unit, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::UnresolvedGeneric {
                    name: "ThreadHandle".to_string(),
                    args: type_args.to_vec(),
                },
                method: method.to_string(),
                span,
            }),
        }
    }

    pub(super) fn resolve_task_handle_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "join" if args.is_empty() => {
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::UnresolvedNamed("JoinError".to_string())),
                };
                self.unify(ret, &result_type, span)
            }
            "detach" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            "cancel" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::UnresolvedGeneric {
                    name: "TaskHandle".to_string(),
                    args: type_args.to_vec(),
                },
                method: method.to_string(),
                span,
            }),
        }
    }

    pub(super) fn resolve_runtime_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let error_ty = Type::UnresolvedNamed("IoError".to_string());
        match (type_name, method) {
            // Instant static constructor and instance methods
            ("Instant", "now") if args.is_empty() => {
                self.unify(ret, &Type::UnresolvedNamed("Instant".to_string()), span)
            }
            ("Instant", "elapsed") if args.is_empty() => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }
            // Duration methods
            ("Duration", "as_seconds_f64") if args.is_empty() => {
                self.unify(ret, &Type::F64, span)
            }
            ("Duration", "as_nanos") if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            ("Duration", "as_seconds") if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            ("Duration", "from_nanos") if args.len() == 1 => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }
            ("Duration", "from_millis") if args.len() == 1 => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }

            // Instant arithmetic: instant + duration -> Instant
            ("Instant", "add") if args.len() == 1 => {
                let duration_ty = Type::UnresolvedNamed("Duration".to_string());
                self.unify(&args[0], &duration_ty, span)?;
                self.unify(ret, &Type::UnresolvedNamed("Instant".to_string()), span)
            }
            // Instant subtraction: overloaded on argument type
            //   instant - instant -> Duration
            //   instant - duration -> Instant
            ("Instant", "sub") if args.len() == 1 => {
                let arg = self.ctx.apply(&args[0]);
                // The RHS can show up three ways depending on where it came
                // from: `UnresolvedNamed("Instant")` fresh off a call chain,
                // `UnresolvedNamed("time.Instant")` off a module-qualified
                // parameter annotation, or `Named(id)` once an ordinary
                // variable gets fully resolved. Only the first matched below,
                // so `end - start` reported "expected Instant, found Instant"
                // (or "found time.Instant") for the other two. `resolve_named`
                // strips the module qualifier to a real type; `nameable` turns
                // that back into the plain name string this match wants.
                let arg = self.nameable(&self.resolve_named(&arg));
                match &arg {
                    Type::UnresolvedNamed(n) if n == "Instant" => {
                        self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
                    }
                    Type::UnresolvedNamed(n) if n == "Duration" => {
                        self.unify(ret, &Type::UnresolvedNamed("Instant".to_string()), span)
                    }
                    Type::Var(_) => {
                        // Argument type not yet resolved — defer
                        self.ctx.add_constraint(TypeConstraint::HasMethod {
                            ty: Type::UnresolvedNamed(type_name.to_string()),
                            method: method.to_string(),
                            args: args.to_vec(),
                            ret: ret.clone(),
                            span,
                            call_node: None,
                        });
                        Ok(false)
                    }
                    _ => Err(TypeError::Mismatch {
                        expected: Type::UnresolvedNamed("Instant".to_string()),
                        found: arg.clone(),
                        span,
                    }),
                }
            }
            // Instant comparisons
            ("Instant", "eq" | "lt" | "le" | "gt" | "ge") if args.len() == 1 => {
                let instant_ty = Type::UnresolvedNamed("Instant".to_string());
                self.unify(&args[0], &instant_ty, span)?;
                self.unify(ret, &Type::Bool, span)
            }

            // Duration arithmetic: duration +/- duration -> Duration
            ("Duration", "add" | "sub") if args.len() == 1 => {
                let duration_ty = Type::UnresolvedNamed("Duration".to_string());
                self.unify(&args[0], &duration_ty, span)?;
                self.unify(ret, &duration_ty, span)
            }
            // Duration comparisons
            ("Duration", "eq" | "lt" | "le" | "gt" | "ge") if args.len() == 1 => {
                let duration_ty = Type::UnresolvedNamed("Duration".to_string());
                self.unify(&args[0], &duration_ty, span)?;
                self.unify(ret, &Type::Bool, span)
            }

            // TcpListener
            ("TcpListener", "accept") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::UnresolvedNamed("TcpConnection".to_string())),
                    err: Box::new(error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            // TcpConnection
            ("TcpConnection", "read_http_request") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::UnresolvedNamed("Request".to_string())),
                    err: Box::new(error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            ("TcpConnection", "write_http_response") if args.len() == 1 => {
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            // Response — allow method-style access for chaining
            ("Response", "status") if args.is_empty() => {
                self.unify(ret, &Type::U16, span)
            }
            // `Shared.new/mutex/local(value)` — the constructor settles the
            // strategy, and it lands in the type so a reader of the annotation
            // sees the same thing (conc.sync/SH2).
            ("Shared", "new" | "mutex" | "local") if args.len() == 1 => {
                let inner = args[0].clone();
                let shared_ty = Self::shared_type(inner, method);
                self.unify(ret, &shared_ty, span)
            }
            // `Cell` is the `Local` strategy now (analysis.storage-consolidation).
            ("Cell", _) => {
                self.errors.push(TypeError::RetiredBoxType {
                    name: "Cell".to_string(),
                    replacement: "Shared.new(…)` / `Shared<T>".to_string(),
                    span,
                });
                Ok(true)
            }
            _ => {
                // Try stub registry before falling through
                if let Some(stub) = rask_stdlib::lookup_method(type_name, method) {
                    let expected_params = stub.params.len();
                    if args.len() != expected_params {
                        return Err(TypeError::ArityMismatch {
                            expected: expected_params,
                            found: args.len(),
                            span,
                        });
                    }
                    for ((_, param_ty_str), arg) in stub.params.iter().zip(args.iter()) {
                        let param_ty = super::builtins::parse_stub_type(param_ty_str);
                        self.unify(arg, &param_ty, span)?;
                    }
                    let ret_ty = super::builtins::parse_stub_type(&stub.ret_ty);
                    return self.unify(ret, &ret_ty, span);
                }
                // Known runtime type but unknown method — hard error
                Err(TypeError::NoSuchMethod {
                    ty: Type::UnresolvedNamed(type_name.to_string()),
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// `Shared<T, S>` for the strategy a constructor names. `new` is `Readers`,
    /// which is what bare `Shared<T>` means everywhere (SH3) — the default
    /// serves the common case, which is a box several tasks reach.
    fn shared_type(inner: Type, constructor: &str) -> Type {
        let strategy = match constructor {
            "local" => "Local",
            "mutex" => "Mutex",
            _ => "Readers",
        };
        Type::UnresolvedGeneric {
            name: "Shared".to_string(),
            args: vec![
                GenericArg::Type(Box::new(inner)),
                GenericArg::Type(Box::new(Type::UnresolvedNamed(strategy.to_string()))),
            ],
        }
    }

    pub(super) fn resolve_concurrency_generic_method(
        &mut self,
        type_name: &str,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // Extract inner type T from generic args
        let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        // What to hand back on a constructed `Sender<T>`/`Receiver<T>`/`Mutex<T>`.
        // Written-out arguments pass through; when there are none this is the
        // fresh variable from just above, so every piece of one channel shares a
        // type instead of each call inventing its own and dropping it (#717).
        let inner_args = if type_args.is_empty() {
            vec![GenericArg::Type(Box::new(inner_type.clone()))]
        } else {
            type_args.to_vec()
        };

        match (type_name, method) {

            // Shared<T>.read() -> T  (inline access, E5/R5)
            ("Shared", "read") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Shared<T>.write() -> T  (inline access, E5/R5)
            ("Shared", "write") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Shared<T>.read(|T| -> R) -> R  (closure-based, try_read)
            ("Shared", "read") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Shared<T>.write(|T| -> R) -> R  (closure-based, try_write)
            ("Shared", "write") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Shared<T>.staged() -> T  (ST1: a working copy under the
            // exclusive lock, committed as one move on any non-panic exit)
            ("Shared", "staged") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Shared<T>.try_read(|T| -> R) -> Option<R>  (non-blocking, R3)
            ("Shared", "try_read") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                let opt_ty = Type::option(result_var);
                self.unify(ret, &opt_ty, span)
            }
            // Shared<T>.try_write(|T| -> R) -> Option<R>  (non-blocking, R3)
            ("Shared", "try_write") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                let opt_ty = Type::option(result_var);
                self.unify(ret, &opt_ty, span)
            }
            // The single-expression shorthands `Cell` had (conc.sync API table).
            ("Shared", "get" | "into_inner") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            ("Shared", "set") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &Type::Unit, span)
            }
            ("Shared", "replace") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &inner_type, span)
            }
            // Shared<T>.clone() -> Shared<T>
            ("Shared", "clone") if args.is_empty() => {
                let shared_ty = Type::UnresolvedGeneric {
                    name: "Shared".to_string(),
                    args: inner_args.clone(),
                };
                self.unify(ret, &shared_ty, span)
            }
            // `Cell` and `Mutex` are strategies now, not types
            // (analysis.storage-consolidation). Say so where the old spelling
            // is used, rather than letting it fail as an unknown name.
            ("Cell", _) => {
                self.errors.push(TypeError::RetiredBoxType {
                    name: "Cell".to_string(),
                    replacement: format!("Shared.new(…)` / `Shared<T>"),
                    span,
                });
                Ok(true)
            }
            ("Mutex", "lock" | "try_lock" | "clone" | "new") => {
                self.errors.push(TypeError::RetiredBoxType {
                    name: "Mutex".to_string(),
                    replacement: format!("Shared.mutex(…)` / `Shared<T, Mutex>"),
                    span,
                });
                Ok(true)
            }
            // Sender<T>.send(value: T) -> () or string
            ("Sender", "send") if args.len() == 1 => {
                // T1: record the call so ownership transfers the sent value.
                self.channel_send_sites.insert(span);
                let _ = self.unify(&args[0], &inner_type, span);
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Sender<T>.try_send(value: T) -> () or string
            ("Sender", "try_send") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Sender<T>.close() -> () or string
            ("Sender", "close") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Sender<T>.clone() -> Sender<T>
            ("Sender", "clone") if args.is_empty() => {
                let sender_ty = Type::UnresolvedGeneric {
                    name: "Sender".to_string(),
                    args: inner_args.clone(),
                };
                self.unify(ret, &sender_ty, span)
            }
            // Receiver<T>.receive() -> T or string
            ("Receiver", "receive") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Receiver<T>.try_receive() -> T or string
            ("Receiver", "try_receive") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Receiver<T>.close() -> () or string
            ("Receiver", "close") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Channel<T>.buffered(n) -> (Sender<T>, Receiver<T>)
            ("Channel", "buffered") if args.len() == 1 => {
                let sender = Type::UnresolvedGeneric {
                    name: "Sender".to_string(),
                    args: inner_args.clone(),
                };
                let receiver = Type::UnresolvedGeneric {
                    name: "Receiver".to_string(),
                    args: inner_args.clone(),
                };
                let tuple_ty = Type::Tuple(vec![sender, receiver]);
                self.unify(ret, &tuple_ty, span)
            }
            // Channel<T>.unbuffered() -> (Sender<T>, Receiver<T>)
            ("Channel", "unbuffered") if args.is_empty() => {
                let sender = Type::UnresolvedGeneric {
                    name: "Sender".to_string(),
                    args: inner_args.clone(),
                };
                let receiver = Type::UnresolvedGeneric {
                    name: "Receiver".to_string(),
                    args: inner_args.clone(),
                };
                let tuple_ty = Type::Tuple(vec![sender, receiver]);
                self.unify(ret, &tuple_ty, span)
            }
            // Written `Shared.new(0)` the element type comes from the value, so
            // pin the variable to it. The strategy comes from the constructor.
            ("Shared", "new" | "mutex" | "local") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let shared_ty = Self::shared_type(inner_type.clone(), method);
                self.unify(ret, &shared_ty, span)
            }
            // Unrecognized method: hand the receiver on exactly as written
            // rather than inventing an argument the solver never asked for.
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::UnresolvedGeneric {
                        name: type_name.to_string(),
                        args: type_args.to_vec(),
                    },
                    method: method.to_string(),
                    args: args.to_vec(),
                    ret: ret.clone(),
                    span,
                    call_node: None,
                });
                Ok(false)
            }
        }
    }

    /// Resolve methods on Pool<T> instances.
    pub(super) fn resolve_pool_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        match method {
            // pool.insert(value: T) -> Handle<T> (panics on failure, like Vec.push)
            "alloc" | "insert" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &handle_ty, span)
            }
            // pool.get(h: Handle<T>) -> T?
            "get" if args.len() == 1 => {
                let result_ty = Type::option(inner_type);
                self.unify(ret, &result_ty, span)
            }
            // pool.remove(h: Handle<T>) -> T?
            "remove" if args.len() == 1 => {
                let result_ty = Type::option(inner_type);
                self.unify(ret, &result_ty, span)
            }
            // pool.len() -> u64
            "len" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // pool.is_empty() -> bool
            "is_empty" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            // pool.handles() -> Vec<Handle<T>>
            "handles" if args.is_empty() => {
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(handle_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // pool.contains(h: Handle<T>) -> bool
            "contains" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // pool.clear() -> ()
            "clear" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // pool.get_mut(h) -> T?
            "get_mut" | "get_clone" if args.len() == 1 => {
                let result_ty = Type::option(inner_type);
                self.unify(ret, &result_ty, span)
            }
            // pool.try_insert(value: T) -> Handle<T>?
            "try_insert" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                let opt_ty = Type::option(handle_ty);
                self.unify(ret, &opt_ty, span)
            }
            // pool.drain() -> Vec<T>
            "drain" | "take_all" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // pool.entries() -> Vec<(Handle<T>, T)>
            "entries" if args.is_empty() => {
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type.clone()))],
                };
                let pair_ty = Type::Tuple(vec![handle_ty, inner_type]);
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // pool.get_unchecked(h) -> T, pool.get_mut_unchecked(h) -> T
            "get_unchecked" | "get_mut_unchecked" if args.len() == 1 => {
                self.unify(ret, &inner_type, span)
            }
            // pool.read(h, closure) -> R?, pool.modify(h, closure) -> R?
            // pool.with_valid(h, closure) -> R?, pool.with_valid_mut(h, closure) -> R?
            "read" | "modify" | "with_valid" | "with_valid_mut" if args.len() == 2 => {
                let result_ty = Type::option(self.ctx.fresh_var());
                self.unify(ret, &result_ty, span)
            }
            // pool.capacity() -> u64, pool.remaining() -> u64
            "capacity" | "remaining" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // pool.weak(h: Handle<T>) -> WeakHandle<T>
            "weak" if args.len() == 1 => {
                let weak_ty = Type::UnresolvedGeneric {
                    name: "WeakHandle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &weak_ty, span)
            }
            // pool.snapshot() -> (Pool<T>, Pool<T>)
            "snapshot" if args.is_empty() => {
                let pool_ty = Type::UnresolvedGeneric {
                    name: "Pool".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type.clone()))],
                };
                let pair_ty = Type::Tuple(vec![pool_ty.clone(), pool_ty]);
                self.unify(ret, &pair_ty, span)
            }
            // pool.clone() -> Pool<T>
            "clone" if args.is_empty() => {
                let pool_ty = Type::UnresolvedGeneric {
                    name: "Pool".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &pool_ty, span)
            }
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::UnresolvedGeneric {
                        name: "Pool".to_string(),
                        args: type_args.to_vec(),
                    },
                    method: method.to_string(),
                    args: args.to_vec(),
                    ret: ret.clone(),
                    span,
                    call_node: None,
                });
                Ok(false)
            }
        }
    }

    /// `Rack<T>` methods (analysis.fourth-option).
    ///
    /// Note what is absent: no `get`. A pool hands out handles that must be
    /// redeemed at the pool; a rack hands out links that are followed
    /// directly, so the only container-level operations left are structural.
    pub(super) fn resolve_rack_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let node_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };
        let link_ty = Type::UnresolvedGeneric {
            name: "Link".to_string(),
            args: vec![GenericArg::Type(Box::new(node_type.clone()))],
        };

        match method {
            // rack.insert(node: T) -> Link<T>
            "insert" if args.len() == 1 => {
                let _ = self.unify(&args[0], &node_type, span);
                self.unify(ret, &link_ty, span)
            }
            // rack.delete(l: Link<T>) -> ()
            // Every edge pointing at the node becomes `none` here.
            "delete" if args.len() == 1 => self.unify(ret, &Type::Unit, span),
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "contains" if args.len() == 1 => self.unify(ret, &Type::Bool, span),
            "clear" if args.is_empty() => self.unify(ret, &Type::Unit, span),
            // rack.nodes() -> Vec<Link<T>>
            "nodes" | "links" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(link_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::UnresolvedGeneric {
                        name: "Rack".to_string(),
                        args: type_args.to_vec(),
                    },
                    method: method.to_string(),
                    args: args.to_vec(),
                    ret: ret.clone(),
                    span,
                    call_node: None,
                });
                Ok(false)
            }
        }
    }

    /// Resolve static methods on bare Pool (e.g. Pool.new()).
    pub(super) fn resolve_pool_static_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            // Pool.new() -> Pool<T> where T is fresh
            "new" if args.is_empty() => {
                let fresh = self.ctx.fresh_var();
                let pool_ty = Type::UnresolvedGeneric {
                    name: "Pool".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &pool_ty, span)
            }
            _ => {
                Err(TypeError::NoSuchMethod {
                    ty: Type::UnresolvedNamed("Pool".to_string()),
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// Resolve static methods on bare Vec (e.g. Vec.new()).
    pub(super) fn resolve_vec_static_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "new" if args.is_empty() => {
                let fresh = self.ctx.fresh_var();
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // Vec.from(array) — construct Vec from array literal
            "from" if args.len() == 1 => {
                // Extract element type from the argument (array literal or Vec)
                // and produce Vec<T>.
                let elem_ty = match &args[0] {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    Type::UnresolvedGeneric { name, args: type_args } if name == "Vec" => {
                        if let Some(GenericArg::Type(t)) = type_args.first() {
                            *t.clone()
                        } else {
                            self.ctx.fresh_var()
                        }
                    }
                    Type::Generic { args: type_args, .. } => {
                        if let Some(GenericArg::Type(t)) = type_args.first() {
                            *t.clone()
                        } else {
                            self.ctx.fresh_var()
                        }
                    }
                    _ => self.ctx.fresh_var(),
                };
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(elem_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            _ => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(self.ctx.fresh_var()))],
                };
                Err(TypeError::NoSuchMethod {
                    ty: vec_ty,
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// Resolve instance methods on Vec<T>.
    pub(super) fn resolve_vec_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        let self_ty = Type::UnresolvedGeneric {
            name: "Vec".to_string(),
            args: type_args.to_vec(),
        };

        match method {
            // The element slot is an argument position like any other, so it
            // widens a bare `T` into a `T?` element. Plain unify accepted the
            // bare value without recording the coercion, so `Vec<i32?>` stored
            // raw ints — every read then came back absent natively and as a
            // bare i64 on the interpreter.
            "push" if args.len() == 1 => {
                let _ = self.coerce_arg(&inner_type, &args[0], span);
                self.unify(ret, &Type::Unit, span)
            }
            "pop" if args.is_empty() => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            "len" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            "get" if args.len() == 1 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            "set" if args.len() == 2 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                let _ = self.coerce_arg(&inner_type, &args[1], span);
                self.unify(ret, &Type::Unit, span)
            }
            "clear" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            "is_empty" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "capacity" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // vec.insert(index, value) -> ()
            "insert" if args.len() == 2 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                let _ = self.coerce_arg(&inner_type, &args[1], span);
                self.unify(ret, &Type::Unit, span)
            }
            // vec.remove(index) -> T
            "remove" if args.len() == 1 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                self.unify(ret, &inner_type, span)
            }
            // vec.chunks(size) -> Vec<Vec<T>>
            "chunks" if args.len() == 1 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                let chunk_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(chunk_ty))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.to_vec() -> Vec<T>
            "to_vec" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            "iter" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            "skip" if args.len() == 1 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                self.unify(ret, &self_ty, span)
            }
            "take" if args.len() == 1 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                self.unify(ret, &self_ty, span)
            }
            "limit" if args.len() == 1 => {
                self.check_integer_arg(&self_ty, &args[0], span);
                self.unify(ret, &self_ty, span)
            }
            // vec.first() -> Option<T>
            "first" if args.is_empty() => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            // vec.last() -> Option<T>
            "last" if args.is_empty() => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            // vec.contains(value) -> bool
            "contains" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &Type::Bool, span)
            }
            // vec.reverse() -> ()
            "reverse" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.join(sep) -> string
            "join" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::String, span);
                self.unify(ret, &Type::String, span)
            }
            // vec.sort() -> ()
            "sort" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.sort_by(comparator) -> ()
            "sort_by" if args.len() == 1 => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.sort_by_key(key_fn) -> ()
            //
            // The key type comes from the closure. Leaving it free was harmless
            // while the body was native — nothing read it — and fatal once
            // `sort_by_key` was written in Rask over `sort_by`: the body compares
            // two keys, and an unresolved key type reached MIR as `compare` on a
            // receiver with no type at all (#887).
            "sort_by_key" if args.len() == 1 => {
                let key = self.ctx.fresh_var();
                let _ = self.unify(
                    &args[0],
                    &Type::Fn {
                        params: vec![inner_type.clone()],
                        ret: Box::new(key),
                    },
                    span,
                );
                self.unify(ret, &Type::Unit, span)
            }
            // vec.remove_adjacent_duplicates() -> ()
            "remove_adjacent_duplicates" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.filter(predicate) -> Vec<T>
            "filter" if args.len() == 1 => {
                self.unify(ret, &self_ty, span)
            }
            // vec.map(transform) -> Vec<U>. U is the closure's return type —
            // without tying the two together the element stayed an unbound var
            // and `doubled[0] == 2` reported "no method eq for type U" (#327).
            "map" if args.len() == 1 => {
                let fresh = self.ctx.fresh_var();
                let expected_fn = Type::Fn {
                    params: vec![inner_type],
                    ret: Box::new(fresh.clone()),
                };
                let _ = self.unify(&args[0], &expected_fn, span);
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.flat_map(transform) -> Vec<U>, where the closure hands back a
            // Vec<U> per element.
            "flat_map" if args.len() == 1 => {
                let fresh = self.ctx.fresh_var();
                let inner_vec = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh.clone()))],
                };
                let expected_fn = Type::Fn {
                    params: vec![inner_type],
                    ret: Box::new(inner_vec),
                };
                let _ = self.unify(&args[0], &expected_fn, span);
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.flatten() -> Vec<T>
            "flatten" if args.is_empty() => {
                let fresh = self.ctx.fresh_var();
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.fold(init, f) -> U, with f: func(U, T) -> U
            "fold" if args.len() == 2 => {
                let acc = args[0].clone();
                let expected_fn = Type::Fn {
                    params: vec![acc.clone(), inner_type],
                    ret: Box::new(acc.clone()),
                };
                let _ = self.unify(&args[1], &expected_fn, span);
                let _ = self.unify(ret, &acc, span);
                Ok(true)
            }
            // vec.reduce(f) -> Option<T>
            "reduce" if args.len() == 1 => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            // vec.enumerate() -> Vec<(i64, T)>
            "enumerate" if args.is_empty() => {
                let pair_ty = Type::Tuple(vec![Type::I64, inner_type]);
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair_ty))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.zip(other) -> Vec<(T, U)>. U never had anywhere to come
            // from: `other` is `Vec<U>` and nothing tied the fresh var to its
            // element, so U stayed unbound and the mangled name carried it as
            // a bare, unresolved type parameter (#887).
            "zip" if args.len() == 1 => {
                let fresh = self.ctx.fresh_var();
                let other_vec = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh.clone()))],
                };
                let _ = self.unify(&args[0], &other_vec, span);
                let pair_ty = Type::Tuple(vec![inner_type, fresh]);
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair_ty))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.any(predicate) -> bool
            "any" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // vec.all(predicate) -> bool
            "all" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // vec.find(predicate) -> Option<T>
            "find" if args.len() == 1 => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            // vec.position(predicate) -> Option<i64>
            "position" if args.len() == 1 => {
                let opt_ty = Type::option(Type::I64);
                self.unify(ret, &opt_ty, span)
            }
            // vec.count() -> u64
            "count" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // vec.take_all() -> Vec<T> (consuming iteration)
            "take_all" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            // vec.sum() -> T
            "sum" if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // vec.min() -> Option<T>
            "min" if args.is_empty() => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            // vec.max() -> Option<T>
            "max" if args.is_empty() => {
                let opt_ty = Type::option(inner_type);
                self.unify(ret, &opt_ty, span)
            }
            // vec.clone() -> Vec<T>
            "clone" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            // vec.eq(other) -> bool
            "eq" | "ne" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // vec.as_ptr() / vec.as_mut_ptr() -> *T. Both name the same buffer
            // address; the `mutate self` on as_mut_ptr is what allows writing.
            // Reached when the receiver is still the unresolved `Vec<T>` shape
            // rather than the registered type — the registered path finds these
            // in Vec's own method table.
            "as_ptr" | "as_mut_ptr" if args.is_empty() => {
                self.unify(ret, &Type::RawPtr(Box::new(inner_type)), span)
            }
            // Fall through to static methods (e.g. Vec<Route>.from(...))
            _ => self.resolve_vec_static_method(method, args, ret, span),
        }
    }

    /// Resolve static methods on bare Map (e.g. Map.new()).
    pub(super) fn resolve_map_static_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "new" if args.is_empty() => {
                let fresh_k = self.ctx.fresh_var();
                let fresh_v = self.ctx.fresh_var();
                let map_ty = Type::UnresolvedGeneric {
                    name: "Map".to_string(),
                    args: vec![
                        GenericArg::Type(Box::new(fresh_k)),
                        GenericArg::Type(Box::new(fresh_v)),
                    ],
                };
                self.unify(ret, &map_ty, span)
            }
            // Map.from(vec_of_pairs) — construct Map from iterable
            "from" if args.len() == 1 => {
                let fresh_k = self.ctx.fresh_var();
                let fresh_v = self.ctx.fresh_var();
                let map_ty = Type::UnresolvedGeneric {
                    name: "Map".to_string(),
                    args: vec![
                        GenericArg::Type(Box::new(fresh_k)),
                        GenericArg::Type(Box::new(fresh_v)),
                    ],
                };
                self.unify(ret, &map_ty, span)
            }
            _ => {
                let map_ty = Type::UnresolvedGeneric {
                    name: "Map".to_string(),
                    args: vec![
                        GenericArg::Type(Box::new(self.ctx.fresh_var())),
                        GenericArg::Type(Box::new(self.ctx.fresh_var())),
                    ],
                };
                Err(TypeError::NoSuchMethod {
                    ty: map_ty,
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// Check one argument against the type the receiver's type arguments say it
    /// must be, and *report* a mismatch.
    ///
    /// The Map methods used to write `let _ = self.unify(&args[0], &key_type, …)`,
    /// so a wrong key was unified and the failure dropped. On a `Map<TaskId, string>`
    /// that let `m.insert(1, "a")` through — a raw `i32` into a nominal slot, which
    /// T9 exists to forbid — and on a struct key it let anything through (#812).
    fn check_arg_against(&mut self, arg: &Type, expected: &Type, span: Span) {
        if let Err(e) = self.unify(arg, expected, span) {
            self.errors.push(e);
        }
    }

    /// Resolve instance methods on Map<K, V>.
    pub(super) fn resolve_map_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let key_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };
        let val_type = if let Some(GenericArg::Type(t)) = type_args.get(1) {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        match method {
            "insert" if args.len() == 2 => {
                self.check_arg_against(&args[0], &key_type, span);
                self.check_arg_against(&args[1], &val_type, span);
                self.unify(ret, &Type::I64, span)
            }
            "contains_key" if args.len() == 1 => {
                self.check_arg_against(&args[0], &key_type, span);
                self.unify(ret, &Type::Bool, span)
            }
            "get" if args.len() == 1 => {
                self.check_arg_against(&args[0], &key_type, span);
                let opt_ty = Type::option(val_type);
                self.unify(ret, &opt_ty, span)
            }
            "remove" if args.len() == 1 => {
                self.check_arg_against(&args[0], &key_type, span);
                self.unify(ret, &Type::I64, span)
            }
            "len" if args.is_empty() => {
                self.unify(ret, &Type::I64, span)
            }
            "is_empty" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "clear" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            "keys" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(key_type))],
                };
                self.unify(ret, &vec_ty, span)
            }
            "values" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(val_type))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // Fall through to static methods (e.g. Map<K,V>.new())
            _ => self.resolve_map_static_method(method, args, ret, span),
        }
    }

    /// Resolve methods on Rng (both static and instance — no type params).
    /// Try to resolve a method call via the hardcoded builtin handlers
    /// using the type's name. Returns None if the name isn't a known builtin,
    /// meaning the caller should produce its own error.
    fn resolve_builtin_method_by_name(
        &mut self,
        type_name: &str,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Option<Result<bool, TypeError>> {
        // Strip generic params from name: "Vec<T>" → "Vec"
        let base_name = type_name.split('<').next().unwrap_or(type_name);
        match base_name {
            "Vec" if type_args.is_empty() => {
                Some(self.resolve_vec_static_method(method, args, ret, span))
            }
            "Vec" => {
                Some(self.resolve_vec_method(type_args, method, args, ret, span))
            }
            "Map" if type_args.is_empty() => {
                Some(self.resolve_map_static_method(method, args, ret, span))
            }
            "Map" => {
                Some(self.resolve_map_method(type_args, method, args, ret, span))
            }
            "Random" => Some(self.resolve_rng_method(method, args, ret, span)),
            "Atomic" => {
                let payload = match type_args.first() {
                    Some(GenericArg::Type(t)) => (**t).clone(),
                    _ => self
                        .atomic_payload(&Type::UnresolvedNamed(type_name.to_string()))
                        .unwrap_or_else(|| self.ctx.fresh_var()),
                };
                Some(self.resolve_atomic_method(payload, method, args, ret, span))
            }
            name if Self::is_simd_type(name) => {
                Some(self.resolve_simd_method(name, method, args, ret, span))
            }
            "Cell" | "Shared" | "Mutex" | "Sender" | "Receiver" | "Channel" if !type_args.is_empty() => {
                Some(self.resolve_concurrency_generic_method(type_name, type_args, method, args, ret, span))
            }
            // `Cell` and `Mutex` are here to be *rejected* with a message that
            // names the strategy that replaced them — without the arm they fall
            // through to "couldn't work out the type", which says nothing.
            name if matches!(name, "Instant" | "Duration" | "TcpListener" | "TcpConnection" | "Response" | "Request" | "Shared" | "Mutex" | "Cell")
                || rask_stdlib::StubRegistry::load().get_type(name).is_some() => {
                Some(self.resolve_runtime_method(name, method, args, ret, span))
            }
            _ => None,
        }
    }

    pub(super) fn resolve_rng_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let rng_ty = Type::UnresolvedNamed("Random".to_string());

        match method {
            "new" if args.is_empty() => {
                self.unify(ret, &rng_ty, span)
            }
            "from_seed" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &rng_ty, span)
            }
            "u64" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            "i64" if args.is_empty() => {
                self.unify(ret, &Type::I64, span)
            }
            "f64" if args.is_empty() => {
                self.unify(ret, &Type::F64, span)
            }
            "f32" if args.is_empty() => {
                self.unify(ret, &Type::F32, span)
            }
            "bool" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "range" if args.len() == 2 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let _ = self.unify(&args[1], &Type::I64, span);
                self.unify(ret, &Type::I64, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: rng_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// The payload written in `Atomic<T>`, if this is one.
    ///
    /// mem.atomics/GA1: `Atomic<T>` is the only spelling. The eleven
    /// `AtomicU64`-style names the registry used to carry are gone — the rule
    /// exists so a reader never has to wonder whether a named form and the
    /// generic form differ.
    fn atomic_payload(&mut self, ty: &Type) -> Option<Type> {
        match ty {
            Type::UnresolvedGeneric { name, args } if name == "Atomic" => match args.first() {
                Some(GenericArg::Type(t)) => Some((**t).clone()),
                _ => Some(self.ctx.fresh_var()),
            },
            // `Atomic.new(0)` with no written argument — the value settles it.
            Type::UnresolvedNamed(name) if name == "Atomic" => Some(self.ctx.fresh_var()),
            // A static call keeps its written arguments in the name.
            Type::UnresolvedNamed(name) if name.starts_with("Atomic<") => {
                let inner = name.strip_prefix("Atomic<")?.strip_suffix('>')?.trim();
                Some(crate::checker::parse_type_string(inner, &self.types).ok()?)
            }
            _ => None,
        }
    }

    /// GA2: why this payload can't go in an atomic, or `None` when it can.
    ///
    /// "One machine word" is the whole rule, and under Rask's layout that means
    /// a primitive, or a struct with a single word-sized field. A two-field
    /// struct is 16 bytes here however small its fields are written — every
    /// field gets a word — so `Atomic<Slot>` over `{ index: i32, gen: i32 }` is
    /// two words and the hardware has no single instruction for it.
    fn atomic_payload_problem(&self, ty: &Type) -> Option<String> {
        match self.resolve_named(ty) {
            // Still open — the value that settles it decides, and rejecting on
            // "don't know" would refuse code the checker later accepts.
            Type::Var(_) | Type::Error | Type::UnresolvedNamed(_) => None,
            Type::I8 | Type::I16 | Type::I32 | Type::I64
            | Type::U8 | Type::U16 | Type::U32 | Type::U64
            | Type::F32 | Type::F64 | Type::Bool | Type::Char => None,
            Type::I128 | Type::U128 => Some(
                "a 128-bit payload needs `target.has_atomic128`, which isn't wired up yet (AT7)"
                    .to_string(),
            ),
            Type::Named(id) => match self.types.get(id) {
                Some(TypeDef::Struct { fields, .. }) => match fields.len() {
                    1 => self.atomic_payload_problem(&fields[0].1).map(|why| {
                        format!("its one field can't go in an atomic either — {why}")
                    }),
                    n => Some(format!(
                        "it is {n} fields, so {} bytes; an atomic is one machine word (GA2)",
                        n * 8
                    )),
                },
                _ => Some("only a struct of word-sized data can be a payload (GA2)".to_string()),
            },
            other => Some(format!(
                "`{}` isn't something the hardware can read or write in one instruction (GA2)",
                self.render_type(&other)
            )),
        }
    }

    /// GA3: the fetch family exists where the payload can do arithmetic.
    fn is_countable_payload(ty: &Type) -> bool {
        matches!(
            ty,
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
                | Type::F32 | Type::F64
                | Type::Var(_)
        )
    }

    /// Resolve methods on `Atomic<T>` (mem.atomics).
    pub(super) fn resolve_atomic_method(
        &mut self,
        val_ty: Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let self_ty = Type::UnresolvedGeneric {
            name: "Atomic".to_string(),
            args: vec![GenericArg::Type(Box::new(val_ty.clone()))],
        };
        let ordering_ty = self.ordering_type();
        // GA2 is a rule about the type, so it's reported once at the type
        // rather than at every operation on it.
        if let Some(reason) = self.atomic_payload_problem(&val_ty) {
            return Err(TypeError::AtomicPayload {
                ty: val_ty,
                reason,
                span,
            });
        }

        match method {
            // ── Construction ────────────────────────────────
            "new" if args.len() == 1 => {
                let _ = self.unify(&args[0], &val_ty, span);
                self.unify(ret, &self_ty, span)
            }
            "default" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }

            // ── Load / Store / Swap ─────────────────────────
            "load" if args.len() == 1 => {
                let _ = self.unify(&args[0], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }
            "store" if args.len() == 2 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &Type::Unit, span)
            }
            "swap" if args.len() == 2 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }

            // ── Compare-and-Exchange ────────────────────────
            "compare_exchange" | "compare_exchange_weak" if args.len() == 4 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &val_ty, span);
                let _ = self.unify(&args[2], &ordering_ty, span);
                let _ = self.unify(&args[3], &ordering_ty, span);
                // `T or CasFailed<T>`, not `T or T`. Both sides carry a `T` —
                // the old value on success, the observed one on failure — so
                // without the wrapper every match arm was ambiguous and `r is
                // …` had no error type to name.
                let result_ty = Type::Result {
                    ok: Box::new(val_ty.clone()),
                    err: Box::new(Type::UnresolvedGeneric {
                        name: "CasFailed".to_string(),
                        args: vec![GenericArg::Type(Box::new(val_ty))],
                    }),
                };
                self.unify(ret, &result_ty, span)
            }

            // ── Integer fetch operations ────────────────────
            "fetch_add" | "fetch_sub" | "fetch_max" | "fetch_min"
                if args.len() == 2 && Self::is_countable_payload(&val_ty) =>
            {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }

            // ── Bitwise fetch (integers + bool) ─────────────
            "fetch_and" | "fetch_or" | "fetch_xor" | "fetch_nand" if args.len() == 2 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }

            // ── Non-atomic access ───────────────────────────
            "into_inner" if args.is_empty() => {
                self.unify(ret, &val_ty, span)
            }

            // GA3: adding two structs, or two bools, means nothing.
            "fetch_add" | "fetch_sub" | "fetch_max" | "fetch_min"
                if !Self::is_countable_payload(&val_ty) =>
            {
                Err(TypeError::NoSuchMethod {
                    ty: self_ty,
                    method: method.to_string(),
                    span,
                })
            }

            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Check whether a type name is a SIMD vector type.
    fn is_simd_type(name: &str) -> bool {
        matches!(
            name,
            "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
        )
    }

    /// Parse SIMD type name to (element Type, lane count).
    fn simd_elem_type(name: &str) -> (Type, usize) {
        match name {
            "f32x4" => (Type::F32, 4),
            "f32x8" => (Type::F32, 8),
            "f64x2" => (Type::F64, 2),
            "f64x4" => (Type::F64, 4),
            "i32x4" => (Type::I32, 4),
            "i32x8" => (Type::I32, 8),
            _ => (Type::F32, 4), // unreachable given is_simd_type guard
        }
    }

    /// Resolve methods on SIMD vector types (type.simd spec).
    pub(super) fn resolve_simd_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let (elem_ty, lanes) = Self::simd_elem_type(type_name);
        let self_ty = Type::UnresolvedNamed(type_name.to_string());
        let _vec_ty = Type::SimdVector {
            elem: Box::new(elem_ty.clone()),
            lanes,
        };

        match method {
            // ── Construction ────────────────────────────────
            // splat(scalar) → vec
            "splat" if args.len() == 1 => {
                let _ = self.unify(&args[0], &elem_ty, span);
                self.unify(ret, &self_ty, span)
            }
            // load(slice) → vec (static method)
            "load" if args.len() == 1 => {
                let slice_ty = Type::Slice(Box::new(elem_ty.clone()));
                let _ = self.unify(&args[0], &slice_ty, span);
                self.unify(ret, &self_ty, span)
            }

            // ── Memory ──────────────────────────────────────
            // store(slice) → ()
            "store" if args.len() == 1 => {
                let slice_ty = Type::Slice(Box::new(elem_ty.clone()));
                let _ = self.unify(&args[0], &slice_ty, span);
                self.unify(ret, &Type::Unit, span)
            }

            // ── Element-wise arithmetic ─────────────────────
            // add(other) → vec, sub(other) → vec, mul(other) → vec, div(other) → vec
            "add" | "sub" | "mul" | "div" if args.len() == 1 => {
                let _ = self.unify(&args[0], &self_ty, span);
                self.unify(ret, &self_ty, span)
            }

            // ── Scalar broadcast ops ────────────────────────
            // scale(scalar) → vec (multiply by scalar)
            "scale" if args.len() == 1 => {
                let _ = self.unify(&args[0], &elem_ty, span);
                self.unify(ret, &self_ty, span)
            }

            // ── Reductions ──────────────────────────────────
            "sum" | "product" | "min" | "max" if args.is_empty() => {
                self.unify(ret, &elem_ty, span)
            }

            // ── Lane access ─────────────────────────────────
            // get(index) → elem
            "get" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &elem_ty, span)
            }
            // set(index, value) → ()
            "set" if args.len() == 2 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let _ = self.unify(&args[1], &elem_ty, span);
                self.unify(ret, &Type::Unit, span)
            }

            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Resolve methods on Option<T> (i.e., T?)
    fn resolve_option_method(
        &mut self,
        inner: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let self_ty = Type::option(inner.clone());
        match method {
            // `x == none` desugars to `x.eq(none)`. Equality on a zero-field
            // type is ordinary; `tool.lint/I5` steers it toward `x is none`.
            //
            // A bare *literal* on the other side is meant as the payload, and
            // nothing else constrains it — `let a: i64? = 5` then `a == 5` left
            // the `5` free, so it defaulted to `i32` and the comparison was
            // between an `i64?` and an `i32`. Lowering then had a mismatch it
            // couldn't wrap, and codegen read the scalar as an address (#834).
            // Only a literal: a typed argument keeps its own type, and `none`
            // has its own.
            "eq" | "ne" if args.len() == 1 => {
                if let Type::Var(id) = self.ctx.apply(&args[0]) {
                    if self.ctx.literal_vars.contains_key(&id) {
                        let _ = self.unify(&args[0], inner, span);
                    }
                }
                // The other side is an optional too — `x == none` most of the
                // time. `none` has no payload type of its own, so unless it's
                // tied to the receiver here it stays `?` forever: the node came
                // out of the checker still holding a variable, and MIR had to
                // guess a width for it. Comparing two optionals means comparing
                // the same optional, so say so.
                if self.ctx.apply(&args[0]).is_option() {
                    let _ = self.unify(&args[0], &self_ty, span);
                }
                self.unify(ret, &Type::Bool, span)
            }
            _ => Err(Self::wrapper_method_cut(self_ty, method, span)),
        }
    }

    /// std.api/SD4: neither wrapper has methods — the operators are the whole
    /// API. Name the operator that does this job, so the migration is one
    /// reading rather than a search.
    fn wrapper_method_cut(receiver: Type, method: &str, span: Span) -> TypeError {
        let optional = receiver.is_option();
        let fix = match (method, optional) {
            ("unwrap_or", true) => "write `x ?? <value>` — the right side is lazy by construction",
            ("unwrap_or", false) => "write `r catch _ => <value>`, which says an error is being dropped",
            ("unwrap_or_else", true) => "write `x ?? <expr>`",
            ("unwrap_or_else", false) => "write `r catch e => <expr using e>`",
            ("unwrap", _) | ("expect", _) => "write `x!`, or `x! \"message\"` to say why it can't be absent",
            ("map_err", _) => "write `r catch e => return <wrapped>` — transform and leave in one line",
            ("ok", _) | ("to_option", _) => "write `r catch _ => none` — the discard is acknowledged",
            ("ok_or", _) | ("ok_or_else", _) => "write `x ?? return <error>`",
            ("is_some", _) | ("is_ok", _) => "write `x?`",
            ("is_none", _) => "write `x is none`",
            ("is_err", _) => "write `r is <ErrorType>`",
            ("map", _) | ("and_then", _) | ("filter", _) => {
                "extract first — `try x`, `x ?? <value>` or `r catch e => …` — then work with the value"
            }
            _ if optional => "reach the value with `try`, `??`, `!` or an `if x? as v` bind",
            _ => "reach the value with `try`, `catch`, `!` or a `match`",
        };
        TypeError::WrapperMethodCut {
            method: method.to_string(),
            receiver,
            fix: fix.to_string(),
            span,
        }
    }

    /// Resolve methods on Result<T, E> (i.e., T or E)
    fn resolve_result_method(
        &mut self,
        ok: &Type,
        err: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let self_ty = Type::Result {
            ok: Box::new(ok.clone()),
            err: Box::new(err.clone()),
        };
        let _ = (args, ret);
        Err(Self::wrapper_method_cut(self_ty, method, span))
    }

    /// ORD4: arithmetic is homogeneous, so a signed and an unsigned operand
    /// can't meet under `+ - * / % & | ^ << >>`. There's no result type that
    /// holds both — `u64` can't hold a negative `i32` and `i32` can't hold a
    /// large `u64` — so widening one side would be picking a winner silently.
    /// That's the conversion C makes, and it's why `-1 < 1u` is true there.
    ///
    /// Comparison keeps crossing signedness (it answers by value, so there's
    /// nothing to guess) — its arm deliberately doesn't call this.
    ///
    /// Fires only when both sides are settled integer primitives. An untyped
    /// literal is a variable at this point, and the unify below is what pins it
    /// to the receiver, so `a + 1` on a `u64` still works.
    fn reject_mixed_signedness(
        &mut self,
        method: &str,
        recv: &Type,
        arg: &Type,
        span: Span,
    ) -> Result<(), TypeError> {
        let arg = self.ctx.apply(arg);
        let (Some(recv_signed), Some(arg_signed)) =
            (Self::integer_is_signed(recv), Self::integer_is_signed(&arg))
        else {
            return Ok(());
        };
        if recv_signed == arg_signed {
            return Ok(());
        }
        Err(TypeError::MixedSignednessArithmetic {
            op: Self::operator_spelling(method),
            left: recv.clone(),
            right: arg,
            span,
        })
    }

    /// CV1a: int→float is never implicit, so an integer and a float can't meet
    /// under an arithmetic operator either. `i64 + f64` type-checked and native
    /// answered with an integer — the float operand was dropped, silently, while
    /// the interpreter refused the same program at runtime (#816).
    ///
    /// Both sides have to be settled primitives. An unsuffixed literal is still a
    /// variable here, which is what lets `x + 1` on an `f64` take the slot's type
    /// and become `x + 1.0`.
    fn reject_int_float_mix(
        &mut self,
        method: &str,
        recv: &Type,
        arg: &Type,
        span: Span,
    ) -> Result<(), TypeError> {
        let arg = self.ctx.apply(arg);
        let recv_int = Self::integer_is_signed(recv).is_some();
        let arg_int = Self::integer_is_signed(&arg).is_some();
        let recv_float = matches!(recv, Type::F32 | Type::F64);
        let arg_float = matches!(arg, Type::F32 | Type::F64);
        if (recv_int && arg_float) || (recv_float && arg_int) {
            return Err(TypeError::IntFloatArithmetic {
                op: Self::operator_spelling(method),
                left: recv.clone(),
                right: arg,
                span,
            });
        }
        Ok(())
    }

    /// Reject an operand that isn't the same *kind* of thing as the receiver.
    ///
    /// The comparison arms unify the operand with the receiver and throw the
    /// result away, deliberately — mixed signedness is allowed (ORD4) and would
    /// fail that unification. Discarding it also discarded every real mismatch,
    /// so `some_u8 == 'h'` type-checked and native compared the char as its
    /// underlying scalar: `true`, because 104 is both a byte and `'h'` (#1034).
    /// The interpreter refused the same program at runtime.
    ///
    /// Only concrete primitives are judged. Anything still settling is left to
    /// the rest of inference.
    fn reject_incomparable_operand(
        &mut self,
        method: &str,
        recv: &Type,
        arg: &Type,
        span: Span,
    ) -> Result<(), TypeError> {
        let arg = self.resolve_named(&self.ctx.apply(arg));
        let same_kind = match (&recv, &arg) {
            _ if Self::integer_is_signed(recv).is_some()
                && Self::integer_is_signed(&arg).is_some() => true,
            (Type::F32 | Type::F64, Type::F32 | Type::F64) => true,
            _ => *recv == arg,
        };
        if same_kind {
            return Ok(());
        }
        // A *resolved* operand is judged whether it's a primitive or not.
        // `f64 * Meters` type-checked and natively multiplied the struct's
        // address — `2.0 * Meters { v: 3.0 }` printed 281465035398656 — because
        // the unify that would have caught it was discarded (#978). A struct, an
        // enum and a string are all as wrong here as a `char` is.
        //
        // `Named` is a resolved nominal type: a struct, an enum, an alias that
        // has already been followed. `UnresolvedNamed` and `Generic` are not —
        // they may still turn into something that fits — so they, and every
        // inference variable, are left to the rest of inference.
        //
        // A `T or E` and a `T?` are resolved too, and they were the ones that
        // survived the first pass: `total + rx.receive()` type-checked and
        // added the *wrapper*, so a channel sum came out as `1422162048`. There
        // is no reading of an operator where the wrapper is the operand —
        // ER16's `try`, `!` or a `match` extracts first — so both are as wrong
        // here as a struct is.
        let arg_is_resolved = matches!(
            arg,
            Type::Bool | Type::Char | Type::String
            | Type::F32 | Type::F64
            | Type::Named(_) | Type::Tuple(_)
            | Type::Result { .. } | Type::Union(_)
        ) || Self::integer_is_signed(&arg).is_some();
        if !arg_is_resolved {
            return Ok(());
        }
        Err(TypeError::IncomparableOperands {
            left: recv.clone(),
            right: arg,
            op: Self::operator_spelling(method).to_string(),
            span,
        })
    }

    /// Signedness of an integer primitive, `None` for anything else.
    fn integer_is_signed(ty: &Type) -> Option<bool> {
        match ty {
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 => Some(true),
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => Some(false),
            _ => None,
        }
    }

    /// The operator a desugared method name came from, so the message quotes
    /// what was written rather than `add`.
    pub(super) fn operator_spelling(method: &str) -> &'static str {
        match method {
            "add" => "+",
            "sub" => "-",
            "mul" => "*",
            "div" => "/",
            "rem" => "%",
            "bit_and" => "&",
            "bit_or" => "|",
            "bit_xor" => "^",
            "shl" => "<<",
            "shr" => ">>",
            "eq" => "==",
            "ne" => "!=",
            "lt" => "<",
            "le" => "<=",
            "gt" => ">",
            "ge" => ">=",
            _ => "this operator",
        }
    }

    /// Resolve methods on primitive integer types (i8..i128, u8..u128).
    /// Desugared operators (add, bit_and, etc.) resolve here instead of
    /// bouncing through HasMethod → unsolved constraint suppression.
    pub(super) fn resolve_integer_method(
        &mut self,
        ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let is_signed = matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128);
        match method {
            // Binary arithmetic → same type. `mod` (AR3, Euclidean) rides
            // here rather than beside it: it takes the same two operands and
            // answers in the same type, so the mixed-signedness rule and the
            // result type are the ones `%` already has.
            "add" | "sub" | "mul" | "div" | "rem" | "mod"
            | "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr"
                if args.len() == 1 => {
                if let Err(mixed) = self
                    .reject_mixed_signedness(method, ty, &args[0], span)
                    .and_then(|()| self.reject_int_float_mix(method, ty, &args[0], span))
                    .and_then(|()| self.reject_incomparable_operand(method, ty, &args[0], span))
                {
                    // Pin the result to the receiver on the way out. The
                    // operands are the complaint; leaving `ret` open turns one
                    // error into two, the second being "couldn't work out the
                    // type of x" pointing at a binding that's fine.
                    let _ = self.unify(ret, ty, span);
                    return Err(mixed);
                }
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            // Unary → same type
            "neg" if args.is_empty() && is_signed => self.unify(ret, ty, span),
            "bit_not" | "abs" if args.is_empty() => self.unify(ret, ty, span),
            // Comparison → bool
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" if args.len() == 1 => {
                // The comparison answers `bool` whatever is wrong with the
                // operands, so pin that before reporting. Returning first left
                // the binding's type open and turned one error into two, the
                // second blaming a `let` that is fine.
                if let Err(bad) = self.reject_incomparable_operand(method, ty, &args[0], span) {
                    let _ = self.unify(ret, &Type::Bool, span);
                    return Err(bad);
                }
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &Type::Bool, span)
            }
            "compare" if args.len() == 1 => {
                if let Err(bad) = self.reject_incomparable_operand(method, ty, &args[0], span) {
                    let ord = self.ordering_type();
                    let _ = self.unify(ret, &ord, span);
                    return Err(bad);
                }
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &self.ordering_type(), span)
            }
            "to_float" if args.is_empty() => self.unify(ret, &Type::F64, span),
            // std.bits B1 — bit inspection and permutation. All of these answer
            // in the receiver's own type: a count can't exceed the width, so
            // there's no reason to widen it to a separate counter type.
            "count_ones" | "count_zeros"
            | "leading_zeros" | "trailing_zeros"
            | "leading_ones" | "trailing_ones"
            | "reverse_bits" | "swap_bytes"
                if args.is_empty() => self.unify(ret, ty, span),
            "rotate_left" | "rotate_right" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            // type.integer-overflow OV5/SH2 — the ways out of the checked
            // default. Same two operands and the same answer type as the
            // operator each one shadows; only what happens on overflow differs.
            // The shift forms take an amount of the receiver's own type, the
            // way `shl` does.
            "wrapping_add" | "wrapping_sub" | "wrapping_mul"
            | "saturating_add" | "saturating_sub" | "saturating_mul"
            | "wrapping_shl" | "wrapping_shr"
                if args.len() == 1
                    && !matches!(ty, Type::I128 | Type::U128) => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            // UN1: the last-resort hatch — no overflow check at all, UB if it
            // actually overflows. Same shape as `wrapping_*` (same type in,
            // same type out); the only difference is the compiler makes you
            // say `unsafe` before it'll let you have it.
            "unchecked_add" | "unchecked_sub" | "unchecked_mul"
                if args.len() == 1
                    && !matches!(ty, Type::I128 | Type::U128) => {
                // Recorded whether or not it's actually inside `unsafe` — an
                // out-of-context call still gets flagged below, but `rask
                // unsafe`'s report is about auditing the unsafe surface a
                // file already has, and this op belongs on it either way
                // (every other unsafe op here — transmute, raw pointer
                // deref, extern calls, union access — records unconditionally
                // too).
                self.unsafe_ops.push((span, super::UnsafeCategory::UncheckedArith));
                if !self.in_unsafe {
                    self.errors.push(TypeError::UnsafeRequired {
                        operation: format!(".{}() — UB on overflow (type.overflow/UN1)", method),
                        span,
                    });
                }
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            // The same table's fallible forms. `checked_*` hands back `T?` —
            // `none` when the answer doesn't exist — and `overflowing_*` hands
            // back both the wrapped answer and whether it wrapped.
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div"
                if args.len() == 1
                    && !matches!(ty, Type::I128 | Type::U128) => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &Type::option(ty.clone()), span)
            }
            "overflowing_add" | "overflowing_sub" | "overflowing_mul"
                if args.len() == 1
                    && !matches!(ty, Type::I128 | Type::U128) => {
                let _ = self.unify(&args[0], ty, span);
                let pair = Type::Tuple(vec![ty.clone(), Type::Bool]);
                self.unify(ret, &pair, span)
            }
            // std.bits B2/B3 — byte order. Same width in, same width out;
            // bits.rk documents these as methods on the integer types and
            // uses them (`hton_u16` is `x.to_be()`), but nothing registered
            // them, so the stdlib's own definitions didn't resolve.
            "to_be" | "to_le" if args.is_empty() => self.unify(ret, ty, span),
            // HA1: every integer width is Hashable, and `Hashable` is
            // `hash(self) -> u64`. The trait table said so and this didn't, so the
            // conformance held while the call was rejected (#813).
            "hash" if args.is_empty() => self.unify(ret, &Type::U64, span),
            _ => Err(TypeError::NoSuchMethod {
                ty: ty.clone(),
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Resolve methods on primitive float types (f32, f64).
    pub(super) fn resolve_float_method(
        &mut self,
        ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        use rask_stdlib::FloatSig;

        let no_such = || TypeError::NoSuchMethod {
            ty: ty.clone(),
            method: method.to_string(),
            span,
        };
        let entry = rask_stdlib::float_methods::lookup(method).ok_or_else(no_such)?;

        // Arity comes from the signature shape; a call with the wrong count
        // isn't this method.
        let wants_arg = matches!(
            entry.sig,
            FloatSig::BinaryFloat | FloatSig::BinaryInt | FloatSig::Comparison | FloatSig::Compare
        );
        if wants_arg != (args.len() == 1) {
            return Err(no_such());
        }

        match entry.sig {
            FloatSig::Unary => self.unify(ret, ty, span),
            FloatSig::BinaryFloat => {
                // The other side of #816: `f64 + i64` was accepted too, and the
                // discarded unify is why nothing said so. #816 added the
                // int/float guard and left the discard, so every *other* wrong
                // operand still sailed through — `f64 * Meters` type-checked and
                // multiplied the struct's address (#978).
                if let Err(bad) = self
                    .reject_int_float_mix(method, ty, &args[0], span)
                    .and_then(|()| self.reject_incomparable_operand(method, ty, &args[0], span))
                {
                    // Pin the result to the receiver on the way out, so one
                    // error doesn't become two with the second blaming a
                    // binding that is fine.
                    let _ = self.unify(ret, ty, span);
                    return Err(bad);
                }
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            FloatSig::BinaryInt => {
                let _ = self.unify(&args[0], &Type::I32, span);
                self.unify(ret, ty, span)
            }
            FloatSig::Predicate | FloatSig::Comparison => {
                if entry.sig == FloatSig::Comparison {
                    if let Err(bad) =
                        self.reject_incomparable_operand(method, ty, &args[0], span)
                    {
                        let _ = self.unify(ret, &Type::Bool, span);
                        return Err(bad);
                    }
                    let _ = self.unify(&args[0], ty, span);
                }
                self.unify(ret, &Type::Bool, span)
            }
            FloatSig::Compare => {
                if let Err(bad) = self.reject_incomparable_operand(method, ty, &args[0], span) {
                    let ord = self.ordering_type();
                    let _ = self.unify(ret, &ord, span);
                    return Err(bad);
                }
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &self.ordering_type(), span)
            }
            FloatSig::ToString => self.unify(ret, &Type::String, span),
            FloatSig::ToInt => self.unify(ret, &Type::I64, span),
            // u64 at both widths — see the note on FloatSig::ToBits.
            FloatSig::ToBits => self.unify(ret, &Type::U64, span),
        }
    }

    /// Resolve methods on raw pointer types (`*T`) once the receiver is known.
    ///
    /// The same table the eager path in check_expr uses, so the two can't
    /// disagree about what a pointer answers to.
    pub(super) fn resolve_raw_ptr_method(
        &mut self,
        inner: &Type,
        ptr_ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        use rask_stdlib::PtrSig;

        let no_such = || TypeError::NoSuchMethod {
            ty: ptr_ty.clone(),
            method: method.to_string(),
            span,
        };
        let entry = rask_stdlib::ptr_methods::lookup(method).ok_or_else(no_such)?;

        let wants_arg = matches!(
            entry.sig,
            PtrSig::Write | PtrSig::Arith | PtrSig::PredicateInt | PtrSig::Comparison | PtrSig::ToInt
        );
        if wants_arg != (args.len() == 1) {
            return Err(no_such());
        }

        // The walk recorded whether this site sat inside `unsafe`. Without the
        // check here a pointer method on a late-resolved receiver skipped the
        // rule entirely — `ptr.read()` outside `unsafe` was accepted.
        if entry.needs_unsafe && !self.ptr_method_sites.get(&span).copied().unwrap_or(true) {
            return Err(TypeError::UnsafeRequired {
                operation: format!("pointer method .{}()", method),
                span,
            });
        }

        match entry.sig {
            PtrSig::Read => self.unify(ret, inner, span),
            PtrSig::Write => {
                let _ = self.unify(&args[0], inner, span);
                self.unify(ret, &Type::Unit, span)
            }
            // The step count is left open — a `usize` index and an `i32` loop
            // variable are both fine here.
            PtrSig::Arith => self.unify(ret, ptr_ty, span),
            PtrSig::Predicate => self.unify(ret, &Type::Bool, span),
            PtrSig::PredicateInt => self.unify(ret, &Type::Bool, span),
            PtrSig::Comparison => {
                // `null` is typed `*_`, so this ties its pointee to ours.
                let _ = self.unify(&args[0], ptr_ty, span);
                self.unify(ret, &Type::Bool, span)
            }
            PtrSig::ToInt => self.unify(ret, &Type::I64, span),
            // Same answer the eager path gives: the written `<U>` when the call
            // had one, a fresh variable when it didn't (#986). The two paths
            // disagreeing about a pointer method is how #696 got its "no method
            // `offset` found for type `*u8`".
            PtrSig::Cast => {
                let target = match self.ptr_cast_targets.get(&span).cloned() {
                    Some(t) => t,
                    None => self.ctx.fresh_var(),
                };
                self.unify(ret, &Type::RawPtr(Box::new(target)), span)
            }
        }
    }
}

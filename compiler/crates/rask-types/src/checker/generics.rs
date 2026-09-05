// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Generic type substitution and type variable instantiation.

use std::collections::HashMap;

use super::type_defs::TypeDef;
use super::TypeChecker;

use crate::types::{GenericArg, Type, TypeVarId};

impl TypeChecker {
    /// Resolve the self type for an extend block, handling generic params.
    /// "SpscRingBuffer<T, N>" -> Type::Generic { base, args: [T, N] }
    /// "TimingStats" -> Type::Named(id)
    ///
    /// The header wins where it says anything: `extend Sequence<(K, V)>` makes
    /// `Self` a `Sequence<(K, V)>`, not the `Sequence<T>` the declaration spells.
    /// Rebuilding it from the declaration's own names gave `self` an element
    /// type of `T`, which no binding in such a block ever fills — so the copy
    /// dropped the record and `for (k, v) in self` fell through to the
    /// index-based path and walked a closure as if it were a Vec (#1046).
    pub(super) fn resolve_impl_self_type(&self, target_ty: &str) -> Option<Type> {
        let base_name = target_ty.split('<').next().unwrap_or(target_ty);
        let type_id = self.types.get_type_id(base_name)?;

        let declared = self.declared_type_params(type_id).len();
        let header_args = Self::target_type_args(target_ty);
        if !header_args.is_empty() && header_args.len() == declared {
            let args = header_args
                .iter()
                .map(|arg| {
                    // A bare parameter name stays symbolic — resolving it would
                    // find any type that happens to share the letter.
                    let ty = if super::declarations::is_type_param_name(arg) {
                        Type::UnresolvedNamed(arg.clone())
                    } else {
                        crate::parse_type_string(arg, &self.types)
                            .unwrap_or_else(|_| Type::UnresolvedNamed(arg.clone()))
                    };
                    GenericArg::Type(Box::new(ty))
                })
                .collect();
            return Some(Type::Generic { base: type_id, args });
        }

        // Check if the struct/enum has type params
        let has_type_params = self.types.get(type_id).map_or(false, |def| {
            match def {
                TypeDef::Struct { type_params, .. } | TypeDef::Enum { type_params, .. } => {
                    !type_params.is_empty()
                }
                _ => false,
            }
        });

        if has_type_params {
            // Build generic args from the struct's type params
            let args = self.types.get(type_id).and_then(|def| {
                let type_params = match def {
                    TypeDef::Struct { type_params, .. } | TypeDef::Enum { type_params, .. } => type_params,
                    _ => return None,
                };
                Some(type_params.iter().map(|p| {
                    GenericArg::Type(Box::new(Type::UnresolvedNamed(p.clone())))
                }).collect::<Vec<_>>())
            }).unwrap_or_default();
            Some(Type::Generic { base: type_id, args })
        } else {
            Some(Type::Named(type_id))
        }
    }

    /// The inverse of `resolve_named`, for diagnostics: a `Named(TypeId)`
    /// prints as `<type#75>`, which tells the reader nothing. Errors that read
    /// well happen to be holding the unresolved-name form already; this puts a
    /// resolved type back into it so a message can name the struct or enum.
    pub(super) fn nameable(&self, ty: &Type) -> Type {
        match ty {
            Type::Named(id) => Type::UnresolvedNamed(self.types.type_name(*id)),
            other => other.clone(),
        }
    }

    /// The `Ordering` enum, resolved to its registered type where possible.
    ///
    /// Nine places built `UnresolvedNamed("Ordering")` by hand, and an
    /// unresolved name slips past checks that key on a real type: `Ordering`
    /// doesn't implement `Displayable`, so `println("{a.compare(b)}")` is an
    /// error — but only `string.compare` was caught, because its return type
    /// comes from a stub and resolves. `(1).compare(2)` sailed through and
    /// printed the raw tag natively against `Ordering.Less` on the interpreter.
    pub(super) fn ordering_type(&self) -> Type {
        self.resolve_named(&Type::UnresolvedNamed("Ordering".to_string()))
    }

    /// Declared type parameter names of an enum; empty for anything else.
    pub(super) fn enum_type_params(&self, id: crate::types::TypeId) -> Vec<String> {
        match self.types.get(id) {
            Some(TypeDef::Enum { type_params, .. }) => type_params.clone(),
            _ => Vec::new(),
        }
    }

    /// Declared type parameter names of a struct or an enum; empty for anything
    /// that has none.
    pub(super) fn declared_type_params(&self, id: crate::types::TypeId) -> Vec<String> {
        match self.types.get(id) {
            Some(TypeDef::Struct { type_params, .. })
            | Some(TypeDef::Enum { type_params, .. }) => type_params.clone(),
            _ => Vec::new(),
        }
    }

    pub(super) fn resolve_named(&self, ty: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => {
                if name == "Self" {
                    if let Some(self_ty) = &self.current_self_type {
                        return self_ty.clone();
                    }
                }
                if let Some(type_id) = self.types.get_type_id(name) {
                    return Type::Named(type_id);
                }
                // Qualified type name: "pkg.Type" → look up "Type" then "pkg$Type"
                if let Some(dot) = name.find('.') {
                    let type_name = &name[dot + 1..];
                    if let Some(type_id) = self.types.get_type_id(type_name) {
                        return Type::Named(type_id);
                    }
                    let prefixed = format!("{}${}", &name[..dot], type_name);
                    if let Some(type_id) = self.types.get_type_id(&prefixed) {
                        return Type::Named(type_id);
                    }
                }
                ty.clone()
            }
            // The arguments are types too. Resolving only the base left
            // `Map<K2, string>` holding `UnresolvedNamed("K2")` for a declared
            // struct, and an unresolved name is treated as "fits anything" by
            // every check downstream — so `m.insert(1, "a")` on a
            // `Map<K2, string>` type-checked (#812). A name that genuinely
            // doesn't resolve, like the type parameter `T`, still doesn't, so
            // the leniency stays exactly where it's needed.
            Type::UnresolvedGeneric { name, args } => {
                let args = self.resolve_named_args(args);
                match self.types.get_type_id(name) {
                    Some(type_id) => Type::Generic { base: type_id, args },
                    None => Type::UnresolvedGeneric { name: name.clone(), args },
                }
            }
            Type::Generic { base, args } => {
                Type::Generic { base: *base, args: self.resolve_named_args(args) }
            }
            _ => ty.clone(),
        }
    }

    /// `resolve_named` over each type argument, leaving const arguments alone.
    fn resolve_named_args(&self, args: &[GenericArg]) -> Vec<GenericArg> {
        args.iter()
            .map(|a| match a {
                GenericArg::Type(t) => GenericArg::Type(Box::new(self.resolve_named(t))),
                other => other.clone(),
            })
            .collect()
    }

    /// Replace type parameter names (UnresolvedNamed) with concrete types.
    pub(super) fn substitute_type_params(ty: &Type, subst: &HashMap<&str, Type>) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => {
                if let Some(replacement) = subst.get(name.as_str()) {
                    return replacement.clone();
                }
                ty.clone()
            }
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(Self::substitute_type_params(ok, subst))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::substitute_type_params(ok, subst)),
                err: Box::new(Self::substitute_type_params(err, subst)),
            },
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::substitute_type_params(elem, subst)),
                len: *len,
            },
            Type::Slice(elem) => {
                Type::Slice(Box::new(Self::substitute_type_params(elem, subst)))
            }
            // Every other compound type recursed; pointers didn't, so a method
            // returning `*T` — `Vec<T>.as_ptr()` — kept a literal `T` here. The
            // freshening pass right after this then turned that surviving `T`
            // into a variable tied to nothing, and `let p = v.as_ptr()` came
            // back "type is still open here" (#696).
            Type::RawPtr(inner) => {
                Type::RawPtr(Box::new(Self::substitute_type_params(inner, subst)))
            }
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| Self::substitute_type_params(e, subst)).collect())
            }
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| Self::substitute_type_params(p, subst)).collect(),
                ret: Box::new(Self::substitute_type_params(ret, subst)),
            },
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(Self::substitute_type_params(t, subst))),
                    other => other.clone(),
                }).collect(),
            },
            Type::UnresolvedGeneric { name, args } => {
                // Check if whole name is a type param
                if let Some(replacement) = subst.get(name.as_str()) {
                    return replacement.clone();
                }
                Type::UnresolvedGeneric {
                    name: name.clone(),
                    args: args.iter().map(|a| match a {
                        GenericArg::Type(t) => GenericArg::Type(Box::new(Self::substitute_type_params(t, subst))),
                        other => other.clone(),
                    }).collect(),
                }
            }
            _ => ty.clone(),
        }
    }

    /// Build a substitution map from type param names to concrete types from generic args.
    pub(super) fn build_type_param_subst<'a>(
        type_params: &'a [String],
        args: &[GenericArg],
    ) -> HashMap<&'a str, Type> {
        let mut subst = HashMap::new();
        for (param, arg) in type_params.iter().zip(args.iter()) {
            if let GenericArg::Type(ty) = arg {
                subst.insert(param.as_str(), *ty.clone());
            }
        }
        subst
    }

    /// What an extend header's target arguments bind to, given the receiver's
    /// actual arguments.
    ///
    /// `extend Sequence<(K, V)>` on a `Sequence<(i64, string)>` binds `K` to
    /// `i64` and `V` to `string` — the pattern is matched against the argument
    /// member-wise, so a parameter nested in a tuple gets the matching half
    /// rather than the whole thing. A pattern that is just a name is the
    /// ordinary case and binds exactly as `build_type_param_subst` does.
    ///
    /// A pattern that doesn't line up with what arrived contributes nothing,
    /// which leaves the name unbound rather than bound to the wrong type.
    pub(super) fn build_owner_pattern_subst(
        patterns: &[String],
        args: &[GenericArg],
    ) -> HashMap<String, Type> {
        let mut subst = HashMap::new();
        for (pattern, arg) in patterns.iter().zip(args.iter()) {
            if let GenericArg::Type(ty) = arg {
                bind_owner_pattern(pattern, ty, &mut subst);
            }
        }
        subst
    }

    /// Replace a signature's *own* type parameters with fresh inference vars.
    ///
    /// `build_type_param_subst` only knows the receiving type's parameters. A
    /// method can declare more — `Vec<T>.map(self, f: func(T) -> U) -> Vec<U>`
    /// has a `U` that belongs to the call, not the Vec. Left literal it came
    /// out as the element type of the result, and `doubled[0] == 2` reported
    /// "no method `eq` found for type `U`" (#327).
    ///
    /// PC3: a single uppercase name is always a type parameter. `seen` is
    /// shared across the whole signature so every `U` in it is the same var.
    pub(super) fn freshen_free_type_params(
        &mut self,
        ty: &Type,
        seen: &mut HashMap<String, Type>,
    ) -> Type {
        fn is_param(name: &str) -> bool {
            // `parse_stub_type` has already rewritten single uppercase names in
            // a stub signature to `_Any`, so both spellings arrive here.
            if name == "_Any" {
                return true;
            }
            let mut chars = name.chars();
            matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
        }
        match ty {
            Type::UnresolvedNamed(name) if is_param(name) => seen
                .entry(name.clone())
                .or_insert_with(|| self.ctx.fresh_var())
                .clone(),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.freshen_free_type_params(ok, seen)),
                err: Box::new(self.freshen_free_type_params(err, seen)),
            },
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| self.freshen_free_type_params(p, seen)).collect(),
                ret: Box::new(self.freshen_free_type_params(ret, seen)),
            },
            Type::Tuple(elems) => Type::Tuple(
                elems.iter().map(|e| self.freshen_free_type_params(e, seen)).collect(),
            ),
            Type::Union(variants) => Type::Union(
                variants.iter().map(|v| self.freshen_free_type_params(v, seen)).collect(),
            ),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.freshen_free_type_params(elem, seen)),
                len: *len,
            },
            Type::Slice(inner) => Type::Slice(Box::new(self.freshen_free_type_params(inner, seen))),
            Type::RawPtr(inner) => Type::RawPtr(Box::new(self.freshen_free_type_params(inner, seen))),
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| self.freshen_generic_arg(a, seen)).collect(),
            },
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| self.freshen_generic_arg(a, seen)).collect(),
            },
            other => other.clone(),
        }
    }

    fn freshen_generic_arg(
        &mut self,
        arg: &GenericArg,
        seen: &mut HashMap<String, Type>,
    ) -> GenericArg {
        match arg {
            GenericArg::Type(t) => {
                GenericArg::Type(Box::new(self.freshen_free_type_params(t, seen)))
            }
            other => other.clone(),
        }
    }

    pub(super) fn instantiate_type_vars(&mut self, types: &[Type]) -> Vec<Type> {
        let mut subst: HashMap<TypeVarId, Type> = HashMap::new();
        for ty in types {
            self.collect_type_vars(ty, &mut subst);
        }
        types
            .iter()
            .map(|ty| self.apply_type_var_substitution(ty, &subst))
            .collect()
    }

    pub(super) fn collect_type_vars(&mut self, ty: &Type, subst: &mut HashMap<TypeVarId, Type>) {
        match ty {
            Type::Var(id) => {
                subst.entry(*id).or_insert_with(|| self.ctx.fresh_var());
            }
            Type::Tuple(elems) => {
                for e in elems {
                    self.collect_type_vars(e, subst);
                }
            }
            Type::Array { elem, .. } | Type::Slice(elem) => {
                self.collect_type_vars(elem, subst);
            }
            Type::Result { ok, err } => {
                self.collect_type_vars(ok, subst);
                self.collect_type_vars(err, subst);
            }
            Type::Generic { args, .. } => {
                for a in args {
                    self.collect_type_vars_generic_arg(a, subst);
                }
            }
            Type::Fn { params, ret } => {
                for p in params {
                    self.collect_type_vars(p, subst);
                }
                self.collect_type_vars(ret, subst);
            }
            _ => {}
        }
    }

    pub(super) fn collect_type_vars_generic_arg(&mut self, arg: &GenericArg, subst: &mut HashMap<TypeVarId, Type>) {
        match arg {
            GenericArg::Type(ty) => self.collect_type_vars(ty, subst),
            GenericArg::ConstUsize(_) => {}
        }
    }

    pub(super) fn apply_type_var_substitution(
        &self,
        ty: &Type,
        substitution: &HashMap<TypeVarId, Type>,
    ) -> Type {
        match ty {
            Type::Var(id) => substitution.get(id).cloned().unwrap_or_else(|| ty.clone()),
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .iter()
                    .map(|e| self.apply_type_var_substitution(e, substitution))
                    .collect(),
            ),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.apply_type_var_substitution(elem, substitution)),
                len: *len,
            },
            Type::Slice(elem) => {
                Type::Slice(Box::new(self.apply_type_var_substitution(elem, substitution)))
            }
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(self.apply_type_var_substitution(ok, substitution))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.apply_type_var_substitution(ok, substitution)),
                err: Box::new(self.apply_type_var_substitution(err, substitution)),
            },
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args
                    .iter()
                    .map(|a| self.apply_type_var_substitution_generic_arg(a, substitution))
                    .collect(),
            },
            Type::Fn { params, ret } => Type::Fn {
                params: params
                    .iter()
                    .map(|p| self.apply_type_var_substitution(p, substitution))
                    .collect(),
                ret: Box::new(self.apply_type_var_substitution(ret, substitution)),
            },
            _ => ty.clone(),
        }
    }

    pub(super) fn apply_type_var_substitution_generic_arg(
        &self,
        arg: &GenericArg,
        substitution: &HashMap<TypeVarId, Type>,
    ) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => {
                GenericArg::Type(Box::new(self.apply_type_var_substitution(ty, substitution)))
            }
            GenericArg::ConstUsize(n) => GenericArg::ConstUsize(*n),
        }
    }
}

/// The parameter names an extend header's target argument introduces:
/// `["K", "V"]` for `(K, V)`, `["T"]` for `T`, and nothing for a concrete
/// spelling like `i64`.
pub(super) fn pattern_names(pattern: &str) -> Vec<String> {
    let pattern = pattern.trim();
    if let Some(members) = pattern.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return super::parse_type::split_type_args(members)
            .into_iter()
            .flat_map(pattern_names)
            .collect();
    }
    // `Sequence<U>` introduces `U`, the same way `(K, V)` introduces both.
    if let Some(inner) = generic_args_of(pattern) {
        return super::parse_type::split_type_args(inner)
            .into_iter()
            .flat_map(pattern_names)
            .collect();
    }
    if super::declarations::is_type_param_name(pattern) {
        vec![pattern.to_string()]
    } else {
        Vec::new()
    }
}

/// The argument list inside `Name<…>`, or `None` if there isn't one.
fn generic_args_of(pattern: &str) -> Option<&str> {
    let open = pattern.find('<')?;
    let inner = pattern[open + 1..].trim_end().strip_suffix('>')?;
    (!pattern[..open].trim().is_empty()).then_some(inner)
}

/// Match one target argument, as written, against the type that arrived.
fn bind_owner_pattern(pattern: &str, actual: &Type, out: &mut HashMap<String, Type>) {
    let pattern = pattern.trim();
    if let Some(members) = pattern.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        let Type::Tuple(elems) = actual else { return };
        let parts = super::parse_type::split_type_args(members);
        if parts.len() != elems.len() {
            return;
        }
        for (p, a) in parts.iter().zip(elems.iter()) {
            bind_owner_pattern(p, a, out);
        }
        return;
    }
    // A generic in the target binds argument-wise, the same way a tuple does:
    // `extend Sequence<Sequence<U>>` has to give `U` the *inner* sequence's
    // element, not the inner sequence.
    if let Some(inner) = generic_args_of(pattern) {
        let parts = super::parse_type::split_type_args(inner);
        let actual_args = match actual {
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args,
            _ => return,
        };
        if parts.len() != actual_args.len() {
            return;
        }
        for (p, a) in parts.iter().zip(actual_args.iter()) {
            if let GenericArg::Type(t) = a {
                bind_owner_pattern(p, t, out);
            }
        }
        return;
    }
    if super::declarations::is_type_param_name(pattern) {
        out.insert(pattern.to_string(), actual.clone());
    }
}

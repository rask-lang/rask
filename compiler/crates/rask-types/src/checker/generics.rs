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
    pub(super) fn resolve_impl_self_type(&self, target_ty: &str) -> Option<Type> {
        let base_name = target_ty.split('<').next().unwrap_or(target_ty);
        let type_id = self.types.get_type_id(base_name)?;

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
                ty.clone()
            }
            _ => ty.clone(),
        }
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
            Type::Option(inner) => {
                Type::Option(Box::new(Self::substitute_type_params(inner, subst)))
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
            Type::Array { elem, .. } | Type::Slice(elem) | Type::Option(elem) => {
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
            Type::Option(inner) => {
                Type::Option(Box::new(self.apply_type_var_substitution(inner, substitution)))
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

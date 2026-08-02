// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Trait checking for Rask.
//!
//! Implements structural trait satisfaction: a type satisfies a trait if it has
//! all required methods with matching signatures.

use crate::types::{Type, TypeId};
use crate::checker::{TypeTable, TypeDef, MethodSig, SelfParam, ParamMode};
use rask_ast::Span;
use std::collections::HashMap;
use thiserror::Error;

// ============================================================================
// Trait Bound
// ============================================================================

/// A trait bound like `T: Comparable` or `K: Hashable + Clone`.
#[derive(Debug, Clone)]
pub struct TraitBound {
    /// The type parameter name (e.g., "T").
    pub type_param: String,
    /// The traits it must satisfy.
    pub traits: Vec<String>,
}

impl TraitBound {
    pub fn new(type_param: impl Into<String>, traits: Vec<String>) -> Self {
        Self {
            type_param: type_param.into(),
            traits,
        }
    }

    pub fn single(type_param: impl Into<String>, trait_name: impl Into<String>) -> Self {
        Self {
            type_param: type_param.into(),
            traits: vec![trait_name.into()],
        }
    }
}

// ============================================================================
// Trait Errors
// ============================================================================

/// Errors during trait checking.
#[derive(Debug, Error)]
pub enum TraitError {
    #[error("Type {ty} does not satisfy trait {trait_name}")]
    NotSatisfied { ty: String, trait_name: String, span: Span },

    #[error("Missing method '{method}' required by trait {trait_name}")]
    MissingMethod {
        ty: String,
        trait_name: String,
        method: String,
        span: Span,
    },

    #[error("Method '{method}' signature mismatch: expected {expected}, found {found}")]
    SignatureMismatch {
        ty: String,
        method: String,
        expected: String,
        found: String,
        span: Span,
    },

    #[error("Unknown trait: {0}")]
    UnknownTrait(String),

    #[error("Conflicting method signatures in composed traits: {method}")]
    ConflictingMethods { method: String, trait1: String, trait2: String },
}

// ============================================================================
// Trait Checker
// ============================================================================

/// Checks structural trait satisfaction.
pub struct TraitChecker<'a> {
    /// The type table containing all type definitions.
    types: &'a TypeTable,
    /// Collected errors.
    errors: Vec<TraitError>,
    /// Cache for trait method requirements (expanded with composed traits).
    trait_methods: HashMap<String, Vec<MethodSig>>,
}

impl<'a> TraitChecker<'a> {
    pub fn new(types: &'a TypeTable) -> Self {
        let mut checker = Self {
            types,
            errors: Vec::new(),
            trait_methods: HashMap::new(),
        };
        checker.collect_trait_methods();
        checker
    }

    /// Collect all methods from traits (including composed traits).
    fn collect_trait_methods(&mut self) {
        // First pass: collect direct methods
        let mut super_map: Vec<(String, Vec<String>)> = Vec::new();
        for def in self.types.iter() {
            if let TypeDef::Trait { name, super_traits, methods, .. } = def {
                self.trait_methods.insert(name.clone(), methods.clone());
                if !super_traits.is_empty() {
                    super_map.push((name.clone(), super_traits.clone()));
                }
            }
        }
        // Second pass: add inherited methods from super-traits
        for (trait_name, supers) in &super_map {
            let mut inherited = Vec::new();
            for parent in supers {
                if let Some(parent_methods) = self.trait_methods.get(parent) {
                    for m in parent_methods {
                        // Don't duplicate methods already defined directly
                        if !self.trait_methods.get(trait_name)
                            .map_or(false, |ms| ms.iter().any(|existing| existing.name == m.name))
                            && !inherited.iter().any(|im: &MethodSig| im.name == m.name)
                        {
                            inherited.push(m.clone());
                        }
                    }
                }
            }
            if let Some(methods) = self.trait_methods.get_mut(trait_name) {
                methods.extend(inherited);
            }
        }
    }

    /// G1: is this a nominal user-declared trait (registered, not `duck`)?
    /// Builtin/auto-derived traits (Equal, Comparable, …) are handled by
    /// eligibility and keep structural matching; only user-declared traits
    /// require an explicit `extend T with Trait` conformance.
    fn is_nominal_user_trait(&self, trait_name: &str) -> bool {
        let base = trait_name.split('<').next().unwrap_or(trait_name);
        matches!(
            self.types.get_type_id(base).and_then(|id| self.types.get(id)),
            Some(TypeDef::Trait { is_duck: false, .. })
        )
    }

    /// The registered TypeId of a struct/enum type, for conformance lookup.
    fn user_type_id(&self, ty: &Type) -> Option<crate::types::TypeId> {
        let id = match ty {
            Type::Named(id) => *id,
            Type::Generic { base, .. } => *base,
            Type::UnresolvedNamed(name) => self.types.get_type_id(name)?,
            Type::UnresolvedGeneric { name, .. } => self.types.get_type_id(name)?,
            _ => return None,
        };
        matches!(self.types.get(id), Some(TypeDef::Struct { .. } | TypeDef::Enum { .. }))
            .then_some(id)
    }

    /// Check if a type satisfies a trait bound.
    pub fn check_satisfies(
        &mut self,
        ty: &Type,
        trait_name: &str,
        span: Span,
    ) -> Result<(), TraitError> {
        // Encode/Decode are structural markers (std.encoding E12–E17): a type
        // satisfies them by shape, not by a declared `extend`. A base type, a
        // container of encodable elements, or a struct/enum whose fields all
        // encode qualifies. These aren't registered as traits, so short-circuit
        // before the method-based logic (which would fail with UnknownTrait).
        let base_trait = trait_name.split('<').next().unwrap_or(trait_name);
        if matches!(base_trait, "Encode" | "Decode") {
            if self.type_is_encodable(ty, &mut Vec::new()) {
                return Ok(());
            }
            return Err(TraitError::NotSatisfied {
                ty: self.type_name(ty),
                trait_name: trait_name.to_string(),
                span,
            });
        }

        // G1 nominal gate: a user struct/enum satisfies a user-declared trait
        // only through a declared `extend T with Trait` (or auto-derive). A
        // matching shape without the declaration is rejected — the flip.
        if self.is_nominal_user_trait(trait_name) {
            if let Some(type_id) = self.user_type_id(ty) {
                if !self.types.declares_conformance(type_id, trait_name) {
                    return Err(TraitError::NotSatisfied {
                        ty: self.type_name(ty),
                        trait_name: trait_name.to_string(),
                        span,
                    });
                }
                // CC1: a conditional conformance holds only for instantiations
                // that satisfy the `where` clause, checked here per instantiation.
                if let Some(cond) = self.types.conformance_condition(type_id, trait_name).cloned() {
                    if let Some(err) = self.check_conformance_condition(ty, type_id, &cond, span) {
                        return Err(err);
                    }
                }
            }
        }

        // Get the trait's required methods
        let required_methods = self.get_trait_methods(trait_name)?;

        // Get the type's available methods
        let type_methods = self.get_type_methods(ty);

        // Check each required method exists with matching signature
        for required in &required_methods {
            if let Some(found) = type_methods.iter().find(|m| m.name == required.name) {
                // Check signature matches
                if !self.signatures_match(required, found) {
                    return Err(TraitError::SignatureMismatch {
                        ty: self.type_name(ty),
                        method: required.name.clone(),
                        expected: self.format_signature(required),
                        found: self.format_signature(found),
                        span,
                    });
                }
            } else {
                // Check for primitive/builtin methods
                if !self.has_builtin_method(ty, &required.name) {
                    return Err(TraitError::MissingMethod {
                        ty: self.type_name(ty),
                        trait_name: trait_name.to_string(),
                        method: required.name.clone(),
                        span,
                    });
                }
            }
        }

        Ok(())
    }

    /// std.encoding E12–E17: does `ty` encode structurally? Base types, optionals,
    /// tuples/arrays, the `Vec`/`Map`/`Set` containers, and structs/enums whose
    /// public fields (variant payloads) all encode. `visited` breaks cycles in
    /// recursive types — a self-referential field is treated coinductively.
    fn type_is_encodable(&self, ty: &Type, visited: &mut Vec<TypeId>) -> bool {
        use crate::types::GenericArg;
        match ty {
            // E14: base types
            Type::Bool
            | Type::Char
            | Type::String
            | Type::Unit
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::I128
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::U128
            | Type::F32
            | Type::F64 => true,
            // E15: `T?` (Result with an absent err) encodes when its payload does
            Type::Result { ok, err } if matches!(**err, Type::None) => {
                self.type_is_encodable(ok, visited)
            }
            Type::Array { elem, .. } | Type::Slice(elem) => self.type_is_encodable(elem, visited),
            Type::Tuple(elems) => elems.iter().all(|e| self.type_is_encodable(e, visited)),
            Type::Named(id) => self.named_is_encodable(*id, &[], visited),
            Type::UnresolvedNamed(name) => match self.types.get_type_id(name) {
                Some(id) => self.named_is_encodable(id, &[], visited),
                None => false,
            },
            Type::Generic { base, args } => {
                let targs: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                let name = self.types.get(*base).map(Self::type_def_name);
                self.container_or_named_encodable(name.as_deref(), Some(*base), &targs, visited)
            }
            Type::UnresolvedGeneric { name, args } => {
                let targs: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                let id = self.types.get_type_id(name);
                self.container_or_named_encodable(Some(name), id, &targs, visited)
            }
            _ => false,
        }
    }

    /// A container spelling (`Vec`/`Map`/`Set`) encodes when its element types do;
    /// any other generic is a user struct/enum instantiation, checked field-wise.
    fn container_or_named_encodable(
        &self,
        name: Option<&str>,
        id: Option<TypeId>,
        targs: &[Type],
        visited: &mut Vec<TypeId>,
    ) -> bool {
        if matches!(name, Some("Vec" | "Map" | "Set")) {
            return targs.iter().all(|t| self.type_is_encodable(t, visited));
        }
        match id {
            Some(id) => self.named_is_encodable(id, targs, visited),
            None => false,
        }
    }

    fn type_def_name(def: &TypeDef) -> &str {
        match def {
            TypeDef::Struct { name, .. }
            | TypeDef::Enum { name, .. }
            | TypeDef::Trait { name, .. }
            | TypeDef::Union { name, .. }
            | TypeDef::NominalAlias { name, .. } => name,
        }
    }

    /// Encodability of a named struct/enum, with `targs` bound to its type params.
    fn named_is_encodable(&self, id: TypeId, targs: &[Type], visited: &mut Vec<TypeId>) -> bool {
        if visited.contains(&id) {
            return true; // recursive type — assume ok, the non-cyclic fields decide
        }
        visited.push(id);
        let result = match self.types.get(id) {
            Some(TypeDef::Struct { fields, type_params, private_fields, .. }) => {
                let subst = Self::build_subst(type_params, targs);
                // E12: only public fields participate
                fields.iter().all(|(fname, fty)| {
                    private_fields.contains(fname)
                        || self.type_is_encodable(&Self::apply_subst(fty, &subst), visited)
                })
            }
            Some(TypeDef::Enum { variants, type_params, .. }) => {
                let subst = Self::build_subst(type_params, targs);
                variants.iter().all(|(_, payloads)| {
                    payloads
                        .iter()
                        .all(|pty| self.type_is_encodable(&Self::apply_subst(pty, &subst), visited))
                })
            }
            Some(TypeDef::NominalAlias { underlying, .. }) => {
                self.type_is_encodable(&underlying.clone(), visited)
            }
            _ => false,
        };
        visited.pop();
        result
    }

    fn build_subst(type_params: &[String], targs: &[Type]) -> HashMap<String, Type> {
        type_params
            .iter()
            .cloned()
            .zip(targs.iter().cloned())
            .collect()
    }

    /// Replace bare type-parameter references in `ty` with their bound arguments.
    /// Only substitutes at the positions that matter for encodability (the field's
    /// own type and container element args); anything else passes through.
    fn apply_subst(ty: &Type, subst: &HashMap<String, Type>) -> Type {
        use crate::types::GenericArg;
        match ty {
            Type::UnresolvedNamed(name) => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args
                    .iter()
                    .map(|a| match a {
                        GenericArg::Type(t) => GenericArg::Type(Box::new(Self::apply_subst(t, subst))),
                        other => other.clone(),
                    })
                    .collect(),
            },
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|a| match a {
                        GenericArg::Type(t) => GenericArg::Type(Box::new(Self::apply_subst(t, subst))),
                        other => other.clone(),
                    })
                    .collect(),
            },
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::apply_subst(ok, subst)),
                err: Box::new(Self::apply_subst(err, subst)),
            },
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::apply_subst(elem, subst)),
                len: *len,
            },
            Type::Slice(elem) => Type::Slice(Box::new(Self::apply_subst(elem, subst))),
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| Self::apply_subst(e, subst)).collect())
            }
            other => other.clone(),
        }
    }

    /// CC1: verify a conditional conformance's `where` clause against the
    /// concrete generic arguments. Maps the type's params to the instantiation's
    /// args and checks each bound. Returns the first failure, or None if the
    /// condition holds (or the args aren't concrete yet — deferred).
    fn check_conformance_condition(
        &mut self,
        ty: &Type,
        type_id: crate::types::TypeId,
        cond: &[(String, Vec<String>)],
        span: Span,
    ) -> Option<TraitError> {
        use crate::types::GenericArg;
        let type_params = match self.types.get(type_id) {
            Some(TypeDef::Struct { type_params, .. } | TypeDef::Enum { type_params, .. }) => {
                type_params.clone()
            }
            _ => return None,
        };
        let args: Vec<Type> = match ty {
            Type::Generic { args, .. } => args.iter().filter_map(|a| match a {
                GenericArg::Type(t) => Some((**t).clone()),
                _ => None,
            }).collect(),
            // Not instantiated with concrete type args — defer (checked at the
            // outermost concrete use).
            _ => return None,
        };
        // Bail if any argument is still abstract (a type var or bare param) —
        // the condition is verified once the args become concrete.
        if args.iter().any(is_abstract_arg) {
            return None;
        }
        let subst: std::collections::HashMap<&str, &Type> =
            type_params.iter().map(|s| s.as_str()).zip(args.iter().map(|t| t)).collect();
        for (param, bounds) in cond {
            if let Some(arg_ty) = subst.get(param.as_str()) {
                let arg_ty = (*arg_ty).clone();
                for bound in bounds {
                    if let Err(e) = self.check_satisfies(&arg_ty, bound, span) {
                        return Some(e);
                    }
                }
            }
        }
        None
    }

    /// Check if a type satisfies all bounds.
    pub fn check_bounds(
        &mut self,
        concrete_type: &Type,
        bounds: &[TraitBound],
        span: Span,
    ) -> Vec<TraitError> {
        let mut errors = Vec::new();

        for bound in bounds {
            for trait_name in &bound.traits {
                if let Err(e) = self.check_satisfies(concrete_type, trait_name, span) {
                    errors.push(e);
                }
            }
        }

        errors
    }

    /// Get methods required by a trait (public accessor for trait object resolution).
    pub fn get_trait_methods_public(&self, trait_name: &str) -> Vec<MethodSig> {
        self.get_trait_methods(trait_name).unwrap_or_default()
    }

    /// Get methods required by a trait.
    fn get_trait_methods(&self, trait_name: &str) -> Result<Vec<MethodSig>, TraitError> {
        // Strip generic args: "Iterator<i64>" → "Iterator"
        let base_name = trait_name.split('<').next().unwrap_or(trait_name);
        self.trait_methods
            .get(trait_name)
            .or_else(|| self.trait_methods.get(base_name))
            .cloned()
            .or_else(|| self.get_builtin_trait_methods(base_name))
            .ok_or_else(|| TraitError::UnknownTrait(trait_name.to_string()))
    }

    /// Get builtin trait methods for standard traits.
    fn get_builtin_trait_methods(&self, trait_name: &str) -> Option<Vec<MethodSig>> {
        match trait_name {
            "Add" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "add".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)], // Self type
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Sub" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "sub".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Mul" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "mul".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Div" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "div".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Rem" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "rem".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Neg" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "neg".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Equal" | "Eq" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "eq".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Bool,
            }]),
            "Comparable" | "Ord" => Some(vec![
                MethodSig {
                    type_params: Vec::new(),
                    name: "compare".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    // Returns Ordering enum — use Var as placeholder (structural check)
                    ret: Type::Var(crate::types::TypeVarId(0)),
                },
                MethodSig {
                    type_params: Vec::new(),
                    name: "lt".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    type_params: Vec::new(),
                    name: "le".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    type_params: Vec::new(),
                    name: "gt".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    type_params: Vec::new(),
                    name: "ge".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
            ]),
            "Clone" | "Cloneable" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "clone".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Default" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "default".to_string(),
                self_param: SelfParam::None, // Static method
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Hashable" => Some(vec![
                MethodSig {
                    type_params: Vec::new(),
                    name: "hash".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![],
                    ret: Type::U64,
                },
                MethodSig {
                    type_params: Vec::new(),
                    name: "eq".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
            ]),
            "Displayable" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "to_string".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::String,
            }]),
            "Debug" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "debug_string".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::String,
            }]),
            // Iterator<Item> trait — single method `next(mutate self) -> Item?`
            "Iterator" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "next".to_string(),
                self_param: SelfParam::Mutate,
                params: vec![],
                ret: Type::option(Type::Var(crate::types::TypeVarId(0))),
            }]),
            // ER4/ER32: ErrorMessage trait — `func message(self) -> string`
            "ErrorMessage" => Some(vec![MethodSig {
                type_params: Vec::new(),
                name: "message".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::String,
            }]),
            _ => None,
        }
    }

    /// Get methods available on a type.
    fn get_type_methods(&self, ty: &Type) -> Vec<MethodSig> {
        let id = match ty {
            Type::Named(id) => Some(*id),
            // A generic instantiation carries the base type's methods.
            Type::Generic { base, .. } => Some(*base),
            _ => None,
        };
        match id.and_then(|id| self.types.get(id)) {
            Some(TypeDef::Struct { methods, .. }) => methods.clone(),
            Some(TypeDef::Enum { methods, .. }) => methods.clone(),
            Some(TypeDef::Trait { methods, .. }) => methods.clone(),
            // Primitives / unions / aliases have builtin methods checked separately.
            _ => Vec::new(),
        }
    }

    /// Check if a primitive type has a builtin method.
    fn has_builtin_method(&self, ty: &Type, method: &str) -> bool {
        match ty {
            // Integer types: eq, hash, clone, default, arithmetic, compare, to_string
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 |
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => {
                matches!(method,
                    "add" | "sub" | "mul" | "div" | "rem" |
                    "neg" | "eq" | "lt" | "le" | "gt" | "ge" | "compare" |
                    "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr" | "bit_not" |
                    "hash" | "clone" | "default" | "to_string" | "debug_string"
                )
            }
            // Floats: eq, clone, default, but NOT hash (HA4)
            Type::F32 | Type::F64 => {
                matches!(method,
                    "add" | "sub" | "mul" | "div" | "rem" |
                    "neg" | "eq" | "lt" | "le" | "gt" | "ge" | "compare" |
                    "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr" | "bit_not" |
                    "clone" | "default" | "to_string" | "debug_string"
                )
            }
            // Bool: eq, hash, clone, default, compare, to_string
            Type::Bool => matches!(method, "eq" | "compare" | "hash" | "clone" | "default" | "to_string" | "debug_string"),
            // Char: eq, hash, clone, default, comparison, to_string
            Type::Char => matches!(method, "eq" | "lt" | "le" | "gt" | "ge" | "compare" | "hash" | "clone" | "default" | "to_string" | "debug_string"),
            // String: eq, hash, clone, default, len, comparison, to_string
            Type::String => matches!(method, "eq" | "lt" | "le" | "gt" | "ge" | "compare" | "len" | "clone" | "hash" | "default" | "to_string" | "debug_string"),
            // Unit: eq, hash, clone, default
            Type::Unit => matches!(method, "eq" | "hash" | "clone" | "default" | "to_string" | "debug_string"),
            _ => false,
        }
    }

    /// Check if two method signatures match.
    fn signatures_match(&self, required: &MethodSig, found: &MethodSig) -> bool {
        if required.self_param != found.self_param {
            return false;
        }

        if required.params.len() != found.params.len() {
            return false;
        }

        // Check parameter modes and types per position.
        // Type::Var represents Self in builtin trait signatures, and
        // `UnresolvedNamed("Self")` is the written-out Self of a declared
        // trait — both stand in for the implementing type, so skip the type
        // comparison when either side is one of those.
        for ((req_ty, req_mode), (found_ty, found_mode)) in
            required.params.iter().zip(found.params.iter())
        {
            if req_mode != found_mode {
                return false;
            }
            if !is_self_placeholder(req_ty)
                && !matches!(found_ty, Type::Var(_))
                && req_ty != found_ty
            {
                return false;
            }
        }

        // Check return type (skip Self placeholders)
        if !is_self_placeholder(&required.ret)
            && !matches!(found.ret, Type::Var(_))
            && required.ret != found.ret
        {
            return false;
        }

        true
    }

    /// Format a method signature for error messages.
    fn format_signature(&self, sig: &MethodSig) -> String {
        let self_str = match sig.self_param {
            SelfParam::None => "",
            SelfParam::Value => "self, ",
            SelfParam::Mutate => "mutate self, ",
            SelfParam::Take => "take self, ",
        };
        let params_str: Vec<String> = sig.params.iter().map(|(t, mode)| {
            match mode {
                ParamMode::Take => format!("take {:?}", t),
                ParamMode::Mutate => format!("mutate {:?}", t),
                ParamMode::Default => format!("{:?}", t),
            }
        }).collect();
        format!("fn {}({}{}) -> {:?}", sig.name, self_str, params_str.join(", "), sig.ret)
    }

    /// Get a human-readable name for a type.
    fn type_name(&self, ty: &Type) -> String {
        match ty {
            Type::Named(id) => {
                if let Some(def) = self.types.get(*id) {
                    match def {
                        TypeDef::Struct { name, .. } => name.clone(),
                        TypeDef::Enum { name, .. } => name.clone(),
                        TypeDef::Trait { name, .. } => name.clone(),
                        TypeDef::Union { name, .. } => name.clone(),
                        TypeDef::NominalAlias { name, .. } => name.clone(),
                    }
                } else {
                    format!("Type({})", id.0)
                }
            }
            _ => format!("{:?}", ty),
        }
    }

    /// Consume the checker and return any errors.
    pub fn into_errors(self) -> Vec<TraitError> {
        self.errors
    }
}

// ============================================================================
// Trait Satisfaction Verification
// ============================================================================

/// Verify trait satisfaction at a generic instantiation site.
pub fn verify_instantiation(
    types: &TypeTable,
    concrete_type: &Type,
    bounds: &[TraitBound],
    span: Span,
) -> Result<(), Vec<TraitError>> {
    let mut checker = TraitChecker::new(types);
    let errors = checker.check_bounds(concrete_type, bounds, span);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Check if a type implements a specific trait.
/// True if `ty` stands in for the implementing type in a trait signature:
/// a builtin-trait type variable, or the written-out `Self`.
fn is_self_placeholder(ty: &Type) -> bool {
    matches!(ty, Type::Var(_)) || matches!(ty, Type::UnresolvedNamed(n) if n == "Self")
}

/// A generic argument that isn't a concrete type yet — an inference var or a
/// bare type parameter. CC1 conditions on these are deferred until concrete.
fn is_abstract_arg(ty: &Type) -> bool {
    match ty {
        Type::Var(_) | Type::Error => true,
        Type::UnresolvedNamed(n) => {
            let mut chars = n.chars();
            matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
        }
        _ => false,
    }
}

pub fn implements_trait(
    types: &TypeTable,
    ty: &Type,
    trait_name: &str,
) -> bool {
    let mut checker = TraitChecker::new(types);
    checker.check_satisfies(ty, trait_name, Span::new(0, 0)).is_ok()
}

/// Get all traits that a type implements.
pub fn implemented_traits(types: &TypeTable, ty: &Type) -> Vec<String> {
    let mut result = Vec::new();
    // Check against known traits
    let known_traits = [
        "Add", "Sub", "Mul", "Div", "Rem", "Neg",
        "Equal", "Eq", "Comparable", "Ord",
        "Clone", "Cloneable", "Default", "Hashable",
        "Displayable", "Debug",
    ];

    for trait_name in known_traits {
        let mut checker = TraitChecker::new(types);
        if checker.check_satisfies(ty, trait_name, Span::new(0, 0)).is_ok() {
            result.push(trait_name.to_string());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_primitive_trait_satisfaction() {
        let types = TypeTable::new();

        // i32 should implement Add
        assert!(implements_trait(&types, &Type::I32, "Add"));
        assert!(implements_trait(&types, &Type::I32, "Equal"));
        assert!(implements_trait(&types, &Type::I32, "Comparable"));
    }

    // CC1: `extend Ring<T> with Show where T: Show` — the conformance holds for
    // Ring<Coin> (Coin: Show) and fails for Ring<Blob> (Blob not Show).
    #[test]
    fn conditional_conformance_checks_argument() {
        use crate::checker::{MethodSig, SelfParam};
        use crate::types::GenericArg;
        use rask_ast::Span;

        let mut types = TypeTable::new();
        let show = || MethodSig {
            type_params: Vec::new(),
            name: "show".to_string(),
            self_param: SelfParam::Value,
            params: vec![],
            ret: Type::String,
        };

        types.register_type(TypeDef::Trait {
            name: "Show".to_string(),
            super_traits: vec![],
            methods: vec![show()],
            generic_methods: vec![],
            is_unsafe: false,
            is_duck: false,
        });
        let ring = types.register_type(TypeDef::Struct {
            name: "Ring".to_string(),
            type_params: vec!["T".to_string()],
            fields: vec![],
            methods: vec![show()],
            is_resource: false,
            is_unique: false,
            is_binary: false,
            private_fields: vec![],
            is_transitive_resource: false,
        });
        let coin = types.register_type(TypeDef::Struct {
            name: "Coin".to_string(),
            type_params: vec![],
            fields: vec![],
            methods: vec![show()],
            is_resource: false,
            is_unique: false,
            is_binary: false,
            private_fields: vec![],
            is_transitive_resource: false,
        });
        let blob = types.register_type(TypeDef::Struct {
            name: "Blob".to_string(),
            type_params: vec![],
            fields: vec![],
            methods: vec![],
            is_resource: false,
            is_unique: false,
            is_binary: false,
            private_fields: vec![],
            is_transitive_resource: false,
        });

        // extend Ring<T> with Show where T: Show
        types.record_conformance(ring, "Show");
        types.record_conformance_condition(ring, "Show", vec![("T".to_string(), vec!["Show".to_string()])]);
        // extend Coin with Show
        types.record_conformance(coin, "Show");

        let ring_of = |arg: crate::types::TypeId| Type::Generic {
            base: ring,
            args: vec![GenericArg::Type(Box::new(Type::Named(arg)))],
        };

        let mut checker = TraitChecker::new(&types);
        assert!(checker.check_satisfies(&ring_of(coin), "Show", Span::new(0, 0)).is_ok(),
            "Ring<Coin> should satisfy Show (Coin: Show)");
        assert!(checker.check_satisfies(&ring_of(blob), "Show", Span::new(0, 0)).is_err(),
            "Ring<Blob> must NOT satisfy Show (Blob is not Show)");
    }
}

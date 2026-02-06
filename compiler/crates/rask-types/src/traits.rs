//! Trait checking for Rask.
//!
//! Implements structural trait satisfaction: a type satisfies a trait if it has
//! all required methods with matching signatures.

use crate::types::Type;
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
        for def in self.types.iter() {
            if let TypeDef::Trait { name, methods } = def {
                self.trait_methods.insert(name.clone(), methods.clone());
            }
        }
        // Note: Trait composition (`: SuperTrait`) would require parsing the
        // trait definition and collecting supertraits. For now, we just
        // collect direct methods.
    }

    /// Check if a type satisfies a trait bound.
    pub fn check_satisfies(
        &mut self,
        ty: &Type,
        trait_name: &str,
        span: Span,
    ) -> Result<(), TraitError> {
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

    /// Get methods required by a trait.
    fn get_trait_methods(&self, trait_name: &str) -> Result<Vec<MethodSig>, TraitError> {
        self.trait_methods
            .get(trait_name)
            .cloned()
            .or_else(|| self.get_builtin_trait_methods(trait_name))
            .ok_or_else(|| TraitError::UnknownTrait(trait_name.to_string()))
    }

    /// Get builtin trait methods for standard traits.
    fn get_builtin_trait_methods(&self, trait_name: &str) -> Option<Vec<MethodSig>> {
        match trait_name {
            "Add" => Some(vec![MethodSig {
                name: "add".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)], // Self type
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Sub" => Some(vec![MethodSig {
                name: "sub".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Mul" => Some(vec![MethodSig {
                name: "mul".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Div" => Some(vec![MethodSig {
                name: "div".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Rem" => Some(vec![MethodSig {
                name: "rem".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Neg" => Some(vec![MethodSig {
                name: "neg".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Equal" | "Eq" => Some(vec![MethodSig {
                name: "eq".to_string(),
                self_param: SelfParam::Value,
                params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                ret: Type::Bool,
            }]),
            "Comparable" | "Ord" => Some(vec![
                MethodSig {
                    name: "lt".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    name: "le".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    name: "gt".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
                MethodSig {
                    name: "ge".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
            ]),
            "Clone" => Some(vec![MethodSig {
                name: "clone".to_string(),
                self_param: SelfParam::Value,
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Default" => Some(vec![MethodSig {
                name: "default".to_string(),
                self_param: SelfParam::None, // Static method
                params: vec![],
                ret: Type::Var(crate::types::TypeVarId(0)),
            }]),
            "Hashable" => Some(vec![
                MethodSig {
                    name: "hash".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![],
                    ret: Type::U64,
                },
                MethodSig {
                    name: "eq".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Var(crate::types::TypeVarId(0)), ParamMode::Default)],
                    ret: Type::Bool,
                },
            ]),
            _ => None,
        }
    }

    /// Get methods available on a type.
    fn get_type_methods(&self, ty: &Type) -> Vec<MethodSig> {
        match ty {
            Type::Named(id) => {
                if let Some(def) = self.types.get(*id) {
                    match def {
                        TypeDef::Struct { methods, .. } => methods.clone(),
                        TypeDef::Enum { methods, .. } => methods.clone(),
                        TypeDef::Trait { methods, .. } => methods.clone(),
                    }
                } else {
                    Vec::new()
                }
            }
            // Primitives have builtin methods checked separately
            _ => Vec::new(),
        }
    }

    /// Check if a primitive type has a builtin method.
    fn has_builtin_method(&self, ty: &Type, method: &str) -> bool {
        match ty {
            // Numeric types have arithmetic methods
            Type::I8 | Type::I16 | Type::I32 | Type::I64 |
            Type::U8 | Type::U16 | Type::U32 | Type::U64 |
            Type::F32 | Type::F64 => {
                matches!(method,
                    "add" | "sub" | "mul" | "div" | "rem" |
                    "neg" | "eq" | "lt" | "le" | "gt" | "ge" |
                    "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr" | "bit_not"
                )
            }
            // Bool has equality
            Type::Bool => matches!(method, "eq"),
            // Char has equality and comparison
            Type::Char => matches!(method, "eq" | "lt" | "le" | "gt" | "ge"),
            // String has many methods
            Type::String => matches!(method, "eq" | "len" | "clone"),
            _ => false,
        }
    }

    /// Check if two method signatures match.
    fn signatures_match(&self, required: &MethodSig, found: &MethodSig) -> bool {
        // Check self parameter
        if required.self_param != found.self_param {
            return false;
        }

        // Check parameter count
        if required.params.len() != found.params.len() {
            return false;
        }

        // Note: For full signature matching, we'd need to unify type variables
        // and check parameter/return types. For now, we do a simpler check
        // that the shapes match (same number of params).

        true
    }

    /// Format a method signature for error messages.
    fn format_signature(&self, sig: &MethodSig) -> String {
        let self_str = match sig.self_param {
            SelfParam::None => "",
            SelfParam::Value => "self, ",
            SelfParam::Read => "read self, ",
            SelfParam::Take => "take self, ",
        };
        let params_str: Vec<String> = sig.params.iter().map(|(t, mode)| {
            match mode {
                ParamMode::Take => format!("take {:?}", t),
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
        "Clone", "Default", "Hashable",
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
}

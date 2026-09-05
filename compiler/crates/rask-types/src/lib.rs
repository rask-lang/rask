// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type system and type checker for the Rask language.
//!
//! Performs type inference and checking on the AST.

mod types;
mod checker;
mod traits;
pub mod reflect;

pub use types::{GenericArg, Type, TypeId, TypeVarId};
pub use checker::{
    typecheck, typecheck_with_stdlib, typecheck_with_stdlib_lenient, TypeChecker, TypedProgram, TypeTable, TypeDef,
    TypeError, MapKeyFix, InvalidCastClass, IndexErrorKind, TraitBoundContext, InferenceContext, TypeConstraint, MethodSig, SelfParam,
    ParamMode, Callee, ErrorWrap, receiver_name, BoundFrom, TypeBinding,
    parse_type_string, signature_type_param_names, struct_type_param_names,
    enum_type_param_names, UnsafeCategory, binary_field_runtime_type,
};
pub use traits::{
    TraitBound, TraitChecker, TraitError,
    verify_instantiation, implements_trait, implemented_traits,
    COMPILER_PROVIDED_TRAITS, builtin_trait_method_names, object_compatible_methods,
};

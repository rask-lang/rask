// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type system and type checker for the Rask language.
//!
//! Performs type inference and checking on the AST.

mod types;
mod checker;
mod interfaces;
mod copy_rule;
pub mod reflect;

pub use types::{GenericArg, Type, TypeId, TypeVarId};
pub use copy_rule::CopyVerdict;
pub use checker::{
    typecheck, typecheck_with_stdlib, typecheck_with_stdlib_lenient, TypeChecker, TypedProgram, WrapperFns, TypeTable, TypeDef,
    TypeError, TypeArgSite, MapKeyFix, InvalidCastClass, IndexErrorKind, InterfaceBoundContext, InferenceContext, TypeConstraint, MethodSig, SelfParam,
    ParamMode, Callee, ErrorWrap, receiver_name, conformance_symbol, BoundFrom, TypeBinding,
    OperatorTarget, operator_interface, primitive_spelling, TaskBound,
    signature_type_param_names, struct_type_param_names,
    bind_header_pattern, bind_header_patterns,
    enum_type_param_names, UnsafeCategory, binary_field_runtime_type,
};
pub use interfaces::{
    InterfaceBound, InterfaceChecker, InterfaceError,
    verify_instantiation, implements_interface, bound_implies_copy, implemented_interfaces, substitute_type,
    COMPILER_PROVIDED_TRAITS, builtin_interface_method_names, object_compatible_methods, interface_vtable_methods,
};

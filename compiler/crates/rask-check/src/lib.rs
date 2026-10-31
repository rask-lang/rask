// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The type checker: inference and checking on the AST. What it produces is
//! the data in `rask-types`.

mod checker;
mod interfaces;
mod copy_rule;
pub mod reflect;
pub mod layout;
mod diagnostic;

// The data this crate produces and reads, by the names the checker's modules
// have always used for it.
use rask_types::{FnParam, GenericArg, Type, TypeId};
pub use copy_rule::CopyVerdict;
pub use checker::{typecheck, typecheck_with_stdlib, typecheck_with_stdlib_lenient, CContext, TypeChecker, TypedProgram, WrapperFns, TypeTable, ArgProblem, TypeError, TypeArgSite, MapKeyFix, InvalidCastClass, IndexErrorKind, InterfaceBoundContext, KeptBy, InferenceContext, TypeConstraint, Callee, receiver_name, BoundFrom, OperatorTarget, operator_interface, primitive_spelling, TaskBound, header_type_params, signature_type_param_names, struct_type_param_names, bind_header_pattern, bind_header_patterns, enum_type_param_names, UnsafeCategory, binary_field_runtime_type, is_type_param_name};
pub use rask_types::TypeDef;
pub use interfaces::{
    InterfaceBound, InterfaceChecker, InterfaceError,
    verify_instantiation, implements_interface, bound_implies_copy, substitute_type,
    COMPILER_PROVIDED_TRAITS, builtin_interface_method_names, object_compatible_methods, interface_vtable_methods,
};

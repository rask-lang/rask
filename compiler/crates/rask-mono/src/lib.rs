// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Monomorphization pass - eliminates generics by instantiating concrete copies.
//!
//! Takes type-checked AST and produces monomorphized program with:
//! - Concrete function instances for each unique (function_id, [type_args])
//! - Computed memory layouts for all structs and enums
//! - Reachability analysis starting from main()

mod instantiate;
mod layout;
mod reachability;

pub use instantiate::instantiate_function;
pub use layout::{compute_enum_layout, compute_struct_layout, EnumLayout, FieldLayout, StructLayout, VariantLayout};
pub use reachability::collect_reachable;

use rask_ast::decl::Decl;
use rask_types::{Type, TypedProgram};

/// Monomorphized program with all generics eliminated
pub struct MonoProgram {
    pub functions: Vec<MonoFunction>,
    pub struct_layouts: Vec<StructLayout>,
    pub enum_layouts: Vec<EnumLayout>,
}

/// Monomorphized function instance
pub struct MonoFunction {
    pub name: String,
    pub type_args: Vec<Type>,
    pub body: Decl,
}

/// Monomorphize a type-checked program
pub fn monomorphize(program: &TypedProgram) -> Result<MonoProgram, MonomorphizeError> {
    todo!("Implement monomorphization")
}

#[derive(Debug)]
pub enum MonomorphizeError {
    NoEntryPoint,
    UnresolvedGeneric { function_name: String, type_param: String },
    LayoutError { type_name: String, reason: String },
}

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
pub fn monomorphize(_program: &TypedProgram) -> Result<MonoProgram, MonomorphizeError> {
    // TODO: Full implementation requires:
    // 1. Find main() or entry point in program
    // 2. Run reachability analysis: collect_reachable(main_decl)
    // 3. For each (function_name, type_args) pair:
    //    - Find function declaration in program
    //    - Call instantiate_function(decl, &type_args)
    // 4. Compute layouts: compute_struct_layout/compute_enum_layout for all types
    // 5. Return MonoProgram with functions and layouts

    // For now, return empty program to allow integration testing
    Ok(MonoProgram {
        functions: Vec::new(),
        struct_layouts: Vec::new(),
        enum_layouts: Vec::new(),
    })
}

#[derive(Debug)]
pub enum MonomorphizeError {
    NoEntryPoint,
    UnresolvedGeneric { function_name: String, type_param: String },
    LayoutError { type_name: String, reason: String },
}

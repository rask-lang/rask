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
pub use layout::{
    compute_enum_layout, compute_struct_layout, EnumLayout, FieldLayout, StructLayout,
    VariantLayout,
};
pub use reachability::Monomorphizer;

use rask_ast::decl::{Decl, DeclKind};
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

/// Monomorphize a type-checked program.
///
/// Architecture: reachability drives instantiation (tree-shaking).
/// Only functions reachable from main() get instantiated.
///
/// 1. Build function lookup table from declarations
/// 2. BFS from main(): discover calls → instantiate on demand → walk instantiated body
/// 3. Compute layouts for all referenced structs/enums
pub fn monomorphize(
    _program: &TypedProgram,
    decls: &[Decl],
) -> Result<MonoProgram, MonomorphizeError> {
    let mut mono = Monomorphizer::new(decls);

    if !mono.add_entry("main") {
        return Err(MonomorphizeError::NoEntryPoint);
    }

    mono.run();

    // Compute layouts for all referenced struct/enum types
    let mut struct_layouts = Vec::new();
    let mut enum_layouts = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Struct(_) => {
                struct_layouts.push(compute_struct_layout(decl, &[]));
            }
            DeclKind::Enum(_) => {
                enum_layouts.push(compute_enum_layout(decl, &[]));
            }
            _ => {}
        }
    }

    Ok(MonoProgram {
        functions: mono.results,
        struct_layouts,
        enum_layouts,
    })
}

#[derive(Debug)]
pub enum MonomorphizeError {
    NoEntryPoint,
    UnresolvedGeneric {
        function_name: String,
        type_param: String,
    },
    LayoutError {
        type_name: String,
        reason: String,
    },
}

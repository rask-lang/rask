// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function instantiation - clone AST and substitute type parameters.

use rask_ast::decl::Decl;
use rask_types::Type;

/// Instantiate a generic function with concrete type arguments
///
/// Clones the function AST and replaces all type parameters with concrete types
pub fn instantiate_function(decl: &Decl, type_args: &[Type]) -> Decl {
    todo!("Implement function instantiation")
}

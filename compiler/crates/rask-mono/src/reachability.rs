// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Reachability analysis - walk call graph from main() to find generic instantiations.

use rask_ast::decl::Decl;
use rask_types::Type;

/// Collect reachable function instances starting from entry point
///
/// Returns (function_id, concrete_type_args) pairs for all reachable calls
pub fn collect_reachable(entry: &Decl) -> Vec<(String, Vec<Type>)> {
    todo!("Implement reachability analysis")
}

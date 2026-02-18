// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type method definitions for the Rask stdlib.
//!
//! Delegates to the stub registry — .rk files in stdlib/ are the source of truth.

pub use crate::stubs::MethodStub;

use crate::stubs::StubRegistry;

/// Look up a method by type name and method name.
pub fn lookup_method(type_name: &str, method_name: &str) -> Option<&'static MethodStub> {
    let reg = StubRegistry::load();
    let normalized = match type_name {
        "String" => "string",
        _ => type_name,
    };
    reg.lookup_method(normalized, method_name)
}

/// Check if a method exists on a type.
pub fn has_method(type_name: &str, method_name: &str) -> bool {
    lookup_method(type_name, method_name).is_some()
}

/// Get all methods for a type.
pub fn methods_for(type_name: &str) -> &'static [MethodStub] {
    let reg = StubRegistry::load();
    let normalized = match type_name {
        "String" => "string",
        _ => type_name,
    };
    reg.methods(normalized)
}

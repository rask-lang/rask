// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type method definitions for the Rask stdlib.
//!
//! Delegates to the stub registry — .rk files in stdlib/ are the source of truth.

pub use crate::stubs::MethodStub;

use crate::stubs::StubRegistry;
use std::collections::HashSet;
use std::sync::OnceLock;

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

/// True if any stdlib type has a method with this name declared `mutate self`.
/// Used as a fallback when the receiver's type isn't resolved yet (e.g.
/// `const v = Vec.new(); v.push(1)` before constraint solving runs).
pub fn any_builtin_method_mutates(method_name: &str) -> bool {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    let set = CACHE.get_or_init(|| {
        let reg = StubRegistry::load();
        let mut names = HashSet::new();
        for type_name in reg.type_names() {
            for m in reg.methods(type_name) {
                if m.mutate_self {
                    names.insert(m.name.clone());
                }
            }
        }
        names
    });
    set.contains(method_name)
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What the stdlib's interfaces require, read off their declarations.
//!
//! Passes with a type table ask it; this is for the ones that have only a
//! name in hand (MIR asking whether `hash` on a channel end is a provided
//! method). The checker used to keep its own copy of these signatures in
//! Rust, which nothing held to the declarations.

use rask_ast::decl::{DeclKind, InterfaceDecl};

/// The stdlib's public interface called `name`.
pub fn declared(name: &str) -> Option<&'static InterfaceDecl> {
    crate::stubs::parsed_decls().find_map(|d| match &d.kind {
        DeclKind::Interface(i) if i.is_pub && i.name == name => Some(i),
        _ => None,
    })
}

/// Whether the stdlib interface `interface` requires a method `method`, its
/// super-interfaces' included.
pub fn requires(interface: &str, method: &str) -> bool {
    requires_seen(interface, method, &mut Vec::new())
}

fn requires_seen(interface: &str, method: &str, seen: &mut Vec<String>) -> bool {
    if seen.iter().any(|s| s == interface) {
        return false;
    }
    seen.push(interface.to_string());
    let Some(decl) = declared(interface) else { return false };
    decl.methods.iter().any(|m| m.name == method)
        || decl
            .super_interfaces
            .iter()
            .filter_map(|p| p.name())
            .any(|p| requires_seen(&p, method, seen))
}

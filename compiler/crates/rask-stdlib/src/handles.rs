// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime handles the checker leaves as spelled types.
//!
//! Every declared handle is asked about by its declaration
//! (`TypeDef::runtime_handle`, `TypeTable::is_handle`). This answers the
//! question for a type that reaches a later pass as a name with no id
//! (`Type::UnresolvedGeneric`), and only for those.

use rask_ast::decl::DeclKind;
use rask_types::RuntimeHandle;

/// The compiler-provided types (mem.shared-rack-heap, BX1–BX4). They have no
/// declaration, so no `TypeId`: the checker can only spell them.
const COMPILER_PROVIDED: &[(&str, RuntimeHandle<'static>)] = &[
    ("Shared", RuntimeHandle { release: None, niche: false }),
    ("Heap", RuntimeHandle { release: None, niche: false }),
    // Only ever named to make a pair of ends; the ends are the values.
    ("Channel", RuntimeHandle { release: None, niche: false }),
];

/// Declared in stdlib/async.rk, but the checker builds a channel's two ends
/// by name (`channel_ends`, `Sender.clone`) rather than from the declaration,
/// so they arrive spelled. Read off their declaration, so the facts live in
/// one place; drop these once the checker gives the ends their ids (#1855).
const CHECKER_SPELLED: &[&str] = &["Sender", "Receiver"];

/// The handle a spelled type name is, if it is one. `name` is the bare type
/// name, without type arguments, and must come from a type with no id: a
/// resolved type is asked about by its declaration.
pub fn spelled_handle(name: &str) -> Option<RuntimeHandle<'static>> {
    if let Some((_, h)) = COMPILER_PROVIDED.iter().find(|(n, _)| *n == name) {
        return Some(*h);
    }
    if !CHECKER_SPELLED.contains(&name) {
        return None;
    }
    crate::stubs::parsed_decls().find_map(|decl| match &decl.kind {
        DeclKind::Struct(s) if s.name == name => RuntimeHandle::declared(&s.attrs),
        _ => None,
    })
}

/// What frees a stdlib type that declares `@handle(release: "…")`, by its
/// name: the C symbol the declaration names.
pub fn declared_release(type_name: &str) -> Option<&'static str> {
    crate::stubs::parsed_decls().find_map(|decl| match &decl.kind {
        DeclKind::Struct(s) if s.name == type_name => RuntimeHandle::declared(&s.attrs)?.release,
        _ => None,
    })
}

/// Every release a stdlib handle declares.
pub fn declared_releases() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = crate::stubs::parsed_decls()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Struct(s) => RuntimeHandle::declared(&s.attrs)?.release,
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether `symbol` is some stdlib handle's declared release.
pub fn is_declared_release(symbol: &str) -> bool {
    crate::stubs::parsed_decls().any(|decl| match &decl.kind {
        DeclKind::Struct(s) => RuntimeHandle::declared(&s.attrs).and_then(|h| h.release) == Some(symbol),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_provided_types_are_handles() {
        assert!(spelled_handle("Shared").is_some());
        assert!(spelled_handle("Heap").is_some());
        assert!(spelled_handle("Channel").is_some());
    }

    #[test]
    fn spelled_channel_ends_read_their_declaration() {
        assert_eq!(spelled_handle("Sender"), Some(RuntimeHandle { release: None, niche: false }));
        assert!(spelled_handle("Receiver").is_some());
    }

    /// A declared handle has an id, and is asked about by it.
    #[test]
    fn a_declared_handle_is_not_answered_by_name() {
        assert!(spelled_handle("Vec").is_none());
        assert!(spelled_handle("Link").is_none());
        assert!(spelled_handle("Set").is_none());
    }
}

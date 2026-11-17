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

/// What frees a `Shared<T, S>`, one per strategy (conc.sync/SH2): a counted
/// box, a mutex and a cell. `Shared` has no declaration to carry
/// `@handle(release: …)`, and which of the three a value needs is its second
/// type argument, so they are listed here and
/// `rask_mono::drop_names::box_release_for` picks one.
pub const SHARED_RELEASES: &[&str] = &["rask_shared_drop_i64", "rask_mutex_drop", "rask_cell_free"];

/// The release of a `Shared` box with strategy `strategy` (`Local`, `Mutex`;
/// anything else is `Readers`).
pub fn shared_release(strategy: Option<&str>) -> &'static str {
    match strategy {
        Some("Local") => SHARED_RELEASES[2],
        Some("Mutex") => SHARED_RELEASES[1],
        _ => SHARED_RELEASES[0],
    }
}

/// Every release a handle declares, the compiler-provided boxes' included.
pub fn declared_releases() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = crate::stubs::parsed_decls()
        .filter_map(|decl| match &decl.kind {
            DeclKind::Struct(s) => RuntimeHandle::declared(&s.attrs)?.release,
            _ => None,
        })
        .chain(SHARED_RELEASES.iter().copied())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether `symbol` is some handle's declared release.
pub fn is_declared_release(symbol: &str) -> bool {
    SHARED_RELEASES.contains(&symbol)
        || crate::stubs::parsed_decls().any(|decl| match &decl.kind {
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
        assert_eq!(
            spelled_handle("Sender"),
            Some(RuntimeHandle { release: Some("rask_sender_drop_i64"), niche: false })
        );
        assert_eq!(
            spelled_handle("Receiver"),
            Some(RuntimeHandle { release: Some("rask_recver_drop_i64"), niche: false })
        );
    }

    /// A declared handle has an id, and is asked about by it.
    #[test]
    fn a_declared_handle_is_not_answered_by_name() {
        assert!(spelled_handle("Vec").is_none());
        assert!(spelled_handle("Link").is_none());
        assert!(spelled_handle("Set").is_none());
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared helpers used by more than one feature module.

use crate::backend::CompilationResult;

/// Given a `.` position, figure out what type the receiver evaluates to so
/// we can look up methods on it. Returns a stub-registry-ready type name.
///
/// Handles:
///   - literal receivers (`Vec.new()`)
///   - identifiers whose type was inferred
///   - falls back to the identifier name itself (e.g., stdlib modules)
pub fn resolve_receiver_type(
    source: &str,
    dot_pos: usize,
    cached: &CompilationResult,
) -> Option<String> {
    let before = source.get(..dot_pos)?;
    let ident_start = before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let receiver = &before[ident_start..];

    if receiver.is_empty() {
        return None;
    }

    // Direct stub type or module.
    let reg = rask_stdlib::StubRegistry::load();
    if reg.get_type(receiver).is_some() {
        return Some(receiver.to_string());
    }

    // Look up through the typed program.
    for (_span, node_id, name) in &cached.position_index.idents {
        if name == receiver {
            if let Some(ty) = cached.typed.node_types.get(node_id) {
                return type_to_stub_name(ty, cached);
            }
        }
    }

    None
}

pub fn type_to_stub_name(
    ty: &rask_types::Type,
    cached: &CompilationResult,
) -> Option<String> {
    match ty {
        rask_types::Type::String => Some("string".to_string()),
        rask_types::Type::Named(id) => Some(cached.typed.types.type_name(*id)),
        rask_types::Type::Generic { base, .. } => Some(cached.typed.types.type_name(*base)),
        rask_types::Type::UnresolvedNamed(name) => Some(name.clone()),
        rask_types::Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
        ty if ty.is_option() => Some("Option".to_string()),
        rask_types::Type::Result { .. } => Some("Result".to_string()),
        rask_types::Type::Array { .. } | rask_types::Type::Slice(_) => Some("Vec".to_string()),
        _ => None,
    }
}

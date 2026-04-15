// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Find references + rename.
//!
//! Both operations start the same way: find the `SymbolId` under the cursor,
//! then walk the resolution table collecting every `NodeId` that resolves to
//! it. Rename assembles those into a WorkspaceEdit; references produces a
//! list of Locations.

use std::collections::HashMap;

use tower_lsp::lsp_types::*;

use crate::backend::CompilationResult;

pub fn references(
    uri: &Url,
    position: Position,
    cached: &CompilationResult,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let symbol_id = symbol_at(position, cached)?;
    let mut locations = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Usages from the resolution map.
    for (node_id, &sid) in &cached.typed.resolutions {
        if sid != symbol_id {
            continue;
        }
        if let Some(span) = span_of_ident(*node_id, cached) {
            if seen.insert((span.start, span.end)) {
                locations.push(Location {
                    uri: uri.clone(),
                    range: cached.line_index.span_to_range(&cached.source, span),
                });
            }
        }
    }

    // Include the declaration site if the caller asked for it.
    if include_declaration {
        if let Some(sym) = cached.typed.symbols.get(symbol_id) {
            let span = sym.span;
            if span.end > 0 && seen.insert((span.start, span.end)) {
                locations.push(Location {
                    uri: uri.clone(),
                    range: cached.line_index.span_to_range(&cached.source, span),
                });
            }
        }
    }

    Some(locations)
}

pub fn rename(
    uri: &Url,
    position: Position,
    new_name: &str,
    cached: &CompilationResult,
) -> Option<WorkspaceEdit> {
    // Cheap sanity: identifier-like only. Bail if the client passes something
    // weird — better a no-op than a corrupted file.
    if !is_valid_identifier(new_name) {
        return None;
    }

    let locations = references(uri, position, cached, true)?;
    if locations.is_empty() {
        return None;
    }
    let mut edits: Vec<TextEdit> = locations
        .into_iter()
        .map(|loc| TextEdit::new(loc.range, new_name.to_string()))
        .collect();

    // Stable ordering (by range) helps clients show predictable previews.
    edits.sort_by(|a, b| {
        (a.range.start.line, a.range.start.character)
            .cmp(&(b.range.start.line, b.range.start.character))
    });

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    Some(WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    })
}

fn symbol_at(position: Position, cached: &CompilationResult) -> Option<rask_resolve::SymbolId> {
    let offset = cached.line_index.position_to_offset(&cached.source, position);
    let (node_id, name) = cached.position_index.ident_at_position(offset)?;

    // Case 1: usage site — the resolutions table points directly at the symbol.
    if let Some(&sid) = cached.typed.resolutions.get(&node_id) {
        return Some(sid);
    }

    // Case 2: declaration site (binding, struct decl name, parameter name).
    // Position index gives stmt.id or decl.id which isn't in resolutions.
    // Find the symbol by name + containing span.
    let (ident_span, _, _) = cached
        .position_index
        .idents
        .iter()
        .find(|(s, _, n)| s.start <= offset && offset <= s.end && n == &name)?;

    // Match by defining-span being identical to the ident span — that covers
    // let/const bindings and parameters, whose symbol.span is the name itself.
    cached.typed.symbols.iter().enumerate().find_map(|(idx, sym)| {
        if sym.name == name
            && sym.span.start == ident_span.start
            && sym.span.end == ident_span.end
        {
            Some(rask_resolve::SymbolId(idx as u32))
        } else {
            None
        }
    })
}

fn span_of_ident(node_id: rask_ast::NodeId, cached: &CompilationResult) -> Option<rask_ast::Span> {
    cached
        .position_index
        .idents
        .iter()
        .find(|(_, id, _)| *id == node_id)
        .map(|(span, _, _)| *span)
}

fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

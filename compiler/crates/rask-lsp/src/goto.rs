// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Go-to-definition resolution.

use tower_lsp::lsp_types::*;

use crate::backend::CompilationResult;
use crate::convert::LineIndex;

pub fn goto_definition(
    uri: &Url,
    position: Position,
    cached: &CompilationResult,
    root_uri: Option<&Url>,
) -> Option<GotoDefinitionResponse> {
    let offset = cached
        .line_index
        .position_to_offset(&cached.source, position);

    let (node_id, name) = cached.position_index.ident_at_position(offset)?;
    let symbol = cached
        .typed
        .resolutions
        .get(&node_id)
        .and_then(|&sid| cached.typed.symbols.get(sid));

    if let Some(symbol) = symbol {
        // Builtin: navigate to stub file if available.
        if symbol.span.start == 0 && symbol.span.end == 0 {
            return resolve_builtin_location(&symbol.name, None, root_uri);
        }

        if let Some(sibling) = cached.sibling_decl_names.get(&symbol.name) {
            if symbol.span.end <= sibling.source.len() {
                let sibling_idx = LineIndex::new(&sibling.source);
                let range = sibling_idx.span_to_range(&sibling.source, symbol.span);
                let sibling_uri = Url::from_file_path(&sibling.path).unwrap_or_else(|_| uri.clone());
                return Some(GotoDefinitionResponse::Scalar(Location {
                    uri: sibling_uri,
                    range,
                }));
            }
        }

        let range = cached.line_index.span_to_range(&cached.source, symbol.span);
        return Some(GotoDefinitionResponse::Scalar(Location { uri: uri.clone(), range }));
    }

    // Method calls on builtins don't have a symbol — try the stub registry.
    try_method_goto(&cached.source, offset, &name, cached, root_uri)
}

fn try_method_goto(
    source: &str,
    offset: usize,
    method_name: &str,
    cached: &CompilationResult,
    root_uri: Option<&Url>,
) -> Option<GotoDefinitionResponse> {
    let (ident_span, _, _) = cached
        .position_index
        .idents
        .iter()
        .find(|(s, _, name)| s.start <= offset && offset <= s.end && name == method_name)?;

    if ident_span.start == 0 {
        return None;
    }
    if *source.as_bytes().get(ident_span.start - 1)? != b'.' {
        return None;
    }

    let type_name = crate::util::resolve_receiver_type(source, ident_span.start - 1, cached)?;
    resolve_builtin_location(&type_name, Some(method_name), root_uri)
}

fn resolve_builtin_location(
    name: &str,
    method: Option<&str>,
    root_uri: Option<&Url>,
) -> Option<GotoDefinitionResponse> {
    let reg = rask_stdlib::StubRegistry::load();
    let (source_file, span) = if let Some(method_name) = method {
        let normalized = match name {
            "String" => "string",
            _ => name,
        };
        let m = reg.lookup_method(normalized, method_name)?;
        (&m.source_file, m.span)
    } else if let Some(ts) = reg.get_type(name) {
        (&ts.source_file, ts.span)
    } else {
        let f = reg.functions().iter().find(|f| f.name == name)?;
        (&f.source_file, f.span)
    };

    let (start_line, start_col) = reg.offset_to_lsp_position(source_file, span.start)?;
    let (end_line, end_col) = reg.offset_to_lsp_position(source_file, span.end)?;

    let root = root_uri?;
    let stub_path = format!("{}/{}", root.as_str().trim_end_matches('/'), source_file);
    let stub_uri = Url::parse(&stub_path).ok()?;

    Some(GotoDefinitionResponse::Scalar(Location {
        uri: stub_uri,
        range: Range::new(
            Position::new(start_line, start_col),
            Position::new(end_line, end_col),
        ),
    }))
}

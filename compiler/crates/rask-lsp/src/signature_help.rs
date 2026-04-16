// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Signature help — parameter hints shown as you type inside a call.
//!
//! Triggered by `(` and re-triggered by `,`. We parse the text to the left
//! of the cursor looking for the most recent unmatched `(` — the identifier
//! just before that paren is the function being called. The active parameter
//! is the number of top-level commas between the paren and the cursor.

use tower_lsp::lsp_types::*;

use rask_resolve::{Symbol, SymbolKind};

use crate::backend::CompilationResult;
use crate::type_format::TypeFormatter;

pub fn signature_help(position: Position, cached: &CompilationResult) -> Option<SignatureHelp> {
    let source = &cached.source;
    let offset = cached.line_index.position_to_offset(source, position);

    let (callee, active_param) = locate_call(source, offset)?;

    let formatter = TypeFormatter::new(&cached.typed.types);

    // User-defined function lookup.
    let user_sig = cached.typed.symbols.iter().find_map(|s| {
        if s.name == callee && matches!(s.kind, SymbolKind::Function { .. }) {
            Some(user_signature(s, cached, &formatter))
        } else {
            None
        }
    });
    if let Some(sig) = user_sig {
        return Some(wrap(vec![sig], active_param));
    }

    // Stdlib function lookup.
    let reg = rask_stdlib::StubRegistry::load();
    if let Some(f) = reg.functions().iter().find(|f| f.name == callee) {
        let param_info: Vec<ParameterInformation> = f
            .params
            .iter()
            .map(|(n, t)| ParameterInformation {
                label: ParameterLabel::Simple(format!("{}: {}", n, t)),
                documentation: None,
            })
            .collect();
        let params_display = f
            .params
            .iter()
            .map(|(n, t)| format!("{}: {}", n, t))
            .collect::<Vec<_>>()
            .join(", ");
        let label = format!("{}({}) -> {}", f.name, params_display, f.ret_ty);
        let info = SignatureInformation {
            label,
            documentation: f.doc.as_ref().map(|d| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: d.clone(),
                })
            }),
            parameters: Some(param_info),
            active_parameter: Some(active_param),
        };
        return Some(wrap(vec![info], active_param));
    }

    None
}

fn wrap(signatures: Vec<SignatureInformation>, active_param: u32) -> SignatureHelp {
    SignatureHelp {
        signatures,
        active_signature: Some(0),
        active_parameter: Some(active_param),
    }
}

fn user_signature(
    symbol: &Symbol,
    cached: &CompilationResult,
    formatter: &TypeFormatter,
) -> SignatureInformation {
    let SymbolKind::Function { params, ret_ty, .. } = &symbol.kind else {
        unreachable!()
    };
    let parts: Vec<(String, Option<String>)> = params
        .iter()
        .filter_map(|&sid| {
            let p = cached.typed.symbols.get(sid)?;
            let ty = cached
                .typed
                .span_types
                .get(&(p.span.start, p.span.end, p.span.file_id))
                .map(|t| formatter.format(t));
            Some((p.name.clone(), ty))
        })
        .collect();
    let params_display = parts
        .iter()
        .map(|(n, t)| match t {
            Some(ty) => format!("{}: {}", n, ty),
            None => n.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let label = match ret_ty {
        Some(rt) => format!("{}({}) -> {}", symbol.name, params_display, rt),
        None => format!("{}({})", symbol.name, params_display),
    };
    let info_params: Vec<ParameterInformation> = parts
        .into_iter()
        .map(|(n, t)| ParameterInformation {
            label: ParameterLabel::Simple(match t {
                Some(ty) => format!("{}: {}", n, ty),
                None => n,
            }),
            documentation: None,
        })
        .collect();
    SignatureInformation {
        label,
        documentation: None,
        parameters: Some(info_params),
        active_parameter: None,
    }
}

/// Locate the enclosing call at `offset`. Returns (callee_name, active_param).
fn locate_call(source: &str, offset: usize) -> Option<(String, u32)> {
    let bytes = source.as_bytes();
    let end = offset.min(bytes.len());
    let mut depth = 0i32;
    let mut commas = 0u32;
    let mut i = end;
    let mut in_string = false;
    let mut string_quote = b'"';

    while i > 0 {
        i -= 1;
        let b = bytes[i];
        if in_string {
            if b == string_quote && (i == 0 || bytes[i - 1] != b'\\') {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' | b'\'' => {
                in_string = true;
                string_quote = b;
            }
            b')' | b']' | b'}' => depth += 1,
            b'(' if depth > 0 => depth -= 1,
            b'[' | b'{' if depth > 0 => depth -= 1,
            b'(' => {
                // Found the unmatched open paren — read the identifier before it.
                let name_end = i;
                let name_start = source[..name_end]
                    .rfind(|c: char| !c.is_alphanumeric() && c != '_')
                    .map(|x| x + 1)
                    .unwrap_or(0);
                let name = &source[name_start..name_end];
                if name.is_empty() {
                    return None;
                }
                return Some((name.to_string(), commas));
            }
            b',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_call_at_cursor() {
        let src = "foo(a, b, c)";
        let (name, active) = locate_call(src, 10).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(active, 2);
    }

    #[test]
    fn handles_nested_parens() {
        let src = "foo(bar(1, 2), baz(3))";
        // Cursor right before the trailing `)` on `baz`
        let (name, active) = locate_call(src, 20).unwrap();
        assert_eq!(name, "baz");
        assert_eq!(active, 0);
    }

    #[test]
    fn ignores_commas_in_strings() {
        let src = r#"foo("a, b, c", d)"#;
        let (name, active) = locate_call(src, 16).unwrap();
        assert_eq!(name, "foo");
        assert_eq!(active, 1);
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Completion: dot-triggered (fields/methods) and identifier (symbols/keywords).

use tower_lsp::lsp_types::*;
use rask_types::{MethodSig, SelfParam, Type, TypeDef, TypeTable};

use crate::backend::CompilationResult;
use crate::type_format::TypeFormatter;

pub fn completion(
    position: Position,
    cached: &CompilationResult,
    live_text: Option<&str>,
    is_dot: bool,
) -> Option<CompletionResponse> {
    // Prefer the live text if we have it — dot completion cares about what
    // the user just typed, which may be ahead of the last good compile.
    let source = live_text.unwrap_or(&cached.source);
    let idx = if live_text.is_some() {
        crate::convert::LineIndex::new(source)
    } else {
        cached.line_index.clone()
    };
    let offset = idx.position_to_offset(source, position);

    if is_dot {
        dot_completion(source, offset, cached)
    } else {
        identifier_completion(source, offset, cached)
    }
}

fn dot_completion(source: &str, offset: usize, cached: &CompilationResult) -> Option<CompletionResponse> {
    let mut items = Vec::new();

    if let Some(receiver_type) = find_receiver_type(source, offset, cached) {
        let formatter = TypeFormatter::new(&cached.typed.types);
        collect_for_type(&receiver_type, &formatter, &cached.typed.types, &mut items);
    } else {
        let receiver_name = extract_receiver_ident(source, offset)?;
        add_all_stub_methods(&receiver_name, &mut items);
    }

    if items.is_empty() { None } else { Some(CompletionResponse::Array(items)) }
}

fn find_receiver_type(source: &str, offset: usize, cached: &CompilationResult) -> Option<Type> {
    let before_dot = if offset > 0 && source.as_bytes().get(offset - 1) == Some(&b'.') {
        offset - 1
    } else {
        offset
    };
    let text_before = source.get(..before_dot)?;
    let ident_start = text_before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ident = &text_before[ident_start..];
    if ident.is_empty() {
        return None;
    }
    for (_span, node_id, name) in &cached.position_index.idents {
        if name == ident {
            if let Some(ty) = cached.typed.node_types.get(node_id) {
                return Some(ty.clone());
            }
        }
    }
    cached.typed.types.lookup(ident)
}

fn identifier_completion(source: &str, offset: usize, cached: &CompilationResult) -> Option<CompletionResponse> {
    let text_before = source.get(..offset).unwrap_or("");
    let prefix_start = text_before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let prefix = &text_before[prefix_start..];

    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for symbol in cached.typed.symbols.iter() {
        if symbol.name.is_empty() || symbol.name.starts_with('_') {
            continue;
        }
        if !prefix.is_empty() && !symbol.name.starts_with(prefix) {
            continue;
        }
        if !seen.insert(symbol.name.clone()) {
            continue;
        }

        let (kind, detail) = match &symbol.kind {
            rask_resolve::SymbolKind::Function { .. } => (CompletionItemKind::FUNCTION, "func".to_string()),
            rask_resolve::SymbolKind::Struct { .. } => (CompletionItemKind::STRUCT, "struct".to_string()),
            rask_resolve::SymbolKind::Enum { .. } => (CompletionItemKind::ENUM, "enum".to_string()),
            rask_resolve::SymbolKind::Trait { .. } => (CompletionItemKind::INTERFACE, "trait".to_string()),
            rask_resolve::SymbolKind::Variable { mutable } => {
                let kw = if *mutable { "mut" } else { "const" };
                (CompletionItemKind::VARIABLE, kw.to_string())
            }
            rask_resolve::SymbolKind::Parameter { .. } => (CompletionItemKind::VARIABLE, "param".to_string()),
            rask_resolve::SymbolKind::EnumVariant { .. } => (CompletionItemKind::ENUM_MEMBER, "variant".to_string()),
            rask_resolve::SymbolKind::BuiltinType { .. } => (CompletionItemKind::CLASS, "type".to_string()),
            rask_resolve::SymbolKind::BuiltinFunction { .. } => (CompletionItemKind::FUNCTION, "builtin".to_string()),
            rask_resolve::SymbolKind::BuiltinModule { .. } => (CompletionItemKind::MODULE, "module".to_string()),
            rask_resolve::SymbolKind::ExternalPackage { .. } => (CompletionItemKind::MODULE, "package".to_string()),
            rask_resolve::SymbolKind::ExternFunction { .. } => (CompletionItemKind::FUNCTION, "extern func".to_string()),
            rask_resolve::SymbolKind::TypeAlias { .. } => (CompletionItemKind::CLASS, "type alias".to_string()),
            rask_resolve::SymbolKind::CNamespace { .. } => (CompletionItemKind::MODULE, "c namespace".to_string()),
            rask_resolve::SymbolKind::Field { .. } => continue,
        };

        items.push(CompletionItem {
            label: symbol.name.clone(),
            kind: Some(kind),
            detail: Some(detail),
            ..Default::default()
        });
    }

    let keywords = [
        "const", "mut", "func", "struct", "enum", "trait", "extend",
        "if", "else", "match", "for", "while", "loop", "return",
        "try", "ensure", "import", "public", "spawn", "with",
    ];
    for kw in &keywords {
        if prefix.is_empty() || kw.starts_with(prefix) {
            if seen.insert(kw.to_string()) {
                items.push(CompletionItem {
                    label: kw.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    ..Default::default()
                });
            }
        }
    }

    Some(CompletionResponse::Array(items))
}

fn collect_for_type(
    ty: &Type,
    formatter: &TypeFormatter,
    types: &TypeTable,
    items: &mut Vec<CompletionItem>,
) {
    match ty {
        Type::Named(id) => {
            if let Some(def) = types.get(*id) {
                let type_name = types.type_name(*id);
                add_typedef(def, formatter, items);
                add_stdlib_methods(&type_name, items);
            }
        }
        Type::Generic { base, .. } => {
            if let Some(def) = types.get(*base) {
                let type_name = types.type_name(*base);
                add_typedef(def, formatter, items);
                add_stdlib_methods(&type_name, items);
            }
        }
        Type::String => add_stdlib_methods("string", items),
        ty if ty.is_option() => add_stdlib_methods("Option", items),
        Type::Result { .. } => add_stdlib_methods("Result", items),
        Type::Array { .. } | Type::Slice(_) => add_stdlib_methods("Vec", items),
        _ => {}
    }
}

fn add_typedef(def: &TypeDef, formatter: &TypeFormatter, items: &mut Vec<CompletionItem>) {
    match def {
        TypeDef::Struct { fields, methods, .. } => {
            for (name, field_ty) in fields {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FIELD),
                    detail: Some(formatter.format(field_ty)),
                    ..Default::default()
                });
            }
            for method in methods {
                if method.self_param != SelfParam::None {
                    items.push(method_to_completion(method, formatter));
                }
            }
        }
        TypeDef::Enum { variants, methods, .. } => {
            for (name, fields) in variants {
                let detail = if fields.is_empty() {
                    name.clone()
                } else {
                    let args = fields.iter().map(|t| formatter.format(t)).collect::<Vec<_>>().join(", ");
                    format!("{}({})", name, args)
                };
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some(detail),
                    ..Default::default()
                });
            }
            for method in methods {
                if method.self_param != SelfParam::None {
                    items.push(method_to_completion(method, formatter));
                }
            }
        }
        TypeDef::Trait { methods, .. } => {
            for method in methods {
                items.push(method_to_completion(method, formatter));
            }
        }
        TypeDef::Union { fields, .. } => {
            for (name, field_ty) in fields {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FIELD),
                    detail: Some(formatter.format(field_ty)),
                    ..Default::default()
                });
            }
        }
        TypeDef::NominalAlias { underlying, .. } => {
            items.push(CompletionItem {
                label: "value".to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some(formatter.format(underlying)),
                ..Default::default()
            });
        }
    }
}

fn method_to_completion(sig: &MethodSig, formatter: &TypeFormatter) -> CompletionItem {
    let params_str = sig.params.iter()
        .map(|(ty, _)| formatter.format(ty))
        .collect::<Vec<_>>()
        .join(", ");
    let detail = format!("({}) -> {}", params_str, formatter.format(&sig.ret));
    CompletionItem {
        label: sig.name.clone(),
        kind: Some(CompletionItemKind::METHOD),
        detail: Some(detail),
        insert_text: Some(if sig.params.is_empty() {
            format!("{}()", sig.name)
        } else {
            format!("{}($1)", sig.name)
        }),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..Default::default()
    }
}

fn add_stdlib_methods(type_name: &str, items: &mut Vec<CompletionItem>) {
    add_stub_methods_filtered(type_name, items, true);
}

fn add_all_stub_methods(type_name: &str, items: &mut Vec<CompletionItem>) {
    add_stub_methods_filtered(type_name, items, false);
}

fn add_stub_methods_filtered(type_name: &str, items: &mut Vec<CompletionItem>, self_only: bool) {
    let methods = rask_stdlib::methods_for(type_name);
    for method in methods {
        if self_only && !method.takes_self {
            continue;
        }
        if items.iter().any(|i| i.label == method.name) {
            continue;
        }
        let params_str = method.params.iter()
            .map(|(n, t)| format!("{}: {}", n, t))
            .collect::<Vec<_>>()
            .join(", ");
        let detail = format!("({}) -> {}", params_str, method.ret_ty);
        let kind = if self_only { CompletionItemKind::METHOD } else { CompletionItemKind::FUNCTION };
        items.push(CompletionItem {
            label: method.name.clone(),
            kind: Some(kind),
            detail: Some(detail),
            documentation: method.doc.as_ref().map(|d| Documentation::String(d.clone())),
            insert_text: Some(if method.params.is_empty() {
                format!("{}()", method.name)
            } else {
                format!("{}($1)", method.name)
            }),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        });
    }
}

fn extract_receiver_ident(source: &str, offset: usize) -> Option<String> {
    let before_dot = if offset > 0 && source.as_bytes().get(offset - 1) == Some(&b'.') {
        offset - 1
    } else {
        offset
    };
    let text_before = source.get(..before_dot)?;
    let ident_start = text_before
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let ident = &text_before[ident_start..];
    if ident.is_empty() { None } else { Some(ident.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_ident() {
        assert_eq!(extract_receiver_ident("foo.", 4), Some("foo".to_string()));
    }

    #[test]
    fn extract_ident_with_prefix() {
        assert_eq!(extract_receiver_ident("    myVar.", 10), Some("myVar".to_string()));
    }

    #[test]
    fn extract_empty_at_start() {
        assert_eq!(extract_receiver_ident(".foo", 1), None);
    }

    #[test]
    fn vec_completion_core_methods() {
        let mut items = Vec::new();
        add_stdlib_methods("Vec", &mut items);
        let labels: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
        for expected in &["push", "pop", "len", "is_empty"] {
            assert!(labels.contains(&expected.to_string()), "missing {}", expected);
        }
    }

    #[test]
    fn static_new_excluded_from_instance() {
        let mut items = Vec::new();
        add_stdlib_methods("Vec", &mut items);
        assert!(!items.iter().any(|i| i.label == "new"));
    }

    #[test]
    fn no_duplicates() {
        let mut items = Vec::new();
        add_stdlib_methods("Vec", &mut items);
        add_stdlib_methods("Vec", &mut items);
        assert_eq!(items.iter().filter(|i| i.label == "push").count(), 1);
    }
}

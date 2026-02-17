// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dot-completion and identifier completion logic.

use tower_lsp::lsp_types::*;
use rask_types::{MethodSig, SelfParam, Type, TypeDef, TypeTable};

use crate::backend::{Backend, CompilationResult};
use crate::type_format::TypeFormatter;

impl Backend {
    /// Dot-completion: suggest fields and methods for the receiver type.
    pub fn dot_completion(
        &self,
        source: &str,
        offset: usize,
        cached: &CompilationResult,
    ) -> Option<CompletionResponse> {
        let receiver_type = self.find_receiver_type(source, offset, cached)?;
        let mut items = Vec::new();
        let formatter = TypeFormatter::new(&cached.typed.types);
        collect_completions_for_type(
            &receiver_type,
            &formatter,
            &cached.typed.types,
            &mut items,
        );
        Some(CompletionResponse::Array(items))
    }

    /// Find the type of the expression before the dot at `offset`.
    fn find_receiver_type(
        &self,
        source: &str,
        offset: usize,
        cached: &CompilationResult,
    ) -> Option<Type> {
        // offset points right after the dot trigger, so the dot is at offset-1
        let before_dot = if offset > 0 && source.as_bytes().get(offset - 1) == Some(&b'.') {
            offset - 1
        } else {
            offset
        };

        // Scan backwards to extract identifier before the dot
        let text_before = &source[..before_dot];
        let ident_end = text_before.len();
        let ident_start = text_before
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let ident = &text_before[ident_start..ident_end];

        if ident.is_empty() {
            return None;
        }

        // Look up identifier in cached position index by name
        for (_span, node_id, name) in &cached.position_index.idents {
            if name == ident {
                if let Some(ty) = cached.typed.node_types.get(node_id) {
                    return Some(ty.clone());
                }
            }
        }

        // Try as a type name (e.g., Vec.new(), string.new())
        cached.typed.types.lookup(ident)
    }

    /// Identifier completion: suggest all symbols and keywords.
    pub fn identifier_completion(
        &self,
        source: &str,
        offset: usize,
        cached: &CompilationResult,
    ) -> Option<CompletionResponse> {
        // Extract partial identifier being typed
        let text_before = &source[..offset];
        let prefix_start = text_before
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &text_before[prefix_start..];

        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Symbols from the symbol table
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
                rask_resolve::SymbolKind::Function { .. } => {
                    (CompletionItemKind::FUNCTION, "func".to_string())
                }
                rask_resolve::SymbolKind::Struct { .. } => {
                    (CompletionItemKind::STRUCT, "struct".to_string())
                }
                rask_resolve::SymbolKind::Enum { .. } => {
                    (CompletionItemKind::ENUM, "enum".to_string())
                }
                rask_resolve::SymbolKind::Trait { .. } => {
                    (CompletionItemKind::INTERFACE, "trait".to_string())
                }
                rask_resolve::SymbolKind::Variable { mutable } => {
                    let kw = if *mutable { "let" } else { "const" };
                    (CompletionItemKind::VARIABLE, kw.to_string())
                }
                rask_resolve::SymbolKind::Parameter { .. } => {
                    (CompletionItemKind::VARIABLE, "param".to_string())
                }
                rask_resolve::SymbolKind::EnumVariant { .. } => {
                    (CompletionItemKind::ENUM_MEMBER, "variant".to_string())
                }
                rask_resolve::SymbolKind::BuiltinType { .. } => {
                    (CompletionItemKind::CLASS, "type".to_string())
                }
                rask_resolve::SymbolKind::BuiltinFunction { .. } => {
                    (CompletionItemKind::FUNCTION, "builtin".to_string())
                }
                rask_resolve::SymbolKind::BuiltinModule { .. } => {
                    (CompletionItemKind::MODULE, "module".to_string())
                }
                rask_resolve::SymbolKind::ExternalPackage { .. } => {
                    (CompletionItemKind::MODULE, "package".to_string())
                }
                rask_resolve::SymbolKind::ExternFunction { .. } => {
                    (CompletionItemKind::FUNCTION, "extern func".to_string())
                }
                rask_resolve::SymbolKind::Field { .. } => continue,
            };

            items.push(CompletionItem {
                label: symbol.name.clone(),
                kind: Some(kind),
                detail: Some(detail),
                ..Default::default()
            });
        }

        // Rask keywords
        let keywords = [
            "const", "let", "func", "struct", "enum", "trait", "extend",
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
}

/// Collect completion items for a given type (fields, methods, stdlib methods).
fn collect_completions_for_type(
    ty: &Type,
    formatter: &TypeFormatter,
    types: &TypeTable,
    items: &mut Vec<CompletionItem>,
) {
    match ty {
        Type::Named(id) => {
            if let Some(def) = types.get(*id) {
                let type_name = types.type_name(*id);
                add_typedef_completions(def, formatter, items);
                // Also add stdlib methods for well-known types (Vec, Map, etc.)
                add_stdlib_methods(&type_name, items);
            }
        }
        Type::Generic { base, .. } => {
            if let Some(def) = types.get(*base) {
                let type_name = types.type_name(*base);
                add_typedef_completions(def, formatter, items);
                add_stdlib_methods(&type_name, items);
            }
        }
        Type::String => add_stdlib_methods("string", items),
        Type::Option(_) => add_stdlib_methods("Option", items),
        Type::Result { .. } => add_stdlib_methods("Result", items),
        Type::Array { .. } | Type::Slice(_) => add_stdlib_methods("Vec", items),
        _ => {}
    }
}

/// Add fields and methods from a TypeDef.
fn add_typedef_completions(
    def: &TypeDef,
    formatter: &TypeFormatter,
    items: &mut Vec<CompletionItem>,
) {
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
                    let args = fields
                        .iter()
                        .map(|t| formatter.format(t))
                        .collect::<Vec<_>>()
                        .join(", ");
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
    }
}

/// Convert a MethodSig to a CompletionItem.
fn method_to_completion(sig: &MethodSig, formatter: &TypeFormatter) -> CompletionItem {
    let params_str = sig
        .params
        .iter()
        .map(|(ty, _mode)| formatter.format(ty))
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

/// Add stdlib methods for well-known builtin types.
fn add_stdlib_methods(type_name: &str, items: &mut Vec<CompletionItem>) {
    let methods: &[rask_stdlib::MethodDef] = match type_name {
        "Vec" => rask_stdlib::types::vec_methods(),
        "Map" => rask_stdlib::types::map_methods(),
        "string" | "String" => rask_stdlib::types::string_methods(),
        "Pool" => rask_stdlib::types::pool_methods(),
        "Option" => rask_stdlib::types::option_methods(),
        "Result" => rask_stdlib::types::result_methods(),
        _ => return,
    };

    for method in methods {
        if !method.takes_self {
            continue;
        }
        // Skip if already added from TypeDef
        if items.iter().any(|i| i.label == method.name) {
            continue;
        }
        let params_str = method
            .params
            .iter()
            .map(|(name, ty)| format!("{}: {}", name, ty))
            .collect::<Vec<_>>()
            .join(", ");
        let detail = format!("({}) -> {}", params_str, method.ret_ty);
        items.push(CompletionItem {
            label: method.name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            detail: Some(detail),
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

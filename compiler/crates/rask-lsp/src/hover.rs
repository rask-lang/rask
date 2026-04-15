// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Hover (Ctrl+K Ctrl+I / mouse hover) logic.
//!
//! Builds a Markdown tooltip with:
//!   - the symbol kind + name + type
//!   - stdlib docs if available
//!   - struct fields / enum variants / trait methods for user types

use tower_lsp::lsp_types::*;

use crate::backend::CompilationResult;
use crate::type_format::TypeFormatter;

pub fn hover(position: Position, cached: &CompilationResult) -> Option<Hover> {
    let offset = cached
        .line_index
        .position_to_offset(&cached.source, position);

    let formatter = TypeFormatter::new(&cached.typed.types);

    // Try expression node first, then fall back to binding/param span_types.
    let on_ident = cached.position_index.ident_at_position(offset).is_some();
    let expr_node_id = if on_ident {
        cached.position_index.node_at_position(offset)
    } else {
        None
    };
    let mut ty_opt = expr_node_id.and_then(|nid| cached.typed.node_types.get(&nid));
    if ty_opt.is_none() {
        if let Some((span, _, _)) = cached
            .position_index
            .idents
            .iter()
            .find(|(s, _, _)| s.start <= offset && offset <= s.end)
        {
            ty_opt = cached.typed.span_types.get(&(span.start, span.end));
        }
    }

    // Fall back to the stub registry if we have an identifier but no type.
    if ty_opt.is_none() {
        if let Some((_, name)) = cached.position_index.ident_at_position(offset) {
            if let Some(markdown) = stdlib_type_hover(&name) {
                return Some(md_hover(markdown));
            }
            if let Some(markdown) = method_hover(&cached.source, offset, &name, cached) {
                return Some(md_hover(markdown));
            }
        }
        return None;
    }

    let ty = ty_opt.unwrap();
    let type_str = formatter.format(ty);
    let mut md = format!("**Type:** `{}`", type_str);

    if let Some((_, name)) = cached.position_index.ident_at_position(offset) {
        let symbol = expr_node_id
            .and_then(|nid| cached.typed.resolutions.get(&nid))
            .and_then(|&sid| cached.typed.symbols.get(sid));
        if let Some(symbol) = symbol {
            md = format!("**{}:** `{}`\n\n**Type:** `{}`", kind_label(&symbol.kind), name, type_str);
            append_symbol_doc(&mut md, symbol, &formatter, cached);
        } else if let Some(extra) = method_hover(&cached.source, offset, &name, cached) {
            md.push_str("\n\n---\n\n");
            md.push_str(&extra);
        }
    }

    if let rask_types::Type::UnresolvedNamed(name) = ty {
        append_stdlib_type(&mut md, name);
    }

    Some(md_hover(md))
}

fn md_hover(markdown: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    }
}

fn kind_label(kind: &rask_resolve::SymbolKind) -> &'static str {
    match kind {
        rask_resolve::SymbolKind::Variable { mutable } => {
            if *mutable { "Variable (mutable)" } else { "Variable" }
        }
        rask_resolve::SymbolKind::Parameter { .. } => "Parameter",
        rask_resolve::SymbolKind::Function { .. } => "Function",
        rask_resolve::SymbolKind::Struct { .. } => "Struct",
        rask_resolve::SymbolKind::Enum { .. } => "Enum",
        rask_resolve::SymbolKind::Field { .. } => "Field",
        rask_resolve::SymbolKind::Trait { .. } => "Trait",
        rask_resolve::SymbolKind::EnumVariant { .. } => "Enum Variant",
        rask_resolve::SymbolKind::BuiltinType { .. } => "Built-in Type",
        rask_resolve::SymbolKind::BuiltinFunction { .. } => "Built-in Function",
        rask_resolve::SymbolKind::BuiltinModule { .. } => "Built-in Module",
        rask_resolve::SymbolKind::ExternFunction { .. } => "Extern Function",
        rask_resolve::SymbolKind::ExternalPackage { .. } => "Package",
        rask_resolve::SymbolKind::TypeAlias { .. } => "Type Alias",
        rask_resolve::SymbolKind::CNamespace { .. } => "C Namespace",
    }
}

fn append_symbol_doc(
    md: &mut String,
    symbol: &rask_resolve::Symbol,
    formatter: &TypeFormatter,
    cached: &CompilationResult,
) {
    match &symbol.kind {
        rask_resolve::SymbolKind::BuiltinType { .. }
        | rask_resolve::SymbolKind::BuiltinFunction { .. }
        | rask_resolve::SymbolKind::BuiltinModule { .. } => {
            append_stdlib_type(md, &symbol.name);
            let reg = rask_stdlib::StubRegistry::load();
            if let Some(f) = reg.functions().iter().find(|f| f.name == symbol.name) {
                if let Some(doc) = &f.doc {
                    md.push_str(&format!("\n\n---\n\n{}", doc));
                }
            }
        }
        rask_resolve::SymbolKind::Struct { .. } | rask_resolve::SymbolKind::Enum { .. } => {
            if let Some(type_id) = cached.typed.types.get_type_id(&symbol.name) {
                if let Some(def) = cached.typed.types.get(type_id) {
                    match def {
                        rask_types::TypeDef::Struct { fields, methods, .. } => {
                            if !fields.is_empty() {
                                md.push_str("\n\n**Fields:**\n");
                                for (fname, fty) in fields {
                                    md.push_str(&format!("\n- `{}: {}`", fname, formatter.format(fty)));
                                }
                            }
                            if !methods.is_empty() {
                                md.push_str("\n\n**Methods:**\n");
                                for m in methods {
                                    md.push_str(&format!("\n- `{}`", m.name));
                                }
                            }
                        }
                        rask_types::TypeDef::Enum { variants, methods, .. } => {
                            if !variants.is_empty() {
                                md.push_str("\n\n**Variants:**\n");
                                for (vname, fields) in variants {
                                    if fields.is_empty() {
                                        md.push_str(&format!("\n- `{}`", vname));
                                    } else {
                                        let args = fields
                                            .iter()
                                            .map(|t| formatter.format(t))
                                            .collect::<Vec<_>>()
                                            .join(", ");
                                        md.push_str(&format!("\n- `{}({})`", vname, args));
                                    }
                                }
                            }
                            if !methods.is_empty() {
                                md.push_str("\n\n**Methods:**\n");
                                for m in methods {
                                    md.push_str(&format!("\n- `{}`", m.name));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

fn stdlib_type_hover(name: &str) -> Option<String> {
    let reg = rask_stdlib::StubRegistry::load();
    let ts = reg.get_type(name)?;
    let mut md = format!("**Stdlib Type:** `{}`", name);
    if let Some(doc) = &ts.doc {
        md.push_str(&format!("\n\n---\n\n{}", doc));
    }
    if !ts.methods.is_empty() {
        md.push_str("\n\n**Methods:**\n");
        for m in &ts.methods {
            let params = m.params.iter()
                .map(|(n, t)| format!("{}: {}", n, t))
                .collect::<Vec<_>>()
                .join(", ");
            let self_prefix = if m.takes_self { "self, " } else { "" };
            md.push_str(&format!(
                "\n- `{}({}{}) -> {}`",
                m.name, self_prefix, params, m.ret_ty
            ));
        }
    }
    Some(md)
}

fn append_stdlib_type(md: &mut String, name: &str) {
    let reg = rask_stdlib::StubRegistry::load();
    let Some(ts) = reg.get_type(name) else { return };
    if let Some(doc) = &ts.doc {
        md.push_str(&format!("\n\n---\n\n{}", doc));
    }
    if !ts.methods.is_empty() {
        md.push_str("\n\n**Methods:**\n");
        for m in &ts.methods {
            let params = m.params.iter()
                .map(|(n, t)| format!("{}: {}", n, t))
                .collect::<Vec<_>>()
                .join(", ");
            let self_prefix = if m.takes_self { "self, " } else { "" };
            md.push_str(&format!(
                "\n- `{}({}{}) -> {}`",
                m.name, self_prefix, params, m.ret_ty
            ));
        }
    }
}

pub(crate) fn method_hover(
    source: &str,
    offset: usize,
    method_name: &str,
    cached: &CompilationResult,
) -> Option<String> {
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
    let reg = rask_stdlib::StubRegistry::load();
    let normalized = match type_name.as_str() {
        "String" => "string",
        _ => &type_name,
    };
    let method = reg.lookup_method(normalized, method_name)?;

    let params = method.params.iter()
        .map(|(n, t)| format!("{}: {}", n, t))
        .collect::<Vec<_>>()
        .join(", ");
    let self_prefix = if method.takes_self { "self, " } else { "" };
    let mut out = format!(
        "**Method:** `{}.{}({}{}) -> {}`",
        normalized, method.name, self_prefix, params, method.ret_ty
    );
    if let Some(doc) = &method.doc {
        out.push_str(&format!("\n\n---\n\n{}", doc));
    }
    Some(out)
}

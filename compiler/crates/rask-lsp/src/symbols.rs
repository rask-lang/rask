// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Document and workspace symbol providers (file outline + global search).

use std::sync::Arc;

use tower_lsp::lsp_types::*;

use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, TraitDecl, ImplDecl};
use rask_ast::Span;

use crate::backend::CompilationResult;
use crate::convert::LineIndex;

/// Outline for a single document.
pub fn document_symbols(cached: &CompilationResult) -> Option<DocumentSymbolResponse> {
    let mut out = Vec::new();
    for decl in &cached.decls {
        // Only include decls from the *current* file — siblings were appended
        // to `decls` by the pipeline for cross-file resolution, we don't want
        // them in the outline.
        if !is_current_file(cached, decl.span) {
            continue;
        }
        if let Some(sym) = decl_to_symbol(decl, &cached.source, &cached.line_index) {
            out.push(sym);
        }
    }
    Some(DocumentSymbolResponse::Nested(out))
}

/// Global symbol search across all open documents.
pub fn workspace_symbols(
    query: &str,
    compiled: &[(Url, Arc<CompilationResult>)],
) -> Option<Vec<SymbolInformation>> {
    let query_lc = query.to_lowercase();
    let mut out = Vec::new();

    for (uri, cached) in compiled {
        for decl in &cached.decls {
            if !is_current_file(cached, decl.span) {
                continue;
            }
            push_matching(decl, &query_lc, &cached.source, &cached.line_index, uri, &mut out);
        }
    }
    // Cap the result size to avoid dumping a whole project in one response.
    out.truncate(500);
    Some(out)
}

fn is_current_file(cached: &CompilationResult, span: Span) -> bool {
    cached
        .current_file_spans
        .iter()
        .any(|s| span.start >= s.start && span.end <= s.end)
}

fn decl_to_symbol(decl: &Decl, source: &str, idx: &LineIndex) -> Option<DocumentSymbol> {
    let range = idx.span_to_range(source, decl.span);
    match &decl.kind {
        DeclKind::Fn(f) => Some(symbol(&f.name, f_detail(f), SymbolKind::FUNCTION, range, None)),
        DeclKind::Struct(s) => Some(symbol(
            &s.name, None, SymbolKind::STRUCT, range,
            Some(struct_children(s, source, idx)),
        )),
        DeclKind::Enum(e) => Some(symbol(
            &e.name, None, SymbolKind::ENUM, range,
            Some(enum_children(e, source, idx)),
        )),
        DeclKind::Trait(t) => Some(symbol(
            &t.name, None, SymbolKind::INTERFACE, range,
            Some(trait_children(t, source, idx)),
        )),
        DeclKind::Impl(i) => Some(symbol(
            &impl_name(i), None, SymbolKind::NAMESPACE, range,
            Some(impl_children(i, source, idx)),
        )),
        DeclKind::Const(c) => Some(symbol(&c.name, None, SymbolKind::CONSTANT, range, None)),
        DeclKind::Union(u) => Some(symbol(&u.name, None, SymbolKind::STRUCT, range, None)),
        DeclKind::Extern(e) => Some(symbol(&e.name, Some(format!("extern \"{}\"", e.abi)), SymbolKind::FUNCTION, range, None)),
        DeclKind::TypeAlias(t) => Some(symbol(&t.name, Some(format!("= {}", t.target)), SymbolKind::TYPE_PARAMETER, range, None)),
        DeclKind::Test(t) => Some(symbol(&t.name, Some("test".into()), SymbolKind::METHOD, range, None)),
        DeclKind::Benchmark(b) => Some(symbol(&b.name, Some("bench".into()), SymbolKind::METHOD, range, None)),
        _ => None,
    }
}

fn impl_name(i: &ImplDecl) -> String {
    match &i.trait_name {
        Some(t) => format!("{} for {}", t, i.target_ty),
        None => i.target_ty.clone(),
    }
}

fn struct_children(s: &StructDecl, source: &str, idx: &LineIndex) -> Vec<DocumentSymbol> {
    let mut children = Vec::new();
    for field in &s.fields {
        let range = idx.span_to_range(source, field.name_span);
        children.push(symbol(&field.name, Some(field.ty.clone()), SymbolKind::FIELD, range, None));
    }
    for m in &s.methods {
        let range = idx.span_to_range(source, m.span);
        children.push(symbol(&m.name, f_detail(m), SymbolKind::METHOD, range, None));
    }
    children
}

fn enum_children(e: &EnumDecl, source: &str, idx: &LineIndex) -> Vec<DocumentSymbol> {
    let mut children = Vec::new();
    for v in &e.variants {
        // No dedicated name span on variant — fall back to decl span.
        let range = idx.span_to_range(source, Span::new(0, 0));
        children.push(symbol(&v.name, None, SymbolKind::ENUM_MEMBER, range, None));
    }
    for m in &e.methods {
        let range = idx.span_to_range(source, m.span);
        children.push(symbol(&m.name, f_detail(m), SymbolKind::METHOD, range, None));
    }
    children
}

fn trait_children(t: &TraitDecl, source: &str, idx: &LineIndex) -> Vec<DocumentSymbol> {
    t.methods.iter().map(|m| {
        let range = idx.span_to_range(source, m.span);
        symbol(&m.name, f_detail(m), SymbolKind::METHOD, range, None)
    }).collect()
}

fn impl_children(i: &ImplDecl, source: &str, idx: &LineIndex) -> Vec<DocumentSymbol> {
    i.methods.iter().map(|m| {
        let range = idx.span_to_range(source, m.span);
        symbol(&m.name, f_detail(m), SymbolKind::METHOD, range, None)
    }).collect()
}

fn f_detail(f: &FnDecl) -> Option<String> {
    let params = f.params.iter()
        .map(|p| format!("{}: {}", p.name, p.ty))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = f.ret_ty.clone().unwrap_or_default();
    Some(if ret.is_empty() {
        format!("({})", params)
    } else {
        format!("({}) -> {}", params, ret)
    })
}

#[allow(deprecated)]
fn symbol(
    name: &str,
    detail: Option<String>,
    kind: SymbolKind,
    range: Range,
    children: Option<Vec<DocumentSymbol>>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.to_string(),
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children,
    }
}

fn push_matching(
    decl: &Decl,
    query_lc: &str,
    source: &str,
    idx: &LineIndex,
    uri: &Url,
    out: &mut Vec<SymbolInformation>,
) {
    let (name, kind, container): (&str, SymbolKind, Option<String>) = match &decl.kind {
        DeclKind::Fn(f) => (&f.name, SymbolKind::FUNCTION, None),
        DeclKind::Struct(s) => (&s.name, SymbolKind::STRUCT, None),
        DeclKind::Enum(e) => (&e.name, SymbolKind::ENUM, None),
        DeclKind::Trait(t) => (&t.name, SymbolKind::INTERFACE, None),
        DeclKind::Const(c) => (&c.name, SymbolKind::CONSTANT, None),
        DeclKind::Union(u) => (&u.name, SymbolKind::STRUCT, None),
        DeclKind::Extern(e) => (&e.name, SymbolKind::FUNCTION, None),
        DeclKind::TypeAlias(t) => (&t.name, SymbolKind::TYPE_PARAMETER, None),
        DeclKind::Test(t) => (&t.name, SymbolKind::METHOD, Some("test".into())),
        _ => return,
    };
    if !query_lc.is_empty() && !name.to_lowercase().contains(query_lc) {
        return;
    }
    let range = idx.span_to_range(source, decl.span);
    #[allow(deprecated)]
    out.push(SymbolInformation {
        name: name.to_string(),
        kind,
        tags: None,
        deprecated: None,
        location: Location { uri: uri.clone(), range },
        container_name: container,
    });
    // Also include children (struct/enum/trait methods) for completeness.
    match &decl.kind {
        DeclKind::Struct(s) => push_methods(&s.methods, &s.name, query_lc, source, idx, uri, out),
        DeclKind::Enum(e) => push_methods(&e.methods, &e.name, query_lc, source, idx, uri, out),
        DeclKind::Trait(t) => push_methods(&t.methods, &t.name, query_lc, source, idx, uri, out),
        DeclKind::Impl(i) => push_methods(&i.methods, &impl_name(i), query_lc, source, idx, uri, out),
        _ => {}
    }
}

fn push_methods(
    methods: &[FnDecl],
    container: &str,
    query_lc: &str,
    source: &str,
    idx: &LineIndex,
    uri: &Url,
    out: &mut Vec<SymbolInformation>,
) {
    for m in methods {
        if !query_lc.is_empty() && !m.name.to_lowercase().contains(query_lc) {
            continue;
        }
        let range = idx.span_to_range(source, m.span);
        #[allow(deprecated)]
        out.push(SymbolInformation {
            name: m.name.clone(),
            kind: SymbolKind::METHOD,
            tags: None,
            deprecated: None,
            location: Location { uri: uri.clone(), range },
            container_name: Some(container.to_string()),
        });
    }
}

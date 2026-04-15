// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Inlay hints — inferred types shown inline after `const x` / `let x`.
//!
//! The editor renders these as ghosted text so you can see the type the
//! compiler inferred without littering the source with annotations.

use tower_lsp::lsp_types::*;

use rask_ast::decl::{DeclKind, FnDecl};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::backend::CompilationResult;
use crate::type_format::TypeFormatter;

pub fn inlay_hints(cached: &CompilationResult, range: Range) -> Vec<InlayHint> {
    let mut out = Vec::new();
    let formatter = TypeFormatter::new(&cached.typed.types);
    let range_start = cached.line_index.position_to_offset(&cached.source, range.start);
    let range_end = cached.line_index.position_to_offset(&cached.source, range.end);

    for decl in &cached.decls {
        match &decl.kind {
            DeclKind::Fn(f) => visit_fn(f, cached, &formatter, &mut out, range_start, range_end),
            DeclKind::Impl(i) => {
                for m in &i.methods {
                    visit_fn(m, cached, &formatter, &mut out, range_start, range_end);
                }
            }
            DeclKind::Test(t) => {
                for stmt in &t.body {
                    visit_stmt(stmt, cached, &formatter, &mut out, range_start, range_end);
                }
            }
            _ => {}
        }
    }
    out
}

fn visit_fn(
    f: &FnDecl,
    cached: &CompilationResult,
    formatter: &TypeFormatter,
    out: &mut Vec<InlayHint>,
    lo: usize,
    hi: usize,
) {
    for stmt in &f.body {
        visit_stmt(stmt, cached, formatter, out, lo, hi);
    }
}

fn visit_stmt(
    stmt: &Stmt,
    cached: &CompilationResult,
    formatter: &TypeFormatter,
    out: &mut Vec<InlayHint>,
    lo: usize,
    hi: usize,
) {
    match &stmt.kind {
        StmtKind::Let { name_span, ty: None, .. } | StmtKind::Const { name_span, ty: None, .. } => {
            if name_span.start < lo || name_span.end > hi {
                return;
            }
            if let Some(ty) = cached.typed.span_types.get(&(name_span.start, name_span.end)) {
                let label = format!(": {}", formatter.format(ty));
                let pos = cached.line_index.offset_to_position(&cached.source, name_span.end);
                out.push(InlayHint {
                    position: pos,
                    label: InlayHintLabel::String(label),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: None,
                    padding_right: Some(true),
                    data: None,
                });
            }
        }
        StmtKind::While { body, .. }
        | StmtKind::WhileLet { body, .. }
        | StmtKind::Loop { body, .. }
        | StmtKind::For { body, .. } => {
            for s in body {
                visit_stmt(s, cached, formatter, out, lo, hi);
            }
        }
        StmtKind::Ensure { body, else_handler } => {
            for s in body {
                visit_stmt(s, cached, formatter, out, lo, hi);
            }
            if let Some((_, handler)) = else_handler {
                for s in handler {
                    visit_stmt(s, cached, formatter, out, lo, hi);
                }
            }
        }
        StmtKind::Comptime(body) | StmtKind::ComptimeFor { body, .. } => {
            for s in body {
                visit_stmt(s, cached, formatter, out, lo, hi);
            }
        }
        _ => {}
    }
}


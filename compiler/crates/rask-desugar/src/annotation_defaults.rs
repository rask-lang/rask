// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Fill declared defaults into annotation attachments (type.annotations/AN3).
//!
//! `@indexed()` on a field, where `annotation @indexed { weight: i64 = 1 }`,
//! becomes `@indexed(weight: 1)`. AN3 says an attachment checks like
//! construction of the annotation's record, and construction gets its defaults
//! filled right here in desugar — so attachments do too.
//!
//! Filling early means every later consumer sees a complete attachment: the
//! checker validating it, MIR lowering splicing `get<A>().weight`, and the
//! interpreter answering the same question. None of them has to look the
//! declaration back up, so none of them can disagree about what a default was.

use rask_ast::decl::{field_attrs, AnnotationDecl, Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, UnaryOp};
use std::collections::HashMap;

/// Rewrite every attachment of a user-declared annotation to name all its
/// fields. Attachments of compiler-known annotations (`@rename`, `@test`, …)
/// are left alone — they are not declared here and have no default table.
pub fn fill_annotation_defaults(decls: &mut [Decl]) {
    let declared: HashMap<String, Vec<(String, String)>> = decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Annotation(a) => Some((a.name.clone(), defaults_of(a))),
            _ => None,
        })
        .filter(|(_, defaults)| !defaults.is_empty())
        .collect();
    if declared.is_empty() {
        return;
    }

    for decl in decls.iter_mut() {
        match &mut decl.kind {
            DeclKind::Struct(s) => {
                fill(&mut s.attrs, &declared);
                for field in &mut s.fields {
                    fill(&mut field.attrs, &declared);
                }
            }
            DeclKind::Enum(e) => {
                fill(&mut e.attrs, &declared);
                for variant in &mut e.variants {
                    fill(&mut variant.attrs, &declared);
                    for field in &mut variant.fields {
                        fill(&mut field.attrs, &declared);
                    }
                }
            }
            DeclKind::Fn(f) => fill_fn(f, &declared),
            DeclKind::Impl(i) => {
                for m in &mut i.methods {
                    fill_fn(m, &declared);
                }
            }
            _ => {}
        }
    }
}

fn fill_fn(f: &mut FnDecl, declared: &HashMap<String, Vec<(String, String)>>) {
    fill(&mut f.attrs, declared);
}

/// Fields with a declared default, as `(name, value text)`. Values are
/// rendered back to source text so the filled attachment reads exactly like a
/// hand-written one, and one reader handles both.
fn defaults_of(ann: &AnnotationDecl) -> Vec<(String, String)> {
    ann.fields
        .iter()
        .filter_map(|f| {
            let text = render_const(f.default.as_ref()?)?;
            Some((f.name.clone(), text))
        })
        .collect()
}

/// A defaulted annotation field is a comptime constant (AN1), so this only
/// has to render literals. Anything else is left unfilled and the checker's
/// missing-field error names it — better than inventing a value.
fn render_const(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Int(v, _) => Some(v.to_string()),
        ExprKind::Float(v, _) => Some(format!("{:?}", v)),
        ExprKind::Bool(v) => Some(v.to_string()),
        ExprKind::String(v) => Some(format!("{:?}", v)),
        // `-1` arrives as a unary negation over a literal.
        ExprKind::Unary { op: UnaryOp::Neg, operand } => {
            Some(format!("-{}", render_const(operand)?))
        }
        // `Color.Red` — an enum variant, kept as the path text.
        ExprKind::Field { object, field } => match &object.kind {
            ExprKind::Ident(name) => Some(format!("{}.{}", name, field)),
            _ => None,
        },
        ExprKind::Array(elements) => {
            let parts: Option<Vec<String>> = elements.iter().map(render_const).collect();
            Some(format!("[{}]", parts?.join(", ")))
        }
        _ => None,
    }
}

fn fill(attrs: &mut [String], declared: &HashMap<String, Vec<(String, String)>>) {
    for attr in attrs.iter_mut() {
        let name = field_attrs::attachment_name(attr).to_string();
        let Some(defaults) = declared.get(&name) else { continue };

        let given: Vec<String> = field_attrs::attachment_args(attr)
            .into_iter()
            .map(|(n, _)| n.to_string())
            .collect();
        let missing: Vec<&(String, String)> =
            defaults.iter().filter(|(n, _)| !given.contains(n)).collect();
        if missing.is_empty() {
            continue;
        }

        let mut args: Vec<String> = field_attrs::attachment_args(attr)
            .into_iter()
            .map(|(n, v)| format!("{}: {}", n, v))
            .collect();
        for (n, v) in missing {
            args.push(format!("{}: {}", n, v));
        }
        *attr = format!("{}({})", name, args.join(", "));
    }
}

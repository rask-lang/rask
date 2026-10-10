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
//!
//! Package scoping runs first. A dependency's annotation is declared in the
//! merged program under its qualified name, and its attachments — in the
//! dependency, and `@liba.validate` in the program — have been rewritten to
//! that name too. So the table is just the declarations in `decls`, keyed by
//! the name the attachments use.

use rask_ast::attr::{Attr, AttrArg};
use rask_ast::decl::{AnnotationDecl, Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, UnaryOp};
use std::collections::HashMap;

/// Rewrite every attachment of a user-declared annotation to name all its
/// fields. Attachments of compiler-known annotations (`@rename`, `@test`, …)
/// are left alone — they are not declared here and have no default table.
pub fn fill_annotation_defaults(decls: &mut [Decl]) {
    let declared: HashMap<String, Vec<(String, String)>> = decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Annotation(ann) => Some((ann.name.clone(), defaults_of(ann))),
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
                for m in &mut s.methods {
                    fill_fn(m, &declared);
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
                for m in &mut e.methods {
                    fill_fn(m, &declared);
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

fn fill(attrs: &mut [Attr], declared: &HashMap<String, Vec<(String, String)>>) {
    for attr in attrs.iter_mut() {
        if attr.path.len() != 1 {
            continue;
        }
        let Some(defaults) = declared.get(&attr.path[0]) else { continue };
        let given: Vec<String> = attr.labelled().map(|(l, _)| l.to_string()).collect();
        let args = attr.args.get_or_insert_with(Vec::new);
        for (label, value) in defaults {
            if !given.contains(label) {
                args.push(AttrArg { label: Some(label.clone()), value: value.clone() });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decls(src: &str) -> Vec<Decl> {
        let lexed = rask_lexer::Lexer::new(src).tokenize();
        let parsed = rask_parser::Parser::new(lexed.tokens).parse();
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        parsed.decls
    }

    fn field_attrs(decls: &[Decl]) -> Vec<String> {
        decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Struct(s) => Some(s),
                _ => None,
            })
            .flat_map(|s| s.fields.iter().flat_map(|f| f.attrs.iter().map(|a| a.to_string())))
            .collect()
    }

    #[test]
    fn fills_the_missing_defaults() {
        let mut d = decls(
            "annotation @indexed { weight: i64 = 1, label: string = \"none\" }\n\
             struct Doc {\n@indexed(weight: 3)\ntitle: string\n@indexed()\nbody: string }",
        );
        fill_annotation_defaults(&mut d);
        assert_eq!(
            field_attrs(&d),
            vec![
                "indexed(weight: 3, label: \"none\")".to_string(),
                "indexed(weight: 1, label: \"none\")".to_string(),
            ]
        );
    }

    // After package scoping a dependency's annotation and its attachments
    // share the qualified name, so they match like any other.
    #[test]
    fn a_qualified_name_matches_its_declaration() {
        let mut d = decls(
            "annotation @validate_liba { max: i64 = 10 }\n\
             struct Form {\n@validate_liba()\ncode: i64\n@validate(max: 2)\nother: i64 }",
        );
        fill_annotation_defaults(&mut d);
        assert_eq!(
            field_attrs(&d),
            vec!["validate_liba(max: 10)".to_string(), "validate(max: 2)".to_string()]
        );
    }

    #[test]
    fn compiler_known_attributes_are_left_alone() {
        let mut d = decls(
            "annotation @indexed { weight: i64 = 1 }\n\
             struct Doc {\n@rename(\"t\")\ntitle: string }",
        );
        fill_annotation_defaults(&mut d);
        assert_eq!(field_attrs(&d), vec!["rename(\"t\")".to_string()]);
    }
}

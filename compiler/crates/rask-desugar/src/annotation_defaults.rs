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
//! The price of being this early is that name resolution hasn't run, so a
//! dependency's declarations have to be handed in and matched against the
//! file's own `import` lines by hand.

use rask_ast::decl::{field_attrs, AnnotationDecl, Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, UnaryOp};
use std::collections::{HashMap, HashSet};

/// Rewrite every attachment of a user-declared annotation to name all its
/// fields. Attachments of compiler-known annotations (`@rename`, `@test`, …)
/// are left alone — they are not declared here and have no default table.
///
/// `dep_annotations` is `(package name, declaration)` for every public
/// annotation in this package's dependencies.
pub fn fill_annotation_defaults(decls: &mut [Decl], dep_annotations: &[(String, Decl)]) {
    let declared = default_table(decls, dep_annotations);
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

/// Annotation name → the fields that have defaults, as `(name, value text)`.
///
/// A declaration in this package wins over an imported one. Two *imported*
/// packages declaring the same name is left unfilled rather than resolved: the
/// name alone can't say which was meant, and filling from whichever came last
/// is a wrong value with no error attached to it — an app importing both `liba`
/// and `libb` got `libb`'s default for `liba`'s annotation. The read then fails,
/// which is worse than a diagnostic but better than a silent lie (#967).
fn default_table(
    decls: &[Decl],
    dep_annotations: &[(String, Decl)],
) -> HashMap<String, Vec<(String, String)>> {
    let imported = imported_packages(decls);
    let mut table: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    let mut source_package: HashMap<String, String> = HashMap::new();

    for (pkg, decl) in dep_annotations {
        let DeclKind::Annotation(ann) = &decl.kind else { continue };
        if !imported.names(pkg, &ann.name) {
            continue;
        }
        match source_package.get(&ann.name) {
            Some(other) if other != pkg => {
                ambiguous.insert(ann.name.clone());
            }
            _ => {
                source_package.insert(ann.name.clone(), pkg.clone());
                insert_defaults(&mut table, ann);
            }
        }
    }
    for name in &ambiguous {
        table.remove(name);
    }

    // This package's own declarations last, so they win.
    for decl in decls {
        if let DeclKind::Annotation(ann) = &decl.kind {
            insert_defaults(&mut table, ann);
        }
    }
    table
}

fn insert_defaults(table: &mut HashMap<String, Vec<(String, String)>>, ann: &AnnotationDecl) {
    let defaults = defaults_of(ann);
    if defaults.is_empty() {
        table.remove(&ann.name);
    } else {
        table.insert(ann.name.clone(), defaults);
    }
}

/// What this file imported, in the two shapes that matter: `import pkg` brings
/// the package's whole public surface, `import pkg.Member` brings one name.
struct Imports {
    whole: HashSet<String>,
    members: HashSet<(String, String)>,
}

impl Imports {
    /// Whether `pkg`'s annotation `name` is in scope here.
    fn names(&self, pkg: &str, name: &str) -> bool {
        self.whole.contains(pkg) || self.members.contains(&(pkg.to_string(), name.to_string()))
    }
}

fn imported_packages(decls: &[Decl]) -> Imports {
    let mut whole = HashSet::new();
    let mut members = HashSet::new();
    for decl in decls {
        let DeclKind::Import(imp) = &decl.kind else { continue };
        match imp.path.len() {
            1 => {
                whole.insert(imp.path[0].clone());
            }
            2 => {
                members.insert((imp.path[0].clone(), imp.path[1].clone()));
            }
            _ => {}
        }
    }
    Imports { whole, members }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{AnnotationDecl, Field, FieldVisibility, ImportDecl, StructDecl};
    use rask_ast::{NodeId, Span};

    fn annotation(name: &str, default: i128) -> Decl {
        Decl {
            id: NodeId(1),
            span: Span::new(0, 0),
            kind: DeclKind::Annotation(AnnotationDecl {
                name: name.to_string(),
                name_span: Span::new(0, 0),
                fields: vec![Field {
                    name: "max".to_string(),
                    name_span: Span::new(0, 0),
                    ty: "i64".to_string(),
                    visibility: FieldVisibility::Public,
                    attrs: vec![],
                    default: Some(Expr {
                        id: NodeId(2),
                        span: Span::new(0, 0),
                        kind: ExprKind::Int(default, None),
                    }),
                }],
                is_pub: true,
                doc: None,
            }),
        }
    }

    fn import(path: &[&str]) -> Decl {
        Decl {
            id: NodeId(3),
            span: Span::new(0, 0),
            kind: DeclKind::Import(ImportDecl {
                path: path.iter().map(|s| s.to_string()).collect(),
                alias: None,
                is_glob: false,
                is_lazy: false,
            }),
        }
    }

    /// One field carrying a bare `@validate` attachment, for `fill` to rewrite.
    fn attached() -> Decl {
        Decl {
            id: NodeId(4),
            span: Span::new(0, 0),
            kind: DeclKind::Struct(StructDecl {
                name: "Local".to_string(),
                type_params: vec![],
                fields: vec![Field {
                    name: "x".to_string(),
                    name_span: Span::new(0, 0),
                    ty: "i64".to_string(),
                    visibility: FieldVisibility::Public,
                    attrs: vec!["validate".to_string()],
                    default: None,
                }],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
        }
    }

    fn attachment_of(decls: &[Decl]) -> String {
        let DeclKind::Struct(s) = &decls[decls.len() - 1].kind else { panic!("want struct") };
        s.fields[0].attrs[0].clone()
    }

    #[test]
    fn fills_from_the_imported_package() {
        let mut decls = vec![import(&["liba", "validate"]), attached()];
        let deps = vec![
            ("liba".to_string(), annotation("validate", 10)),
            ("libb".to_string(), annotation("validate", 99)),
        ];
        fill_annotation_defaults(&mut decls, &deps);
        // Not libb's 99: only liba is imported. Taking whichever dependency
        // came last is a wrong value with no error attached to it.
        assert_eq!(attachment_of(&decls), "validate(max: 10)");
    }

    #[test]
    fn leaves_an_ambiguous_name_unfilled() {
        let mut decls = vec![
            import(&["liba", "validate"]),
            import(&["libb", "validate"]),
            attached(),
        ];
        let deps = vec![
            ("liba".to_string(), annotation("validate", 10)),
            ("libb".to_string(), annotation("validate", 99)),
        ];
        fill_annotation_defaults(&mut decls, &deps);
        // Untouched — the checker reports the ambiguous import (AN5), and
        // guessing here would put one of the two values behind that error.
        assert_eq!(attachment_of(&decls), "validate");
    }

    #[test]
    fn a_local_declaration_wins() {
        let mut decls = vec![
            import(&["liba", "validate"]),
            annotation("validate", 3),
            attached(),
        ];
        let deps = vec![("liba".to_string(), annotation("validate", 10))];
        fill_annotation_defaults(&mut decls, &deps);
        assert_eq!(attachment_of(&decls), "validate(max: 3)");
    }

    #[test]
    fn an_uninvolved_dependency_is_ignored() {
        let mut decls = vec![import(&["libb"]), attached()];
        let deps = vec![("liba".to_string(), annotation("validate", 10))];
        fill_annotation_defaults(&mut decls, &deps);
        assert_eq!(attachment_of(&decls), "validate");
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! User annotation validation (type.annotations/AN3-AN5).
//!
//! An attachment arrives as an `Attr`: a name and labelled arguments, each
//! value a parsed expression. A dependency's annotation, and every
//! attachment naming it (`@validate` inside it, `@liba.validate` outside),
//! carry the name package scoping gave the declaration, so attachments are
//! matched to declarations by name alone.

use rask_ast::attr::{AttrValue, Literal};
use rask_ast::decl::{AnnotationDecl, Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::Span;
use rask_ast::ty::TypeExpr;
use std::collections::HashMap;

use super::errors::TypeError;
// Reasons, one per rule. A diagnostic that explains the wrong rule is worse
// than one that explains none, so these travel with the error.

const CONSTRUCTION_WHY: &str =
    "an attachment checks like construction of the annotation's declared record, so readers can trust every value they get back [type.annotations/AN3]";
const DUPLICATE_WHY: &str =
    "two attachments of one annotation leave a reader no way to say which it meant — repetition belongs in an array field [type.annotations/AN4]";
const NAME_WHY: &str =
    "readers ask for an annotation by name, so two declarations sharing one name would answer each other's questions [type.annotations/AN5]";
const RESERVED_WHY: &str =
    "the compiler already acts on this name, so a user annotation would either be shadowed or silently change what the compiler does [type.annotations/AN5]";
const CONST_WHY: &str =
    "attached values are embedded as constants and read identically in every instantiation, which only works for the comptime-constant types [type.annotations/AN1, ctrl.comptime/CT58]";
const UNRESOLVED_WHY: &str =
    "a user annotation is an ordinary name: `@pkg.name` reaches one a dependency declares `public`, the same way `pkg.Type` reaches a type [type.annotations/AN5, structure.modules/IM1]";
const TYPE_POSITION_WHY: &str =
    "an annotation value cannot be constructed, so a slot typed as one could never be filled — annotations are read at comptime, never held [type.annotations/AN8]";

use super::TypeChecker;

/// Names the compiler consumes itself (AN5). A user annotation with one of
/// these names would either be shadowed or silently change compiler behavior.
const RESERVED: &[&str] = &[
    "rename", "no_serialize", "skip", "default", "tag",
    "resource", "unique", "binary", "message",
    "entry", "no_alloc", "inline", "native", "unimplemented",
    "allow", "test", "benchmark", "derive",
    "call_text", "call_location", "comptime_quota", "embed_file", "comptime_print", "comptime_assert",
];

/// Field types an annotation may declare (AN1) — the const-representable set
/// (ctrl.comptime/CT58): primitives, `string`, fixed arrays, and (assumed) enums.
///
/// `string` belongs: an attached value is a literal, and a literal is already
/// a constant the backends splice — reading one back materializes it exactly
/// as a string literal in source does. What's excluded is anything whose value
/// can't be written as a literal at all.
fn const_representable(ty: &TypeExpr) -> bool {
    match ty {
        // fixed array; element type checked at attachment
        TypeExpr::Array { .. } | TypeExpr::FixedCount { .. } => true,
        // primitives, string, and names assumed to be enums
        TypeExpr::Named { args, .. } => args.is_empty(),
        // containers, generics, function types, optionals, unions
        _ => false,
    }
}

fn int_type(ty: &str) -> bool {
    matches!(ty, "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize")
}

/// What kind of constant an argument value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lit {
    Int,
    Float,
    Str,
    Bool,
    Array,
    /// Bare identifier or dotted path — an enum variant like `Color.Red`.
    /// Accepted without deeper checking for now.
    Path,
    Other,
}

fn classify(value: &AttrValue) -> Lit {
    let Some(expr) = value.expr() else { return Lit::Other };
    if let Some(lit) = rask_ast::attr::literal(expr) {
        return match lit {
            Literal::Int(_) => Lit::Int,
            Literal::Float(_) => Lit::Float,
            Literal::Str(_) => Lit::Str,
            Literal::Bool(_) => Lit::Bool,
            Literal::Char(_) => Lit::Other,
        };
    }
    match &expr.kind {
        ExprKind::Array(_) => Lit::Array,
        _ if is_path(expr) => Lit::Path,
        _ => Lit::Other,
    }
}

/// `Color` or `Color.Red`.
fn is_path(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Ident(_) => true,
        ExprKind::Field { object, .. } => is_path(object),
        _ => false,
    }
}

fn lit_matches(lit: Lit, written: &TypeExpr) -> bool {
    if lit == Lit::Array {
        return matches!(written, TypeExpr::Array { .. } | TypeExpr::FixedCount { .. });
    }
    let Some(ty) = written.bare_name() else { return lit == Lit::Path };
    match lit {
        Lit::Int => int_type(ty) || ty == "f32" || ty == "f64",
        Lit::Float => ty == "f32" || ty == "f64",
        Lit::Str => ty == "string",
        Lit::Bool => ty == "bool",
        Lit::Array => false,
        // Enum variants and consts: no type table lookup yet, accept.
        Lit::Path => !int_type(ty) && !matches!(ty, "f32" | "f64" | "bool" | "string"),
        Lit::Other => false,
    }
}

impl TypeChecker {
    /// AN8 at a binding's declared type. Declaration syntax is swept in
    /// `check_user_annotations`, but a `let`/`mut` annotation lives in a
    /// statement, and the sweep never gets there.
    ///
    /// Not folded into `resolve_type_expr`: that's a free function over the
    /// type table with no span and no error channel, called from stdlib and
    /// stub paths where a user diagnostic has nowhere to go.
    pub(super) fn reject_annotation_binding_type(&mut self, ty: &TypeExpr, span: Span) {
        for name in type_name_parts(ty) {
            if self.annotation_types.contains(&name) {
                self.errors.push(TypeError::BadAnnotation {
                    name: name.clone(),
                    problem: "an annotation is not a type, so a binding cannot be declared as one".to_string(),
                    fix: format!(
                        "drop the annotation from the type — a read gives you a field, not a record: `let w = field.get<{}>().<field>`",
                        name
                    ),
                    why: TYPE_POSITION_WHY,
                    span,
                });
                return;
            }
        }
    }

    /// AN3-AN5 over the whole program. Runs after type declarations are
    /// collected; attachment sites are struct/enum/func decls and struct
    /// fields (variants and params carry no attrs yet).
    pub(super) fn check_user_annotations(&mut self, decls: &[Decl]) {
        // Collect declarations; duplicates and reserved names error here.
        let mut declared: HashMap<&str, &AnnotationDecl> = HashMap::new();
        for decl in decls {
            let DeclKind::Annotation(ann) = &decl.kind else { continue };
            if RESERVED.contains(&ann.name.as_str()) {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: "this name is reserved for a compiler-known annotation".to_string(),
                    fix: "pick a different name".to_string(),
                    why: RESERVED_WHY,
                    span: ann.name_span,
                });
                continue;
            }
            for field in &ann.fields {
                if !const_representable(&field.ty) {
                    let fix = "annotation fields are limited to primitives, string, enums, and fixed arrays of these".to_string();
                    self.errors.push(TypeError::BadAnnotation {
                        name: ann.name.clone(),
                        problem: format!("field `{}: {}` is not const-representable", field.name, field.ty),
                        fix,
                        why: CONST_WHY,
                        span: field.name_span,
                    });
                }
            }
            if declared.insert(ann.name.as_str(), ann).is_some() {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: "an annotation with this name is already declared".to_string(),
                    fix: "rename one of them — readers look annotations up by name".to_string(),
                    why: NAME_WHY,
                    span: ann.name_span,
                });
            }
        }
        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => {
                    self.check_attachment_site(&s.attrs, "struct", decl.span, &declared);
                    for field in &s.fields {
                        self.check_attachment_site(&field.attrs, "field", field.name_span, &declared);
                        self.reject_annotation_type(&field.ty, "field type", field.name_span, &declared);
                    }
                    for m in &s.methods {
                        self.check_attachment_site(&m.attrs, "method", m.span, &declared);
                        self.check_signature_types(m, &declared);
                    }
                }
                DeclKind::Enum(e) => {
                    self.check_attachment_site(&e.attrs, "enum", decl.span, &declared);
                    for variant in &e.variants {
                        self.check_attachment_site(&variant.attrs, "variant", variant.name_span, &declared);
                        for field in &variant.fields {
                            self.reject_annotation_type(&field.ty, "variant field type", variant.name_span, &declared);
                        }
                    }
                    for m in &e.methods {
                        self.check_attachment_site(&m.attrs, "method", m.span, &declared);
                        self.check_signature_types(m, &declared);
                    }
                }
                DeclKind::Fn(f) => {
                    self.check_attachment_site(&f.attrs, "func", decl.span, &declared);
                    self.check_signature_types(f, &declared);
                }
                DeclKind::Impl(i) => {
                    for m in &i.methods {
                        self.check_attachment_site(&m.attrs, "method", m.span, &declared);
                        self.check_signature_types(m, &declared);
                    }
                }
                DeclKind::TypeAlias(a) => {
                    self.reject_annotation_type(&a.target, "type alias", decl.span, &declared);
                }
                _ => {}
            }
        }
    }

    /// `@allow(name)` where nothing answers to `name`.
    ///
    /// The names come from two registries that can't see each other — compiler
    /// warnings here, lint rule ids in `rask-lint` — so a misspelled one used to
    /// match nothing and the warning fired as if the annotation weren't there
    /// (#1085). `rask_ast::allow_names` holds both lists, below both crates.
    pub(super) fn check_allow_names(&mut self, decls: &[Decl]) {
        for decl in decls {
            let (attrs, span): (&[rask_ast::attr::Attr], Span) = match &decl.kind {
                DeclKind::Fn(f) => (&f.attrs, f.span),
                DeclKind::Struct(s) => (&s.attrs, decl.span),
                DeclKind::Enum(e) => (&e.attrs, decl.span),
                DeclKind::Interface(t) => (&t.attrs, decl.span),
                DeclKind::Test(t) => (&t.attrs, decl.span),
                DeclKind::Benchmark(b) => (&b.attrs, decl.span),
                DeclKind::Const(c) => (&c.attrs, decl.span),
                _ => (&[], decl.span),
            };
            self.check_allow_attrs(attrs, span);
            match &decl.kind {
                DeclKind::Struct(s) => {
                    for m in &s.methods {
                        self.check_allow_attrs(&m.attrs, m.span);
                    }
                }
                DeclKind::Enum(e) => {
                    for m in &e.methods {
                        self.check_allow_attrs(&m.attrs, m.span);
                    }
                }
                DeclKind::Impl(i) => {
                    for m in &i.methods {
                        self.check_allow_attrs(&m.attrs, m.span);
                    }
                }
                _ => {}
            }
        }
    }

    fn check_allow_attrs(&mut self, attrs: &[rask_ast::attr::Attr], span: Span) {
        for attr in attrs {
            let Some(name) = rask_ast::allow_names::allowed_name(attr) else { continue };
            if rask_ast::allow_names::is_known(name) {
                continue;
            }
            self.errors.push(TypeError::UnknownAllowName {
                name: name.to_string(),
                suggestion: rask_ast::allow_names::nearest(name).map(str::to_string),
                span,
            });
        }
    }

    /// AN8: parameter and return types of one function.
    fn check_signature_types(
        &mut self,
        f: &rask_ast::decl::FnDecl,
        declared: &HashMap<&str, &AnnotationDecl>,
    ) {
        for param in &f.params {
            if let Some(ty) = &param.ty {
                self.reject_annotation_type(ty, "parameter type", param.name_span, declared);
            }
        }
        if let Some(ret) = &f.ret_ty {
            self.reject_annotation_type(ret, "return type", f.span, declared);
        }
    }

    /// AN8: an annotation name in a type position. The checker registers
    /// annotations as nominal structs so `has<validate>()` can name one as a
    /// type argument, and that registration made `func peek(a: validate)`
    /// type-check — a parameter nothing can ever fill, since AN3 forbids
    /// constructing the value.
    ///
    /// Matches the name anywhere in the written type, so `Vec<validate>` and
    /// `validate?` are caught too. Type arguments of `has`/`get` never come
    /// through here — this walks declaration syntax, not expressions.
    fn reject_annotation_type(
        &mut self,
        ty: &TypeExpr,
        position: &str,
        span: Span,
        declared: &HashMap<&str, &AnnotationDecl>,
    ) {
        for name in type_name_parts(ty) {
            if declared.contains_key(name.as_str()) {
                self.errors.push(TypeError::BadAnnotation {
                    name: name.clone(),
                    problem: format!("an annotation is not a type, so it cannot be a {}", position),
                    why: TYPE_POSITION_WHY,
                    fix: format!(
                        "annotations are read, not held: attach `@{}(...)` and ask `field.has<{}>()` or `field.get<{}>().<field>` at comptime",
                        name, name, name
                    ),
                    span,
                });
                return;
            }
        }
    }

    /// `site` is only used for message wording ("attached twice to the same
    /// field") — placement itself is not constrained: an annotation is data,
    /// and where it is useful is the reader's business, not the compiler's
    /// (Principle 5). A misplaced attachment is lint territory.
    fn check_attachment_site(
        &mut self,
        attrs: &[rask_ast::attr::Attr],
        site: &str,
        span: Span,
        declared: &HashMap<&str, &AnnotationDecl>,
    ) {
        let mut seen: Vec<&str> = Vec::new();
        for attr in attrs {
            // A dotted name is always a user annotation reached through a
            // package. Package scoping rewrites every one that names a public
            // annotation, so one still dotted names nothing.
            if attr.path.len() > 1 {
                self.errors.push(TypeError::BadAnnotation {
                    name: attr.name(),
                    problem: format!(
                        "`@{}` names no annotation — `{}` is not an imported package with a public annotation `{}`",
                        attr.name(),
                        attr.path[..attr.path.len() - 1].join("."),
                        attr.path[attr.path.len() - 1]
                    ),
                    fix: format!("import the package that declares it: `import {}`", attr.path[0]),
                    why: UNRESOLVED_WHY,
                    span: attr_span(attr, span),
                });
                continue;
            }
            let name = attr.path[0].as_str();
            let Some(ann) = declared.get(name) else { continue };

            // AN4: duplicates on one item are ambiguous for readers.
            if seen.contains(&name) {
                self.errors.push(TypeError::BadAnnotation {
                    name: name.to_string(),
                    problem: format!("attached twice to the same {}", site),
                    fix: "one attachment per item — repetition wants an array field".to_string(),
                    why: DUPLICATE_WHY,
                    span: attr_span(attr, span),
                });
                continue;
            }
            seen.push(name);

            self.check_attachment_args(ann, attr, attr_span(attr, span));
        }
    }

    /// AN3: `@name(args)` checks like the struct literal `name { args }` —
    /// named form, every non-defaulted field present, names and literal kinds
    /// checked against the declaration.
    fn check_attachment_args(&mut self, ann: &AnnotationDecl, attr: &rask_ast::attr::Attr, span: Span) {
        let mut given: Vec<&str> = Vec::new();
        for arg in attr.args.iter().flatten() {
            let value = &arg.value;
            let Some(arg_name) = arg.label.as_deref() else {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: format!("argument `{}` is not named", value),
                    why: CONSTRUCTION_WHY,
                    fix: format!("annotation arguments use field names, like a struct literal: @{}(field: value)", ann.name),
                    span,
                });
                continue;
            };
            let Some(field) = ann.fields.iter().find(|f| f.name == arg_name) else {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: format!("no field `{}` on this annotation", arg_name),
                    why: CONSTRUCTION_WHY,
                    fix: format!(
                        "fields: {}",
                        ann.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
                    ),
                    span,
                });
                continue;
            };
            if given.contains(&field.name.as_str()) {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: format!("field `{}` given twice", arg_name),
                    fix: "remove one".to_string(),
                    why: CONSTRUCTION_WHY,
                    span,
                });
                continue;
            }
            given.push(&field.name);

            let lit = classify(value);
            if !lit_matches(lit, &field.ty) {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: format!("`{}` is not a `{}` for field `{}`", value, field.ty, arg_name),
                    fix: "annotation values are comptime constants matching the declared field type".to_string(),
                    why: CONST_WHY,
                    span,
                });
            }
        }

        // Non-defaulted fields are required — same rule as construction.
        let missing: Vec<&str> = ann
            .fields
            .iter()
            .filter(|f| f.default.is_none() && !given.contains(&f.name.as_str()))
            .map(|f| f.name.as_str())
            .collect();
        if !missing.is_empty() {
            self.errors.push(TypeError::BadAnnotation {
                name: ann.name.clone(),
                problem: format!("missing field{}: {}", if missing.len() > 1 { "s" } else { "" }, missing.join(", ")),
                why: CONSTRUCTION_WHY,
                fix: format!("fields without defaults must be given: @{}({}: ...)", ann.name, missing[0]),
                span,
            });
        }
    }
}

/// Every name inside a written type: `Vec<Map<str, validate>>` yields
/// `Vec`, `Map`, `str`, `validate`.
fn type_name_parts(ty: &TypeExpr) -> Vec<String> {
    let mut out = Vec::new();
    ty.walk_paths(&mut |path| out.push(path.join(".")));
    out
}

/// Where an attachment is, for its diagnostics: its own span when the parser
/// gave it one, else the item it is on.
fn attr_span(attr: &rask_ast::attr::Attr, item: Span) -> Span {
    if attr.span.end > attr.span.start { attr.span } else { item }
}

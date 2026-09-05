// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! User annotation validation (type.annotations/AN3-AN5).
//!
//! Attachments arrive as verbatim strings (`validate(max:100)`, same storage
//! as the serialization annotations), so argument checking re-parses that
//! text. Values are literals for now — the full "any comptime constant" form
//! wants structured attr storage, tracked separately.

use rask_ast::decl::{AnnotationDecl, Decl, DeclKind};
use rask_ast::Span;
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
const AMBIGUOUS_IMPORT_WHY: &str =
    "an attachment records the annotation's name as written, not which package it resolved to — so two same-named annotations in scope can't be told apart [type.annotations/AN5]";
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
fn const_representable(ty: &str) -> bool {
    let ty = ty.trim();
    if ty.starts_with('[') {
        return true; // fixed array; element type checked at attachment
    }
    if ty.contains('<') || ty.contains("func(") || ty.contains('?') || ty.contains('|') {
        return false; // containers, generics, function types, optionals, unions
    }
    true // primitives, string, and names assumed to be enums
}

fn int_type(ty: &str) -> bool {
    matches!(ty, "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize")
}

/// What kind of literal an argument value is, from its reconstructed text.
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

fn classify(value: &str) -> Lit {
    let v = value.trim();
    if v.is_empty() {
        return Lit::Other;
    }
    if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
        return Lit::Str;
    }
    if v == "true" || v == "false" {
        return Lit::Bool;
    }
    if v.starts_with('[') && v.ends_with(']') {
        return Lit::Array;
    }
    let num = v.strip_prefix('-').unwrap_or(v);
    if !num.is_empty() && num.chars().next().unwrap().is_ascii_digit() {
        if num.chars().all(|c| c.is_ascii_digit() || c == '_') {
            return Lit::Int;
        }
        if num.chars().all(|c| c.is_ascii_digit() || c == '_' || c == '.') {
            return Lit::Float;
        }
        return Lit::Other;
    }
    if v.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
        return Lit::Path;
    }
    Lit::Other
}

fn lit_matches(lit: Lit, ty: &str) -> bool {
    let ty = ty.trim();
    match lit {
        Lit::Int => int_type(ty) || ty == "f32" || ty == "f64",
        Lit::Float => ty == "f32" || ty == "f64",
        Lit::Str => ty == "string",
        Lit::Bool => ty == "bool",
        Lit::Array => ty.starts_with('['),
        // Enum variants and consts: no type table lookup yet, accept.
        Lit::Path => !int_type(ty) && !matches!(ty, "f32" | "f64" | "bool" | "string"),
        Lit::Other => false,
    }
}

/// Split `name(args)` into the name and the raw argument text. Name
/// extraction is the shared `field_attrs::attachment_name` — same answer the
/// backends use for `has<A>()`.
fn split_attr(attr: &str) -> (&str, Option<&str>) {
    let attr = attr.trim();
    let name = rask_ast::decl::field_attrs::attachment_name(attr);
    let args = (name.len() < attr.len() && attr.ends_with(')'))
        .then(|| &attr[name.len() + 1..attr.len() - 1]);
    (name, args)
}

/// Split an argument list on top-level commas — commas inside strings and
/// brackets belong to the value.
fn split_args(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '[' | '(' if !in_str => depth += 1,
            ']' | ')' if !in_str => depth = depth.saturating_sub(1),
            ',' if !in_str && depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        out.push(&text[start..]);
    }
    out
}

/// One argument: `name: value`. The colon split has to skip string contents.
fn split_named(arg: &str) -> Option<(&str, &str)> {
    let mut in_str = false;
    for (i, c) in arg.char_indices() {
        match c {
            '"' => in_str = !in_str,
            ':' if !in_str => return Some((arg[..i].trim(), arg[i + 1..].trim())),
            _ => {}
        }
    }
    None
}

impl TypeChecker {
    /// Public annotations from the packages this file imports, registered into
    /// the type table on the way through so `has`/`get` can name them.
    ///
    /// Without this the importer's checker didn't know the annotation existed:
    /// `has<A>()` still answered (it matches attachment text, which travels
    /// with the struct), but an attachment written here wasn't validated at all
    /// — `@validate(bogus: 1)` was accepted with a field that doesn't exist —
    /// and `get<A>().max` reached MIR with no declaration to read `max` from.
    fn imported_annotations(&mut self, decls: &[Decl]) -> Vec<AnnotationDecl> {
        let locally_declared: Vec<&str> = decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Annotation(a) => Some(a.name.as_str()),
                _ => None,
            })
            .collect();

        let mut found: Vec<AnnotationDecl> = Vec::new();
        let mut source_package: HashMap<String, String> = HashMap::new();
        for decl in decls {
            let DeclKind::Import(imp) = &decl.kind else { continue };
            let Some(pkg) = imp.path.first() else { continue };
            let Some(ext) = self.resolved.external_decls.get(pkg).cloned() else { continue };
            // `import pkg.Member` names one member; `import pkg` takes the lot.
            let wanted = (imp.path.len() > 1).then(|| imp.path[1].clone());
            for ext_decl in &ext {
                let DeclKind::Annotation(ann) = &ext_decl.kind else { continue };
                if !ann.is_pub || wanted.as_deref().is_some_and(|w| w != ann.name) {
                    continue;
                }
                match source_package.get(&ann.name) {
                    // AN5: two packages, one name, and the written name can't
                    // say which was meant. A local declaration of the same name
                    // settles it, so only report when there isn't one.
                    Some(other) if other != pkg && !locally_declared.contains(&ann.name.as_str()) => {
                        self.errors.push(TypeError::BadAnnotation {
                            name: ann.name.clone(),
                            problem: format!(
                                "imported from both `{}` and `{}`, so `@{}` here is ambiguous",
                                other, pkg, ann.name
                            ),
                            fix: format!(
                                "import only one of them, or declare `annotation @{}` in this package to settle it",
                                ann.name
                            ),
                            why: AMBIGUOUS_IMPORT_WHY,
                            span: decl.span,
                        });
                    }
                    Some(_) => {}
                    None => {
                        source_package.insert(ann.name.clone(), pkg.clone());
                        found.push(ann.clone());
                    }
                }
            }
        }
        for ann in &found {
            if self.annotation_types.contains(&ann.name) {
                continue;
            }
            self.annotation_types.insert(ann.name.clone());
            let s = rask_ast::decl::StructDecl {
                name: ann.name.clone(),
                type_params: vec![],
                fields: ann.fields.clone(),
                methods: vec![],
                is_pub: true,
                attrs: vec![],
                doc: ann.doc.clone(),
            };
            self.register_struct(&s);
        }
        found
    }

    /// AN8 at a binding's declared type. Declaration syntax is swept in
    /// `check_user_annotations`, but a `let`/`mut` annotation lives in a
    /// statement, and the sweep never gets there.
    ///
    /// Not folded into `parse_type_string`: that's a free function over the
    /// type table with no span and no error channel, called from stdlib and
    /// stub paths where a user diagnostic has nowhere to go.
    pub(super) fn reject_annotation_binding_type(&mut self, ty: &str, span: Span) {
        for name in type_name_parts(ty) {
            if self.annotation_types.contains(name) {
                let name = name.to_string();
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
        // An annotation a dependency declares `public` is one this package can
        // attach and read, so it has to be in the table before either happens.
        // Registering it here rather than with the other external types: that
        // runs in a later pass, and both the attachment check below and
        // `get<A>()`'s field lookup need it now.
        let imported = self.imported_annotations(decls);

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
        // A dependency's annotation fills in only where this package hasn't
        // declared the name itself — a local declaration wins, and two
        // *packages* sharing a name is a separate problem (see AN5's note).
        for ann in &imported {
            declared.entry(ann.name.as_str()).or_insert(ann);
        }

        if declared.is_empty() {
            return;
        }

        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => {
                    self.check_attachment_site(&s.attrs, "struct", decl.span, &declared);
                    for field in &s.fields {
                        self.check_attachment_site(&field.attrs, "field", field.name_span, &declared);
                        self.reject_annotation_type(&field.ty, "field type", field.name_span, &declared);
                    }
                }
                DeclKind::Enum(e) => {
                    self.check_attachment_site(&e.attrs, "enum", decl.span, &declared);
                    for variant in &e.variants {
                        for field in &variant.fields {
                            self.reject_annotation_type(&field.ty, "variant field type", variant.name_span, &declared);
                        }
                    }
                }
                DeclKind::Fn(f) => {
                    self.check_attachment_site(&f.attrs, "func", decl.span, &declared);
                    self.check_signature_types(f, &declared);
                }
                DeclKind::Impl(i) => {
                    for m in &i.methods {
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
            let (attrs, span): (&[String], Span) = match &decl.kind {
                DeclKind::Fn(f) => (&f.attrs, f.span),
                DeclKind::Struct(s) => (&s.attrs, decl.span),
                DeclKind::Enum(e) => (&e.attrs, decl.span),
                DeclKind::Trait(t) => (&t.attrs, decl.span),
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

    fn check_allow_attrs(&mut self, attrs: &[String], span: Span) {
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
            self.reject_annotation_type(&param.ty, "parameter type", param.name_span, declared);
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
        ty: &str,
        position: &str,
        span: Span,
        declared: &HashMap<&str, &AnnotationDecl>,
    ) {
        for name in type_name_parts(ty) {
            if declared.contains_key(name) {
                self.errors.push(TypeError::BadAnnotation {
                    name: name.to_string(),
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
        attrs: &[String],
        site: &str,
        span: Span,
        declared: &HashMap<&str, &AnnotationDecl>,
    ) {
        let mut seen: Vec<&str> = Vec::new();
        for attr in attrs {
            let (name, args) = split_attr(attr);
            let Some(ann) = declared.get(name) else { continue };

            // AN4: duplicates on one item are ambiguous for readers.
            if seen.contains(&name) {
                self.errors.push(TypeError::BadAnnotation {
                    name: name.to_string(),
                    problem: format!("attached twice to the same {}", site),
                    fix: "one attachment per item — repetition wants an array field".to_string(),
                    why: DUPLICATE_WHY,
                    span,
                });
                continue;
            }
            seen.push(name);

            self.check_attachment_args(ann, args, span);
        }
    }

    /// AN3: `@name(args)` checks like the struct literal `name { args }` —
    /// named form, every non-defaulted field present, names and literal kinds
    /// checked against the declaration.
    fn check_attachment_args(&mut self, ann: &AnnotationDecl, args: Option<&str>, span: Span) {
        let mut given: Vec<&str> = Vec::new();
        for arg in args.map(split_args).unwrap_or_default() {
            let arg = arg.trim();
            if arg.is_empty() {
                continue;
            }
            let Some((arg_name, value)) = split_named(arg) else {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: format!("argument `{}` is not named", arg),
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
                    problem: format!("`{}` is not a `{}` for field `{}`", value.trim(), field.ty, arg_name),
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

/// Every bare name inside a written type: `Vec<Map<str, validate>>` yields
/// `Vec`, `Map`, `str`, `validate`. Splits on the type punctuation rather than
/// parsing, which is enough to spot a name that must not appear at all.
fn type_name_parts(ty: &str) -> impl Iterator<Item = &str> {
    ty.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map(str::trim)
        .filter(|p| !p.is_empty())
}

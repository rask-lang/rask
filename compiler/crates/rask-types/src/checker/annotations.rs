// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! User annotation validation (type.annotations/AN2-AN5).
//!
//! Attachments arrive as verbatim strings (`validate(max:100)`, same storage
//! as the serialization annotations), so argument checking re-parses that
//! text. Values are literals for now — the full "any comptime constant" form
//! wants structured attr storage, tracked separately.

use rask_ast::decl::{AnnotationDecl, AnnotationTarget, Decl, DeclKind};
use rask_ast::Span;
use std::collections::HashMap;

use super::errors::TypeError;
use super::TypeChecker;

/// Names the compiler consumes itself (AN5). A user annotation with one of
/// these names would either be shadowed or silently change compiler behavior.
const RESERVED: &[&str] = &[
    "rename", "no_serialize", "skip", "default", "tag",
    "resource", "unique", "binary", "message",
    "entry", "no_alloc", "inline", "native", "unimplemented",
    "allow", "test", "benchmark", "derive",
    "call_site", "comptime_quota", "embed_file", "comptime_print", "comptime_assert",
];

/// Field types an annotation may declare (AN1) — the const-representable set
/// (ctrl.comptime/CT58): primitives, `str`, fixed arrays, and (assumed) enums.
fn const_representable(ty: &str) -> bool {
    let ty = ty.trim();
    if ty.starts_with('[') {
        return true; // fixed array; element type checked at attachment
    }
    if ty == "string" || ty.contains('<') || ty.contains("func(") {
        return false; // heap string, containers, generics, function types
    }
    true // primitives, str, and names assumed to be enums
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
        Lit::Str => ty == "str" || ty == "string",
        Lit::Bool => ty == "bool",
        Lit::Array => ty.starts_with('['),
        // Enum variants and consts: no type table lookup yet, accept.
        Lit::Path => !int_type(ty) && !matches!(ty, "f32" | "f64" | "bool" | "str" | "string"),
        Lit::Other => false,
    }
}

/// Split `name(args)` into the name and the raw argument text.
fn split_attr(attr: &str) -> (&str, Option<&str>) {
    let attr = attr.trim();
    match attr.find('(') {
        Some(open) if attr.ends_with(')') => (&attr[..open], Some(&attr[open + 1..attr.len() - 1])),
        _ => (attr, None),
    }
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
    /// AN2-AN5 over the whole program. Runs after type declarations are
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
                    span: ann.name_span,
                });
                continue;
            }
            for field in &ann.fields {
                if !const_representable(&field.ty) {
                    let fix = if field.ty.trim() == "string" {
                        "use `str` — annotation values are embedded constants".to_string()
                    } else {
                        "annotation fields are limited to primitives, str, enums, and fixed arrays of these".to_string()
                    };
                    self.errors.push(TypeError::BadAnnotation {
                        name: ann.name.clone(),
                        problem: format!("field `{}: {}` is not const-representable", field.name, field.ty),
                        fix,
                        span: field.name_span,
                    });
                }
            }
            if declared.insert(ann.name.as_str(), ann).is_some() {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: "an annotation with this name is already declared".to_string(),
                    fix: "rename one of them — readers look annotations up by name".to_string(),
                    span: ann.name_span,
                });
            }
        }
        if declared.is_empty() {
            return;
        }

        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => {
                    self.check_attachment_site(&s.attrs, AnnotationTarget::Struct, decl.span, &declared);
                    for field in &s.fields {
                        self.check_attachment_site(&field.attrs, AnnotationTarget::Field, field.name_span, &declared);
                    }
                }
                DeclKind::Enum(e) => {
                    self.check_attachment_site(&e.attrs, AnnotationTarget::Enum, decl.span, &declared);
                }
                DeclKind::Fn(f) => {
                    self.check_attachment_site(&f.attrs, AnnotationTarget::Func, decl.span, &declared);
                }
                _ => {}
            }
        }
    }

    fn check_attachment_site(
        &mut self,
        attrs: &[String],
        site: AnnotationTarget,
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
                    problem: format!("attached twice to the same {}", site.as_str()),
                    fix: "one attachment per item — repetition wants an array field".to_string(),
                    span,
                });
                continue;
            }
            seen.push(name);

            // AN2: declared targets bound where the annotation may sit.
            if !ann.targets.is_empty() && !ann.targets.contains(&site) {
                let targets: Vec<&str> = ann.targets.iter().map(|t| t.as_str()).collect();
                self.errors.push(TypeError::BadAnnotation {
                    name: name.to_string(),
                    problem: format!("cannot attach to a {} — declared `on {}`", site.as_str(), targets.join(", ")),
                    fix: format!("attach it to a {}, or widen the declaration's `on` clause", targets.join(" or ")),
                    span,
                });
                continue;
            }

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
                    fix: format!("annotation arguments use field names, like a struct literal: @{}(field: value)", ann.name),
                    span,
                });
                continue;
            };
            let Some(field) = ann.fields.iter().find(|f| f.name == arg_name) else {
                self.errors.push(TypeError::BadAnnotation {
                    name: ann.name.clone(),
                    problem: format!("no field `{}` on this annotation", arg_name),
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
                fix: format!("fields without defaults must be given: @{}({}: ...)", ann.name, missing[0]),
                span,
            });
        }
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Naming convention rules.
//!
//! Check that method prefixes match their return type semantics:
//! from_* → returns Self, into_* → takes self, is_* → returns bool, etc.

use rask_ast::decl::*;

use crate::types::*;
use crate::util;

/// Context for a method: which type it belongs to.
struct MethodContext<'a> {
    type_name: &'a str,
    method: &'a FnDecl,
    span: rask_ast::Span,
}

/// Collect all methods with their owning type name.
fn collect_methods(decls: &[Decl]) -> Vec<MethodContext<'_>> {
    let mut methods = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    if !is_suppressed(m, "") {
                        methods.push(MethodContext {
                            type_name: &s.name,
                            method: m,
                            span: decl.span,
                        });
                    }
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    if !is_suppressed(m, "") {
                        methods.push(MethodContext {
                            type_name: &e.name,
                            method: m,
                            span: decl.span,
                        });
                    }
                }
            }
            DeclKind::Impl(imp) => {
                for m in &imp.methods {
                    if !is_suppressed(m, "") {
                        methods.push(MethodContext {
                            type_name: &imp.target_ty,
                            method: m,
                            span: decl.span,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    methods
}

fn is_suppressed(f: &FnDecl, rule_id: &str) -> bool {
    f.attrs.iter().any(|a| {
        a == &format!("allow({})", rule_id)
            || a.starts_with("allow(") && rule_id.is_empty()
    })
}

fn is_rule_suppressed(f: &FnDecl, rule_id: &str) -> bool {
    f.attrs
        .iter()
        .any(|a| a == &format!("allow({})", rule_id))
}

fn make_diagnostic(
    rule: &str,
    severity: Severity,
    message: String,
    fix: String,
    source: &str,
    span: rask_ast::Span,
) -> LintDiagnostic {
    let (line, col) = util::line_col(source, span.start);
    let source_line = util::get_source_line(source, line);

    LintDiagnostic {
        rule: rule.to_string(),
        severity,
        message,
        location: LintLocation {
            line,
            column: col,
            source_line,
        },
        fix,
    }
}

/// naming/from: `from_*` should return Self or Self or E.
pub fn check_from(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/from") {
            continue;
        }
        if !ctx.method.name.starts_with("from_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            let ret_lower = ret.to_lowercase();
            let type_lower = ctx.type_name.to_lowercase();
            if !ret_lower.contains(&type_lower) && !ret.contains("Self") {
                diags.push(make_diagnostic(
                    "naming/from",
                    Severity::Warning,
                    format!(
                        "`{}` should return `{}` or `{} or E`, found `{}`",
                        ctx.method.name, ctx.type_name, ctx.type_name, ret
                    ),
                    format!(
                        "change return type to `{}` or `{} or E`",
                        ctx.type_name, ctx.type_name
                    ),
                    source,
                    ctx.span,
                ));
            }
        }
    }
    diags
}

/// naming/into: `into_*` should have `take self`.
pub fn check_into(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/into") {
            continue;
        }
        if !ctx.method.name.starts_with("into_") {
            continue;
        }
        let has_take_self = ctx
            .method
            .params
            .first()
            .map(|p| p.name == "self" && p.is_take)
            .unwrap_or(false);
        if !has_take_self {
            diags.push(make_diagnostic(
                "naming/into",
                Severity::Warning,
                format!(
                    "`{}` should take ownership of self",
                    ctx.method.name
                ),
                "change `self` or `read self` to `take self`".to_string(),
                source,
                ctx.span,
            ));
        }
    }
    diags
}

/// naming/as: `as_*` should return a reference or primitive (cheap view).
pub fn check_as(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    let cheap_types = [
        "bool", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64",
        "f32", "f64", "char", "usize", "isize",
    ];
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/as") {
            continue;
        }
        if !ctx.method.name.starts_with("as_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            // Heuristic: references start with & or [], primitives are in the list
            let is_cheap = ret.starts_with('&')
                || ret.starts_with("[]")
                || cheap_types.contains(&ret.as_str());
            if !is_cheap {
                diags.push(make_diagnostic(
                    "naming/as",
                    Severity::Warning,
                    format!(
                        "`{}` returns `{}` which may allocate — `as_*` should be a cheap view",
                        ctx.method.name, ret
                    ),
                    "rename to `to_*` if it allocates, or keep `as_*` if it's a cheap cast"
                        .to_string(),
                    source,
                    ctx.span,
                ));
            }
        }
    }
    diags
}

/// naming/to: `to_*` should return a different type than Self.
pub fn check_to(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/to") {
            continue;
        }
        if !ctx.method.name.starts_with("to_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if ret == ctx.type_name || ret == "Self" {
                diags.push(make_diagnostic(
                    "naming/to",
                    Severity::Warning,
                    format!(
                        "`{}` returns `{}` (same type) — `to_*` should convert to a different type",
                        ctx.method.name, ret
                    ),
                    "rename to `with_*` for builder-style methods on the same type".to_string(),
                    source,
                    ctx.span,
                ));
            }
        }
    }
    diags
}

/// naming/is: `is_*` must return `bool`.
pub fn check_is(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    // Check methods
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/is") {
            continue;
        }
        if !ctx.method.name.starts_with("is_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if ret != "bool" {
                diags.push(make_diagnostic(
                    "naming/is",
                    Severity::Error,
                    format!(
                        "`{}` must return `bool`, found `{}`",
                        ctx.method.name, ret
                    ),
                    format!(
                        "change return type to `bool`, or rename to remove the `is_` prefix"
                    ),
                    source,
                    ctx.span,
                ));
            }
        }
    }

    // Also check standalone functions
    for decl in decls {
        if let DeclKind::Fn(f) = &decl.kind {
            if is_rule_suppressed(f, "naming/is") || !f.name.starts_with("is_") {
                continue;
            }
            if let Some(ret) = &f.ret_ty {
                if ret != "bool" {
                    diags.push(make_diagnostic(
                        "naming/is",
                        Severity::Error,
                        format!("`{}` must return `bool`, found `{}`", f.name, ret),
                        "change return type to `bool`, or rename to remove the `is_` prefix"
                            .to_string(),
                        source,
                        decl.span,
                    ));
                }
            }
        }
    }

    diags
}

/// naming/with: `with_*` should return Self.
pub fn check_with(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/with") {
            continue;
        }
        if !ctx.method.name.starts_with("with_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if ret != ctx.type_name && ret != "Self" {
                diags.push(make_diagnostic(
                    "naming/with",
                    Severity::Warning,
                    format!(
                        "`{}` should return `{}` (builder pattern), found `{}`",
                        ctx.method.name, ctx.type_name, ret
                    ),
                    format!("change return type to `{}`", ctx.type_name),
                    source,
                    ctx.span,
                ));
            }
        }
    }
    diags
}

/// naming/try: `try_*` must return `T or E`.
pub fn check_try(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    // Check methods
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/try") {
            continue;
        }
        if !ctx.method.name.starts_with("try_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if !ret.contains(" or ") {
                diags.push(make_diagnostic(
                    "naming/try",
                    Severity::Error,
                    format!(
                        "`{}` must return a result type (`T or E`), found `{}`",
                        ctx.method.name, ret
                    ),
                    "change return type to `T or E`".to_string(),
                    source,
                    ctx.span,
                ));
            }
        }
    }

    // Standalone functions
    for decl in decls {
        if let DeclKind::Fn(f) = &decl.kind {
            if is_rule_suppressed(f, "naming/try") || !f.name.starts_with("try_") {
                continue;
            }
            if let Some(ret) = &f.ret_ty {
                if !ret.contains(" or ") {
                    diags.push(make_diagnostic(
                        "naming/try",
                        Severity::Error,
                        format!(
                            "`{}` must return a result type (`T or E`), found `{}`",
                            f.name, ret
                        ),
                        "change return type to `T or E`".to_string(),
                        source,
                        decl.span,
                    ));
                }
            }
        }
    }

    diags
}

/// naming/or_suffix: `*_or(default)` should return unwrapped T.
pub fn check_or_suffix(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
        if is_rule_suppressed(ctx.method, "naming/or_suffix") {
            continue;
        }
        if !ctx.method.name.ends_with("_or") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if ret.contains(" or ") || ret.ends_with('?') {
                diags.push(make_diagnostic(
                    "naming/or_suffix",
                    Severity::Warning,
                    format!(
                        "`{}` should return unwrapped `T`, found `{}`",
                        ctx.method.name, ret
                    ),
                    "return the unwrapped value type — `*_or` provides a fallback".to_string(),
                    source,
                    ctx.span,
                ));
            }
        }
    }
    diags
}

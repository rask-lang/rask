// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Naming convention rules.
//!
//! Check that method prefixes match their return type semantics:
//! from_* → returns Self, into_* → takes self, is_* → returns bool, etc.

use rask_ast::decl::*;
use rask_ast::type_str;
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

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
                    methods.push(MethodContext {
                        type_name: &s.name,
                        method: m,
                        span: m.span,
                    });
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    methods.push(MethodContext {
                        type_name: &e.name,
                        method: m,
                        span: m.span,
                    });
                }
            }
            DeclKind::Impl(imp) => {
                for m in &imp.methods {
                    methods.push(MethodContext {
                        type_name: &imp.target_ty,
                        method: m,
                        span: m.span,
                    });
                }
            }
            _ => {}
        }
    }

    methods
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

/// naming/as: `as_*` hands back something the value already has, without
/// allocating (`canonical-patterns`).
///
/// Two things say so, and either is enough. The **type**: a primitive, a raw
/// pointer, a slice, or a `string`/`StringView` — a string is a sixteen-byte
/// value plus a refcount bump, which is what `std.strings/V1` calls a view.
/// Or the **body**: every `return` hands back a name, a field, or `none`,
/// constructing nothing. That second test is what lets `as_array` hand back the
/// `Vec` its enum payload already holds while `as_bytes`, which calls
/// `.clone()`, still fails.
///
/// The old test asked whether the return type started with `&`. No Rask type
/// string ever does — that is a Rust reference sigil — so the clause was dead
/// and every `as_ptr` in the stdlib was reported as possibly allocating (#993).
pub fn check_as(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
        if !ctx.method.name.starts_with("as_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            let is_cheap = cheap_view_type(ret) || returns_without_building(&ctx.method.body);
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

/// A type that can be handed back without allocating.
fn cheap_view_type(ret: &str) -> bool {
    const CHEAP: &[&str] = &[
        "bool", "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64",
        "u128", "f32", "f64", "char", "usize", "isize", "string", "StringView",
        "Span", "()",
    ];
    // An optional over a cheap thing is still cheap — the flag costs nothing.
    let bare = ret.trim().trim_end_matches('?').trim();
    // `*T` is a cast, `[]T` a view: both as cheap as it gets.
    bare.starts_with('*') || bare.starts_with("[]") || CHEAP.contains(&bare)
}

/// Whether every `return` in the body hands back something that already exists
/// — a name, a field path, `self`, or `none`. A call, a method call or a
/// literal builds something, and building is where the allocation is.
///
/// An empty body (a `@native` or `@unimplemented` declaration) has no returns
/// to look at, so it decides nothing and the type test stands alone.
fn returns_without_building(body: &[Stmt]) -> bool {
    let mut saw_return = false;
    let mut all_plain = true;
    walk_returns(body, &mut |e| {
        saw_return = true;
        if !hands_back_existing(e) {
            all_plain = false;
        }
    });
    saw_return && all_plain
}

fn hands_back_existing(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Ident(_) | ExprKind::None => true,
        ExprKind::Field { object, .. } => hands_back_existing(object),
        _ => false,
    }
}

/// Every `return` expression in a body.
///
/// An `if` or a `match` is an expression in Rask, so a return inside one is
/// reached through the statement that holds it rather than through a statement
/// kind of its own.
fn walk_returns(body: &[Stmt], f: &mut impl FnMut(&Expr)) {
    for stmt in body {
        match &stmt.kind {
            StmtKind::Return(Some(e)) => f(e),
            StmtKind::Expr(e) => walk_returns_in_expr(e, f),
            StmtKind::While { body, .. } | StmtKind::Loop { body, .. } => walk_returns(body, f),
            StmtKind::For { body, .. } => walk_returns(body, f),
            _ => {}
        }
    }
}

fn walk_returns_in_expr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    match &expr.kind {
        ExprKind::Block(stmts) => walk_returns(stmts, f),
        ExprKind::If { then_branch, else_branch, .. } => {
            walk_returns_in_expr(then_branch, f);
            if let Some(eb) = else_branch {
                walk_returns_in_expr(eb, f);
            }
        }
        ExprKind::IfLet { then_branch, else_branch, .. } => {
            walk_returns_in_expr(then_branch, f);
            if let Some(eb) = else_branch {
                walk_returns_in_expr(eb, f);
            }
        }
        ExprKind::Match { arms, .. } => {
            for arm in arms {
                walk_returns_in_expr(&arm.body, f);
            }
        }
        _ => {}
    }
}

/// naming/to: `to_*` should return a different type than Self.
pub fn check_to(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();
    for ctx in collect_methods(decls) {
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
            if !f.name.starts_with("is_") {
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
        if !ctx.method.name.starts_with("try_") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if !type_str::is_result(ret) {
                diags.push(make_diagnostic(
                    "naming/try",
                    Severity::Error,
                    format!(
                        "`{}` must return a result type (`T or E`), found `{}`",
                        ctx.method.name, type_str::to_source(ret)
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
            if !f.name.starts_with("try_") {
                continue;
            }
            if let Some(ret) = &f.ret_ty {
                if !type_str::is_result(ret) {
                    diags.push(make_diagnostic(
                        "naming/try",
                        Severity::Error,
                        format!(
                            "`{}` must return a result type (`T or E`), found `{}`",
                            f.name, type_str::to_source(ret)
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
        if !ctx.method.name.ends_with("_or") {
            continue;
        }
        if let Some(ret) = &ctx.method.ret_ty {
            if type_str::is_result(ret) || type_str::is_optional(ret) {
                diags.push(make_diagnostic(
                    "naming/or_suffix",
                    Severity::Warning,
                    format!(
                        "`{}` should return unwrapped `T`, found `{}`",
                        ctx.method.name, type_str::to_source(ret)
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

// ─── naming/try ────────────────────────────────────────────
// The rule reads a *rendered* type string, and the parser renders `T or E` in
// a canonical form the source never uses: `Result<T, E>`. Asking whether that
// string contains `" or "` is asking about the surface syntax, which no
// rendered result type has — so the rule rejected every correctly written
// `try_*` in the language, the stdlib's own included (#893).
#[cfg(test)]
mod try_rule_tests {
    use crate::types::{LintOpts, Severity};

    fn try_errors(src: &str) -> Vec<String> {
        crate::lint(src, "t.rk", LintOpts::default())
            .diagnostics
            .into_iter()
            .filter(|d| d.rule == "naming/try" && d.severity == Severity::Error)
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn accepts_a_result_return_written_in_rask() {
        // Each of these is `T or E`, so each is what the rule asks for. All four
        // were reported as violations before: `void` renders as `()`, and a
        // nested generic error type has to survive the split.
        let src = r#"
struct Queue9<T> { }

extend Queue9<T> {
    public func try_send(self, value: T) -> void or SendError9 {}
    public func try_receive(self) -> T or ReceiveError9 {}
    public func try_push(mutate self, value: T) -> void or GrowError9<T> {}
    public func try_swap(mutate self, value: T) -> Option<T> or GrowError9<T> {}
}

func try_parse(s: string) -> i64 or ParseError9 {
    return 0
}
"#;
        assert_eq!(try_errors(src), Vec::<String>::new());
    }

    #[test]
    fn still_rejects_a_non_result_return() {
        let src = r#"
struct Pool9<T> { }

extend Pool9<T> {
    public func try_insert(mutate self, value: T) -> Handle9<T>? {}
    public func try_peek(self) -> i64 {}
}
"#;
        let errs = try_errors(src);
        assert_eq!(errs.len(), 2, "both should still be flagged: {:?}", errs);
    }

    #[test]
    fn names_the_type_the_way_it_was_written() {
        // Telling someone to write `T or E` while printing `Result<(), E>` names
        // a spelling Rask doesn't have.
        let src = r#"
struct S9 { }
extend S9 {
    public func try_thing(self) -> Handle9<i64>? {}
}
"#;
        let errs = try_errors(src);
        assert_eq!(errs.len(), 1);
        assert!(
            !errs[0].contains("Result<"),
            "message should use Rask syntax, not the internal rendering: {}",
            errs[0]
        );
    }
}

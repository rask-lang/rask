// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Style rules: naming conventions, visibility.

use rask_ast::decl::*;

use crate::types::*;
use crate::util;

/// style/snake-case-func: Function names should be snake_case.
pub fn check_snake_case_func(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => check_fn_snake_case(f, source, decl.span, &mut diags),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    check_fn_snake_case(m, source, decl.span, &mut diags);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    check_fn_snake_case(m, source, decl.span, &mut diags);
                }
            }
            DeclKind::Impl(imp) => {
                for m in &imp.methods {
                    check_fn_snake_case(m, source, decl.span, &mut diags);
                }
            }
            _ => {}
        }
    }

    diags
}

fn check_fn_snake_case(
    f: &FnDecl,
    source: &str,
    span: rask_ast::Span,
    diags: &mut Vec<LintDiagnostic>,
) {
    if is_suppressed(f, "style/snake-case-func") {
        return;
    }
    if !is_snake_case(&f.name) {
        let (line, col) = util::line_col(source, span.start);
        let source_line = util::get_source_line(source, line);
        diags.push(LintDiagnostic {
            rule: "style/snake-case-func".to_string(),
            severity: Severity::Warning,
            message: format!("`{}` should be `snake_case`", f.name),
            location: LintLocation {
                line,
                column: col,
                source_line,
            },
            fix: format!("rename to `{}`", to_snake_case(&f.name)),
        });
    }
}

/// style/pascal-case-type: Type names should be PascalCase.
pub fn check_pascal_case_type(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        let (name, kind) = match &decl.kind {
            DeclKind::Struct(s) => (&s.name, "struct"),
            DeclKind::Enum(e) => (&e.name, "enum"),
            DeclKind::Trait(t) => (&t.name, "trait"),
            _ => continue,
        };

        if !is_pascal_case(name) {
            let (line, col) = util::line_col(source, decl.span.start);
            let source_line = util::get_source_line(source, line);
            diags.push(LintDiagnostic {
                rule: "style/pascal-case-type".to_string(),
                severity: Severity::Warning,
                message: format!("{} `{}` should be `PascalCase`", kind, name),
                location: LintLocation {
                    line,
                    column: col,
                    source_line,
                },
                fix: format!("rename to `{}`", to_pascal_case(name)),
            });
        }
    }

    diags
}

/// style/public-return-type: Public functions should have explicit return types.
pub fn check_public_return_type(decls: &[Decl], source: &str) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => check_fn_return_type(f, source, decl.span, &mut diags),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    check_fn_return_type(m, source, decl.span, &mut diags);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    check_fn_return_type(m, source, decl.span, &mut diags);
                }
            }
            DeclKind::Impl(imp) => {
                for m in &imp.methods {
                    check_fn_return_type(m, source, decl.span, &mut diags);
                }
            }
            _ => {}
        }
    }

    diags
}

fn check_fn_return_type(
    f: &FnDecl,
    source: &str,
    span: rask_ast::Span,
    diags: &mut Vec<LintDiagnostic>,
) {
    if !f.is_pub || is_suppressed(f, "style/public-return-type") {
        return;
    }
    // Only flag if no return type AND body is non-empty (not just a declaration)
    if f.ret_ty.is_none() && !f.body.is_empty() {
        let (line, col) = util::line_col(source, span.start);
        let source_line = util::get_source_line(source, line);
        diags.push(LintDiagnostic {
            rule: "style/public-return-type".to_string(),
            severity: Severity::Error,
            message: format!(
                "public function `{}` is missing a return type annotation",
                f.name
            ),
            location: LintLocation {
                line,
                column: col,
                source_line,
            },
            fix: "add `-> ReturnType` to the function signature".to_string(),
        });
    }
}

fn is_suppressed(f: &FnDecl, rule_id: &str) -> bool {
    f.attrs
        .iter()
        .any(|a| a == &format!("allow({})", rule_id))
}

/// Strip generic type parameters from a name (e.g., "wrap<T>" → "wrap").
fn strip_generics(s: &str) -> &str {
    s.split('<').next().unwrap_or(s)
}

fn is_snake_case(s: &str) -> bool {
    let s = strip_generics(s);
    if s.is_empty() {
        return true;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
}

fn is_pascal_case(s: &str) -> bool {
    let s = strip_generics(s);
    if s.is_empty() {
        return true;
    }
    s.starts_with(|c: char| c.is_ascii_uppercase())
        && !s.contains('_')
        && s.chars().all(|c| c.is_ascii_alphanumeric())
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut result = first.to_ascii_uppercase().to_string();
                    result.extend(chars);
                    result
                }
            }
        })
        .collect()
}

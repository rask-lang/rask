// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rule registry and dispatch.

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::Span;

use crate::types::{LintDiagnostic, LintOpts};
use crate::{naming, idiom, style, util};

/// A lint rule: id, check function.
struct Rule {
    id: &'static str,
    check: fn(&[Decl], &str) -> Vec<LintDiagnostic>,
}

/// All registered rules.
fn all_rules() -> Vec<Rule> {
    vec![
        // Naming conventions
        Rule { id: "naming/from", check: naming::check_from },
        Rule { id: "naming/into", check: naming::check_into },
        Rule { id: "naming/as", check: naming::check_as },
        Rule { id: "naming/to", check: naming::check_to },
        Rule { id: "naming/is", check: naming::check_is },
        Rule { id: "naming/with", check: naming::check_with },
        Rule { id: "naming/try", check: naming::check_try },
        Rule { id: "naming/or_suffix", check: naming::check_or_suffix },
        // Idiomatic patterns
        Rule { id: "idiom/unwrap-production", check: idiom::check_unwrap_production },
        Rule { id: "idiom/missing-ensure", check: idiom::check_missing_ensure },
        Rule { id: "idiom/ensure-ordering", check: idiom::check_ensure_ordering },
        Rule { id: "idiom/large-unsafe-block", check: idiom::check_large_unsafe_blocks },
        Rule { id: "idiom/duck-trait", check: idiom::check_duck_trait },
        Rule { id: "idiom/equality-absent-check", check: idiom::check_equality_absent_check },
        Rule { id: "idiom/mod-for-index", check: idiom::check_mod_for_index },
        Rule { id: "idiom/too-many-contexts", check: idiom::check_too_many_contexts },
        // Style
        Rule { id: "style/snake-case-func", check: style::check_snake_case_func },
        Rule { id: "style/pascal-case-type", check: style::check_pascal_case_type },
        Rule { id: "style/public-return-type", check: style::check_public_return_type },
    ]
}

/// Every registered rule id, for the check that `@allow` names one that exists.
pub fn rule_ids() -> Vec<&'static str> {
    all_rules().into_iter().map(|r| r.id).collect()
}

/// Run selected rules against declarations.
pub fn run_rules(decls: &[Decl], source: &str, opts: &LintOpts) -> Vec<LintDiagnostic> {
    let mut results = Vec::new();

    for rule in all_rules() {
        if !should_run(rule.id, opts) {
            continue;
        }
        results.extend((rule.check)(decls, source));
    }

    let scopes = allow_scopes(decls, source);
    results.retain(|d| !suppressed(d, &scopes));
    results
}

/// A declaration that carries `@allow(...)`, and the lines it covers.
struct AllowScope {
    first_line: usize,
    last_line: usize,
    allowed: Vec<String>,
}

/// `@allow` used to be honoured by each rule that remembered to ask, so eleven
/// of the twenty did and the rest ignored it in silence. One filter over the
/// results means a name that's accepted is a name that works.
fn allow_scopes(decls: &[Decl], source: &str) -> Vec<AllowScope> {
    let mut scopes = Vec::new();
    let mut add = |span: Span, attrs: &[String]| {
        let allowed: Vec<String> = attrs
            .iter()
            .filter_map(|a| rask_ast::allow_names::allowed_name(a).map(str::to_string))
            .collect();
        if allowed.is_empty() {
            return;
        }
        scopes.push(AllowScope {
            first_line: util::line_col(source, span.start).0,
            last_line: util::line_col(source, span.end).0,
            allowed,
        });
    };
    for decl in decls {
        match &decl.kind {
            // The declaration's span, not the function's: it starts at the
            // first `@`, and a rule that underlines the attribute line itself
            // would otherwise fall outside its own scope.
            DeclKind::Fn(f) => add(decl.span, &f.attrs),
            DeclKind::Struct(s) => {
                add(decl.span, &s.attrs);
                for m in &s.methods {
                    add(m.span, &m.attrs);
                }
            }
            DeclKind::Enum(e) => {
                add(decl.span, &e.attrs);
                for m in &e.methods {
                    add(m.span, &m.attrs);
                }
            }
            DeclKind::Impl(i) => {
                for m in &i.methods {
                    add(m.span, &m.attrs);
                }
            }
            DeclKind::Trait(t) => add(decl.span, &t.attrs),
            DeclKind::Test(t) => add(decl.span, &t.attrs),
            DeclKind::Benchmark(b) => add(decl.span, &b.attrs),
            _ => {}
        }
    }
    scopes
}

fn suppressed(diag: &LintDiagnostic, scopes: &[AllowScope]) -> bool {
    scopes.iter().any(|s| {
        diag.location.line >= s.first_line
            && diag.location.line <= s.last_line
            && s.allowed.iter().any(|a| a == &diag.rule)
    })
}

/// Check if a rule should run based on include/exclude filters.
fn should_run(rule_id: &str, opts: &LintOpts) -> bool {
    // Exclude takes priority
    for pattern in &opts.excludes {
        if matches_rule(rule_id, pattern) {
            return false;
        }
    }

    // If no include filters, run all
    if opts.rules.is_empty() {
        return true;
    }

    // Must match at least one include filter
    for pattern in &opts.rules {
        if matches_rule(rule_id, pattern) {
            return true;
        }
    }

    false
}

/// Match a rule ID against a glob pattern.
/// Supports: exact match, "category/*" for all rules in a category.
fn matches_rule(rule_id: &str, pattern: &str) -> bool {
    if pattern == rule_id {
        return true;
    }

    // "naming/*" matches "naming/from", "naming/to", etc.
    if let Some(prefix) = pattern.strip_suffix("/*") {
        if let Some(rule_prefix) = rule_id.split('/').next() {
            return rule_prefix == prefix;
        }
    }

    false
}

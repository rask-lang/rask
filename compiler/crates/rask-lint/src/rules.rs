// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rule registry and dispatch.

use rask_ast::decl::Decl;

use crate::types::{LintDiagnostic, LintOpts};
use crate::{naming, idiom, style};

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
        // Style
        Rule { id: "style/snake-case-func", check: style::check_snake_case_func },
        Rule { id: "style/pascal-case-type", check: style::check_pascal_case_type },
        Rule { id: "style/public-return-type", check: style::check_public_return_type },
    ]
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

    results
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

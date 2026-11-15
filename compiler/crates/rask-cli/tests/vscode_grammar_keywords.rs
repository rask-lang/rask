// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The VS Code grammar colors every keyword the lexer has, and no word the
//! lexer doesn't.
//!
//! `rask_ast::token::KEYWORDS` is the one keyword list. The grammar's own copy
//! had drifted: `let` was missing (so it came out as a plain variable), and it
//! still listed `deliver`, `own` and `step`, which are ordinary identifiers.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use rask_ast::token::KEYWORDS;

fn grammar() -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../editors/vscode/syntaxes/rask.tmLanguage.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("grammar is valid JSON")
}

/// The words in a pattern like `\b(a|b|c)\b`, `\bvoid\b` or
/// `(?<=\.)\s*(a|b)(?=\s*\()`.
fn words_in(pattern: &str) -> Vec<String> {
    let inner = pattern
        .trim_start_matches("(?<=\\.)\\s*")
        .trim_end_matches("(?=\\s*\\()")
        .trim_start_matches("\\b")
        .trim_end_matches("\\b");
    inner
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split('|')
        .map(str::to_string)
        .collect()
}

fn lexer_keywords() -> BTreeSet<String> {
    KEYWORDS.iter().map(|(_, word)| word.to_string()).collect()
}

fn diff(grammar_words: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    let lexer = lexer_keywords();
    let missing = lexer.difference(grammar_words).cloned().collect();
    let extra = grammar_words.difference(&lexer).cloned().collect();
    (missing, extra)
}

#[test]
fn keyword_rules_match_the_lexers_keyword_table() {
    let grammar = grammar();
    let patterns = grammar["repository"]["keywords"]["patterns"].as_array().expect("keywords patterns");
    let mut words = BTreeSet::new();
    for pattern in patterns {
        let m = pattern["match"].as_str().expect("keyword rule has a match");
        words.extend(words_in(m));
    }
    // `self` is an ordinary name the language gives a meaning, not a keyword.
    words.remove("self");

    let (missing, extra) = diff(&words);
    assert!(missing.is_empty(), "keywords the grammar doesn't color: {missing:?}");
    assert!(extra.is_empty(), "words the grammar colors that aren't keywords: {extra:?}");
}

#[test]
fn keyword_method_names_match_the_lexers_keyword_table() {
    let grammar = grammar();
    let rule = &grammar["repository"]["method-calls"]["patterns"][0];
    let words: BTreeSet<String> = words_in(rule["match"].as_str().expect("match")).into_iter().collect();

    let (missing, extra) = diff(&words);
    assert!(missing.is_empty(), "keywords `x.kw(..)` would color as keywords: {missing:?}");
    assert!(extra.is_empty(), "non-keywords in the keyword-method rule: {extra:?}");
}

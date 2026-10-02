// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! No pass reads a type back out of text.
//!
//! The parser builds a `TypeExpr`, the checker a `Type`, and each pass hands the
//! next what it worked out. The compiler used to render types to strings and
//! parse them again downstream — seven parsers that had drifted apart, each
//! disagreement a bug that showed on one backend only. This test fails the build
//! when that shape comes back: slicing a name at `<`, or asking a rendered type
//! whether it ends in `?` or contains ` or `.
//!
//! If a pass needs to know something an earlier one knew, add a field and pass
//! it along (compiler/CLAUDE.md, "Hand information down").

use std::fs;
use std::path::{Path, PathBuf};

/// Text that is source code or another language's syntax, read on purpose.
const ALLOWED: &[(&str, &str)] = &[
    ("rask-lexer/", "reads source text"),
    ("rask-parser/", "reads source text"),
    ("rask-c-parse/", "reads C headers"),
    ("rask-ast/src/fmt_spec.rs", "reads format specs like `{:<10}`"),
    ("rask-resolve/src/advisory.rs", "reads version constraints like `<1.0`"),
    ("rask-wasm/", "escapes HTML"),
];

/// What re-reading a type out of text looks like.
const PATTERNS: &[&str] = &[
    "split('<')",
    "split_once('<')",
    "find('<')",
    "rfind('<')",
    "contains('<')",
    "starts_with('<')",
    "strip_suffix('>')",
    "trim_end_matches('>')",
    "ends_with('?')",
    "contains(\" or \")",
    "'<' =>",
    "'<' |",
];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_pass_parses_type_text() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates).unwrap().flatten() {
        rust_sources(&entry.path().join("src"), &mut files);
    }
    assert!(!files.is_empty(), "found no compiler sources under {}", crates.display());

    let mut found = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(&crates).unwrap().to_string_lossy().replace('\\', "/");
        if ALLOWED.iter().any(|(prefix, _)| rel.starts_with(prefix)) {
            continue;
        }
        // Tests may spell out what text a type renders to.
        if rel.ends_with("/tests.rs") {
            continue;
        }
        let src = fs::read_to_string(path).unwrap();
        // Everything after a crate's `#[cfg(test)] mod tests` is test code.
        let body = src.split("#[cfg(test)]\nmod tests").next().unwrap_or(&src);
        for (i, line) in body.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            if let Some(p) = PATTERNS.iter().find(|p| code.contains(*p)) {
                found.push(format!("{}:{}: `{}`\n    {}", rel, i + 1, p, line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these lines read a type back out of text:\n\n{}\n\n\
         Hand the structured form down instead (`TypeExpr`, `Type`, a field on \
         the record the earlier pass builds) — see \"Hand information down\" in \
         compiler/CLAUDE.md.",
        found.join("\n")
    );
}

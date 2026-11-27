// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The native pipeline finds a type's layout and its handle-ness from the type,
//! never from its name (rask-lang/rask#1567).
//!
//! Two shapes are forbidden:
//!
//! - Searching a struct or enum layout list by a layout's `.name`
//!   (`layouts.iter().find(|l| l.name == name)`) in rask-mir, rask-codegen,
//!   rask-mono or rask-layout. A checked type carries its declaration's id, and
//!   `rask_layout::struct_for` / `enum_for` find the layout by it; a MIR type
//!   carries the layout index. Two layouts can share a name, and a type can
//!   reach a pass under a spelling its layout doesn't use.
//! - Asking whether a type is a runtime handle by its name
//!   (`rask_stdlib::is_runtime_handle` / `runtime_handle`) anywhere outside
//!   rask-stdlib. A declared handle is asked about by its id
//!   (`TypeTable::is_handle`, `TypeDef::runtime_handle`); only a type with no
//!   id goes through `rask_stdlib::spelled_handle`.
//!
//! `ALLOWED` lists the layout searches that remain, each with the issue that
//! removes it. Delete an entry when its search goes.

use std::fs;
use std::path::{Path, PathBuf};

/// (file under `compiler/crates/`, enclosing function) of each search left,
/// all tracked by rask-lang/rask#1856. `find_struct` / `find_enum` answer
/// "does this written name name a type" for MIR lowering, which should come
/// from the checker's resolution. `lower_field`'s last resort takes the first
/// layout anywhere with a field of the wanted name.
const ALLOWED: &[(&str, &str)] = &[
    ("rask-mir/src/lower/mod.rs", "find_struct"),
    ("rask-mir/src/lower/mod.rs", "find_enum"),
    ("rask-mir/src/lower/expr.rs", "lower_field"),
];

const LAYOUT_CRATES: &[&str] = &["rask-mir", "rask-codegen", "rask-mono", "rask-layout"];

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

/// The source with comments and all whitespace gone, so a chain split over
/// lines reads as one run.
fn squashed(src: &str) -> String {
    src.lines()
        .map(|l| l.split("//").next().unwrap_or(l))
        .flat_map(|l| l.chars().filter(|c| !c.is_whitespace()))
        .collect()
}

/// The last `fn name` before `at`.
fn enclosing_fn(code: &str, at: usize) -> &str {
    let Some(start) = code[..at].rfind("fn") else { return "" };
    let rest = &code[start + 2..];
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn no_layout_list_is_searched_by_name() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for krate in LAYOUT_CRATES {
        let mut files = Vec::new();
        rust_sources(&crates.join(krate).join("src"), &mut files);
        assert!(!files.is_empty(), "found no sources for {krate}");
        for path in files {
            let rel = path.strip_prefix(&crates).unwrap().to_string_lossy().replace('\\', "/");
            let code = squashed(&fs::read_to_string(&path).unwrap());
            for (at, _) in code.match_indices(".name==") {
                // The statement this comparison sits in, back to its start.
                let from = code[..at].rfind(';').map_or(0, |i| i + 1);
                let stmt = &code[from..at];
                let searches_layouts = ["layouts.iter()", "enums.iter()"].iter().any(|l| stmt.contains(l))
                    && [".find(", ".position(", ".any(", ".filter("].iter().any(|s| stmt.contains(s));
                if !searches_layouts {
                    continue;
                }
                let func = enclosing_fn(&code, at);
                if !ALLOWED.contains(&(rel.as_str(), func)) {
                    found.push(format!("{rel}: fn {func}"));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "these search a layout list by the layout's name:\n  {}\n\nFind the layout from the \
         type instead: `rask_layout::struct_for` / `enum_for` with a checked `Type`, or the \
         index a `MirType::Struct` / `MirType::Enum` already carries (rask-lang/rask#1567).",
        found.join("\n  ")
    );
}

#[test]
fn no_handle_is_asked_about_by_name() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_sources(&crates, &mut files);
    let mut found = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&crates).unwrap().to_string_lossy().replace('\\', "/");
        if rel.starts_with("rask-stdlib/") || rel.ends_with("no_lookup_by_name.rs") {
            continue;
        }
        let code = squashed(&fs::read_to_string(&path).unwrap());
        for banned in ["rask_stdlib::is_runtime_handle", "rask_stdlib::runtime_handle"] {
            if code.contains(banned) {
                found.push(format!("{rel}: {banned}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "these ask whether a type is a runtime handle by its name:\n  {}\n\nAsk the declaration: \
         `TypeTable::is_handle(id)` or `TypeDef::runtime_handle()`. Only a type with no id goes \
         through `rask_stdlib::spelled_handle` (rask-lang/rask#1567).",
        found.join("\n  ")
    );
}

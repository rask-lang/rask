// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Every attribute the compiler reads by name is in `rask_ast::attr::COMPILER_ATTRS`.
//!
//! That table is the one list of compiler-owned attribute names: the parser
//! asks it how an attribute's arguments are written, and the checker refuses
//! its names to user annotations. A pass that starts reading `@foo` without
//! listing it lets a user declare `annotation @foo` and have the compiler act
//! on it. Before the table, the checker kept its own reserved list and it had
//! already fallen behind what the compiler read (`layout`, `small`, `no_io`).

use std::fs;
use std::path::{Path, PathBuf};

/// How a pass asks for an attribute by name.
const READS: &[&str] = &[".is(\"", "attr::find(", "attr::has(", "has_attr(", "Attr::synthetic(\""];

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

/// The attribute name a read on this line asks for: the first string literal
/// after the read.
fn names_read(code: &str) -> Vec<String> {
    let mut names = Vec::new();
    for read in READS {
        let mut rest = code;
        while let Some(at) = rest.find(read) {
            rest = &rest[at + read.len()..];
            let from = if read.ends_with('"') { Some(0) } else { rest.find('"').map(|q| q + 1) };
            let Some(from) = from else { continue };
            let Some(len) = rest[from..].find('"') else { continue };
            names.push(rest[from..from + len].to_string());
        }
    }
    names
}

#[test]
fn every_attribute_the_compiler_reads_is_listed() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates).unwrap().flatten() {
        rust_sources(&entry.path().join("src"), &mut files);
    }
    assert!(!files.is_empty(), "found no compiler sources under {}", crates.display());

    let mut missing = Vec::new();
    let mut seen = 0;
    for path in &files {
        let rel = path.strip_prefix(&crates).unwrap().to_string_lossy().replace('\\', "/");
        if rel.ends_with("/tests.rs") {
            continue;
        }
        let src = fs::read_to_string(path).unwrap();
        // Everything after a crate's `#[cfg(test)] mod tests` is test code.
        let body = src.split("#[cfg(test)]\nmod tests").next().unwrap_or(&src);
        for (i, line) in body.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            for name in names_read(code) {
                seen += 1;
                if rask_ast::attr::compiler_attr(&name).is_none() {
                    missing.push(format!("{}:{}: `@{}`\n    {}", rel, i + 1, name, line.trim()));
                }
            }
        }
    }
    // The scan has to find the reads it's guarding, or it guards nothing.
    assert!(seen > 20, "only {} attribute reads found; has the way passes read attributes changed?", seen);
    assert!(
        missing.is_empty(),
        "the compiler reads these attributes, and `COMPILER_ATTRS` in \
         rask-ast/src/attr.rs doesn't list them:\n\n{}\n\n\
         Add each to the table, so the parser knows its argument form and no \
         user annotation can take the name.",
        missing.join("\n")
    );
}

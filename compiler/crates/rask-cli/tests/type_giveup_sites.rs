// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The number of places MIR lowering gives up on a type only goes down.
//!
//! Every `unknown_type("site")` call is a place lowering couldn't read a type
//! the checker should have handed it (rask-lang/rask#725). None of the
//! current sites is reached by any program in the corpus, and each one is
//! fatal when reached, so they are not a bug today. A new one would be: it
//! means a new lowering path was written to guess instead of asking the
//! checker. Adding one to a path that genuinely has no checked type to read
//! is fixing the wrong end — give the checker a table to carry the fact, then
//! read it here.
//!
//! When a site is deleted, lower the number.

use std::fs;
use std::path::{Path, PathBuf};

/// Sites in `rask-mir/src/lower/` today. Only ever decrease it.
const SITES: usize = 49;

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
fn lowering_does_not_grow_new_places_to_give_up_on_a_type() {
    let mir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rask-mir/src");
    let mut files = Vec::new();
    rust_sources(&mir, &mut files);
    assert!(!files.is_empty(), "found no sources under {}", mir.display());

    let mut sites = Vec::new();
    for path in &files {
        let rel = path.strip_prefix(&mir).unwrap().to_string_lossy().replace('\\', "/");
        let src = fs::read_to_string(path).unwrap();
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            if code.contains("fallback::unknown_type(") {
                sites.push(format!("{}:{}: {}", rel, i + 1, line.trim()));
            }
        }
    }

    assert!(
        sites.len() <= SITES,
        "{} places give up on a type; the count was {SITES}. A new one is a \
         lowering path guessing where it should read what the checker worked \
         out — carry the type down from the checker instead (rask-lang/rask#725).\n\n{}",
        sites.len(),
        sites.join("\n")
    );
    assert!(
        sites.len() == SITES,
        "{} places give up on a type, down from {SITES} — lower `SITES` in this test \
         so the number can't creep back up.",
        sites.len()
    );
}

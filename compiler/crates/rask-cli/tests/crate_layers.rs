// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Dependencies between compiler crates point one way.
//!
//! A crate that can reach another crate's types can re-derive that crate's
//! answers, and over time it does. `rask-diagnostics` used to convert every
//! crate's errors, so it depended on all of them, and because MIR used it, MIR
//! and codegen were compiled against the interpreter. Each crate now converts
//! its own errors (`src/diagnostic.rs`), and this test keeps the graph that
//! way. It reads the `Cargo.toml` files directly, so a new edge fails here
//! before anything uses it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// `rask-*` crate → the `rask-*` crates it depends on, dev-dependencies left out.
fn graph() -> BTreeMap<String, BTreeSet<String>> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(&crates).unwrap().flatten() {
        let Ok(toml) = fs::read_to_string(entry.path().join("Cargo.toml")) else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        let mut deps = BTreeSet::new();
        let mut section = "";
        for line in toml.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line;
            } else if section == "[dependencies]" && line.starts_with("rask-") {
                deps.insert(line.split([' ', '=']).next().unwrap().to_string());
            }
        }
        out.insert(name, deps);
    }
    assert!(out.len() > 20, "found only {} crates under {}", out.len(), crates.display());
    out
}

fn reaches(graph: &BTreeMap<String, BTreeSet<String>>, from: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from.to_string()];
    while let Some(c) = stack.pop() {
        for d in graph.get(&c).into_iter().flatten() {
            if seen.insert(d.clone()) {
                stack.push(d.clone());
            }
        }
    }
    seen
}

#[test]
fn diagnostics_depends_only_on_the_ast() {
    let g = graph();
    let deps = &g["rask-diagnostics"];
    let extra: Vec<_> = deps.iter().filter(|d| *d != "rask-ast").collect();
    assert!(
        extra.is_empty(),
        "rask-diagnostics depends on {extra:?}. A crate's errors are converted in that \
         crate's own src/diagnostic.rs, so diagnostics never needs to see them"
    );
}

#[test]
fn the_native_pipeline_never_reaches_the_interpreter() {
    let g = graph();
    let mut found = Vec::new();
    for c in ["rask-mono", "rask-mir", "rask-codegen"] {
        if reaches(&g, c).contains("rask-interp") {
            found.push(c);
        }
    }
    assert!(
        found.is_empty(),
        "{found:?} can reach rask-interp. The interpreter is the reference the native \
         backend is checked against; anything below the checker that shares its code \
         can't disagree with it, and the differential gate stops measuring"
    );
}

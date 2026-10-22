// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The Copy rule's sizes are the sizes codegen lays values out in.
//!
//! Two walks compute a type's size: `TypeTable::value_layout`, over the
//! checker's types, which the 16-byte Copy threshold and `reflect.size_of`
//! read; and mono's layouts, over the declarations, which codegen reads. Both
//! take their arithmetic from `rask_types::abi`, but each decides on its own
//! what a field is. When they disagreed, a type could copy as 10 bytes and
//! lay out as 24, and `reflect.size_of` couldn't answer without contradicting
//! one of them (#791). This compiles every suite program and example and
//! holds every non-generic struct and enum to one answer.

use std::path::{Path, PathBuf};

use rask_compiler::{compile_file, CfgConfig, CompilerConfig};
use rask_types::{Type, TypeDef};

fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    for dir in ["tests/suite", "examples"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "rk") {
                files.push(p);
            }
        }
    }
    files.sort();
    files
}

/// One program's disagreements, and how many types it checked.
fn check(file: &Path, config: &CompilerConfig) -> (Vec<String>, usize) {
    let out = compile_file(file.to_str().unwrap(), config);
    let Some(result) = out.result else { return (Vec::new(), 0) };
    let types = &result.typed.types;
    let structs = result.mono.struct_layouts.iter().map(|l| (&l.name, l.size, l.align, true));
    let enums = result.mono.enum_layouts.iter().map(|l| (&l.name, l.size, l.align, false));
    let mut mismatches = Vec::new();
    let mut checked = 0;
    for (name, size, align, is_struct) in structs.chain(enums) {
        if rask_types::abi::opaque_layout(name).is_some() {
            continue;
        }
        let Some(id) = types.get_type_id(name) else { continue };
        // The same name can be the program's interface and a stdlib struct.
        let same_kind = match types.get(id) {
            Some(TypeDef::Struct { type_params, .. }) => is_struct && type_params.is_empty(),
            Some(TypeDef::Enum { type_params, .. }) => !is_struct && type_params.is_empty(),
            _ => false,
        };
        if !same_kind {
            continue;
        }
        checked += 1;
        let want = types.value_layout(&Type::Named(id));
        if want != (size, align) {
            mismatches.push(format!(
                "{}: {name} is ({size}, {align}) laid out, ({}, {}) to the Copy rule",
                file.file_name().unwrap().to_string_lossy(),
                want.0,
                want.1
            ));
        }
    }
    (mismatches, checked)
}

#[test]
fn copy_rule_sizes_match_layouts() {
    let config = CompilerConfig { cfg: CfgConfig::from_host("debug", vec![]) };
    let files = corpus();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = files.len().div_ceil(workers).max(1);
    let (mut mismatches, checked) = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                let config = &config;
                scope.spawn(move || {
                    part.iter().fold((Vec::new(), 0usize), |(mut all, n), f| {
                        let (m, c) = check(f, config);
                        all.extend(m);
                        (all, n + c)
                    })
                })
            })
            .collect();
        handles.into_iter().fold((Vec::new(), 0usize), |(mut all, n), h| {
            let (m, c) = h.join().expect("a worker panicked");
            all.extend(m);
            (all, n + c)
        })
    });
    mismatches.sort();
    mismatches.dedup();
    assert!(checked > 100, "only {checked} types checked; is the corpus there?");
    assert!(mismatches.is_empty(), "{} disagreements:\n{}", mismatches.len(), mismatches.join("\n"));
}

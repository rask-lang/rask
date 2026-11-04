// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Compiles the runtime's `math.c` and the CORE-MATH sources behind it into the
//! interpreter.
//!
//! Float math has to give the same bits on both backends (`determinism/D12`),
//! and Rust's `f64::sin` is whatever the platform's libm answers. So the
//! interpreter calls the same C functions native code links, compiled from the
//! same files. `-ffp-contract=off` is part of that: `rask-cli`'s link passes it
//! for the native runtime, and an interpreter built without it would fuse
//! multiply-adds on aarch64 and drift from native there.
//!
//! wasm32 (the playground) has no C toolchain in the build, so it keeps Rust's
//! own math: deterministic, but not correctly rounded, so it can differ from
//! native in the last bit (`stdlib::float_math`).

use std::path::{Path, PathBuf};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let runtime = manifest.join("../../runtime");
    let core_math = runtime.join("core-math");

    let mut files = vec![runtime.join("math.c")];
    collect(&core_math, &mut files);
    files.sort();

    println!("cargo:rerun-if-changed={}", runtime.join("math.c").display());
    println!("cargo:rerun-if-changed={}", runtime.join("rask_runtime.h").display());
    println!("cargo:rerun-if-changed={}", runtime.join("rask_abi.h").display());

    cc::Build::new()
        .files(&files)
        .include(&runtime)
        .flag("-ffp-contract=off")
        .flag_if_supported("-std=gnu11")
        .define("_GNU_SOURCE", None)
        .define("_DARWIN_C_SOURCE", None)
        .opt_level(2)
        .warnings(false)
        .compile("rask_math");
}

/// The `.c` files under `dir`; every file and directory is watched.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            collect(&path, out);
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
            if path.extension().and_then(|s| s.to_str()) == Some("c") {
                out.push(path);
            }
        }
    }
}

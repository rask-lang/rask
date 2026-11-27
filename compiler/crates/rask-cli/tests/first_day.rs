// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The first day with Rask, as a newcomer lives it: make a project, run it,
//! test it, check it, and write the small programs and the common mistakes
//! everyone writes in their first hour. Each test here is something that was
//! broken or misleading for that newcomer once; it fails if it comes back.
//!
//! Programs are written inline so the test reads as the story it guards.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn rask_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rask"))
}

/// A fresh directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rask_first_day_{}_{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit {}\n--- stdout\n{}--- stderr\n{}", self.code, self.stdout, self.stderr)
    }
}

fn finish(out: Output) -> Run {
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// `rask <args>` in `dir`.
fn rask_in(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(rask_binary())
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run rask");
    finish(out)
}

/// `rask run` on `source`, natively or on the interpreter.
fn run_program(source: &str, interp: bool) -> Run {
    let dir = TempDir::new();
    dir.write("main.rk", source);
    let mut args = vec!["run"];
    if interp {
        args.push("--interp");
    }
    args.push("main.rk");
    rask_in(dir.path(), &args)
}

/// `rask check` on `source`.
fn check_program(source: &str) -> Run {
    let dir = TempDir::new();
    dir.write("main.rk", source);
    rask_in(dir.path(), &["check", "main.rk"])
}

/// Both backends panic, and both say `needle`.
fn panics_on_both(source: &str, needle: &str) {
    for interp in [true, false] {
        let run = run_program(source, interp);
        let backend = if interp { "interpreter" } else { "native" };
        assert_eq!(run.code, 101, "{backend} should panic:\n{run:?}");
        assert!(run.all().contains(needle), "{backend} should say `{needle}`:\n{run:?}");
    }
}

/// Both backends run to completion and print `expected`.
fn prints_on_both(source: &str, expected: &str) {
    for interp in [true, false] {
        let run = run_program(source, interp);
        let backend = if interp { "interpreter" } else { "native" };
        assert_eq!(run.code, 0, "{backend} should succeed:\n{run:?}");
        assert_eq!(run.stdout, expected, "{backend} printed the wrong thing:\n{run:?}");
    }
}

// ─── The first collection: a fixed-size array ──────────────────────────────
//
// `[1, 2, 3]` is an array, and it is the first collection anyone writes.
// Natively an out-of-range index read or wrote past it (#1724): `v[5]`
// printed 0, `a[4] = 9` overwrote the neighbouring stack slot.

const OOB: &str = "index out of bounds: index is 4 but length is 3";

#[test]
fn reading_an_array_past_its_end_panics() {
    panics_on_both(
        "func main() {\n    let a = [1, 2, 3]\n    let i = 4\n    println(\"{a[i]}\")\n}\n",
        OOB,
    );
}

#[test]
fn reading_an_array_at_a_negative_index_panics() {
    panics_on_both(
        "func main() {\n    let a = [1, 2, 3]\n    let i = -1\n    println(\"{a[i]}\")\n}\n",
        "index is -1 but length is 3",
    );
}

#[test]
fn writing_an_array_past_its_end_panics() {
    panics_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    a[i] = 9\n    println(\"{a[0]}\")\n}\n",
        "index",
    );
    // `+=` reaches the same store.
    panics_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    a[i] += 1\n    println(\"{a[0]}\")\n}\n",
        "index",
    );
}

#[test]
fn lending_an_array_element_past_its_end_panics() {
    panics_on_both(
        "func bump(mutate x: i64) {\n    x += 1\n}\n\nfunc main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    bump(mutate a[i])\n    println(\"{a[0]}\")\n}\n",
        OOB,
    );
    panics_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    with a[i] as x {\n        x = 5\n    }\n    println(\"{a[0]}\")\n}\n",
        "index",
    );
}

#[test]
fn a_field_of_an_array_element_past_its_end_panics() {
    panics_on_both(
        "struct Pt {\n    x: i64\n    y: i64\n    z: i64\n}\n\nfunc main() {\n    let a = [Pt(x: 1, y: 2, z: 3), Pt(x: 4, y: 5, z: 6)]\n    let i = 4\n    println(\"{a[i].x}\")\n}\n",
        "index is 4 but length is 2",
    );
}

#[test]
fn an_array_index_in_range_still_works() {
    prints_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    for i in 0..3 {\n        a[i] += 10\n    }\n    let j = 2\n    println(\"{a[0]} {a[1]} {a[j]}\")\n}\n",
        "11 12 13\n",
    );
}

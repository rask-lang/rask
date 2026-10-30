// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Claims (type.claims) end to end: every verdict a claim can end with, the
//! shrunk counterexample a false one reports, which commands run them, and
//! where a claim may be written.

use std::path::{Path, PathBuf};
use std::process::Command;

fn rask_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("rask");
    path
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("claims").join(name)
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// `rask <args>` in the package directory `dir`.
fn rask(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(rask_binary())
        .args(args)
        .current_dir(dir)
        .env("RASK_RUNTIME_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("runtime"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).replace(&dir.display().to_string(), "<pkg>"),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A copy of a fixture package, so the builds it runs write nowhere in the repo.
fn copy_of(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir()
        .join(format!("rask_claims_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(fixture(name)).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
        }
    }
    dir
}

#[test]
fn every_verdict_and_the_shrunk_counterexamples() {
    let dir = copy_of("ledger");
    let run = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(run.code, 1, "a false claim fails the check:\n{}{}", run.stdout, run.stderr);
    let expected = std::fs::read_to_string(dir.join("expected_stdout.txt")).unwrap();
    assert_eq!(run.stdout, expected, "stderr:\n{}", run.stderr);

    // Shrinking finds the smallest input that still fails, not the first.
    for want in [
        "claim `never_overdrawn` is false",
        "counterexample: ops = [Op.Withdraw(1)]",
        "assertion failed: -1 >= 0",
        "claim `no_bad_index` is false",
        "counterexample: x = 40001",
        "index out of bounds",
        "E0950",
    ] {
        assert!(run.stderr.contains(want), "missing `{want}` in:\n{}", run.stderr);
    }
}

#[test]
fn a_build_fails_on_a_false_claim_and_check_alone_runs_none() {
    let dir = copy_of("ledger");
    let built = rask(&dir, &["build", "."]);
    assert_eq!(built.code, 1, "{}{}", built.stdout, built.stderr);
    assert!(built.stderr.contains("claim `never_overdrawn` is false"), "{}", built.stderr);

    // A cached build runs them too: the object didn't change, the verdict did not either.
    let again = rask(&dir, &["build", "."]);
    assert_eq!(again.code, 1, "{}{}", again.stdout, again.stderr);

    // `rask check` generates no code, so it only type-checks the claims.
    let checked = rask(&dir, &["check", "."]);
    assert_eq!(checked.code, 0, "{}{}", checked.stdout, checked.stderr);
    assert!(!checked.stdout.contains("Claims"), "{}", checked.stdout);
}

#[test]
fn a_claim_outside_a_claims_file_is_refused() {
    let dir = copy_of("misplaced");
    let run = rask(&dir, &["check", "."]);
    assert_ne!(run.code, 0);
    let all = format!("{}{}", run.stdout, run.stderr);
    assert!(all.contains("E0951"), "{all}");
    assert!(all.contains("claim `never_negative` is outside a claims file"), "{all}");
}

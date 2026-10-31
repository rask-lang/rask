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

#[test]
fn an_input_the_build_cant_make_is_reported_at_the_parameter() {
    let dir = copy_of("inputs");
    let run = rask(&dir, &["check", "."]);
    assert_ne!(run.code, 0);
    let all = format!("{}{}", run.stdout, run.stderr);
    // At the parameter in claims.rk, not in the generated code.
    assert!(all.contains("claims.rk:6:"), "{all}");
    assert!(!all.contains("<claims runner>"), "{all}");
    assert!(all.contains("input `v` of claim `opens`"), "{all}");

    // A `mutate` input is refused before anything compiles.
    std::fs::write(dir.join("claims.rk"), "claim drains(mutate q: Vec<i64>) {\n    assert true\n}\n").unwrap();
    let run = rask(&dir, &["check", "."]);
    let all = format!("{}{}", run.stdout, run.stderr);
    assert_ne!(run.code, 0);
    assert!(all.contains("E0952"), "{all}");
}

#[test]
fn strict_mode_fails_a_sampled_claim_and_more_seeds_look_further() {
    let dir = copy_of("strict");
    let plain = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(plain.code, 0, "{}{}", plain.stdout, plain.stderr);
    assert!(plain.stdout.contains("small_pass_through       checked all 256 inputs"), "{}", plain.stdout);

    // Only "checked all" passes under strict.
    let strict = rask(&dir, &["check", "--claims=strict", "."]);
    assert_eq!(strict.code, 1, "{}{}", strict.stdout, strict.stderr);
    assert!(strict.stderr.contains("E0953"), "{}", strict.stderr);
    assert!(strict.stderr.contains("claim `wide_pass_through` held, but wasn't checked in full"), "{}", strict.stderr);
    assert!(!strict.stderr.contains("small_pass_through"), "{}", strict.stderr);

    // The build's own thousand inputs miss the few above 65,500; more seeds
    // find one, and it shrinks to the smallest.
    let seeds = rask(&dir, &["check", "--claim-seeds", "20", "."]);
    assert_eq!(seeds.code, 1, "{}{}", seeds.stdout, seeds.stderr);
    assert!(seeds.stderr.contains("counterexample: x = 65501"), "{}", seeds.stderr);
    assert!(seeds.stderr.contains("found on extra seed"), "{}", seeds.stderr);
}

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

#[test]
fn results_wide_integers_and_maps_keyed_by_numbers_are_built_and_shrunk() {
    let dir = copy_of("kinds");
    let run = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("small_result             checked all 259 inputs"), "{}", run.stdout);
    for want in [
        "counterexample: r = Result.Ok(200)",
        "counterexample: m = Map { -1: \"\", 0: \"\" }",
        "counterexample: big = 1000",
    ] {
        assert!(run.stderr.contains(want), "missing `{want}` in:\n{}", run.stderr);
    }
}

#[test]
fn a_verdict_is_kept_until_something_the_claim_reaches_changes() {
    let dir = copy_of("strict");
    let keys = |dir: &Path| -> Vec<String> {
        std::fs::read_to_string(dir.join("build/.cache/claims"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\t').take(2).collect::<Vec<_>>().join(" "))
            .collect()
    };
    let first = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(first.code, 0, "{}{}", first.stdout, first.stderr);
    let before = keys(&dir);
    assert_eq!(before.len(), 2, "{before:?}");

    // Served from the cache, it reads the same.
    let again = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(again.stdout, first.stdout);

    // A function no claim reaches doesn't touch a verdict.
    let lib = dir.join("lib.rk");
    let mut src = std::fs::read_to_string(&lib).unwrap();
    src.push_str("\npublic func unrelated(x: i64) -> i64 {\n    return x + 1\n}\n");
    std::fs::write(&lib, &src).unwrap();
    assert_eq!(rask(&dir, &["check", "--claims", "."]).code, 0);
    assert_eq!(keys(&dir), before);

    // One both claims call does.
    std::fs::write(&lib, src.replace("return 65500", "return 65499 + 1")).unwrap();
    assert_eq!(rask(&dir, &["check", "--claims", "."]).code, 0);
    let after = keys(&dir);
    assert!(after.iter().all(|k| !before.contains(k)), "{before:?} -> {after:?}");
}

fn rask_env(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Run {
    let mut cmd = Command::new(rask_binary());
    cmd.args(args)
        .current_dir(dir)
        .env("RASK_RUNTIME_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("runtime"))
        .env("NO_COLOR", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).replace(&dir.display().to_string(), "<pkg>"),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_package_is_held_to_claims_from_a_path_and_from_git() {
    let dir = copy_of("shared");
    let outside = dir.parent().unwrap();

    // A directory of claims beside the package, listed under [claims].
    let policy = outside.join("policy");
    std::fs::create_dir_all(&policy).unwrap();
    std::fs::write(
        policy.join("bounds.rk"),
        "import shared\n\nclaim stays_small(x: u8) {\n    assert shared.double(x) <= 510\n}\n",
    )
    .unwrap();

    // A repository of claims, pinned at a commit.
    let repo = outside.join("company-claims");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    std::fs::write(
        repo.join("growth.rk"),
        "import shared\n\nclaim never_shrinks(x: u8) {\n    assert shared.double(x) >= x.to<u16>()!\n}\n",
    )
    .unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "claims"]);
    let rev = git(&repo, &["rev-parse", "HEAD"]);
    let manifest = dir.join("rask.toml");
    let mut toml = std::fs::read_to_string(&manifest).unwrap();
    toml.push_str(&format!("company = {{ git = \"{}\", rev = \"{rev}\" }}\n", repo.display()));
    std::fs::write(&manifest, toml).unwrap();

    let cache = outside.join("claims-cache");
    let env = [("RASK_CLAIMS_CACHE", cache.as_path())];
    let run = rask_env(&dir, &["check", "--claims", "."], &env);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    for want in [
        "doubles_are_even         checked all 256 inputs",
        "stays_small              checked all 256 inputs",
        "never_shrinks            checked all 256 inputs",
    ] {
        assert!(run.stdout.contains(want), "missing `{want}` in:\n{}", run.stdout);
    }

    // The git source is pinned in the lock, and a checkout edited after
    // that fails the build.
    let lock = std::fs::read_to_string(dir.join("rask.lock")).unwrap();
    assert!(lock.contains("[[claims]]\nname = \"company\""), "{lock}");
    let checkout = std::fs::read_dir(&cache).unwrap().flatten().find(|e| e.path().is_dir()).unwrap().path();
    std::fs::write(
        checkout.join("growth.rk"),
        "import shared\n\nclaim never_shrinks(x: u8) {\n    assert true\n}\n",
    )
    .unwrap();
    let edited = rask_env(&dir, &["check", "--claims", "."], &env);
    assert_eq!(edited.code, 1, "{}{}", edited.stdout, edited.stderr);
    assert!(edited.stderr.contains("[claims] company: the checkout at"), "{}", edited.stderr);
}

#[test]
fn a_generic_claim_and_the_stdlib_laws_hold_each_conforming_type() {
    let dir = copy_of("generic");
    let run = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    // Every public type that conforms and can be built: `Size` holds, the
    // hand-written order on `Score` doesn't, and `Vault` can't be built.
    assert!(run.stdout.contains("antisymmetric<generic.Size> checked all 4 inputs"), "{}", run.stdout);
    assert!(run.stderr.contains("claim `antisymmetric<generic.Score>` is false"), "{}", run.stderr);
    assert!(!format!("{}{}", run.stdout, run.stderr).contains("Vault"), "{}{}", run.stdout, run.stderr);
    // The stdlib's laws hold the type that wrote its own conformance, and
    // only that one.
    assert!(run.stderr.contains("claim `compare_is_reflexive<generic.Score>` is false"), "{}", run.stderr);
    assert!(run.stderr.contains("counterexample: a = Score(points: 0)"), "{}", run.stderr);
    assert!(run.stderr.contains("fix its `implements` block"), "{}", run.stderr);
    assert!(!run.stderr.contains("compare_is_reflexive<generic.Size>"), "{}", run.stderr);
}

#[test]
fn rask_claims_lists_verdicts_and_measures_what_they_constrain() {
    let dir = copy_of("strict");
    let json = rask(&dir, &["claims", "--json", "."]);
    assert_eq!(json.code, 0, "{}{}", json.stdout, json.stderr);
    assert!(json.stdout.contains("\"name\":\"small_pass_through\",\"file\":\"claims.rk\",\"verdict\":\"checked_all\""), "{}", json.stdout);
    assert!(json.stdout.contains("\"name\":\"wide_pass_through\",\"file\":\"claims.rk\",\"verdict\":\"sampled\""), "{}", json.stdout);

    // `limit` can return 65501 past the edge and neither claim notices:
    // the sampled one never draws an input that high.
    let mutate = rask(&dir, &["claims", "--mutate", "."]);
    assert_eq!(mutate.code, 1, "{}{}", mutate.stdout, mutate.stderr);
    assert!(mutate.stdout.contains("wide_pass_through        held after"), "{}", mutate.stdout);
    assert!(mutate.stdout.contains("lib.rk:6 in `limit`: `65500` → `65501`"), "{}", mutate.stdout);
}

#[test]
fn since_lists_commits_that_changed_a_claim_and_code_it_reaches() {
    let dir = copy_of("strict");
    git(&dir, &["init", "--quiet"]);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "--quiet", "-m", "start"]);
    let start = git(&dir, &["rev-parse", "HEAD"]);

    // The code and the claim that checks it, in one commit.
    let lib = dir.join("lib.rk");
    let claims_rk = dir.join("claims.rk");
    let edit = |path: &Path, from: &str, to: &str| {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains(from), "{from} not in {}", path.display());
        std::fs::write(path, text.replacen(from, to, 1)).unwrap();
    };
    edit(&lib, "return 65500", "return 65000");
    edit(&claims_rk, "assert strict.limit(x) == x", "assert strict.limit(x) <= x");
    git(&dir, &["commit", "--quiet", "-am", "loosen the limit and its claim together"]);

    // The code alone, reviewed against an unchanged claim.
    edit(&lib, "if x > 65500", "if x > 65000");
    git(&dir, &["commit", "--quiet", "-am", "move the edge"]);

    let run = rask(&dir, &["claims", "--since", &start, "."]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("loosen the limit and its claim together"), "{}", run.stdout);
    assert!(run.stdout.contains("changed claim `wide_pass_through` and `limit`, which it reaches"), "{}", run.stdout);
    assert!(!run.stdout.contains("move the edge"), "{}", run.stdout);
}

#[test]
fn a_type_with_its_own_generator_is_built_through_it_and_shrinks() {
    let dir = copy_of("generated");
    let run = rask(&dir, &["check", "--claims", "."]);
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    // Built through `generate` wherever it sits, so always sorted, though
    // its one field is private.
    assert!(run.stdout.contains("generated_are_sorted     held for 1,000 inputs"), "{}", run.stdout);
    // And shrunk like any other input.
    assert!(run.stderr.contains("counterexample: s = Sorted(items: [0]), x = -1"), "{}", run.stderr);
}

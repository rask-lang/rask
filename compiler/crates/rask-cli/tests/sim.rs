// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `rask test --sim` end to end: the replay line reproduces the failure it
//! printed, a deadlock fails instead of hanging, and time is virtual.

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

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("sim")
}

/// Run `rask test --sim <args>` from the fixture directory; stdout and the exit code.
fn sim(args: &[&str]) -> (String, i32) {
    let out = Command::new(rask_binary())
        .arg("test")
        .arg("--sim")
        .args(args)
        .current_dir(fixtures())
        .env(
            "RASK_RUNTIME_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("runtime"),
        )
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run rask");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    (format!("{stdout}{stderr}"), out.status.code().unwrap_or(-1))
}

fn line_with<'a>(out: &'a str, needle: &str) -> &'a str {
    out.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?} in:\n{out}"))
}

#[test]
fn seed_search_finds_a_lost_update_and_its_replay_line_reproduces_it() {
    let (out, code) = sim(&["--seed", "11", "--seeds", "50", "race.rk"]);
    assert_eq!(code, 1, "the race should lose an update on some seed:\n{out}");
    let assertion = line_with(&out, "assertion failed").trim().to_string();
    let position = line_with(&out, "step ").trim().to_string();

    // The printed command, pasted back, gives the same failure at the same step.
    let replay = line_with(&out, "replay: ").trim().trim_start_matches("replay: ");
    let seed = replay.split_whitespace().skip_while(|w| *w != "--seed").nth(1).unwrap();
    let (again, code) = sim(&["--seed", seed, "-f", "lost update", "race.rk"]);
    assert_eq!(code, 1, "{again}");
    assert_eq!(line_with(&again, "assertion failed").trim(), assertion);
    assert_eq!(line_with(&again, "step ").trim(), position);
}

#[test]
fn a_run_is_a_function_of_its_seed() {
    let (a, _) = sim(&["--seed", "5", "race.rk"]);
    let (b, _) = sim(&["--seed", "5", "race.rk"]);
    assert_eq!(a, b);
}

#[test]
fn a_deadlock_fails_with_who_waits_on_what() {
    let (out, code) = sim(&["--seed", "1", "deadlock.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("deadlock: no task can make progress"), "{out}");
    assert!(out.contains("(main)      waiting on join(task"), "{out}");
    assert!(out.contains("waiting on channel receive"), "{out}");
}

#[test]
fn sleeping_costs_virtual_time_only() {
    let started = std::time::Instant::now();
    let (out, code) = sim(&["--seed", "2", "--seeds", "20", "sleep.rk"]);
    assert_eq!(code, 0, "{out}");
    // Thirty virtual days, twenty times over. The bound is generous; the point
    // is that it isn't thirty days.
    assert!(started.elapsed() < std::time::Duration::from_secs(120));
}

#[test]
fn a_worker_bound_holds_under_sim() {
    let (out, code) = sim(&["--seed", "4", "--seeds", "30", "workers.rk"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn thread_spawn_is_refused() {
    let (out, code) = sim(&["--seed", "1", "thread_spawn.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Thread.spawn is not simulated"), "{out}");
}

#[test]
fn file_changes_stay_in_memory() {
    let before = std::fs::read_to_string(fixtures().join("io.rk")).unwrap();
    let (out, code) = sim(&["--seed", "1", "io.rk"]);
    assert_eq!(code, 0, "{out}");
    // The tests append to io.rk, remove race.rk and write sim_* files. None
    // of it reaches the disk.
    assert_eq!(std::fs::read_to_string(fixtures().join("io.rk")).unwrap(), before);
    assert!(fixtures().join("race.rk").exists());
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(!name.to_string_lossy().starts_with("sim_"), "{name:?} reached the disk");
    }
}

#[test]
fn loopback_sockets_work_and_the_outside_world_is_refused() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "20", "net.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("✓ a server greets a client"), "{out}");
    assert!(out.contains("✓ two clients, arrival order from the seed"), "{out}");
    assert!(out.contains("✓ nobody listening is a refused connection"), "{out}");
    assert!(out.contains("`net.tcp_connect(\"example.com:80\")` — sim's network is loopback only"), "{out}");
}

#[test]
fn an_http_request_survives_short_reads() {
    // The server used to take one read as the whole request, so under sim's
    // short reads it answered for `/ite` instead of `/items/7`.
    let (out, code) = sim(&["--seed", "1", "--seeds", "50", "http.rk"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn faults_land_on_sick_resources_and_the_report_names_them() {
    let (out, code) = sim(&["--seed", "2", "--seeds", "40", "faults.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("✓ a write either lands or says it didn't"), "{out}");
    assert!(out.contains("✓ a cut connection is an error, never a short message"), "{out}");
    assert!(out.contains("✓ SystemTime may leap forward; Instant never does"), "{out}");
    // Code that throws the write's error away is the bug the faults are for.
    assert!(out.contains("FAIL: code that ignores the error is caught"), "{out}");
    assert!(out.contains("sick this seed: file `sim_ignored_"), "{out}");
    assert!(out.contains("faults: write failed on `sim_ignored_"), "{out}");
}

#[test]
fn a_fault_test_is_skipped_outside_sim() {
    let out = Command::new(rask_binary())
        .args(["test", "faults.rk"])
        .current_dir(fixtures())
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("4 skipped"), "{stdout}");
    assert!(stdout.contains("sim-only"), "{stdout}");
}

#[test]
fn a_directory_runs_every_file_in_it() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "8", "-f", "month", "."]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("sleep.rk"), "{out}");
    assert!(out.contains("✓ a month passes instantly"), "{out}");
    assert!(out.contains("1 tests, 1 passed, 0 failed (8 runs)"), "{out}");
}

#[test]
fn seed_search_flags_need_sim() {
    let out = Command::new(rask_binary())
        .args(["test", "--seeds", "5", "race.rk"])
        .current_dir(fixtures())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("add `--sim`"));
}

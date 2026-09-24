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
    sim_in(&fixtures(), args)
}

/// A fresh copy of the fixtures, for tests that write files. If the overlay
/// ever leaked a write to the disk, it lands here and not in the repo.
fn fixture_copy() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "rask_sim_fixtures_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
    }
    dir
}

fn sim_in(dir: &Path, args: &[&str]) -> (String, i32) {
    sim_with(dir, args, &[])
}

fn sim_with(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, i32) {
    let out = Command::new(rask_binary())
        .envs(env.iter().copied())
        .arg("test")
        .arg("--sim")
        .args(args)
        .current_dir(dir)
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
    let dir = fixture_copy();
    let before = std::fs::read_to_string(dir.join("io.rk")).unwrap();
    let (out, code) = sim_in(&dir, &["--seed", "1", "io.rk"]);
    assert_eq!(code, 0, "{out}");
    // The tests append to io.rk, remove race.rk and write sim_* files. None
    // of it reaches the disk.
    assert_eq!(std::fs::read_to_string(dir.join("io.rk")).unwrap(), before);
    assert!(dir.join("race.rk").exists());
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(!name.to_string_lossy().starts_with("sim_"), "{name:?} reached the disk");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_has_one_name_however_it_is_spelled() {
    let dir = fixture_copy();
    let (out, code) = sim_in(&dir, &["--seed", "1", "paths.rk"]);
    assert_eq!(code, 0, "{out}");
    assert!(dir.join("race.rk").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_open_stream_keeps_its_file() {
    let dir = fixture_copy();
    let (out, code) = sim_in(&dir, &["--seed", "1", "streams.rk"]);
    assert_eq!(code, 0, "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_race_through_a_file_is_found() {
    let dir = fixture_copy();
    let (out, code) = sim_in(&dir, &["--seed", "1", "--seeds", "100", "file_race.rk"]);
    assert_eq!(code, 1, "the two read-modify-writes should collide on some seed:\n{out}");
    assert!(out.contains("FAIL: lost update through a file"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_fault_has_no_effect() {
    let dir = fixture_copy();
    let (out, code) = sim_in(&dir, &["--seed", "1", "--seeds", "20", "file_faults.rk"]);
    assert_eq!(code, 0, "{out}");
    let _ = std::fs::remove_dir_all(&dir);
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
    assert!(out.contains("failed on `sim_ignored_"), "{out}");
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
fn output_that_looks_like_a_result_is_output() {
    let (out, code) = sim(&["--seed", "1", "lookalike.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("FAIL: claims to pass, then fails"), "{out}");
    assert!(out.contains("assertion failed: 1 == 2"), "{out}");
    assert!(out.contains("✓ prints a JSON body and passes"), "{out}");
}

#[test]
fn module_constants_are_built_inside_sim() {
    let (a, code) = sim(&["--seed", "3", "constants.rk"]);
    assert_eq!(code, 1, "{a}");
    assert!(a.contains("✓ a constant map finds its keys"), "{a}");
    assert!(a.contains("== -1 (left: "), "{a}");
    let (b, _) = sim(&["--seed", "3", "constants.rk"]);
    assert_eq!(a, b, "a constant read a value the seed doesn't decide");
}

#[test]
fn a_thread_pool_keeps_its_worker_count() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "20", "pool.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("✓ a pool of one runs one job at a time"), "{out}");
    assert!(out.contains("FAIL: a job waiting on the job behind it"), "{out}");
    assert!(out.contains("pool worker        waiting on channel receive"), "{out}");
}

#[test]
fn a_join_outside_a_task_leaves_the_slots_alone() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "100", "join_slot.rk"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn a_spinning_test_fails_at_its_step_budget_and_replays() {
    let (out, code) = sim(&["--seed", "4", "--max-steps", "5000", "spin.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("the test used its 5000 scheduling steps"), "{out}");
    assert!(out.contains("task 0 (main)      running"), "{out}");
    assert!(out.contains("step 5001,"), "{out}");
    let replay = line_with(&out, "replay: ").trim().trim_start_matches("replay: ");
    assert_eq!(replay, "rask test --sim --seed 4 --max-steps 5000 -f 'spins on a flag nobody sets' spin.rk");
}

#[test]
fn the_network_corners_answer_instead_of_hanging() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "30", "net_edges.rk"]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn two_writers_with_full_windows_are_a_deadlock() {
    let (out, code) = sim(&["--seed", "1", "window.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("deadlock: no task can make progress"), "{out}");
    assert!(out.contains("waiting on room in the peer's receive window"), "{out}");
}

#[test]
fn one_bug_is_reported_once_across_a_search() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "200", "--keep-going", "race.rk"]);
    assert_eq!(code, 1, "{out}");
    assert_eq!(out.matches("FAIL: lost update").count(), 1, "{out}");
}

#[test]
fn the_leak_check_works_under_sim() {
    let (out, code) = sim_with(&fixtures(), &["--seed", "1", "leak.rk"], &[("RASK_LEAK_CHECK", "1")]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("passed, but left allocations unreleased"), "{out}");
    let (out, code) = sim_with(&fixtures(), &["--seed", "1", "sleep.rk"], &[("RASK_LEAK_CHECK", "1")]);
    assert_eq!(code, 0, "{out}");
}

#[test]
fn reading_stdin_is_refused() {
    let (out, code) = sim(&["--seed", "1", "stdin.rk"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no simulated implementation for reading stdin"), "{out}");
}

#[test]
fn two_senders_on_an_unbuffered_channel_both_get_through() {
    let (out, code) = sim(&["--seed", "1", "--seeds", "50", "unbuffered.rk"]);
    assert_eq!(code, 0, "{out}");
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

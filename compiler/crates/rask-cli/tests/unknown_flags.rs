// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A flag a command doesn't take is an error, not a no-op (#1934).
//!
//! `rask run --relase f.rk` used to run a debug build and exit 0. Each case
//! runs the built binary against a program that would otherwise succeed, so a
//! zero exit means the flag was ignored.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A program of its own for each test, since they run in parallel.
fn program() -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rask_flags_{}_{}", std::process::id(), id));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.rk");
    std::fs::write(
        &file,
        "// SPDX-License-Identifier: (MIT OR Apache-2.0)\nfunc main() {\n    println(\"hi\")\n}\n",
    )
    .unwrap();
    file
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn rask(args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_rask"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed to run rask");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn a_misspelled_flag_is_refused_with_the_nearest_one() {
    let file = program();
    let file = file.to_str().unwrap();
    for backend in [&["run"][..], &["run", "--interp"][..]] {
        let mut args = backend.to_vec();
        args.extend(["--relase", file]);
        let run = rask(&args);
        assert_eq!(run.code, 1, "rask {}: {}", args.join(" "), run.stderr);
        assert!(run.stderr.contains("`--relase`"), "{}", run.stderr);
        assert!(run.stderr.contains("did you mean `--release`?"), "{}", run.stderr);
        assert!(!run.stdout.contains("hi"), "the program ran: {}", run.stdout);
    }
}

#[test]
fn a_flag_nothing_answers_to_is_refused() {
    let file = program();
    let file = file.to_str().unwrap();
    let run = rask(&["run", "--bogus-flag", file]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(run.stderr.contains("`--bogus-flag`"), "{}", run.stderr);
    assert!(!run.stdout.contains("hi"), "the program ran: {}", run.stdout);
}

#[test]
fn every_command_that_reads_flags_refuses_a_stranger() {
    let file = program();
    let file = file.to_str().unwrap();
    for command in ["check", "build", "compile", "test", "fmt", "lint", "api", "clean", "bench"] {
        let run = rask(&[command, "--deny-warnings", file]);
        assert_eq!(run.code, 1, "rask {command}: {}{}", run.stdout, run.stderr);
        assert!(run.stderr.contains("`--deny-warnings`"), "rask {command}: {}", run.stderr);
    }
}

#[test]
fn a_flag_of_another_command_is_refused() {
    let file = program();
    let run = rask(&["check", "--release", file.to_str().unwrap()]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(run.stderr.contains("`rask check` has no flag `--release`"), "{}", run.stderr);
}

#[test]
fn the_flags_a_command_takes_still_work() {
    let file = program();
    let file = file.to_str().unwrap();
    let run = rask(&["run", "--interp", "--verbose", file]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("hi"), "{}", run.stdout);
    let run = rask(&["check", "--verbose", file]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    // A value that looks like a flag is the value of the flag before it.
    let run = rask(&["test", "-f", "-odd-pattern", file]);
    assert!(!run.stderr.contains("has no flag"), "{}", run.stderr);
}

#[test]
fn arguments_after_the_delimiter_belong_to_the_program() {
    let file = program();
    let run = rask(&["run", "--interp", file.to_str().unwrap(), "--", "--not-a-rask-flag"]);
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

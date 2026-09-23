// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask test --sim` — the runner half of sim mode (specs/sim.md).
//!
//! The binary is built once against the sim runtime and started once per test
//! and seed, with the test's name and seed in its environment. One test per
//! process is what keeps tests from seeing each other (sim/I3, I6): nothing a
//! test leaves behind survives into the next.

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::process;

use colored::Colorize;

use super::run::{
    build_test_binary, death_description, parse_json_i64, parse_json_str, unescape_json_str,
    TestOutcome,
};
use crate::{output, Format};

pub struct SimOptions {
    /// `--seed N`. Drawn from entropy when absent and printed in the header.
    pub seed: Option<u64>,
    /// `--seeds N`: how many seeds each test runs under.
    pub seeds: u64,
    /// `--keep-going`: run every seed even after a test has failed.
    pub keep_going: bool,
}

// ─── Seeds ──────────────────────────────────────────────────

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The seed of the `i`th sweep. A single run uses the run seed itself, so
/// the header seed is the one a replay line passes back.
fn sweep_seed(run_seed: u64, i: u64, sweeps: u64) -> u64 {
    if sweeps == 1 {
        run_seed
    } else {
        splitmix64(run_seed ^ splitmix64(i))
    }
}

/// A test's seed depends on the sweep seed and the test's own name, and on
/// nothing else, so it replays the same whichever other tests ran (sim/I3).
fn test_seed(sweep: u64, full_name: &str) -> u64 {
    splitmix64(sweep ^ fnv1a(full_name))
}

fn entropy_seed() -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(process::id());
    h.finish()
}

// ─── Running ────────────────────────────────────────────────

struct Run {
    passed: bool,
    skipped: Option<String>,
    error: Option<String>,
    step: Option<i64>,
    time_ns: Option<i64>,
    output: Vec<String>,
    stderr: String,
}

fn run_one(bin: &Path, name: &str, seed: u64) -> Run {
    let out = process::Command::new(bin)
        .env("RASK_SIM_TEST", name)
        .env("RASK_SIM_SEED", seed.to_string())
        .output();
    let out = match out {
        Ok(out) => out,
        Err(e) => {
            return Run {
                passed: false,
                skipped: None,
                error: Some(format!("could not start the test binary: {e}")),
                step: None,
                time_ns: None,
                output: vec![],
                stderr: String::new(),
            }
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let mut output = Vec::new();
    let mut record = None;
    for line in stdout.lines() {
        if line.trim_start().starts_with('{') && record.is_none() {
            record = Some(line.trim().to_string());
        } else {
            output.push(line.to_string());
        }
    }

    let Some(rec) = record else {
        return Run {
            passed: false,
            skipped: None,
            error: Some(format!("the test binary {}", death_description(&out.status))),
            step: None,
            time_ns: None,
            output,
            stderr,
        };
    };
    Run {
        passed: rec.contains("\"passed\":true"),
        skipped: parse_json_str(&rec, "skipped").map(unescape_json_str),
        error: parse_json_str(&rec, "error").map(unescape_json_str),
        step: parse_json_i64(&rec, "sim_step"),
        time_ns: parse_json_i64(&rec, "sim_time_ns"),
        output,
        stderr,
    }
}

/// `HH:MM:SS.uuuuuu`. Microseconds, because the clock ticks 1 µs per step
/// (sim/C3) and a short test's whole run fits inside one millisecond.
fn format_virtual_time(ns: i64) -> String {
    let us = ns / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:06}",
        us / 3_600_000_000,
        us / 60_000_000 % 60,
        us / 1_000_000 % 60,
        us % 1_000_000
    )
}

/// Double-quoted for a POSIX shell: the replay line has to paste as-is.
fn shell_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn replay_line(seed: u64, name: &str, path: &str) -> String {
    format!("rask test --sim --seed {seed} -f {} {path}", shell_quote(name))
}

fn print_failure(name: &str, run: &Run, seed: u64, path: &str) {
    println!("{}: {}", "FAIL".red().bold(), name);
    if let Some(err) = &run.error {
        for line in err.lines() {
            println!("  {}", line.red());
        }
    }
    if let (Some(step), Some(t)) = (run.step, run.time_ns) {
        println!("  step {step}, virtual time {}", format_virtual_time(t));
    }
    if !run.output.is_empty() {
        println!("  {}", "output:".dimmed());
        for l in &run.output {
            println!("  {} {}", "│".dimmed(), l);
        }
    }
    let stderr = run.stderr.trim_end();
    if !stderr.is_empty() {
        println!("  {}", "stderr:".dimmed());
        for l in stderr.lines() {
            println!("  {} {}", "│".dimmed(), l);
        }
    }
    println!("  replay: {}", replay_line(seed, name, path));
}

pub fn cmd_test_sim(path: &str, filter: Option<String>, format: Format, opts: SimOptions) {
    if Path::new(path).is_dir() {
        eprintln!(
            "{}: `rask test --sim` takes one file for now — pass a `.rk` file, not the directory {}",
            output::error_label(),
            path,
        );
        process::exit(1);
    }
    if format != Format::Human {
        eprintln!("{}: `rask test --sim` has no JSON output yet", output::error_label());
        process::exit(1);
    }

    let bin = match build_test_binary(path, filter.as_deref(), format, true, true) {
        Ok(bin) => bin,
        Err(TestOutcome::Failed) => process::exit(1),
        Err(_) => return,
    };

    let run_seed = opts.seed.unwrap_or_else(entropy_seed);
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let names: Vec<String> = bin.tests.iter().map(|(name, _)| name.clone()).collect();

    if opts.seeds == 1 {
        println!("sim: seed {run_seed}, {} tests\n", names.len());
    } else {
        println!(
            "sim: seed {run_seed}, {} tests × {} seeds\n",
            names.len(),
            opts.seeds
        );
    }

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut runs = 0u64;

    for name in &names {
        let full_name = format!("{stem}::{name}");
        // Distinct failures by message, each with the first seed that hit it
        // (sim/R3).
        let mut distinct: BTreeMap<String, (u64, Run)> = BTreeMap::new();
        let mut test_skipped = None;

        for i in 0..opts.seeds {
            let sweep = sweep_seed(run_seed, i, opts.seeds);
            let run = run_one(&bin.path, name, test_seed(sweep, &full_name));
            runs += 1;
            if let Some(reason) = run.skipped.clone() {
                test_skipped = Some(reason);
                break;
            }
            if !run.passed {
                let key = run.error.clone().unwrap_or_default();
                distinct.entry(key).or_insert((sweep, run));
                if !opts.keep_going {
                    break;
                }
            }
        }

        if let Some(reason) = test_skipped {
            skipped += 1;
            println!("  {} {} {}", "SKIP".yellow(), name, format!("({reason})").dimmed());
        } else if distinct.is_empty() {
            passed += 1;
            println!("  {} {}", output::status_pass(), name);
        } else {
            failed += 1;
            println!();
            for (sweep, run) in distinct.values() {
                print_failure(name, run, *sweep, path);
                println!();
            }
        }
    }

    println!();
    println!("{}", output::separator(50));
    let mut summary = format!(
        "{} tests, {}, {}",
        passed + failed + skipped,
        output::passed_count(passed),
        output::failed_count(failed),
    );
    if skipped > 0 {
        summary.push_str(&format!(", {skipped} skipped"));
    }
    if opts.seeds > 1 {
        summary.push_str(&format!(" ({runs} runs)"));
    }
    println!("{summary}");

    drop(bin);
    if failed > 0 {
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_run_replays_from_the_header_seed() {
        assert_eq!(sweep_seed(42, 0, 1), 42);
    }

    #[test]
    fn test_seeds_depend_on_the_name_only() {
        let a = test_seed(7, "f::alpha");
        assert_eq!(a, test_seed(7, "f::alpha"));
        assert_ne!(a, test_seed(7, "g::alpha"));
    }

    #[test]
    fn virtual_time_keeps_microseconds() {
        assert_eq!(format_virtual_time(48_000), "00:00:00.000048");
        assert_eq!(format_virtual_time(3_723_400_000_000), "01:02:03.400000");
    }

    #[test]
    fn replay_names_survive_a_shell() {
        assert_eq!(shell_quote(r#"a "b" $c"#), r#""a \"b\" \$c""#);
    }
}

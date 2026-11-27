// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The flags each command takes, and the refusal of any other.
//!
//! A flag nobody reads is not harmless: `rask run --relase f.rk` used to run a
//! debug build and exit 0, so a typo in a CI line quietly became a different
//! build, and a flag that isn't built yet looked like it worked. The manifest
//! already refuses an unknown key (`struct.manifest/MF3`); the command line
//! does the same, naming the nearest flag the command does take.
//!
//! `--format`, `--json`, `--warn` and the claims options are taken before a
//! command is looked at (they work everywhere), so they never reach this check.

use crate::output;
use std::process;

/// A flag, and whether the word after it is its value.
type Flag = (&'static str, bool);

const SWITCH: bool = false;
const VALUE: bool = true;

const NONE: &[Flag] = &[];

const CHECK: &[Flag] = &[("--claims", SWITCH), ("--verbose", SWITCH), ("-v", SWITCH)];
const RUN: &[Flag] = &[
    ("--release", SWITCH),
    ("--verbose", SWITCH),
    ("-v", SWITCH),
    ("--link-lib", VALUE),
    ("--link-obj", VALUE),
    ("--profile", VALUE),
    ("--target", VALUE),
    ("--interp", SWITCH),
    ("--native", SWITCH),
    ("--no-default-features", SWITCH),
];
const COMPILE: &[Flag] = &[
    ("--release", SWITCH),
    ("-o", VALUE),
    ("--target", VALUE),
    ("--link-lib", VALUE),
    ("--link-obj", VALUE),
    ("--dump-mir", SWITCH),
];
// `--verbose`, `--sequential` and `--seed` outside sim mode are refused by
// `rask test` itself, with the reason each one isn't built.
const TEST: &[Flag] = &[
    ("-f", VALUE),
    ("--interp", SWITCH),
    ("--native", SWITCH),
    ("--dump-mir", SWITCH),
    ("--target", VALUE),
    ("--sim", SWITCH),
    ("--seed", VALUE),
    ("--seeds", VALUE),
    ("--keep-going", SWITCH),
    ("--max-steps", VALUE),
    ("--trace", VALUE),
    ("--verbose", SWITCH),
    ("--sequential", SWITCH),
];
const BENCH: &[Flag] = &[
    ("-f", VALUE),
    ("--save", VALUE),
    ("--compare", VALUE),
    ("--baseline-O0", SWITCH),
    ("--interp", SWITCH),
];
const COVERAGE: &[Flag] = &[("--specs", VALUE), ("--suite", VALUE), ("--gaps", SWITCH)];
const BUILD: &[Flag] = &[
    ("--release", SWITCH),
    ("--verbose", SWITCH),
    ("-v", SWITCH),
    ("--profile", VALUE),
    ("--target", VALUE),
    ("--no-cache", SWITCH),
    ("--force", SWITCH),
    ("--no-default-features", SWITCH),
];
const VERBOSE: &[Flag] = &[("--verbose", SWITCH), ("-v", SWITCH)];

/// The flags `command` takes, or `None` for a command that reads its own
/// arguments (`add`, which names its own unknown flags) or isn't one.
fn flags_of(command: &str) -> Option<&'static [Flag]> {
    Some(match command {
        "lex" | "parse" | "resolve" | "ownership" | "comptime" | "unsafe" | "mono" | "mir" => NONE,
        "typecheck" | "check" => CHECK,
        "run" => RUN,
        "compile" => COMPILE,
        "test" => TEST,
        "benchmark" | "bench" => BENCH,
        "stdlib-coverage" => COVERAGE,
        "test-specs" | "update" | "remove" | "targets" | "yank" | "mcp" | "explain" | "keys"
        | "manifest" => NONE,
        "build" => BUILD,
        "clean" => &[("--all", SWITCH)],
        "init" => &[("--agents", SWITCH)],
        "fetch" | "vendor" => VERBOSE,
        "migrate" => &[("--to", VALUE)],
        "publish" => &[
            ("--dry-run", SWITCH),
            ("--unsigned", SWITCH),
            ("--verbose", SWITCH),
            ("-v", SWITCH),
        ],
        "audit" => &[
            ("--verbose", SWITCH),
            ("-v", SWITCH),
            ("--ignore", VALUE),
            ("--db", VALUE),
        ],
        "watch" => &[("--no-clear", SWITCH)],
        "fmt" => &[("--check", SWITCH), ("-w", SWITCH), ("--write", SWITCH)],
        "claims" => &[("--since", VALUE), ("--mutate", SWITCH)],
        "api" => &[("--all", SWITCH)],
        "lint" => &[("--rule", VALUE), ("--exclude", VALUE)],
        "bindc" => &[("-o", VALUE), ("--check", VALUE), ("--hiding", VALUE)],
        "help" => &[("--snippet", SWITCH)],
        _ => return None,
    })
}

/// Refuse the first flag among `args` (the words after the command) that the
/// command doesn't take. `--help` and `-h` are answered before this runs.
fn first_unknown<'a>(command: &str, args: &[&'a str]) -> Option<&'a str> {
    let known = flags_of(command)?;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        i += 1;
        if arg == "--help" || arg == "-h" || !arg.starts_with('-') || arg == "-" {
            continue;
        }
        match known.iter().find(|(name, _)| *name == arg) {
            Some((_, takes_value)) => {
                if *takes_value {
                    i += 1;
                }
            }
            None => return Some(arg),
        }
    }
    None
}

/// Exit with an error if `cmd_args` (`rask <command> ...`, with the global
/// flags already removed) carries a flag the command doesn't take.
pub(crate) fn refuse_unknown_flags(cmd_args: &[&str]) {
    let command = cmd_args[1];
    let Some(bad) = first_unknown(command, &cmd_args[2..]) else {
        return;
    };
    let known: Vec<&str> = flags_of(command)
        .into_iter()
        .flatten()
        .map(|(name, _)| *name)
        .collect();
    let hint = rask_diagnostics::suggestions::did_you_mean(bad, known.iter().copied())
        .map(|h| format!(" ({h})"))
        .unwrap_or_default();
    eprintln!(
        "{}: `rask {}` has no flag `{}`{}; see `rask {} --help`",
        output::error_label(),
        command,
        bad,
        hint,
        command,
    );
    process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_flags_pass_and_their_values_are_skipped() {
        assert_eq!(first_unknown("run", &["--release", "f.rk"]), None);
        assert_eq!(first_unknown("run", &["--profile", "--odd-name", "f.rk"]), None);
        assert_eq!(first_unknown("test", &["f.rk", "-f", "-x"]), None);
    }

    #[test]
    fn a_misspelled_flag_is_found() {
        assert_eq!(first_unknown("run", &["--relase", "f.rk"]), Some("--relase"));
        assert_eq!(first_unknown("build", &["--bogus"]), Some("--bogus"));
    }

    #[test]
    fn a_flag_of_another_command_is_unknown() {
        assert_eq!(first_unknown("check", &["--release", "f.rk"]), Some("--release"));
    }

    #[test]
    fn a_bare_dash_is_an_argument() {
        assert_eq!(first_unknown("fmt", &["-"]), None);
    }
}

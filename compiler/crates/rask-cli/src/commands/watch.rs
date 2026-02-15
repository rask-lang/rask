// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Watch mode — struct.build/WA1-WA8.
//!
//! Monitors .rk files and build.rk, re-runs a command on change.
//! Default command: rask check (WA1).

use colored::Colorize;
use std::collections::HashMap;
use std::path::Path;
use std::process::{self, Command};
use std::time::{Duration, SystemTime};
use std::{fs, thread};

use crate::output;

const DEBOUNCE_MS: u64 = 100; // WA3

/// Watch for file changes and re-run a command.
pub fn cmd_watch(subcommand: Option<&str>, no_clear: bool, _prog_args: &[String]) {
    let sub = subcommand.unwrap_or("check"); // WA1: default to check

    // Validate subcommand
    match sub {
        "check" | "build" | "test" | "run" | "lint" | "--no-clear" => {}
        other if other.starts_with('-') => {
            // Flag, not a subcommand — treat as check
            return cmd_watch(Some("check"), true, _prog_args);
        }
        _ => {
            eprintln!("{}: unknown watch subcommand '{}'. Use: check, build, test, run, lint",
                output::error_label(), sub);
            process::exit(2);
        }
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| "rask".into());

    println!("{} ({}). Press {} to stop.",
        "  Watching".green().bold(),
        format!("rask {}", sub).cyan(),
        "Ctrl+C".yellow()
    );

    // Initial snapshot of file modification times
    let mut snapshots = snapshot_files(".");
    let file_count = snapshots.len();
    println!("  {} {} file{}\n",
        "Tracking".dimmed(),
        file_count,
        if file_count == 1 { "" } else { "s" }
    );

    // Run once immediately
    run_command(&exe, sub, no_clear);

    loop {
        thread::sleep(Duration::from_millis(DEBOUNCE_MS));

        let new_snapshots = snapshot_files(".");
        let changed = find_changes(&snapshots, &new_snapshots);

        if !changed.is_empty() {
            let now = chrono_time();
            let changed_display: Vec<&str> = changed.iter().take(3).map(|s| s.as_str()).collect();
            let suffix = if changed.len() > 3 {
                format!(" +{} more", changed.len() - 3)
            } else {
                String::new()
            };

            println!(
                "\n  {} [{}] {}{}",
                "Change:".yellow(),
                now,
                changed_display.join(", "),
                suffix
            );

            run_command(&exe, sub, no_clear);
            snapshots = new_snapshots;
        }
    }
}

/// Run the rask subcommand.
fn run_command(exe: &Path, sub: &str, no_clear: bool) {
    // WA5: clear terminal
    if !no_clear {
        print!("\x1B[2J\x1B[1;1H");
    }

    let mut args = vec![sub.to_string()];
    // For check/build/lint, pass current directory
    match sub {
        "check" | "build" | "lint" => args.push(".".to_string()),
        _ => args.push(".".to_string()),
    }

    let status = Command::new(exe)
        .args(&args)
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("\n  {} {}", output::status_pass(), "OK".green());
        }
        Ok(s) => {
            // WA6: errors stay on screen
            let code = s.code().unwrap_or(1);
            println!("\n  {} exit code {}", output::status_fail(), code);
        }
        Err(e) => {
            eprintln!("{}: failed to run rask {}: {}", output::error_label(), sub, e);
        }
    }
}

/// Collect modification times for all .rk files and build.rk.
fn snapshot_files(root: &str) -> HashMap<String, SystemTime> {
    let mut map = HashMap::new();
    collect_watched_files(Path::new(root), &mut map);
    map
}

fn collect_watched_files(dir: &Path, map: &mut HashMap<String, SystemTime>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Skip build output and hidden directories
        if path.is_dir() {
            if name.starts_with('.') || name.starts_with('_') || name == "build" || name == "target" || name == "vendor" {
                continue;
            }
            collect_watched_files(&path, map);
            continue;
        }

        // Watch .rk files and build.rk (WA4)
        let is_watched = name.ends_with(".rk") || name == "build.rk";
        if !is_watched {
            continue;
        }

        if let Ok(meta) = fs::metadata(&path) {
            if let Ok(mtime) = meta.modified() {
                if let Some(s) = path.to_str() {
                    map.insert(s.to_string(), mtime);
                }
            }
        }
    }
}

/// Find files that changed between two snapshots.
fn find_changes(
    old: &HashMap<String, SystemTime>,
    new: &HashMap<String, SystemTime>,
) -> Vec<String> {
    let mut changed = Vec::new();

    for (path, new_time) in new {
        match old.get(path) {
            Some(old_time) if old_time != new_time => {
                changed.push(path.clone());
            }
            None => {
                // New file
                changed.push(path.clone());
            }
            _ => {}
        }
    }

    changed.sort();
    changed
}

/// Simple HH:MM:SS timestamp.
fn chrono_time() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (now % 86400) / 3600;
    let minutes = (now % 3600) / 60;
    let seconds = now % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

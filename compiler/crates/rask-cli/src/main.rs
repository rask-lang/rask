// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask CLI - REPL and file runner.

mod commands;
mod help;
mod output;

use colored::Colorize;
use rask_diagnostics::{formatter::DiagnosticFormatter, json, Diagnostic};
use std::env;
use std::fs;
use std::path::Path;
use std::process;

/// Output format for diagnostics.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Format {
    /// Rich terminal output with colors and underlines.
    Human,
    /// Structured JSON for IDEs and AI agents.
    Json,
}

fn show_diagnostic(source: &str, file_name: &str, diagnostic: &Diagnostic) {
    let formatter = DiagnosticFormatter::new(source).with_file_name(file_name);
    eprintln!("{}", formatter.format(diagnostic));
}

/// Show multiple diagnostics. In JSON mode, emit a single structured report.
pub(crate) fn show_diagnostics(
    diagnostics: &[Diagnostic],
    source: &str,
    file: &str,
    phase: &str,
    format: Format,
) {
    match format {
        Format::Human => {
            for d in diagnostics {
                show_diagnostic(source, file, d);
            }
        }
        Format::Json => {
            let report = json::to_json_report(diagnostics, source, file, phase);
            println!("{}", json::to_json_string(&report));
        }
    }
}

fn main() {
    output::init();
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        help::print_usage();
        return;
    }

    // Parse --format json / --json flag
    let format = if args.iter().any(|a| a == "--format=json" || a == "--json") {
        Format::Json
    } else if let Some(pos) = args.iter().position(|a| a == "--format") {
        if args.get(pos + 1).map(|s| s.as_str()) == Some("json") {
            Format::Json
        } else {
            Format::Human
        }
    } else {
        Format::Human
    };

    // Split at -- delimiter
    let delimiter_pos = args.iter().position(|a| a == "--");
    let (cli_args, prog_args) = match delimiter_pos {
        Some(pos) => {
            let cli = &args[..pos];
            let prog = &args[pos + 1..];  // Skip the "--" itself
            (cli, prog)
        }
        None => (&args[..], &[] as &[String])
    };

    // Filter out format flags for command dispatch
    let cmd_args: Vec<&str> = cli_args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            let s = a.as_str();
            if s == "--format=json" || s == "--json" {
                return false;
            }
            if s == "--format" {
                return false;
            }
            if *i > 0 && cli_args[i - 1] == "--format" {
                return false;
            }
            true
        })
        .map(|(_, a)| a.as_str())
        .collect();

    if cmd_args.len() < 2 {
        help::print_usage();
        return;
    }

    match cmd_args[1] {
        "lex" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_lex_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("lex"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::phase::cmd_lex(cmd_args[2], format);
        }
        "parse" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_parse_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("parse"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::phase::cmd_parse(cmd_args[2], format);
        }
        "resolve" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_resolve_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("resolve"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::phase::cmd_resolve(cmd_args[2], format);
        }
        "typecheck" | "check" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_typecheck_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("typecheck"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::analysis::cmd_typecheck(cmd_args[2], format);
        }
        "ownership" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_ownership_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("ownership"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::analysis::cmd_ownership(cmd_args[2], format);
        }
        "comptime" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_comptime_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("comptime"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::analysis::cmd_comptime(cmd_args[2], format);
        }
        "run" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_run_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("run"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let native = cmd_args.contains(&"--native");
            let file_arg = find_positional_arg(&cmd_args, 2, &[]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file argument", output::error_label());
                    process::exit(1);
                }
            };
            if native {
                // Native binary doesn't need the source path as argv[0]
                let native_args: Vec<String> = prog_args.iter().map(|s| s.to_string()).collect();
                commands::run::cmd_run_native(file, native_args, format);
            } else {
                let mut program_args: Vec<String> = vec![file.to_string()];
                program_args.extend(prog_args.iter().map(|s| s.to_string()));
                commands::run::cmd_run(file, program_args, format);
            }
        }
        "compile" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_compile_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("compile"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let output_path = extract_flag_value(&cmd_args, "-o");
            let file_arg = find_positional_arg(&cmd_args, 2, &["-o"]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file argument", output::error_label());
                    process::exit(1);
                }
            };
            commands::codegen::cmd_compile(file, output_path.as_deref(), format, false);
        }
        "test" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_test_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {} {}", "Usage".yellow(), output::command("rask"), output::command("test"), output::arg("<file.rk>"), output::arg("[-f pattern]"));
                process::exit(1);
            }
            let filter = extract_filter(&cmd_args);
            let file_arg = find_positional_arg(&cmd_args, 2, &["-f"]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file argument", output::error_label());
                    process::exit(1);
                }
            };
            commands::run::cmd_test(file, filter, format);
        }
        "benchmark" | "bench" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_benchmark_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {} {}", "Usage".yellow(), output::command("rask"), output::command("benchmark"), output::arg("<file.rk>"), output::arg("[-f pattern]"));
                process::exit(1);
            }
            let filter = extract_filter(&cmd_args);
            let file_arg = find_positional_arg(&cmd_args, 2, &["-f"]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file argument", output::error_label());
                    process::exit(1);
                }
            };
            commands::run::cmd_benchmark(file, filter, format);
        }
        "test-specs" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_test_specs_help();
                return;
            }
            let path = cmd_args.get(2).copied();
            commands::specs::cmd_test_specs(path);
        }
        "mono" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_mono_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("mono"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::codegen::cmd_mono(cmd_args[2], format);
        }
        "mir" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_mir_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("mir"), output::arg("<file.rk>"));
                process::exit(1);
            }
            commands::codegen::cmd_mir(cmd_args[2], format);
        }
        "build" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_build_help();
                return;
            }
            let release = cmd_args.contains(&"--release");
            let verbose = cmd_args.contains(&"--verbose") || cmd_args.contains(&"-v");
            let profile = if release {
                "release".to_string()
            } else if let Some(p) = extract_flag_value(&cmd_args, "--profile") {
                p
            } else {
                "debug".to_string()
            };
            let target = extract_flag_value(&cmd_args, "--target");
            let path = find_positional_arg(&cmd_args, 2, &["--profile", "--target"]).unwrap_or(".");
            let opts = commands::build::BuildOptions { profile, verbose, target };
            commands::build::cmd_build(path, opts);
        }
        "clean" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_clean_help();
                return;
            }
            let all = cmd_args.contains(&"--all");
            let path = find_positional_arg(&cmd_args, 2, &[]).unwrap_or(".");
            commands::build::cmd_clean(path, all);
        }
        "add" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_add_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing package name", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("add"), output::arg("<package> [version]"));
                process::exit(1);
            }
            let pkg_name = cmd_args[2];
            let version = find_positional_arg(&cmd_args, 3, &["--dev", "--feature", "--path"]);
            let dev = cmd_args.contains(&"--dev");
            let feature = extract_flag_value(&cmd_args, "--feature");
            let local_path = extract_flag_value(&cmd_args, "--path");
            commands::deps::cmd_add(pkg_name, version, dev, feature.as_deref(), local_path.as_deref());
        }
        "remove" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_remove_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing package name", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("remove"), output::arg("<package>"));
                process::exit(1);
            }
            commands::deps::cmd_remove(cmd_args[2]);
        }
        "targets" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_targets_help();
                return;
            }
            commands::build::cmd_targets();
        }
        "watch" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_watch_help();
                return;
            }
            let subcommand = cmd_args.get(2).copied();
            let no_clear = cmd_args.contains(&"--no-clear");
            commands::watch::cmd_watch(subcommand, no_clear, prog_args);
        }
        "fmt" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_fmt_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("fmt"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let check_only = cmd_args.iter().any(|a| *a == "--check");
            let file_arg = find_positional_arg(&cmd_args, 2, &[]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file argument", output::error_label());
                    process::exit(1);
                }
            };
            commands::tools::cmd_fmt(file, check_only);
        }
        "describe" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_describe_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("describe"), output::arg("<file.rk>"));
                process::exit(1);
            }
            let show_all = cmd_args.iter().any(|a| *a == "--all");
            let file_arg = find_positional_arg(&cmd_args, 2, &[]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file argument", output::error_label());
                    process::exit(1);
                }
            };
            commands::tools::cmd_describe(file, format, show_all);
        }
        "lint" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_lint_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing file or directory argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("lint"), output::arg("<file.rk | dir>"));
                process::exit(1);
            }
            let rules = extract_repeated_flag(&cmd_args, "--rule");
            let excludes = extract_repeated_flag(&cmd_args, "--exclude");
            let file_arg = find_positional_arg(&cmd_args, 2, &["--rule", "--exclude"]);
            let file = match file_arg {
                Some(f) => f,
                None => {
                    eprintln!("{}: missing file or directory argument", output::error_label());
                    process::exit(1);
                }
            };
            commands::tools::cmd_lint(file, format, rules, excludes);
        }
        "explain" => {
            if cmd_args.contains(&"--help") || cmd_args.contains(&"-h") {
                help::print_explain_help();
                return;
            }
            if cmd_args.len() < 3 {
                eprintln!("{}: missing error code argument", output::error_label());
                eprintln!("{}: {} {} {}", "Usage".yellow(), output::command("rask"), output::command("explain"), output::arg("<ERROR_CODE>"));
                process::exit(1);
            }
            commands::tools::cmd_explain(cmd_args[2]);
        }
        "help" | "--help" | "-h" => {
            if cmd_args.len() > 2 && (cmd_args[2] == "--help" || cmd_args[2] == "-h") {
                help::print_help_help();
                return;
            }
            help::print_usage();
        }
        "version" | "--version" | "-V" => {
            println!("{} {}", output::title("rask"), output::version("0.1.0"));
        }
        other => {
            eprintln!("{}: Unknown command '{}'", output::error_label(), other);
            help::print_usage();
            process::exit(1);
        }
    }
}

fn extract_filter(args: &[&str]) -> Option<String> {
    if let Some(pos) = args.iter().position(|a| *a == "-f") {
        args.get(pos + 1).map(|s| s.to_string())
    } else {
        None
    }
}

fn extract_flag_value(args: &[&str], flag: &str) -> Option<String> {
    if let Some(pos) = args.iter().position(|a| *a == flag) {
        args.get(pos + 1).map(|s| s.to_string())
    } else {
        None
    }
}

/// Find the first positional (non-flag) argument after `skip_from`, skipping
/// flag-value pairs listed in `flags_with_values`.
fn find_positional_arg<'a>(args: &[&'a str], skip_from: usize, flags_with_values: &[&str]) -> Option<&'a str> {
    let mut skip_next = false;
    for arg in args.iter().skip(skip_from) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if flags_with_values.contains(arg) {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
    }
    None
}

/// Collect all .rk files in a directory recursively.
pub(crate) fn collect_rk_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_rk_files(&path));
            } else if path.extension().map(|e| e == "rk").unwrap_or(false) {
                if let Some(s) = path.to_str() {
                    files.push(s.to_string());
                }
            }
        }
    }
    files.sort();
    files
}

/// Extract repeated flag values (e.g., --rule naming/* --rule style/*)
pub(crate) fn extract_repeated_flag(args: &[&str], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if i + 1 < args.len() {
                values.push(args[i + 1].to_string());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    values
}

/// Get the line number for a byte offset.
pub(crate) fn get_line_number(source: &str, pos: usize) -> usize {
    source[..pos.min(source.len())].chars().filter(|&c| c == '\n').count() + 1
}

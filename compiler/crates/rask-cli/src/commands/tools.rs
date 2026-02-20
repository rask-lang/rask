// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Developer tool commands: fmt, describe, lint, explain.

use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process;

use crate::{output, Format, collect_rk_files};

pub fn cmd_fmt(path: &str, check_only: bool) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let formatted = rask_fmt::format_source(&source);

    if formatted == source {
        if check_only {
            println!("{} {}", output::status_pass(), output::file_path(path));
        }
        return;
    }

    if check_only {
        println!("{} {} (would reformat)", output::status_fail(), output::file_path(path));
        process::exit(1);
    }

    match fs::write(path, &formatted) {
        Ok(_) => {
            println!("Formatted {}", output::file_path(path));
        }
        Err(e) => {
            eprintln!("{}: writing {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    }
}

pub fn cmd_api(path: &str, format: Format, show_all: bool) {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let opts = rask_describe::DescribeOpts { show_all };
    let desc = rask_describe::describe(&source, path, opts);

    match format {
        Format::Human => print!("{}", rask_describe::describe_text(&desc)),
        Format::Json => println!("{}", rask_describe::describe_json(&desc)),
    }
}

pub fn cmd_lint(path: &str, format: Format, rules: Vec<String>, excludes: Vec<String>) {
    let p = Path::new(path);
    let files: Vec<String> = if p.is_dir() {
        collect_rk_files(p)
    } else {
        vec![path.to_string()]
    };

    if files.is_empty() {
        eprintln!("{}: no .rk files found in {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    let mut total_errors = 0;
    let mut total_warnings = 0;

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(file), e);
                continue;
            }
        };

        let opts = rask_lint::LintOpts {
            rules: rules.clone(),
            excludes: excludes.clone(),
        };
        let report = rask_lint::lint(&source, file, opts);

        total_errors += report.error_count;
        total_warnings += report.warning_count;

        match format {
            Format::Human => {
                for d in &report.diagnostics {
                    let severity_str = match d.severity {
                        rask_lint::Severity::Error => "error".red().bold().to_string(),
                        rask_lint::Severity::Warning => "warning".yellow().bold().to_string(),
                    };
                    eprintln!(
                        "{}[{}]: {}",
                        severity_str,
                        d.rule.dimmed(),
                        d.message
                    );
                    eprintln!(
                        "  {} {}:{}",
                        "-->".cyan(),
                        output::file_path(file),
                        d.location.line
                    );
                    eprintln!("   |");
                    eprintln!(
                        " {} | {}",
                        d.location.line,
                        d.location.source_line
                    );
                    eprintln!("   |");
                    eprintln!("   = {}: {}", "fix".green().bold(), d.fix);
                    eprintln!();
                }
            }
            Format::Json => {
                println!("{}", rask_lint::lint_json(&report));
            }
        }
    }

    if format == Format::Human {
        if total_errors == 0 && total_warnings == 0 {
            println!("{} No lint issues found", output::status_pass());
        } else {
            eprintln!(
                "{} {} error(s), {} warning(s)",
                output::status_fail(),
                total_errors,
                total_warnings
            );
        }
    }

    if total_errors > 0 {
        process::exit(1);
    }
}

pub fn cmd_explain(code: &str) {
    use rask_diagnostics::codes::ErrorCodeRegistry;

    let registry = ErrorCodeRegistry::default();

    if let Some(info) = registry.get(code) {
        println!(
            "{}[{}]: {}",
            "error".red().bold(),
            info.code.red().bold(),
            info.title.bold()
        );
        println!();
        println!("  Category: {}", info.category);
        println!();
        // Description
        for line in info.description.lines() {
            println!("  {}", line);
        }
        println!();
        // Example
        if !info.example.is_empty() {
            println!("  {}:", "Example".bold());
            println!();
            for line in info.example.lines() {
                println!("    {}", line);
            }
            println!();
        }
        println!("  Run `rask check <file>` to see this error in context.");
    } else {
        eprintln!(
            "{}: unknown error code `{}`",
            output::error_label(),
            code
        );
        eprintln!();
        eprintln!("Error codes use the format E0NNN (e.g., E0308, E0800).");
        eprintln!("Run `rask help` for available commands.");
        process::exit(1);
    }
}

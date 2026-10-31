// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Developer tool commands: fmt, describe, lint, explain.

use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process;

use rask_diagnostics::{Diagnostic, ToDiagnostic};

use crate::{output, show_diagnostics, Format, collect_rk_files};

/// Turn a formatter refusal into the same diagnostics `rask check` would print.
/// There's nothing to format from a source the parser didn't understand, and
/// silence there is what made `fmt --check` pass every file with a syntax error
/// in it (#801).
fn report_format_error(err: &rask_fmt::FormatError, source: &str, file: &str) -> usize {
    let (diags, phase): (Vec<Diagnostic>, &str) = match err {
        rask_fmt::FormatError::Lex(errors) => {
            (errors.iter().map(|e| e.to_diagnostic()).collect(), "lex")
        }
        rask_fmt::FormatError::Parse(errors) => {
            (errors.iter().map(|e| e.to_diagnostic()).collect(), "parse")
        }
    };
    let count = diags.len();
    show_diagnostics(&diags, source, file, phase, Format::Human);
    count
}

pub fn cmd_fmt_stdin() {
    use std::io::Read;
    let mut source = String::new();
    std::io::stdin().read_to_string(&mut source).unwrap_or_else(|e| {
        eprintln!("{}: reading stdin: {}", output::error_label(), e);
        process::exit(1);
    });
    match rask_fmt::try_format_source(&source) {
        Ok(formatted) => print!("{}", formatted),
        Err(err) => {
            let count = report_format_error(&err, &source, "<stdin>");
            eprintln!("\n{}", output::banner_fail("Format", count));
            process::exit(1);
        }
    }
}

/// Format every file named, and every `.rk` file under each directory named.
pub fn cmd_fmt(paths: &[&str], check_only: bool, write_in_place: bool) {
    let mut files: Vec<String> = Vec::new();
    for path in paths {
        let p = Path::new(path);
        if p.is_dir() {
            let found = collect_rk_files(p);
            if found.is_empty() {
                eprintln!("{}: no .rk files found in {}", output::error_label(), output::file_path(path));
                process::exit(1);
            }
            files.extend(found);
        } else {
            files.push(path.to_string());
        }
    }

    let mut had_failure = false;

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(file), e);
                had_failure = true;
                continue;
            }
        };

        // A stdlib stub declares `assert`, `print` and friends, whose names are
        // keywords. The stub loader parses them with that allowance, so the
        // formatter needs it too — without it `rask fmt stdlib/` reported the
        // stdlib's own source as a syntax error, which is the one thing
        // `fmt --check` can never be green with.
        let config = rask_fmt::FormatConfig {
            allow_keyword_fn_names: rask_stdlib::StubRegistry::is_stdlib_path(file),
            ..Default::default()
        };
        let formatted = match rask_fmt::try_format_source_with_config(&source, &config) {
            Ok(f) => f,
            Err(err) => {
                report_format_error(&err, &source, file);
                had_failure = true;
                continue;
            }
        };

        if check_only {
            if formatted == source {
                println!("{} {}", output::status_pass(), output::file_path(file));
            } else {
                println!("{} {} (would reformat)", output::status_fail(), output::file_path(file));
                had_failure = true;
            }
            continue;
        }

        if !write_in_place {
            print!("{}", formatted);
            continue;
        }

        if formatted == source {
            continue;
        }

        match fs::write(file, &formatted) {
            Ok(_) => {
                println!("Formatted {}", output::file_path(file));
            }
            Err(e) => {
                eprintln!("{}: writing {}: {}", output::error_label(), output::file_path(file), e);
                had_failure = true;
            }
        }
    }

    if had_failure {
        process::exit(1);
    }
}

pub fn cmd_api(path: &str, format: Format, show_all: bool) {
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

    let multi = files.len() > 1;
    let mut had_failure = false;

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(file), e);
                had_failure = true;
                continue;
            }
        };

        let opts = rask_describe::DescribeOpts { show_all };
        let desc = rask_describe::describe(&source, file, opts);

        match format {
            Format::Human => {
                if multi {
                    println!("{} {} {}", "===".dimmed(), output::file_path(file), "===".dimmed());
                }
                print!("{}", rask_describe::describe_text(&desc));
            }
            Format::Json => println!("{}", rask_describe::describe_json(&desc)),
        }
    }

    if had_failure {
        process::exit(1);
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

/// `rask bindc header.h [-o out.rk | --check out.rk]` (struct.c-interop/BI1).
///
/// The same translation `import c` compiles, formatted, so a library's binding
/// can be read, reviewed and committed.
pub fn cmd_bindc(header: &str, out: Option<&str>, check: Option<&str>, hiding: &[String]) {
    let path = std::path::Path::new(header);
    if !path.is_file() {
        eprintln!("{}: {} isn't a file", output::error_label(), output::file_path(header));
        process::exit(1);
    }
    let cfg = rask_comptime::CfgConfig::from_host("debug", Vec::new());
    let dirs = std::collections::HashMap::new();
    let cx = rask_compiler::c_import::Context { source_dirs: &dirs, arch: &cfg.arch, os: &cfg.os };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| header.to_string());
    let translation = match rask_compiler::c_import::translate(&name, path, hiding, &cx) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {}: {}", output::error_label(), output::file_path(header), e);
            process::exit(1);
        }
    };
    let text = rask_fmt::format_source(&translation.source);
    let c = &translation.counts;
    let summary = format!(
        "{} functions, {} types, {} constants{}",
        c.functions,
        c.types,
        c.constants,
        if c.skipped > 0 { format!(", {} skipped (see the comments)", c.skipped) } else { String::new() },
    );

    if let Some(existing) = check {
        match fs::read_to_string(existing) {
            Ok(have) if have == text => {
                eprintln!("{} {} matches {}", output::status_pass(), output::file_path(existing), header);
            }
            Ok(_) => {
                eprintln!(
                    "{}: {} isn't what {} translates to now; regenerate it with `rask bindc {} -o {}`",
                    output::error_label(), output::file_path(existing), header, header, existing,
                );
                process::exit(1);
            }
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(existing), e);
                process::exit(1);
            }
        }
        return;
    }
    match out {
        Some(file) => {
            if let Err(e) = fs::write(file, &text) {
                eprintln!("{}: writing {}: {}", output::error_label(), output::file_path(file), e);
                process::exit(1);
            }
            eprintln!("{} wrote {}: {}", output::status_pass(), output::file_path(file), summary);
        }
        None => {
            print!("{}", text);
            eprintln!("{} {}", output::status_pass(), summary);
        }
    }
}

pub fn cmd_explain(code: &str) {
    use rask_diagnostics::codes::ErrorCodeRegistry;

    let registry = ErrorCodeRegistry::default();

    if let Some(info) = registry.get(code) {
        // W-codes are warnings; labelling them "error" contradicts what the
        // compiler prints when it actually raises one.
        let (label, paint): (&str, fn(&str) -> colored::ColoredString) =
            if code.starts_with('W') {
                ("warning", |s: &str| s.yellow().bold())
            } else {
                ("error", |s: &str| s.red().bold())
            };
        println!("{}[{}]: {}", paint(label), paint(info.code), info.title.bold());
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

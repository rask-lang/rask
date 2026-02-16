// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Execution commands: run, test, benchmark.

use colored::Colorize;
use rask_diagnostics::ToDiagnostic;
use std::process;

use crate::{output, show_diagnostics, Format};

pub fn cmd_run(path: &str, program_args: Vec<String>, format: Format) {
    let result = super::pipeline::run_frontend(path, format);

    let mut interp = rask_interp::Interpreter::with_args(program_args);
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }
    match interp.run(&result.decls) {
        Ok(_) => {}
        Err(diag) if matches!(diag.error, rask_interp::RuntimeError::Exit(..)) => {
            if let rask_interp::RuntimeError::Exit(code) = diag.error {
                process::exit(code);
            }
        }
        Err(diag) => {
            let diagnostic = diag.to_diagnostic();
            if let Some(source) = &result.source {
                show_diagnostics(&[diagnostic], source, path, "runtime", format);
            } else {
                eprintln!("{}: {}", output::error_label(), diagnostic.message);
            }
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Runtime", 1));
            }
            process::exit(1);
        }
    }
}

pub fn cmd_test(path: &str, filter: Option<String>, format: Format) {
    let result = super::pipeline::run_frontend(path, format);

    let mut interp = rask_interp::Interpreter::new();
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }
    let results = interp.run_tests(&result.decls, filter.as_deref());

    if results.is_empty() {
        if format == Format::Human {
            println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
            println!("  No tests found.");
        }
        return;
    }

    if format == Format::Human {
        println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

        let mut passed = 0;
        let mut failed = 0;
        let mut total_duration = std::time::Duration::ZERO;

        for r in &results {
            total_duration += r.duration;
            if r.passed {
                passed += 1;
                println!("  {} {} {}",
                    output::status_pass(),
                    r.name,
                    format!("({}ms)", r.duration.as_millis()).dimmed(),
                );
            } else {
                failed += 1;
                println!("  {} {}",
                    output::status_fail(),
                    r.name,
                );
                for err in &r.errors {
                    println!("      {}", err.red());
                }
            }
        }

        println!();
        println!("{}", output::separator(50));
        println!(
            "{} tests, {}, {} ({}ms)",
            results.len(),
            output::passed_count(passed),
            output::failed_count(failed),
            total_duration.as_millis(),
        );

        if failed > 0 {
            process::exit(1);
        }
    }
}

/// Compile a .rk file to a temp executable and run it.
pub fn cmd_run_native(path: &str, program_args: Vec<String>, format: Format) {
    let tmp_dir = std::env::temp_dir();
    let bin_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rask_out");
    let bin_path = tmp_dir.join(format!("rask_{}_{}", bin_name, std::process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();

    // Compile quietly — suppress the "Compiled →" banner (errors still show)
    super::codegen::cmd_compile(path, Some(&bin_str), format, true);

    let status = process::Command::new(&bin_str)
        .args(&program_args)
        .status();

    let _ = std::fs::remove_file(&bin_path);

    match status {
        Ok(s) => {
            if !s.success() {
                process::exit(s.code().unwrap_or(1));
            }
        }
        Err(e) => {
            eprintln!("{}: executing {}: {}", output::error_label(), bin_str, e);
            process::exit(1);
        }
    }
}

pub fn cmd_benchmark(path: &str, filter: Option<String>, format: Format) {
    let result = super::pipeline::run_frontend(path, format);

    let mut interp = rask_interp::Interpreter::new();
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }
    let results = interp.run_benchmarks(&result.decls, filter.as_deref());

    if results.is_empty() {
        if format == Format::Human {
            println!("{} Benchmarking {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
            println!("  No benchmarks found.");
        }
        return;
    }

    if format == Format::Human {
        println!("{} Benchmarking {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

        for r in &results {
            let ops_per_sec = if r.mean.as_nanos() > 0 {
                1_000_000_000 / r.mean.as_nanos()
            } else {
                0
            };
            println!("  {} ({} iterations)",
                r.name,
                r.iterations,
            );
            println!("      min: {:>10.3}us  max: {:>10.3}us",
                r.min.as_nanos() as f64 / 1000.0,
                r.max.as_nanos() as f64 / 1000.0,
            );
            println!("     mean: {:>10.3}us  median: {:>7.3}us  ({} ops/sec)",
                r.mean.as_nanos() as f64 / 1000.0,
                r.median.as_nanos() as f64 / 1000.0,
                ops_per_sec,
            );
            println!();
        }
    }
}

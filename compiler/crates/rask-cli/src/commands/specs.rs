// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Spec testing command.

use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use crate::output;

pub fn cmd_test_specs(path: Option<&str>) {
    use rask_spec_test::{extract_tests, has_rk_tests, run_test_with_config, run_rk_test_file, extract_deps, check_staleness, RunConfig, TestSummary};

    let specs_dir = path.unwrap_or("specs");
    let specs_path = Path::new(specs_dir);

    if !specs_path.exists() {
        eprintln!("{}: specs directory not found: {}", output::error_label(), output::file_path(specs_dir));
        process::exit(1);
    }

    // Find the rask binary for native compilation tests.
    // Use current executable since we ARE the rask binary.
    let rask_binary = std::env::current_exe().ok();
    let config = RunConfig {
        rask_binary: rask_binary.clone(),
    };

    if rask_binary.is_none() {
        eprintln!("{}: could not determine rask binary path, native tests will be skipped", "warn".yellow().bold());
    }

    let mut summary = TestSummary::default();
    let mut all_results = Vec::new();
    let mut all_deps = Vec::new();

    let all_files = collect_test_files(specs_path);
    let md_files: Vec<_> = all_files.iter().filter(|p| p.extension().map_or(false, |e| e == "md")).collect();
    let rk_files: Vec<_> = all_files.iter().filter(|p| p.extension().map_or(false, |e| e == "rk")).collect();
    summary.files = all_files.len();

    // Run markdown spec tests
    for md_path in &md_files {
        let content = match fs::read_to_string(md_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(&md_path.display().to_string()), e);
                continue;
            }
        };

        // Collect dependency headers
        let deps = extract_deps(md_path, &content);
        if !deps.depends.is_empty() || !deps.implemented_by.is_empty() {
            all_deps.push(deps);
        }

        let tests = extract_tests(&md_path, &content);
        if tests.is_empty() {
            continue;
        }

        println!("{}", output::file_path(&md_path.display().to_string()));

        for test in tests {
            let result = run_test_with_config(test, &config);
            print_result(&result, &mut summary, &mut all_results);
        }
        println!();
    }

    // Run .rk test files (files with `test "..." { ... }` blocks)
    let mut rk_results: Vec<rask_spec_test::RkTestResult> = Vec::new();
    for rk_path in &rk_files {
        let content = match fs::read_to_string(rk_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(&rk_path.display().to_string()), e);
                continue;
            }
        };

        if !has_rk_tests(&content) {
            continue;
        }

        println!("{}", output::file_path(&rk_path.display().to_string()));
        let rk_result = run_rk_test_file(rk_path, &content, &config);

        // Display results
        let interp_status = if rk_result.interp_ok() {
            format!("interp: {}/{} {}", rk_result.interp_passed, rk_result.interp_total, "ok".green())
        } else {
            format!("interp: {}/{} {}", rk_result.interp_passed, rk_result.interp_total, "FAIL".red().bold())
        };

        let native_status = match (rk_result.native_passed, rk_result.native_total) {
            (Some(p), Some(t)) if p == t && t > 0 => {
                format!("  native: {}/{} {}", p, t, "ok".green())
            }
            (Some(p), Some(t)) => {
                format!("  native: {}/{} {}", p, t, "FAIL".red().bold())
            }
            _ => String::new(),
        };

        println!("  {} {}{}", if rk_result.interp_ok() { output::status_pass() } else { output::status_fail() }, interp_status, native_status);

        // Show failures
        for f in &rk_result.interp_failures {
            println!("       {} {}", "interp:".red(), f);
        }
        for f in &rk_result.native_failures {
            println!("       {} {}", "native:".red(), f);
        }

        // Update summary
        summary.total += rk_result.interp_total;
        summary.passed += rk_result.interp_passed;
        summary.failed += rk_result.interp_total - rk_result.interp_passed;
        if let (Some(np), Some(nt)) = (rk_result.native_passed, rk_result.native_total) {
            summary.native_total += nt;
            summary.native_passed += np;
            summary.native_failed += nt - np;
        }

        if !rk_result.interp_ok() || !rk_result.native_ok() {
            rk_results.push(rk_result);
        }
        println!();
    }

    // Summary
    println!("{}", output::separator(60));
    println!(
        "{} files, {} tests, {}, {}",
        summary.files,
        summary.total,
        output::passed_count(summary.passed),
        output::failed_count(summary.failed)
    );
    if summary.native_total > 0 {
        let native_label = if summary.native_failed > 0 {
            format!("{}/{} native passed", summary.native_passed, summary.native_total).yellow()
        } else {
            format!("{}/{} native passed", summary.native_passed, summary.native_total).green()
        };
        println!("{}", native_label);
    }

    if !all_results.is_empty() {
        // Separate interp failures from native-only failures
        let interp_fails: Vec<_> = all_results.iter().filter(|r| !r.passed).collect();
        let native_only_fails: Vec<_> = all_results.iter()
            .filter(|r| r.passed && r.native_result.as_ref().map_or(false, |n| !n.passed))
            .collect();

        if !interp_fails.is_empty() {
            println!("\n{}", "Failed tests:".red().bold());
            for result in &interp_fails {
                println!(
                    "  {} {}:{} - {}",
                    output::status_fail(),
                    output::file_path(&result.test.path.display().to_string()),
                    result.test.line,
                    result.message
                );
            }
        }

        if !native_only_fails.is_empty() {
            println!("\n{}", "Native codegen failures (interp passed):".yellow().bold());
            for result in &native_only_fails {
                let nr = result.native_result.as_ref().unwrap();
                println!(
                    "  {} {}:{} - {}",
                    "!".yellow().bold(),
                    output::file_path(&result.test.path.display().to_string()),
                    result.test.line,
                    nr.message,
                );
            }
        }
    }

    // .rk file failures
    if !rk_results.is_empty() {
        let rk_interp_fails: Vec<_> = rk_results.iter().filter(|r| !r.interp_ok()).collect();
        let rk_native_fails: Vec<_> = rk_results.iter().filter(|r| r.interp_ok() && !r.native_ok()).collect();

        if !rk_interp_fails.is_empty() {
            println!("\n{}", "Failed .rk test files (interpreter):".red().bold());
            for r in &rk_interp_fails {
                println!("  {} {} ({}/{})", output::status_fail(), output::file_path(&r.path.display().to_string()), r.interp_passed, r.interp_total);
                for f in &r.interp_failures {
                    println!("       {}", f);
                }
            }
        }

        if !rk_native_fails.is_empty() {
            println!("\n{}", "Native codegen failures in .rk files (interp passed):".yellow().bold());
            for r in &rk_native_fails {
                println!(
                    "  {} {} (native: {}/{})",
                    "!".yellow().bold(),
                    output::file_path(&r.path.display().to_string()),
                    r.native_passed.unwrap_or(0),
                    r.native_total.unwrap_or(0),
                );
                for f in &r.native_failures {
                    println!("       {}", f);
                }
            }
        }
    }

    // Staleness check — find project root (where .git lives)
    if !all_deps.is_empty() {
        let project_root = find_project_root(specs_path);
        if let Some(root) = project_root {
            let warnings = check_staleness(&all_deps, &root);
            if !warnings.is_empty() {
                println!("\n{}", "Staleness warnings:".yellow().bold());
                for w in &warnings {
                    println!(
                        "  {} {} may be stale",
                        "!".yellow().bold(),
                        output::file_path(&w.spec.display().to_string()),
                    );
                    println!(
                        "    {} {} (modified more recently: {})",
                        w.direction,
                        output::file_path(&w.dependency),
                        w.dep_commit.dimmed(),
                    );
                }
            }
        }
    }

    if summary.failed > 0 {
        process::exit(1);
    }
}

/// Print a single test result and update summary/failure tracking.
fn print_result(
    result: &rask_spec_test::TestResult,
    summary: &mut rask_spec_test::TestSummary,
    all_results: &mut Vec<rask_spec_test::TestResult>,
) {
    summary.add(result);

    let status = if result.passed {
        output::status_pass()
    } else {
        output::status_fail()
    };

    let native_suffix = match &result.native_result {
        Some(nr) if nr.passed => format!("  native:{}", "ok".green()),
        Some(_nr) => format!("  native:{}", "FAIL".red().bold()),
        None => String::new(),
    };

    println!(
        "  {} line {}: {:?} - {}{}",
        status,
        result.test.line.to_string().dimmed(),
        result.test.expectation,
        result.message,
        native_suffix,
    );

    if let Some(nr) = &result.native_result {
        if !nr.passed {
            println!("       {} {}", "native:".red(), nr.message);
        }
    }

    if !result.passed || result.native_result.as_ref().map_or(false, |n| !n.passed) {
        // Clone the test result for failure tracking
        all_results.push(rask_spec_test::TestResult {
            test: result.test.clone(),
            passed: result.passed,
            message: result.message.clone(),
            native_result: result.native_result.as_ref().map(|n| rask_spec_test::NativeResult {
                passed: n.passed,
                message: n.message.clone(),
                actual_output: n.actual_output.clone(),
            }),
        });
    }
}

/// Walk up from a path to find the project root (directory containing .git).
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Recursively collect all .md and .rk files in a directory.
fn collect_test_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if dir.is_file() {
        let ext = dir.extension().and_then(|e| e.to_str());
        if matches!(ext, Some("md") | Some("rk")) {
            files.push(dir.to_path_buf());
        }
        return files;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_test_files(&path));
            } else {
                let ext = path.extension().and_then(|e| e.to_str());
                if matches!(ext, Some("md") | Some("rk")) {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    files
}

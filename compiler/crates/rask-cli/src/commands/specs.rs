// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Spec testing command.

use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use crate::output;

pub fn cmd_test_specs(path: Option<&str>) {
    use rask_spec_test::{extract_tests, run_test, extract_deps, check_staleness, TestSummary};

    let specs_dir = path.unwrap_or("specs");
    let specs_path = Path::new(specs_dir);

    if !specs_path.exists() {
        eprintln!("{}: specs directory not found: {}", output::error_label(), output::file_path(specs_dir));
        process::exit(1);
    }

    let mut summary = TestSummary::default();
    let mut all_results = Vec::new();
    let mut all_deps = Vec::new();

    let md_files = collect_md_files(specs_path);
    summary.files = md_files.len();

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
            let result = run_test(test);
            summary.add(&result);

            let status = if result.passed {
                output::status_pass()
            } else {
                output::status_fail()
            };
            println!(
                "  {} line {}: {:?} - {}",
                status,
                result.test.line.to_string().dimmed(),
                result.test.expectation,
                result.message
            );

            if !result.passed {
                all_results.push(result);
            }
        }
        println!();
    }

    println!("{}", output::separator(50));
    println!(
        "{} files, {} tests, {}, {}",
        summary.files,
        summary.total,
        output::passed_count(summary.passed),
        output::failed_count(summary.failed)
    );

    if summary.failed > 0 {
        println!("\n{}", "Failed tests:".red().bold());
        for result in &all_results {
            println!(
                "  {} {}:{} - {}",
                output::status_fail(),
                output::file_path(&result.test.path.display().to_string()),
                result.test.line,
                result.message
            );
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

/// Recursively collect all .md files in a directory.
fn collect_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if dir.is_file() && dir.extension().map(|e| e == "md").unwrap_or(false) {
        files.push(dir.to_path_buf());
        return files;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_md_files(&path));
            } else if path.extension().map(|e| e == "md").unwrap_or(false) {
                files.push(path);
            }
        }
    }

    files.sort();
    files
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Execution commands: run, test, benchmark.

use colored::Colorize;
use rask_diagnostics::ToDiagnostic;
use std::process;

use rask_diagnostics::formatter::DiagnosticFormatter;

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
            } else if let Some((file_path, source)) = find_diagnostic_file(&diagnostic, &result.source_files) {
                let file_name = file_path.to_string_lossy();
                let fmt = DiagnosticFormatter::new(&source).with_file_name(&file_name);
                eprintln!("{}", fmt.format(&diagnostic));
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
pub fn cmd_run_native(path: &str, program_args: Vec<String>, format: Format, link_opts: &super::link::LinkOptions) {
    let tmp_dir = std::env::temp_dir();
    let bin_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rask_out");
    let bin_path = tmp_dir.join(format!("rask_{}_{}", bin_name, std::process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();

    // Compile quietly — suppress the "Compiled →" banner (errors still show)
    super::codegen::cmd_compile(path, Some(&bin_str), format, true, link_opts);

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
    // Try native compilation first
    if try_benchmark_native(path, filter.as_deref(), format) {
        return;
    }

    // Fallback: interpreter
    if format == Format::Human {
        eprintln!("{}: native benchmark failed, falling back to interpreter", "note".yellow());
    }
    cmd_benchmark_interp(path, filter, format);
}

/// Run benchmarks via interpreter (original behavior).
fn cmd_benchmark_interp(path: &str, filter: Option<String>, format: Format) {
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
        println!("{} Benchmarking {} {} (interpreter)\n", "===".dimmed(), output::file_path(path), "===".dimmed());

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

/// Try compiling and running benchmarks natively. Returns true on success.
fn try_benchmark_native(path: &str, filter: Option<&str>, format: Format) -> bool {
    let rask_results = run_benchmark_file(path, filter, format);
    if rask_results.is_empty() {
        // run_benchmark_file returns empty on compile failure or no benchmarks
        // Check if the file has benchmarks at all (for the "no benchmarks found" message)
        let result = super::pipeline::run_frontend(path, format);
        let has_benchmarks = result.decls.iter().any(|d|
            matches!(d.kind, rask_ast::decl::DeclKind::Benchmark(_))
        );
        if !has_benchmarks {
            if format == Format::Human {
                println!("{} Benchmarking {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
                println!("  No benchmarks found.");
            }
            return true;
        }
        return false;
    }

    // Check for matching C baseline
    let c_path = std::path::Path::new(path).with_extension("c");
    let c_results = if c_path.exists() {
        run_c_baseline(&c_path, "-O2", format)
    } else {
        Vec::new()
    };

    if format == Format::Human {
        println!("{} Benchmarking {} {} (native)\n", "===".dimmed(), output::file_path(path), "===".dimmed());

        for result in &rask_results {
            let ops_per_sec = if result.mean_ns > 0 {
                1_000_000_000 / result.mean_ns
            } else {
                0
            };
            println!("  {} ({} iterations)",
                result.name, result.iterations);
            println!("      min: {:>10.3}us  max: {:>10.3}us",
                result.min_ns as f64 / 1000.0,
                result.max_ns as f64 / 1000.0);
            println!("     mean: {:>10.3}us  median: {:>7.3}us  ({} ops/sec)",
                result.mean_ns as f64 / 1000.0,
                result.median_ns as f64 / 1000.0,
                ops_per_sec);

            if let Some(c) = c_results.iter().find(|c| c.name == result.name) {
                let ratio = result.median_ns as f64 / c.median_ns as f64;
                let ratio_str = if ratio <= 1.10 {
                    format!("{:.2}x", ratio).green().to_string()
                } else if ratio <= 1.50 {
                    format!("{:.2}x", ratio).yellow().to_string()
                } else {
                    format!("{:.2}x", ratio).red().to_string()
                };
                println!("    C -O2: {:>10.3}us  ratio: {}",
                    c.median_ns as f64 / 1000.0, ratio_str);
            }
            println!();
        }
    } else {
        // JSON mode
        print!("[");
        for (i, result) in rask_results.iter().enumerate() {
            if i > 0 { print!(","); }
            let c_ns = c_results.iter().find(|c| c.name == result.name)
                .map_or(-1, |c| c.median_ns);
            print!("{{\"name\":\"{}\",\"iterations\":{},\"min_ns\":{},\"max_ns\":{},\"mean_ns\":{},\"median_ns\":{},\"c_median_ns\":{}}}",
                result.name, result.iterations,
                result.min_ns, result.max_ns, result.mean_ns, result.median_ns,
                c_ns);
        }
        println!("]");
    }
    true
}

struct BenchResult {
    name: String,
    iterations: i64,
    min_ns: i64,
    max_ns: i64,
    mean_ns: i64,
    median_ns: i64,
}

/// Minimal JSON parser for bench.c output lines.
fn parse_bench_json(line: &str) -> Option<BenchResult> {
    let line = line.trim();
    if !line.starts_with('{') { return None; }

    Some(BenchResult {
        name: parse_bench_json_str(line, "name")?.to_string(),
        iterations: parse_bench_json_i64(line, "iterations")?,
        min_ns: parse_bench_json_i64(line, "min_ns")?,
        max_ns: parse_bench_json_i64(line, "max_ns")?,
        mean_ns: parse_bench_json_i64(line, "mean_ns")?,
        median_ns: parse_bench_json_i64(line, "median_ns")?,
    })
}

pub struct BenchSuiteOpts {
    pub save_path: Option<String>,
    pub compare_path: Option<String>,
    /// Compile C baselines with -O0 instead of -O2 for fair Cranelift comparison.
    pub baseline_o0: bool,
}

/// Run all benchmarks in a directory, with optional C baseline comparison.
///
/// Discovers .rk files, compiles and runs each natively, then compiles
/// matching .c files (if any) and runs them for comparison.
pub fn cmd_benchmark_dir(
    dir: &str,
    filter: Option<String>,
    format: Format,
    opts: BenchSuiteOpts,
) {
    let dir_path = std::path::Path::new(dir);
    if !dir_path.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), dir);
        process::exit(1);
    }

    let c_opt_level = if opts.baseline_o0 { "-O0" } else { "-O2" };

    // Load baseline for comparison (if requested)
    let baseline = opts.compare_path.as_ref().and_then(|p| {
        match std::fs::read_to_string(p) {
            Ok(content) => Some(parse_baseline_json(&content)),
            Err(e) => {
                eprintln!("{}: reading baseline {}: {}", output::error_label(), p, e);
                None
            }
        }
    });

    // Discover .rk benchmark files
    let mut rk_files: Vec<_> = std::fs::read_dir(dir_path)
        .unwrap_or_else(|e| {
            eprintln!("{}: reading {}: {}", output::error_label(), dir, e);
            process::exit(1);
        })
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "rk"))
        .collect();
    rk_files.sort();

    if rk_files.is_empty() {
        if format == Format::Human {
            println!("{} Benchmarking {} {}\n", "===".dimmed(), output::file_path(dir), "===".dimmed());
            println!("  No .rk benchmark files found.");
        }
        return;
    }

    if format == Format::Human {
        println!("{} Benchmark suite: {} {}\n", "===".dimmed(), output::file_path(dir), "===".dimmed());
    }

    struct SuiteEntry {
        name: String,
        rask_median_ns: Option<i64>,
        c_median_ns: Option<i64>,
    }

    let mut entries: Vec<SuiteEntry> = Vec::new();

    // Run each .rk file
    for rk_path in &rk_files {
        let path_str = rk_path.to_string_lossy();
        if format == Format::Human {
            println!("  {} {}", "▸".dimmed(), output::file_path(&path_str));
        }

        let rask_results = run_benchmark_file(&path_str, filter.as_deref(), format);
        let c_path = rk_path.with_extension("c");
        // Only run C baseline if the .rk file produced results (respects filter)
        let c_results = if c_path.exists() && !rask_results.is_empty() {
            run_c_baseline(&c_path, c_opt_level, format)
        } else {
            Vec::new()
        };

        for rr in &rask_results {
            let c_match = c_results.iter().find(|c| c.name == rr.name);
            entries.push(SuiteEntry {
                name: rr.name.clone(),
                rask_median_ns: Some(rr.median_ns),
                c_median_ns: c_match.map(|c| c.median_ns),
            });
        }

        // C-only baselines (no matching Rask benchmark)
        for cr in &c_results {
            if !rask_results.iter().any(|r| r.name == cr.name) {
                entries.push(SuiteEntry {
                    name: cr.name.clone(),
                    rask_median_ns: None,
                    c_median_ns: Some(cr.median_ns),
                });
            }
        }
    }

    if entries.is_empty() {
        if format == Format::Human {
            println!("  No benchmark results collected.");
        }
        return;
    }

    // Save baseline if requested
    if let Some(ref path) = opts.save_path {
        let mut json = String::from("[\n");
        for (i, entry) in entries.iter().enumerate() {
            if i > 0 { json.push_str(",\n"); }
            json.push_str(&format!(
                "  {{\"name\":\"{}\",\"rask_median_ns\":{},\"c_median_ns\":{}}}",
                entry.name,
                entry.rask_median_ns.unwrap_or(-1),
                entry.c_median_ns.unwrap_or(-1),
            ));
        }
        json.push_str("\n]\n");
        if let Err(e) = std::fs::write(path, &json) {
            eprintln!("{}: writing baseline {}: {}", output::error_label(), path, e);
        } else if format == Format::Human {
            println!("\n  Saved baseline to {}", path);
        }
    }

    // Summary table
    let has_baseline = baseline.is_some();
    let c_header = format!("C {} (us)", c_opt_level);
    if format == Format::Human {
        println!();
        if has_baseline {
            println!("{}", output::separator(88));
            println!("  {:<30} {:>10} {:>12} {:>8} {:>12}",
                "Benchmark", "Rask (us)", c_header, "Ratio", "vs baseline");
            println!("{}", output::separator(88));
        } else {
            println!("{}", output::separator(72));
            println!("  {:<30} {:>10} {:>12} {:>8}",
                "Benchmark", "Rask (us)", c_header, "Ratio");
            println!("{}", output::separator(72));
        }

        for entry in &entries {
            let rask_us = entry.rask_median_ns.map(|ns| ns as f64 / 1000.0);
            let c_us = entry.c_median_ns.map(|ns| ns as f64 / 1000.0);

            let rask_str = rask_us.map_or("—".to_string(), |v| format!("{:.1}", v));
            let c_str = c_us.map_or("—".to_string(), |v| format!("{:.1}", v));

            let ratio_str = match (rask_us, c_us) {
                (Some(r), Some(c)) if c > 0.0 => {
                    let ratio = r / c;
                    if ratio <= 1.10 {
                        format!("{:.2}x", ratio).green().to_string()
                    } else if ratio <= 1.50 {
                        format!("{:.2}x", ratio).yellow().to_string()
                    } else {
                        format!("{:.2}x", ratio).red().to_string()
                    }
                }
                _ => "—".to_string(),
            };

            if has_baseline {
                let delta_str = if let (Some(ref bl), Some(cur_ns)) = (&baseline, entry.rask_median_ns) {
                    bl.iter().find(|b| b.0 == entry.name).and_then(|b| {
                        if b.1 <= 0 { return None; }
                        let pct = ((cur_ns as f64 / b.1 as f64) - 1.0) * 100.0;
                        if pct.abs() < 1.0 {
                            Some("~".dimmed().to_string())
                        } else if pct < 0.0 {
                            Some(format!("{:+.1}%", pct).green().to_string())
                        } else {
                            Some(format!("+{:.1}%", pct).red().to_string())
                        }
                    }).unwrap_or_else(|| "new".dimmed().to_string())
                } else {
                    "—".to_string()
                };
                println!("  {:<30} {:>10} {:>12} {:>8} {:>12}",
                    entry.name, rask_str, c_str, ratio_str, delta_str);
            } else {
                println!("  {:<30} {:>10} {:>12} {:>8}",
                    entry.name, rask_str, c_str, ratio_str);
            }
        }
        println!();
    } else {
        // JSON mode: output array of results
        print!("[");
        for (i, entry) in entries.iter().enumerate() {
            if i > 0 { print!(","); }
            print!("{{\"name\":\"{}\",\"rask_median_ns\":{},\"c_median_ns\":{}}}",
                entry.name,
                entry.rask_median_ns.unwrap_or(-1),
                entry.c_median_ns.unwrap_or(-1));
        }
        println!("]");
    }
}

/// Parse a baseline JSON file: returns vec of (name, rask_median_ns).
fn parse_baseline_json(content: &str) -> Vec<(String, i64)> {
    let mut results = Vec::new();
    // Minimal parser: extract {"name":"...","rask_median_ns":N,...} entries
    for line in content.lines() {
        let line = line.trim().trim_matches(|c| c == '[' || c == ']' || c == ',');
        if !line.starts_with('{') { continue; }
        if let (Some(name), Some(ns)) = (
            parse_bench_json_str(line, "name"),
            parse_bench_json_i64(line, "rask_median_ns"),
        ) {
            results.push((name.to_string(), ns));
        }
    }
    results
}

fn parse_bench_json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{}\":\"", key);
    let start = s.find(&pat)? + pat.len();
    let end = s[start..].find('"')? + start;
    Some(&s[start..end])
}

fn parse_bench_json_i64(s: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{}\":", key);
    let start = s.find(&pat)? + pat.len();
    let rest = &s[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Run a single .rk benchmark file natively, return parsed results.
fn run_benchmark_file(path: &str, filter: Option<&str>, format: Format) -> Vec<BenchResult> {
    let mut result = match std::panic::catch_unwind(|| {
        super::pipeline::run_frontend(path, format)
    }) {
        Ok(r) => r,
        Err(_) => {
            if format == Format::Human {
                eprintln!("    {}: frontend panic for {}", output::error_label(), path);
            }
            return Vec::new();
        }
    };

    rask_hidden_params::desugar_hidden_params(&mut result.decls);
    let benchmarks = super::compile::extract_benchmarks(&mut result.decls, filter);
    if benchmarks.is_empty() {
        return Vec::new();
    }

    let mono = match rask_mono::monomorphize(&result.typed, &result.decls) {
        Ok(m) => m,
        Err(e) => {
            if format == Format::Human {
                eprintln!("    {}: mono: {:?}", output::error_label(), e);
            }
            return Vec::new();
        }
    };
    let comptime_globals = super::codegen::evaluate_comptime_globals(&result.decls);

    let tmp_dir = std::env::temp_dir();
    let bin_path = tmp_dir.join(format!("rask_bench_{}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();
    let obj_path = format!("{}.o", bin_str);

    if let Err(errors) = super::compile::compile_benchmarks_to_object(
        &mono, &result.typed, &result.decls, &comptime_globals,
        &benchmarks, Some(path), result.source.as_deref(), &obj_path,
    ) {
        if format == Format::Human {
            for e in &errors {
                eprintln!("    {}: compile: {}", output::error_label(), e);
            }
        }
        let _ = std::fs::remove_file(&obj_path);
        return Vec::new();
    }

    let link_opts = super::link::LinkOptions::default();
    if let Err(e) = super::link::link_executable_with(&obj_path, &bin_str, &link_opts) {
        if format == Format::Human {
            eprintln!("    {}: link: {}", output::error_label(), e);
        }
        return Vec::new();
    }

    let output = process::Command::new(&bin_str).output();
    let _ = std::fs::remove_file(&bin_path);

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines().filter_map(|l| parse_bench_json(l)).collect()
        }
        _ => Vec::new(),
    }
}

/// Compile and run a C baseline file, return parsed results.
fn run_c_baseline(c_path: &std::path::Path, opt_level: &str, format: Format) -> Vec<BenchResult> {
    let runtime_dir = match super::link::find_runtime_dir() {
        Ok(d) => d,
        Err(e) => {
            if format == Format::Human {
                eprintln!("    {}: C baseline: {}", output::error_label(), e);
            }
            return Vec::new();
        }
    };

    let tmp_dir = std::env::temp_dir();
    let bin_path = tmp_dir.join(format!("rask_cbase_{}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();

    // Compile with cc, linking needed runtime sources (not runtime.c — it has its own main)
    let runtime_sources = ["bench.c", "vec.c", "map.c", "pool.c", "string.c",
                           "alloc.c", "panic.c", "args.c", "ptr.c"];
    let mut cmd = process::Command::new("cc");
    cmd.arg(opt_level);
    cmd.arg(c_path);
    for src in &runtime_sources {
        let src_path = runtime_dir.join(src);
        if src_path.exists() {
            cmd.arg(&src_path);
        }
    }
    cmd.arg(format!("-I{}", runtime_dir.display()));
    cmd.args(["-o", &bin_str, "-no-pie", "-lpthread", "-lm"]);

    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) => {
            if format == Format::Human {
                eprintln!("    {}: compiling C baseline: {}", output::error_label(), e);
            }
            return Vec::new();
        }
    };

    if !status.success() {
        if format == Format::Human {
            eprintln!("    {}: C baseline compilation failed", output::error_label());
        }
        return Vec::new();
    }

    let output = process::Command::new(&bin_str).output();
    let _ = std::fs::remove_file(&bin_path);

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.lines().filter_map(|l| parse_bench_json(l)).collect()
        }
        _ => Vec::new(),
    }
}

/// Match a diagnostic to a source file by span validity.
fn find_diagnostic_file<'a>(
    d: &rask_diagnostics::Diagnostic,
    source_files: &'a [(std::path::PathBuf, String)],
) -> Option<(&'a std::path::PathBuf, &'a String)> {
    let end = d.labels.iter()
        .find(|l| l.style == rask_diagnostics::LabelStyle::Primary)
        .map(|l| l.span.end)?;
    let candidates: Vec<_> = source_files.iter()
        .filter(|(_, src)| end <= src.len() && !src.is_empty())
        .collect();
    if candidates.len() == 1 {
        let (p, s) = candidates[0];
        Some((p, s))
    } else {
        None
    }
}

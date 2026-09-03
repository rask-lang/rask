// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Execution commands: run, test, benchmark.

use colored::Colorize;
use rask_diagnostics::ToDiagnostic;
use std::process;

use rask_diagnostics::formatter::DiagnosticFormatter;

use crate::{output, show_diagnostics, Format};

/// Options for test execution (T7, T8 from std.testing spec).
pub struct TestOptions {
    pub verbose: bool,
    pub sequential: bool,
    pub seed: Option<String>,
}

/// What to tell the user about a fatal signal, beyond its name.
fn signal_advice(sig: i32) -> Option<&'static str> {
    match sig {
        // SIGSEGV
        11 => Some("run it again with RASK_RUNTIME_CHECKS=1 to turn a null \
                   dereference into a panic that says where"),
        // SIGILL — how a Cranelift `unreachable` trap surfaces
        4 => Some("this is a Cranelift trap: an `unreachable` was reached, \
                   usually a match on an out-of-range tag"),
        // SIGFPE
        8 => Some("integer division by zero or an overflowing division"),
        // SIGBUS
        7 => Some("a misaligned or out-of-bounds memory access"),
        _ => None,
    }
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    }
}

/// The exit code to pass on for a finished child, reporting a fatal signal on
/// the way out.
///
/// A signal-killed process carries no exit code, so `code().unwrap_or(1)` exited
/// 1 having printed nothing — a segfault read exactly like a silent compile
/// failure (#605). Mechanical safety promises unsafety surfaces as a named
/// failure; this is the surface where that promise is kept.
fn exit_code_reporting_signals(status: &process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            eprintln!(
                "{}: program crashed with {} (signal {})",
                output::error_label(),
                signal_name(sig),
                sig
            );
            if let Some(advice) = signal_advice(sig) {
                eprintln!("  {} {}", "help:".cyan(), advice);
            }
            return 128 + sig;
        }
    }
    status.code().unwrap_or(1)
}

/// The declarations the interpreter runs: the program's, plus the stdlib modules
/// that are written in Rask.
///
/// Native compiles `stdlib/*.rk` including its bodies (`compilable_decls`); the
/// interpreter used to ignore that source entirely and run hand-written Rust
/// from `rask-interp/src/stdlib/` instead. So a module written in Rask still had
/// two implementations, one per backend, and they disagreed — `Path.parent()`
/// answered `none` natively (#688) while the interpreter got it right, and the
/// rest of the Path family segfaulted. Handing the same source to both backends
/// is what makes "written in Rask" mean one implementation.
/// The stdlib goes first and the program second, because registration is
/// last-writer-wins and the program has to be the last writer. A program may
/// reuse a stdlib type's name (rask#258) — `struct JsonError` over stdlib's
/// `enum JsonError` — and with the program first, the stdlib's `message` body
/// overwrote the user's and ran `match self` against a struct.
fn decls_with_rask_stdlib(decls: &[rask_ast::decl::Decl]) -> Vec<rask_ast::decl::Decl> {
    let t = std::time::Instant::now();
    let stdlib = rask_stdlib::StubRegistry::compilable_decls();
    let n = stdlib.len();
    let mut all = stdlib;
    all.extend(decls.to_vec());
    if std::env::var_os("RASK_TIME_STDLIB").is_some() {
        eprintln!("[stdlib] {} decls in {:?}", n, t.elapsed());
    }
    all
}

pub fn cmd_run(path: &str, program_args: Vec<String>, format: Format) {
    let result = crate::run_check_or_exit(path, format);

    // `program_args` already starts with the script path — main.rs puts it
    // there — so this is argv as std.os/A1 describes it.
    let mut interp = rask_interp::Interpreter::with_args(program_args);
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    interp.inject_cfg(&cfg);
    interp.set_node_types(result.typed.node_types.clone());
    interp.set_error_wraps(result.typed.error_wraps.clone());
    interp.set_try_chain_placement(result.typed.try_chain_placement.clone());
    interp.set_fallback_keeps_shape(result.typed.fallback_keeps_shape.clone());
    // Set source info from the first source file (single-file mode).
    if let Some((_, source)) = result.source_files.first() {
        interp.set_source_info(path, source);
    }
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }
    let all = decls_with_rask_stdlib(&result.decls);
    match interp.run(&all) {
        Ok(_) => {}
        Err(diag) if matches!(diag.error, rask_interp::RuntimeError::Exit(..)) => {
            if let rask_interp::RuntimeError::Exit(code) = diag.error {
                process::exit(code);
            }
        }
        Err(diag) => {
            // A panic is a bug, not an error return: exit 101, distinct from an
            // error propagated out of main (exit 1). (struct.targets/EX4, ctrl.panic/P4)
            let exit_code = if diag.error.is_panic() { 101 } else { 1 };
            let diagnostic = diag.to_diagnostic();
            if let Some((file_path, source)) = find_diagnostic_file(&diagnostic, &result.source_files) {
                let file_name = file_path.to_string_lossy();
                let fmt = DiagnosticFormatter::new(&source).with_file_name(&file_name);
                eprintln!("{}", fmt.format(&diagnostic));
            } else if let Some((_, source)) = result.source_files.first() {
                show_diagnostics(&[diagnostic], source, path, "runtime", format);
            } else {
                eprintln!("{}: {}", output::error_label(), diagnostic.message);
            }
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Runtime", 1));
            }
            process::exit(exit_code);
        }
    }
}

/// Build a project directory and run the resulting binary.
pub fn cmd_run_project(path: &str, program_args: Vec<String>, opts: super::build::BuildOptions) {
    let profile = opts.profile.clone();
    let target = opts.target.clone();
    let bin_path = super::build::project_binary_path(path, &profile, target.as_deref());

    // Build (exits on failure)
    super::build::cmd_build(path, opts);

    // Execute
    let status = process::Command::new(&bin_path)
        .args(&program_args)
        .status();

    match status {
        Ok(s) => {
            if !s.success() {
                process::exit(exit_code_reporting_signals(&s));
            }
        }
        Err(e) => {
            eprintln!("{}: executing {}: {}", output::error_label(), bin_path.display(), e);
            process::exit(1);
        }
    }
}

/// Build a project directory and run its tests natively.
/// Uses the full build pipeline (package resolution, build script, deps)
/// but compiles with a test runner entry point instead of main().
pub fn cmd_test_project(path: &str, filter: Option<String>, format: Format) {
    use colored::Colorize;

    let opts = super::build::BuildOptions {
        profile: "debug".to_string(),
        verbose: false,
        target: None,
        no_cache: false,
        force: false,
        jobs: None,
    };

    let prepared = super::build::prepare_build(path, opts);

    if prepared.dep_errors > 0 {
        eprintln!("{}", output::banner_fail("Build", prepared.dep_errors));
        process::exit(1);
    }

    let root_pkg = match prepared.registry.get(prepared.root_id) {
        Some(p) => p,
        None => {
            eprintln!("{}: root package not found", output::error_label());
            process::exit(1);
        }
    };

    let source_files: Vec<_> = root_pkg.files.iter()
        .map(|f| (f.path.clone(), f.source.clone()))
        .collect();
    let root_decls: Vec<_> = root_pkg.all_decls().cloned().collect();

    // Dependency declarations, the way `rask build` collects them.
    let mut dep_decls = Vec::new();
    for pkg in prepared.registry.packages() {
        if pkg.id == prepared.root_id { continue; }
        for decl in pkg.all_decls() {
            match &decl.kind {
                rask_ast::decl::DeclKind::Fn(_)
                | rask_ast::decl::DeclKind::Struct(_)
                | rask_ast::decl::DeclKind::Enum(_)
                | rask_ast::decl::DeclKind::Impl(_)
                | rask_ast::decl::DeclKind::Const(_) => dep_decls.push(decl.clone()),
                _ => {}
            }
        }
    }

    // One frontend, shared with `rask build`. This used to be a hand-rolled
    // copy of resolve → typecheck → ownership → hidden-params → derive → mono,
    // which is how it ended up calling the plain typecheck: the stdlib's own
    // bodies were resolved but never typed, and `rask test examples/validation`
    // failed on 17 stdlib functions `rask build` had simply discarded (#330,
    // #697). Now the test runner is the one thing it contributes — a decl
    // rewrite handed to the pipeline.
    let cfg = rask_comptime::CfgConfig::from_host("debug", prepared.resolved_feature_names);
    let config = rask_compiler::CompilerConfig { cfg: cfg.clone() };
    let mut pkg_ctx = rask_compiler::PackageContext {
        registry: prepared.registry,
        root_id: prepared.root_id,
        all_decls: root_decls,
    };
    let mut tests = Vec::new();
    let output = rask_compiler::compile_package_with(
        &mut pkg_ctx,
        dep_decls,
        &config,
        |decls, _typed| {
            tests = super::compile::extract_tests(decls, filter.as_deref());
        },
    );

    let pipeline_sources = if output.source_files.is_empty() {
        source_files.clone()
    } else {
        output.source_files.clone()
    };
    for diag in &output.diagnostics {
        crate::show_diagnostic_multi(diag, &pipeline_sources);
    }
    let Some(compiled) = output.result else {
        eprintln!("{}", output::banner_fail("Check", output.diagnostics.len()));
        process::exit(1);
    };

    if tests.is_empty() {
        if format == Format::Human {
            println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
            println!("  No tests found.");
        }
        return;
    }

    let (typed, all_decls, mono, comptime_globals) = (
        compiled.typed,
        compiled.decls,
        compiled.mono,
        compiled.comptime_globals,
    );

    let tmp_dir = std::env::temp_dir();
    let bin_path = tmp_dir.join(format!("rask_test_{}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();
    let obj_path = format!("{}.o", bin_str);

    if let Err(errors) = super::compile::compile_tests_to_object(
        &mono, &typed, &all_decls, &comptime_globals,
        &tests, None, None, &obj_path, Some(&cfg),
    ) {
        for e in &errors {
            eprintln!("{}: compile: {}", output::error_label(), e);
        }
        let _ = std::fs::remove_file(&obj_path);
        process::exit(1);
    }

    if let Err(e) = super::link::link_executable_with(
        &obj_path, &bin_str, &prepared.link_opts, false, None,
    ) {
        eprintln!("{}: link: {}", output::error_label(), e);
        let _ = std::fs::remove_file(&obj_path);
        process::exit(1);
    }
    let _ = std::fs::remove_file(&obj_path);

    let run_output = process::Command::new(&bin_str).output();
    let _ = std::fs::remove_file(&bin_path);

    match run_output {
        Ok(out) => {
            forward_test_stderr(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let complete = display_test_results(&stdout, path, format, tests.len());
            if !out.status.success() || !complete {
                process::exit(test_exit_code(&out.status));
            }
        }
        Err(e) => {
            eprintln!("{}: executing test binary: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}

/// The leak checker's exit code, passed through.
///
/// Everything else collapses to 1 — a failing assert and a binary that died
/// mean the same thing to a caller. `RASK_LEAK_CHECK=1` doesn't: the tests all
/// passed and the program simply didn't give its memory back, and a gate wants
/// to tell those apart without reading prose. `tests/leak_gate.sh` used to
/// decide by grepping the output for "never released", which is how it managed
/// to read 180 leaking files as clean when nothing was forwarding that line.
fn test_exit_code(status: &process::ExitStatus) -> i32 {
    match status.code() {
        Some(c) if c == RASK_LEAK_EXIT => c,
        _ => 1,
    }
}

/// `rask_leak_check`'s exit code in `runtime/string.c`.
const RASK_LEAK_EXIT: i32 = 97;

/// Pass the test binary's stderr through to ours.
///
/// `.output()` captures both streams and only the results parser wants stdout,
/// so stderr was read and dropped. Everything the binary said on the way out
/// went with it: a panic message, a runtime warning, and — the one that made
/// this worth chasing — the leak checker's report. `tests/leak_gate.sh` decides
/// by grepping for "never released", so it read every file as clean while the
/// binaries were exiting 97 underneath it.
fn forward_test_stderr(stderr: &[u8]) {
    if !stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(stderr));
    }
}

/// Compile a .rk file's tests natively and run them.
/// Compile and run tests with options (verbose, sequential, seed).
/// Run tests through the tree-walking interpreter (no codegen).
/// Useful for cross-checking codegen-side regressions and for tests that
/// don't yet compile natively.
pub fn cmd_test_interp(path: &str, filter: Option<String>, format: Format) {
    let result = crate::run_check_or_exit(path, format);

    // Same as `cmd_run`: the program name is argv[0], so a test that reads
    // `os.args()` sees what it would see natively (std.os/A1).
    let mut interp = rask_interp::Interpreter::with_args(vec![path.to_string()]);
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    interp.inject_cfg(&cfg);
    interp.set_node_types(result.typed.node_types.clone());
    interp.set_error_wraps(result.typed.error_wraps.clone());
    interp.set_try_chain_placement(result.typed.try_chain_placement.clone());
    interp.set_fallback_keeps_shape(result.typed.fallback_keeps_shape.clone());
    if let Some((_, source)) = result.source_files.first() {
        interp.set_source_info(path, source);
    }
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }

    let all = decls_with_rask_stdlib(&result.decls);
    let test_results = interp.run_tests(&all, filter.as_deref());

    // Render in the same format as native (JSON-per-line, then summarize).
    let mut json_lines = String::new();
    for r in &test_results {
        let escaped_name = json_escape(&r.name);
        let dur_ns = r.duration.as_nanos() as u64;
        // The native runner's stdout has the body's prints sitting just ahead of
        // that test's JSON line, because the line is written once the body is
        // done. Reproduce that here so one renderer serves both backends and the
        // two can't drift (#612).
        json_lines.push_str(&r.output);
        if !r.output.is_empty() && !r.output.ends_with('\n') {
            json_lines.push('\n');
        }
        if let Some(reason) = &r.skipped {
            json_lines.push_str(&format!(
                "{{\"name\":\"{}\",\"passed\":true,\"duration_ns\":{},\"skipped\":\"{}\"}}\n",
                escaped_name, dur_ns, json_escape(reason),
            ));
        } else if r.passed {
            json_lines.push_str(&format!(
                "{{\"name\":\"{}\",\"passed\":true,\"duration_ns\":{}}}\n",
                escaped_name, dur_ns,
            ));
        } else {
            json_lines.push_str(&format!(
                "{{\"name\":\"{}\",\"passed\":false,\"duration_ns\":{},\"error\":\"{}\"}}\n",
                escaped_name, dur_ns, json_escape(&r.errors.join("; ")),
            ));
        }
    }
    display_test_results(&json_lines, path, format, test_results.len());

    if test_results.iter().any(|r| !r.passed && r.skipped.is_none()) {
        process::exit(1);
    }
}

pub fn cmd_test_native_with_opts(path: &str, filter: Option<String>, format: Format, opts: &TestOptions) {
    // --seed is accepted for forward-compatibility (T8) but not yet functional
    if opts.seed.is_some() && format == Format::Human {
        eprintln!("{}: --seed accepted but random ordering not yet implemented", "note".dimmed());
    }
    // --sequential is accepted for forward-compatibility (T7); tests already run sequentially
    cmd_test_native(path, filter, format);
}

pub fn cmd_test_native(path: &str, filter: Option<String>, format: Format) {
    match run_test_file_native(path, filter.as_deref(), format) {
        TestOutcome::Passed => {}
        TestOutcome::Failed => process::exit(1),
        TestOutcome::Leaked => process::exit(RASK_LEAK_EXIT),
    }
}

/// How a test file finished.
///
/// `Leaked` is why this isn't a bool. Under `RASK_LEAK_CHECK=1` a file whose
/// tests all pass and whose memory doesn't come back is neither of the other
/// two, and a gate wants to tell it apart without reading prose:
/// `tests/leak_gate.sh` decided by grepping the output for "never released",
/// which is how it read 180 leaking files as clean while nothing was
/// forwarding that line (#1048).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    Failed,
    Leaked,
}

impl TestOutcome {
    /// True when nothing failed — a leak is not a failed test.
    pub fn tests_passed(self) -> bool {
        self != TestOutcome::Failed
    }
}

/// Run tests for a single file natively. Returns true on success.
/// Unlike `cmd_test_native`, this never calls `process::exit` — failures are
/// reported via diagnostics and the return value, so callers can iterate over
/// multiple files without aborting on the first failure. Panics anywhere in
/// the pipeline are caught and reported as a per-file failure.
pub fn run_test_file_native(path: &str, filter: Option<&str>, format: Format) -> TestOutcome {
    let path_owned = path.to_string();
    let filter_owned = filter.map(|s| s.to_string());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_test_file_native_inner(&path_owned, filter_owned.as_deref(), format)
    }));
    match result {
        Ok(outcome) => outcome,
        Err(_) => {
            eprintln!("{}: panic while testing {}", output::error_label(), path);
            TestOutcome::Failed
        }
    }
}

fn run_test_file_native_inner(path: &str, filter: Option<&str>, format: Format) -> TestOutcome {
    // One frontend, shared with `rask build` — the test runner is the decl
    // rewrite handed to it, not a second copy of the pipeline (#330).
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    let config = rask_compiler::CompilerConfig { cfg: cfg.clone() };
    let mut tests = Vec::new();
    let output = rask_compiler::compile_file_with(path, Vec::new(), &config, |decls, _typed| {
        tests = super::compile::extract_tests(decls, filter);
    });

    // The pipeline reports its own sources; fall back to the file itself when
    // it failed early enough to have none, so a diagnostic still gets a snippet.
    let source_files = if output.source_files.is_empty() {
        std::fs::read_to_string(path)
            .map(|src| vec![(std::path::PathBuf::from(path), src)])
            .unwrap_or_default()
    } else {
        output.source_files.clone()
    };
    for diag in &output.diagnostics {
        crate::show_diagnostic_multi(diag, &source_files);
    }
    let Some(result) = output.result else {
        if format == Format::Human {
            eprintln!("{}", output::banner_fail("Check", output.diagnostics.len()));
        }
        return TestOutcome::Failed;
    };

    if tests.is_empty() {
        if format == Format::Human {
            println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());
            println!("  No tests found.");
        }
        return TestOutcome::Passed;
    }

    let mono = result.mono;
    let comptime_globals = result.comptime_globals;

    let tmp_dir = std::env::temp_dir();
    let bin_path = tmp_dir.join(format!("rask_test_{}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();
    let obj_path = format!("{}.o", bin_str);

    if let Err(errors) = super::compile::compile_tests_to_object(
        &mono, &result.typed, &result.decls, &comptime_globals,
        &tests, Some(path), source_files.first().map(|(_, s)| s.as_str()), &obj_path, Some(&cfg),
    ) {
        for e in &errors {
            eprintln!("{}: compile: {}", output::error_label(), e);
        }
        let _ = std::fs::remove_file(&obj_path);
        return TestOutcome::Failed;
    }

    let link_opts = super::link::LinkOptions::default();
    if let Err(e) = super::link::link_executable_with(&obj_path, &bin_str, &link_opts, false, None) {
        eprintln!("{}: link: {}", output::error_label(), e);
        let _ = std::fs::remove_file(&obj_path);
        return TestOutcome::Failed;
    }
    let _ = std::fs::remove_file(&obj_path);

    let run_output = process::Command::new(&bin_str).output();
    // A test binary that dies mid-run takes the evidence with it. Keeping it
    // is the difference between "something crashed" and a backtrace.
    if std::env::var_os("RASK_KEEP_TEST_BIN").is_some() {
        eprintln!("{}: test binary kept at {}", output::warning_label(), bin_str);
    } else {
        let _ = std::fs::remove_file(&bin_path);
    }

    match run_output {
        Ok(out) => {
            forward_test_stderr(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let complete = display_test_results(&stdout, path, format, tests.len());
            let leaked = out.status.code() == Some(RASK_LEAK_EXIT);
            if complete && (out.status.success() || leaked) {
                if leaked { TestOutcome::Leaked } else { TestOutcome::Passed }
            } else {
                TestOutcome::Failed
            }
        }
        Err(e) => {
            eprintln!("{}: executing test binary: {}", output::error_label(), e);
            TestOutcome::Failed
        }
    }
}

/// Run tests for every `.rk` file in a directory independently.
///
/// Each file is type-checked and compiled in isolation, so identically-named
/// types in different files don't collide. Used when the directory has no
/// `build.rk` manifest — i.e., it's a folder of standalone test files rather
/// than a single multi-file package. Exits 1 if any file fails.
pub fn cmd_test_files_native(dir: &str, filter: Option<String>, format: Format) {
    let dir_path = std::path::Path::new(dir);
    let files = crate::collect_rk_files(dir_path);

    if files.is_empty() {
        if format == Format::Human {
            println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(dir), "===".dimmed());
            println!("  No .rk files found.");
        }
        return;
    }

    if format == Format::Human {
        println!("{} Test suite: {} ({} files) {}\n",
            "===".dimmed(), output::file_path(dir), files.len(), "===".dimmed());
    }

    let mut failed_files = 0;
    for file in &files {
        if !run_test_file_native(file, filter.as_deref(), format).tests_passed() {
            failed_files += 1;
        }
    }

    if format == Format::Human && files.len() > 1 {
        println!();
        println!("{}", output::separator(50));
        if failed_files == 0 {
            println!("{} all {} files passed", output::status_pass(), files.len());
        } else {
            println!("{} {} of {} files failed",
                output::status_fail(), failed_files, files.len());
        }
    }

    if failed_files > 0 {
        process::exit(1);
    }
}

/// Parse and display test results from JSON output lines.
/// Print the run's results. `expected` is how many tests the binary was built
/// with; fewer results than that means it died partway and the run is a
/// failure, not a pass over whatever arrived. Returns false in that case.
fn display_test_results(stdout: &str, path: &str, format: Format, expected: usize) -> bool {
    let reported = stdout.lines().filter(|l| l.trim().starts_with('{')).count();
    let truncated = reported < expected;

    if format != Format::Human {
        // JSON mode: pass through raw output
        print!("{}", stdout);
        if truncated {
            eprintln!(
                "{}: test run stopped after {} of {} tests — the binary died mid-run",
                output::error_label(), reported, expected,
            );
        }
        return !truncated;
    }

    println!("{} Testing {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut total_duration = std::time::Duration::ZERO;

    // Anything that isn't a protocol record is the running test's own output.
    // It arrives before that test's record, since the record is written once the
    // body is done — so hold it and print it under the test it belongs to. These
    // lines used to be dropped on the floor natively while the interpreter wrote
    // them straight to the terminal ahead of the banner, which made any suite
    // test containing a `println` diverge by construction (#612).
    let mut pending_output: Vec<&str> = Vec::new();
    let print_output = |lines: &mut Vec<&str>| {
        if lines.is_empty() {
            return;
        }
        println!("      {}", "output:".dimmed());
        for l in lines.iter() {
            println!("      {} {}", "│".dimmed(), l);
        }
        lines.clear();
    };

    for line in stdout.lines() {
        let raw = line;
        let line = line.trim();
        if !line.starts_with('{') {
            pending_output.push(raw);
            continue;
        }

        // The name is JSON-escaped on the way out, so it comes back escaped.
        // Unescaped nothing used to escape it either, and a test whose name
        // held a quote lost everything after it (#849).
        let name = unescape_json_str(parse_json_str(line, "name").unwrap_or("?"));
        let name = name.as_str();
        let passed_val = line.contains("\"passed\":true");
        let duration_ns = parse_json_i64(line, "duration_ns").unwrap_or(0);
        let duration = std::time::Duration::from_nanos(duration_ns as u64);
        total_duration += duration;

        // Check for skipped tests
        if let Some(reason) = parse_json_str(line, "skipped") {
            skipped += 1;
            println!("  {} {} {}",
                "SKIP".yellow(),
                name,
                format!("({})", format_test_error(reason)).dimmed(),
            );
            print_output(&mut pending_output);
            continue;
        }

        if passed_val {
            passed += 1;
            println!("  {} {} {}",
                output::status_pass(),
                name,
                format!("({}ms)", duration.as_millis()).dimmed(),
            );
            print_output(&mut pending_output);
        } else {
            failed += 1;
            println!("  {} {}",
                output::status_fail(),
                name,
            );
            print_output(&mut pending_output);
            if let Some(error) = parse_json_str(line, "error") {
                println!("      {}", format_test_error(error).red());
            }
        }
    }

    // Output with no record after it — the test that printed it never finished,
    // so there's no result line to sit under. It's the most useful thing on
    // screen when a test binary dies mid-run, so it isn't dropped.
    if !pending_output.is_empty() {
        println!("  {}", "output after the last completed test:".dimmed());
        for l in &pending_output {
            println!("      {} {}", "│".dimmed(), l);
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
        summary.push_str(&format!(", {} skipped", skipped));
    }
    summary.push_str(&format!(" ({}ms)", total_duration.as_millis()));
    println!("{}", summary);

    if truncated {
        println!(
            "{}",
            format!(
                "stopped after {} of {} tests — the test binary died mid-run, \
                 so the rest never ran",
                reported, expected,
            ).red(),
        );
    }
    !truncated
}

/// Escape a string for the one-line-per-test JSON the display layer parses.
///
/// Newlines matter as much as quotes here: a raw newline splits the record
/// across two lines and the parser then sees an unterminated string, so an
/// `assert_eq` diff from the interpreter vanished instead of printing.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Undo the JSON escaping the test binary applies, and indent continuation
/// lines so a multi-line diff stays under the test name instead of falling
/// back to column 0. Without this an `assert_eq` failure printed its `\n`
/// literally and the whole diff ran together on one line.
/// Undo the escaping the test harness applies to a JSON string value. Same
/// rules as `format_test_error`, minus its message-specific newline indent.
fn unescape_json_str(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn format_test_error(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push_str("\n      "),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn parse_json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{}\":\"", key);
    let start = s.find(&pat)? + pat.len();
    // Walk past escaped characters to find the real closing quote
    let bytes = s[start..].as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2; // skip escaped char
        } else if bytes[i] == b'"' {
            return Some(&s[start..start + i]);
        } else {
            i += 1;
        }
    }
    None
}

fn parse_json_i64(s: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{}\":", key);
    let start = s.find(&pat)? + pat.len();
    let rest = &s[start..];
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
    rest[..end].parse().ok()
}



/// Compile a .rk file to a temp executable and run it.
pub fn cmd_run_native(path: &str, program_args: Vec<String>, format: Format, link_opts: &super::link::LinkOptions, release: bool) {
    let tmp_dir = std::env::temp_dir();
    let bin_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("rask_out");
    let bin_path = tmp_dir.join(format!("rask_{}_{}", bin_name, std::process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();

    // Compile quietly — suppress the "Compiled →" banner (errors still show)
    super::codegen::cmd_compile(path, Some(&bin_str), format, true, link_opts, release, None);

    let status = process::Command::new(&bin_str)
        .args(&program_args)
        .status();

    let _ = std::fs::remove_file(&bin_path);

    match status {
        Ok(s) => {
            if !s.success() {
                process::exit(exit_code_reporting_signals(&s));
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
    let result = crate::run_check_or_exit(path, format);

    let mut interp = rask_interp::Interpreter::with_args(vec![path.to_string()]);
    interp.set_node_types(result.typed.node_types.clone());
    interp.set_error_wraps(result.typed.error_wraps.clone());
    interp.set_try_chain_placement(result.typed.try_chain_placement.clone());
    interp.set_fallback_keeps_shape(result.typed.fallback_keeps_shape.clone());
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }
    let all = decls_with_rask_stdlib(&result.decls);
    let results = interp.run_benchmarks(&all, filter.as_deref());

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
        let result = crate::run_check_or_exit(path, format);
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
    // Same one frontend as the test runner — the benchmark runner is the decl
    // rewrite handed to it (#330).
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    let config = rask_compiler::CompilerConfig { cfg: cfg.clone() };
    let path_owned = path.to_string();
    let filter_owned = filter.map(|f| f.to_string());
    let compiled = std::panic::catch_unwind(move || {
        let mut benchmarks = Vec::new();
        let output = rask_compiler::compile_file_with(
            &path_owned,
            Vec::new(),
            &config,
            |decls, _typed| {
                benchmarks = super::compile::extract_benchmarks(decls, filter_owned.as_deref());
            },
        );
        (output, benchmarks)
    });
    let (output, benchmarks) = match compiled {
        Ok(pair) => pair,
        Err(_) => {
            if format == Format::Human {
                eprintln!("    {}: frontend panic for {}", output::error_label(), path);
            }
            return Vec::new();
        }
    };

    let source_files = output.source_files.clone();
    if output.result.is_none() {
        for diag in &output.diagnostics {
            crate::show_diagnostic_multi(diag, &source_files);
        }
        return Vec::new();
    }
    if benchmarks.is_empty() {
        return Vec::new();
    }
    let result = output.result.unwrap();
    let mono = result.mono;
    let comptime_globals = result.comptime_globals;

    let tmp_dir = std::env::temp_dir();
    let bin_path = tmp_dir.join(format!("rask_bench_{}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();
    let obj_path = format!("{}.o", bin_str);

    if let Err(errors) = super::compile::compile_benchmarks_to_object(
        &mono, &result.typed, &result.decls, &comptime_globals,
        &benchmarks, Some(path), source_files.first().map(|(_, s)| s.as_str()), &obj_path, Some(&cfg),
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
    if let Err(e) = super::link::link_executable_with(&obj_path, &bin_str, &link_opts, true, None) {
        if format == Format::Human {
            eprintln!("    {}: link: {}", output::error_label(), e);
        }
        return Vec::new();
    }

    let output = process::Command::new(&bin_str).output();
    let _ = std::fs::remove_file(&bin_path);

    collect_bench_results(output, "benchmark", format)
}

/// Parse a benchmark binary's results, saying so when it didn't produce any.
///
/// Both callers used to answer `Vec::new()` for every failure — a binary that
/// crashed, one that exited non-zero, one that couldn't be spawned. An empty
/// result set is also what a file with no benchmarks in it returns, so a
/// benchmark that died read exactly like a benchmark that wasn't there. The
/// stderr went the same way.
fn collect_bench_results(
    output: std::io::Result<process::Output>,
    what: &str,
    format: Format,
) -> Vec<BenchResult> {
    let out = match output {
        Ok(out) => out,
        Err(e) => {
            if format == Format::Human {
                eprintln!("    {}: running the {}: {}", output::error_label(), what, e);
            }
            return Vec::new();
        }
    };
    if !out.stderr.is_empty() && format == Format::Human {
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
    }
    if !out.status.success() {
        if format == Format::Human {
            eprintln!(
                "    {}: the {} exited {}",
                output::error_label(),
                what,
                out.status.code().map_or_else(|| "on a signal".to_string(), |c| c.to_string()),
            );
        }
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.lines().filter_map(parse_bench_json).collect()
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
                           "unicode_case.c", "alloc.c", "panic.c", "args.c", "ptr.c"];
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

    collect_bench_results(output, "C baseline", format)
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

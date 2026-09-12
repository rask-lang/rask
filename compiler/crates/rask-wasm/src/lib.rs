// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! WebAssembly bindings for the Rask interpreter.
//!
//! This crate provides a thin wrapper around the Rask interpreter to expose it
//! to JavaScript via wasm-bindgen. It enables running Rask code in the browser.

use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;
use web_sys::console;

use rask_compiler::{CfgConfig, CompilerConfig};
use rask_diagnostics::{formatter::DiagnosticFormatter, json, Diagnostic};
use rask_interp::{Interpreter, RuntimeError};

/// Tell the interpreter how much stack it may spend.
///
/// Its recursion guard measures against the stack it was given, and on wasm
/// that size is a link argument rather than something it chose — so hand it the
/// number `build.rs` reserved. Without this it assumes wasm-ld's 1 MiB default
/// and refuses at about 30 frames.
///
/// This crate is in the workspace, so it also builds for the host, where there
/// is no such thing to say.
#[cfg(target_family = "wasm")]
fn announce_stack_budget() {
    if let Some(bytes) = option_env!("RASK_WASM_STACK_BYTES").and_then(|s| s.parse().ok()) {
        rask_interp::set_stack_bytes(bytes);
    }
}

#[cfg(not(target_family = "wasm"))]
fn announce_stack_budget() {}

/// Browser-based Rask playground.
///
/// Provides a simple API for running Rask code and capturing output.
#[wasm_bindgen]
pub struct Playground {
    interpreter: Interpreter,
    output_buffer: Arc<Mutex<String>>,
}

#[wasm_bindgen]
impl Playground {
    /// Create a new playground instance.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        // Better panic messages in browser console
        console_error_panic_hook::set_once();

        announce_stack_budget();

        let (interpreter, output_buffer) = Interpreter::with_captured_output();
        Self {
            interpreter,
            output_buffer,
        }
    }

    /// Run Rask source code and return output or error.
    ///
    /// Same pipeline as `rask run --interp`, by calling the same code: the
    /// compiler frontend (which compiles `stdlib/*.rk` alongside the program),
    /// then the interpreter over the two together. This used to be a
    /// hand-written copy of the pipeline that had drifted — no stdlib, and none
    /// of the typechecker's output handed to the interpreter — so a third of
    /// the examples failed here while running fine on a local build (#1177).
    pub fn run(&mut self, source: &str) -> Result<String, String> {
        self.output_buffer.lock().unwrap().clear();

        let checked = frontend(source)?;
        self.prepare(&checked, source);

        let all = rask_compiler::program_decls(&checked.decls);
        let outcome = self.interpreter.run(&all);

        let output = self.output_buffer.lock().unwrap().clone();
        match outcome {
            Ok(_) => Ok(output),
            Err(diag) => match diag.error {
                RuntimeError::Exit(0) => Ok(output),
                RuntimeError::Exit(code) => {
                    Err(format!("Program exited with code {}\n{}", code, output))
                }
                other => Err(format!("Runtime error:\n{}", other)),
            },
        }
    }

    /// Run the program's `test` blocks and `@test` functions.
    ///
    /// What `rask test` does, minus the timings: the browser gives wasm no
    /// clock, so a duration here would be a row of zeroes pretending to be a
    /// measurement. Benchmarks are refused for the same reason — there is
    /// nothing to measure them with.
    pub fn run_tests(&mut self, source: &str) -> Result<String, String> {
        self.output_buffer.lock().unwrap().clear();

        let checked = frontend(source)?;
        self.prepare(&checked, source);

        let all = rask_compiler::program_decls(&checked.decls);
        let results = self.interpreter.run_tests(&all, None);
        let benchmarks = checked
            .decls
            .iter()
            .filter(|d| matches!(d.kind, rask_ast::decl::DeclKind::Benchmark(_)))
            .count();

        if results.is_empty() && benchmarks == 0 {
            return Err("No tests in this program. A test looks like `test \"name\" { … }`.".into());
        }

        let mut report = String::new();
        let mut failed = 0;
        for r in &results {
            if let Some(reason) = &r.skipped {
                report.push_str(&format!("skip  {}  ({})\n", r.name, reason));
                continue;
            }
            if r.passed {
                report.push_str(&format!("pass  {}\n", r.name));
            } else {
                failed += 1;
                report.push_str(&format!("FAIL  {}\n", r.name));
                for e in &r.errors {
                    report.push_str(&format!("        {}\n", e));
                }
            }
            if !r.output.is_empty() {
                for line in r.output.lines() {
                    report.push_str(&format!("        {}\n", line));
                }
            }
        }

        report.push_str(&format!(
            "\n{} of {} passed\n",
            results.len() - failed,
            results.len()
        ));

        if benchmarks > 0 {
            report.push_str(&format!(
                "\n{} benchmark(s) not run: timing them needs a clock, and the \
                 browser doesn't give wasm one. Run them with `rask benchmark`.\n",
                benchmarks
            ));
        }

        if failed > 0 {
            Err(report)
        } else {
            Ok(report)
        }
    }

    /// Check code for errors without running it.
    ///
    /// Same frontend as `run`, rendered as JSON diagnostics for the editor.
    pub fn check(&self, source: &str) -> String {
        let output = rask_compiler::check_source(PLAYGROUND, source, &config());
        let report = json::to_json_report(&output.diagnostics, source, PLAYGROUND, "check");
        serde_json::to_string(&report).unwrap()
    }

    /// Hand the interpreter what the typechecker worked out.
    ///
    /// `rask run --interp` does exactly this; leaving it out is why the
    /// playground's error wrapping and try-chains behaved differently from a
    /// local run.
    fn prepare(&mut self, checked: &rask_compiler::CheckResult, source: &str) {
        self.interpreter.inject_cfg(&cfg());
        self.interpreter.set_node_types(checked.typed.node_types.clone());
        self.interpreter.set_error_wraps(checked.typed.error_wraps.clone());
        self.interpreter
            .set_try_chain_placement(checked.typed.try_chain_placement.clone());
        self.interpreter
            .set_fallback_keeps_shape(checked.typed.fallback_keeps_shape.clone());
        self.interpreter.set_source_info(PLAYGROUND, source);
        if !checked.package_names.is_empty() {
            self.interpreter.register_packages(&checked.package_names);
        }
    }

    /// Get the version of the Rask compiler.
    pub fn version() -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
}

/// The file name diagnostics are rendered against. There is no file.
const PLAYGROUND: &str = "<playground>";

fn cfg() -> CfgConfig {
    CfgConfig::from_host("debug", vec![])
}

fn config() -> CompilerConfig {
    CompilerConfig { cfg: cfg() }
}

/// Run the frontend, or give back the diagnostics as the browser should see
/// them: rendered, plain text, no ANSI.
fn frontend(source: &str) -> Result<rask_compiler::CheckResult, String> {
    let output = rask_compiler::check_source(PLAYGROUND, source, &config());
    if output.has_errors() {
        return Err(render(source, &output.diagnostics));
    }
    output
        .result
        .ok_or_else(|| render(source, &output.diagnostics))
}

/// Render diagnostics against the editor's buffer.
///
/// Only the ones that belong to it. `stdlib/*.rk` compiles alongside the
/// program, and its spans index its own files — drawn against this source they
/// point at whatever happens to be at that offset, which is how a stdlib error
/// once underlined line 164 column 2309 of a 164-line program. A stdlib error
/// is a compiler bug rather than the reader's, so it is reported as one.
fn render(source: &str, diagnostics: &[Diagnostic]) -> String {
    let formatter = DiagnosticFormatter::new(source).with_file_name(PLAYGROUND);
    let mine: Vec<String> = diagnostics
        .iter()
        .filter(|d| {
            d.primary_span()
                .is_none_or(|s| !rask_compiler::is_stdlib_span(s))
        })
        .map(|d| strip_ansi_codes(&formatter.format(d)))
        .collect();

    if !mine.is_empty() {
        return mine.join("\n");
    }

    let mut report = String::from(
        "The standard library failed to compile, which is a bug in Rask rather \
         than in this program.\nPlease report it at \
         https://github.com/rask-lang/rask/issues\n\n",
    );
    for d in diagnostics {
        report.push_str(&format!("  {}\n", d.message));
    }
    report
}

fn strip_ansi_codes(s: &str) -> String {
    // Debug logging to see what we're converting
    let preview: String = s.chars().take(100).collect();
    console::log_1(&format!("ANSI Input (first 100 chars): {:?}", preview).into());
    console::log_1(&format!("Contains ESC: {}", s.contains('\x1b')).into());

    let mut result = String::with_capacity(s.len() * 2);
    let mut chars = s.chars().peekable();
    let mut open_span = false;
    let mut ansi_codes_found = 0;

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // Skip '['
            ansi_codes_found += 1;

            // Collect the escape sequence
            let mut code = String::new();
            while let Some(&peek) = chars.peek() {
                chars.next();
                if peek.is_ascii_alphabetic() {
                    break;
                }
                code.push(peek);
            }

            // Log the ANSI code we found
            console::log_1(&format!("Found ANSI code: '{}'", code).into());

            // Close previous span if open
            if open_span {
                result.push_str("</span>");
                open_span = false;
            }

            // Convert ANSI code to CSS class (expanded patterns)
            let class = match code.as_str() {
                "31" | "1;31" | "31;1" | "0;31" => Some("error"),      // Red (errors)
                "34" | "1;34" | "34;1" | "0;34" => Some("info"),        // Blue (info/secondary)
                "36" | "1;36" | "36;1" | "0;36" => Some("help"),        // Cyan (help/notes)
                "33" | "1;33" | "33;1" | "0;33" => Some("warning"),     // Yellow (warnings)
                "1" | "01" => Some("bold"),                              // Bold
                "0" | "00" => None,                                      // Reset
                _ => {
                    console::log_1(&format!("Unknown ANSI code: '{}'", code).into());
                    None
                }
            };

            if let Some(class_name) = class {
                result.push_str(&format!("<span class=\"diag-{}\">", class_name));
                open_span = true;
            }
        } else {
            // Escape HTML special chars
            match ch {
                '<' => result.push_str("&lt;"),
                '>' => result.push_str("&gt;"),
                '&' => result.push_str("&amp;"),
                '"' => result.push_str("&quot;"),
                '\n' => result.push_str("\n"),
                _ => result.push(ch),
            }
        }
    }

    // Close final span if open
    if open_span {
        result.push_str("</span>");
    }

    // Log summary
    console::log_1(&format!("Found {} ANSI codes, output contains spans: {}",
        ansi_codes_found, result.contains("<span")).into());
    let output_preview: String = result.chars().take(200).collect();
    console::log_1(&format!("Output (first 200 chars): {:?}", output_preview).into());

    result
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! WebAssembly bindings for the Rask interpreter.
//!
//! This crate provides a thin wrapper around the Rask interpreter to expose it
//! to JavaScript via wasm-bindgen. It enables running Rask code in the browser.

use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;

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
                    Err(plain(&format!("Program exited with code {}\n{}", code, output)))
                }
                other => Err(plain(&format!("Runtime error:\n{}", other))),
            },
        }
    }

    /// Run the program's `test` blocks, `@test` functions and `benchmark` blocks.
    ///
    /// What `rask test` does, minus the timings: the browser gives wasm no
    /// clock, so a duration here would be a row of zeroes pretending to be a
    /// measurement. A benchmark block still runs — once, so you can see it
    /// does — it just isn't timed.
    pub fn run_tests(&mut self, source: &str) -> Result<String, String> {
        self.output_buffer.lock().unwrap().clear();

        let checked = frontend(source)?;
        self.prepare(&checked, source);

        let all = rask_compiler::program_decls(&checked.decls);
        let results = self.interpreter.run_tests(&all, None);
        let benchmarks = self.interpreter.run_benchmarks(&all, None);

        if results.is_empty() && benchmarks.is_empty() {
            return Err(escape_html(
                "No tests in this program. A test looks like `test \"name\" { … }`.",
            ));
        }

        // Every piece of this report that came from the program — test names,
        // assertion messages, whatever the body printed — is escaped as it goes
        // in. The failing report is handed to `innerHTML` on the other side, so
        // a test named `Vec<i32> stuff` would otherwise put an element in the
        // page, and one named `<script>…` would put a script in it. The
        // playground's code travels in a shared URL, so that is someone else's
        // page, not only your own.
        let mut report = String::new();
        let mut failed = 0;
        for r in &results {
            if let Some(reason) = &r.skipped {
                report.push_str(&format!(
                    "skip  {}  ({})\n",
                    escape_html(&r.name),
                    escape_html(reason)
                ));
                continue;
            }
            if r.passed {
                report.push_str(&format!("pass  {}\n", escape_html(&r.name)));
            } else {
                failed += 1;
                report.push_str(&format!("FAIL  {}\n", escape_html(&r.name)));
                for e in &r.errors {
                    report.push_str(&format!("        {}\n", escape_html(e)));
                }
            }
            if !r.output.is_empty() {
                for line in r.output.lines() {
                    report.push_str(&format!("        {}\n", escape_html(line)));
                }
            }
        }

        if !results.is_empty() {
            report.push_str(&format!(
                "\n{} of {} passed\n",
                results.len() - failed,
                results.len()
            ));
        }

        if !benchmarks.is_empty() {
            report.push_str("\nBenchmarks ran once each, untimed — timing needs a clock and the\n");
            report.push_str("browser gives wasm none. `rask benchmark` measures them properly.\n");
            for b in &benchmarks {
                report.push_str(&format!("ran   {}\n", escape_html(&b.name)));
            }
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

/// The machine the playground pretends to be.
///
/// Not the host. The host is wasm32 with no OS, and saying so made `usize` 32
/// bits — `stdlib/fs.rk`'s `n as usize` became a narrowing conversion and the
/// playground rejected its own standard library — while `cfg.os` came back
/// `"unknown"`, so any `comptime if` branch in the stdlib guarded on an OS was
/// eliminated. The playground emits no code: it interprets, on 64-bit values,
/// through the same stdlib a Linux build uses. This is what it is emulating.
fn cfg() -> CfgConfig {
    CfgConfig::from_target("x86_64-linux-gnu", "debug", vec![])
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

/// Make one character safe to put in HTML.
///
/// Everything the playground hands back on the error channel is inserted with
/// `innerHTML`, because a diagnostic carries `<span>`s for its colours. So
/// every character that did *not* come from this file has to be escaped on the
/// way out, or a `Vec<i32>` in a message becomes an element and a `<script>` in
/// one becomes a script.
fn push_escaped(out: &mut String, ch: char) {
    match ch {
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '&' => out.push_str("&amp;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => out.push(ch),
    }
}

/// Put plain text on the error channel.
///
/// Everything returned as `Err` is inserted with `innerHTML` on the other side,
/// because a compiler diagnostic carries `<span>`s for its colours. So the
/// channel is HTML, and text that came from the program can't go on it as it
/// is: a run that printed `<img src=x onerror=…>` and exited non-zero put a
/// live element on the page, with rask-lang.dev's origin under it. The
/// program's code travels in a shared URL, so that is someone else's page.
///
/// This is the only way plain text gets onto that channel. Escaping at each
/// site is what failed: `run_tests` remembered and `run` didn't.
fn plain(text: &str) -> String {
    escape_html(text)
}

/// Escape a whole string for `innerHTML`.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        push_escaped(&mut out, ch);
    }
    out
}

/// Turn the compiler's ANSI colours into spans, escaping everything else.
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    let mut chars = s.chars().peekable();
    let mut open_span = false;

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // Skip '['

            // Collect the escape sequence
            let mut code = String::new();
            while let Some(&peek) = chars.peek() {
                chars.next();
                if peek.is_ascii_alphabetic() {
                    break;
                }
                code.push(peek);
            }

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
                _ => None,
            };

            if let Some(class_name) = class {
                result.push_str(&format!("<span class=\"diag-{}\">", class_name));
                open_span = true;
            }
        } else {
            push_escaped(&mut result, ch);
        }
    }

    // Close final span if open
    if open_span {
        result.push_str("</span>");
    }

    result
}

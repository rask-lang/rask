// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! WebAssembly bindings for the Rask interpreter.
//!
//! This crate provides a thin wrapper around the Rask interpreter to expose it
//! to JavaScript via wasm-bindgen. It enables running Rask code in the browser.

use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;
use web_sys::console;

use rask_diagnostics::{formatter::DiagnosticFormatter, json, ToDiagnostic};
use rask_interp::{Interpreter, RuntimeError};
use rask_lexer::Lexer;
use rask_parser::Parser;

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

        let (interpreter, output_buffer) = Interpreter::with_captured_output();
        Self {
            interpreter,
            output_buffer,
        }
    }

    /// Run Rask source code and return output or error.
    ///
    /// Runs the full compiler pipeline (lex → parse → desugar → resolve →
    /// typecheck → ownership) before interpreting, matching `rask run`.
    pub fn run(&mut self, source: &str) -> Result<String, String> {
        // Clear previous output
        self.output_buffer.lock().unwrap().clear();

        // Lex
        let mut lexer = Lexer::new(source);
        let lex_result = lexer.tokenize();
        if !lex_result.is_ok() {
            return Err(format_errors(source, &lex_result.errors));
        }

        // Parse
        let mut parser = Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();
        if !parse_result.is_ok() {
            return Err(format_errors(source, &parse_result.errors));
        }

        // Desugar
        rask_desugar::desugar(&mut parse_result.decls);

        // Resolve
        let resolved = rask_resolve::resolve(&parse_result.decls)
            .map_err(|errors| format_errors(source, &errors))?;

        // Typecheck
        let typed = rask_types::typecheck(resolved, &parse_result.decls)
            .map_err(|errors| format_errors(source, &errors))?;

        // Ownership
        let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
        if !ownership_result.is_ok() {
            return Err(format_errors(source, &ownership_result.errors));
        }

        // Interpret
        match self.interpreter.run(&parse_result.decls) {
            Ok(_) => {
                let output = self.output_buffer.lock().unwrap().clone();
                Ok(output)
            }
            Err(diag) if matches!(diag.error, RuntimeError::Exit(_)) => {
                let output = self.output_buffer.lock().unwrap().clone();
                if let RuntimeError::Exit(code) = diag.error {
                    if code == 0 {
                        Ok(output)
                    } else {
                        Err(format!(
                            "Program exited with code {}\n{}",
                            code, output
                        ))
                    }
                } else {
                    unreachable!()
                }
            }
            Err(diag) => Err(format!("Runtime error:\n{}", diag.error)),
        }
    }

    /// Check code for errors without running it.
    ///
    /// Runs the full pipeline (lex → parse → desugar → resolve → typecheck →
    /// ownership) and returns JSON diagnostics.
    pub fn check(&self, source: &str) -> String {
        let mut all_diagnostics = Vec::new();

        // Lex
        let mut lexer = Lexer::new(source);
        let lex_result = lexer.tokenize();
        for err in &lex_result.errors {
            all_diagnostics.push(err.to_diagnostic());
        }

        if lex_result.is_ok() {
            // Parse
            let mut parser = Parser::new(lex_result.tokens);
            let mut parse_result = parser.parse();
            for err in &parse_result.errors {
                all_diagnostics.push(err.to_diagnostic());
            }

            if parse_result.is_ok() {
                // Desugar
                rask_desugar::desugar(&mut parse_result.decls);

                // Resolve
                match rask_resolve::resolve(&parse_result.decls) {
                    Ok(resolved) => {
                        // Typecheck
                        match rask_types::typecheck(resolved, &parse_result.decls) {
                            Ok(typed) => {
                                // Ownership
                                let ownership = rask_ownership::check_ownership(
                                    &typed, &parse_result.decls,
                                );
                                for err in &ownership.errors {
                                    all_diagnostics.push(err.to_diagnostic());
                                }
                            }
                            Err(errors) => {
                                for err in &errors {
                                    all_diagnostics.push(err.to_diagnostic());
                                }
                            }
                        }
                    }
                    Err(errors) => {
                        for err in &errors {
                            all_diagnostics.push(err.to_diagnostic());
                        }
                    }
                }
            }
        }

        let report = json::to_json_report(&all_diagnostics, source, "<playground>", "check");
        serde_json::to_string(&report).unwrap()
    }

    /// Get the version of the Rask compiler.
    pub fn version() -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
}

/// Format a list of errors into a single string for browser display.
fn format_errors<E: ToDiagnostic>(source: &str, errors: &[E]) -> String {
    errors
        .iter()
        .map(|err| {
            let diag = err.to_diagnostic();
            let formatter = DiagnosticFormatter::new(source).with_file_name("<playground>");
            strip_ansi_codes(&formatter.format(&diag))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Convert ANSI color codes to HTML with CSS classes.
///
/// The DiagnosticFormatter uses ANSI escapes for terminal colors.
/// This converts them to HTML spans for browser display.
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

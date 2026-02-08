// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! WebAssembly bindings for the Rask interpreter.
//!
//! This crate provides a thin wrapper around the Rask interpreter to expose it
//! to JavaScript via wasm-bindgen. It enables running Rask code in the browser.

use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;

use rask_diagnostics::{formatter::DiagnosticFormatter, ToDiagnostic};
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
    /// Returns Ok(output) on success, Err(error_message) on failure.
    /// The error message includes rich formatting with source context.
    pub fn run(&mut self, source: &str) -> Result<String, String> {
        // Clear previous output
        self.output_buffer.lock().unwrap().clear();

        // Phase 1: Lexing
        let mut lexer = Lexer::new(source);
        let lex_result = lexer.tokenize();

        if !lex_result.is_ok() {
            let mut errors = Vec::new();
            for err in &lex_result.errors {
                let diag = err.to_diagnostic();
                let formatter = DiagnosticFormatter::new(source).with_file_name("<playground>");
                let formatted = formatter.format(&diag);
                errors.push(strip_ansi_codes(&formatted));
            }
            return Err(errors.join("\n\n"));
        }

        // Phase 2: Parsing
        let mut parser = Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();

        if !parse_result.is_ok() {
            let mut errors = Vec::new();
            for err in &parse_result.errors {
                let diag = err.to_diagnostic();
                let formatter = DiagnosticFormatter::new(source).with_file_name("<playground>");
                let formatted = formatter.format(&diag);
                errors.push(strip_ansi_codes(&formatted));
            }
            return Err(errors.join("\n\n"));
        }

        // Phase 3: Desugaring (required before interpretation)
        rask_desugar::desugar(&mut parse_result.decls);

        // Phase 4: Interpretation
        match self.interpreter.run(&parse_result.decls) {
            Ok(_) => {
                let output = self.output_buffer.lock().unwrap().clone();
                Ok(output)
            }
            Err(RuntimeError::Exit(code)) => {
                // Program called exit() - this is normal, return output
                let output = self.output_buffer.lock().unwrap().clone();
                if code == 0 {
                    Ok(output)
                } else {
                    Err(format!(
                        "Program exited with code {}\n{}",
                        code, output
                    ))
                }
            }
            Err(e) => Err(format!("Runtime error:\n{}", e)),
        }
    }

    /// Get the version of the Rask compiler.
    pub fn version() -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
}

/// Convert ANSI color codes to HTML with CSS classes.
///
/// The DiagnosticFormatter uses ANSI escapes for terminal colors.
/// This converts them to HTML spans for browser display.
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

            // Convert ANSI code to CSS class
            let class = match code.as_str() {
                "31" | "31;1" => Some("error"),      // Red (errors)
                "1;31" => Some("error"),              // Bold red
                "34" | "34;1" => Some("info"),        // Blue (info/secondary)
                "1;34" => Some("info"),               // Bold blue
                "36" | "36;1" => Some("help"),        // Cyan (help/notes)
                "1;36" => Some("help"),               // Bold cyan
                "33" | "33;1" => Some("warning"),     // Yellow (warnings)
                "1;33" => Some("warning"),            // Bold yellow
                "1" => Some("bold"),                  // Bold
                "0" => None,                          // Reset
                _ => None,
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

    result
}

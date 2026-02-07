// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! WebAssembly bindings for the Rask interpreter.
//!
//! This crate provides a thin wrapper around the Rask interpreter to expose it
//! to JavaScript via wasm-bindgen. It enables running Rask code in the browser.

use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;

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
    /// The error message includes the phase (lex/parse/runtime) and details.
    pub fn run(&mut self, source: &str) -> Result<String, String> {
        // Clear previous output
        self.output_buffer.lock().unwrap().clear();

        // Phase 1: Lexing
        let mut lexer = Lexer::new(source);
        let lex_result = lexer.tokenize();

        if !lex_result.is_ok() {
            let errors: Vec<String> = lex_result
                .errors
                .iter()
                .map(|e| format!("{}", e))
                .collect();
            return Err(format!("Lexer error:\n{}", errors.join("\n")));
        }

        // Phase 2: Parsing
        let mut parser = Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();

        if !parse_result.is_ok() {
            let errors: Vec<String> = parse_result
                .errors
                .iter()
                .map(|e| format!("{}", e))
                .collect();
            return Err(format!("Parse error:\n{}", errors.join("\n")));
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

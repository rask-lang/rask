// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Core backend struct and compilation pipeline.

use std::collections::HashMap;
use std::sync::RwLock;

use tower_lsp::lsp_types::*;
use tower_lsp::Client;

use rask_ast::decl::Decl;
use rask_diagnostics::ToDiagnostic;
use rask_lexer::Lexer;
use rask_parser::Parser;
use rask_types::TypedProgram;

use crate::convert::{byte_offset_to_position, to_lsp_diagnostic};
use crate::position_index::{build_position_index, PositionIndex};

/// Cached compilation result for a file.
#[derive(Debug)]
pub struct CompilationResult {
    /// Source text (for cache validation)
    pub source: String,
    /// Parsed AST declarations (retained for future use)
    pub _decls: Vec<Decl>,
    /// Type-checked program
    pub typed: TypedProgram,
    /// Original diagnostics (before LSP conversion)
    pub diagnostics: Vec<rask_diagnostics::Diagnostic>,
    /// Position index for fast lookups
    pub position_index: PositionIndex,
}

#[derive(Debug)]
pub struct Backend {
    pub client: Client,
    pub documents: RwLock<HashMap<Url, String>>,
    /// Cached compilation results
    pub compiled: RwLock<HashMap<Url, CompilationResult>>,
    /// Workspace root URI (set during initialization)
    pub root_uri: RwLock<Option<Url>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            compiled: RwLock::new(HashMap::new()),
            root_uri: RwLock::new(None),
        }
    }

    pub async fn publish_diagnostics(&self, uri: Url, text: &str) {
        // Analyze and get diagnostics
        let diagnostics = self.analyze_and_cache(&uri, text);

        // Convert to LSP format
        let lsp_diagnostics: Vec<Diagnostic> = diagnostics
            .iter()
            .map(|d| to_lsp_diagnostic(text, &uri, d))
            .collect();

        self.client
            .publish_diagnostics(uri, lsp_diagnostics, None)
            .await;
    }

    /// Analyze source and return diagnostics.
    pub fn analyze_and_cache(&self, uri: &Url, source: &str) -> Vec<rask_diagnostics::Diagnostic> {
        let mut rask_diagnostics = Vec::new();

        // Run lexer - collect all errors
        let mut lexer = Lexer::new(source);
        let lex_result = lexer.tokenize();

        // Deduplicate adjacent lex errors
        let mut last_lex_line: Option<u32> = None;
        for error in &lex_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_lex_line != Some(line) {
                rask_diagnostics.push(error.to_diagnostic());
                last_lex_line = Some(line);
            }
        }

        // Run parser even if lexer had errors
        let mut parser = Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();

        // Deduplicate parse errors
        let mut last_parse_line: Option<u32> = None;
        for error in &parse_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_parse_line != Some(line) {
                rask_diagnostics.push(error.to_diagnostic());
                last_parse_line = Some(line);
            }
        }

        // Only continue with semantic analysis if parsing succeeded
        if !parse_result.is_ok() {
            return rask_diagnostics;
        }

        // Desugar operators
        rask_desugar::desugar(&mut parse_result.decls);

        // Run name resolution
        let resolved = match rask_resolve::resolve(&parse_result.decls) {
            Ok(r) => r,
            Err(errors) => {
                for error in &errors {
                    rask_diagnostics.push(error.to_diagnostic());
                }
                return rask_diagnostics;
            }
        };

        // Run type checking
        let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
            Ok(t) => t,
            Err(errors) => {
                for error in &errors {
                    rask_diagnostics.push(error.to_diagnostic());
                }
                return rask_diagnostics;
            }
        };

        // Run ownership analysis
        let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
        for error in &ownership_result.errors {
            rask_diagnostics.push(error.to_diagnostic());
        }

        // Build position index for fast lookups
        let mut position_index = build_position_index(&parse_result.decls);
        position_index.finalize();

        let result = CompilationResult {
            source: source.to_string(),
            _decls: parse_result.decls,
            typed,
            diagnostics: rask_diagnostics.clone(),
            position_index,
        };

        // Cache the result (only if successful compilation)
        let mut compiled = self.compiled.write().unwrap();
        compiled.insert(uri.clone(), result);

        rask_diagnostics
    }
}

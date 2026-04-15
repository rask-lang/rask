// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Core backend struct and compilation pipeline.
//!
//! Uses rask-compiler for package detection and shares the same pipeline
//! stages as the CLI. The LSP-specific behavior (continue past errors,
//! filter to current file, editor buffer substitution) wraps around the
//! same functions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use tower_lsp::lsp_types::*;
use tower_lsp::Client;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::Span;
use rask_diagnostics::ToDiagnostic;
use rask_types::TypedProgram;

use crate::convert::{byte_offset_to_position, to_lsp_diagnostic};
use crate::position_index::{build_position_index, PositionIndex};

/// Sibling file info for cross-file navigation.
#[derive(Debug)]
pub struct SiblingFile {
    pub path: PathBuf,
    pub source: String,
}

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
    /// Maps top-level declaration names from sibling files to their source info.
    pub sibling_decl_names: HashMap<String, SiblingFile>,
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
        let diagnostics = self.analyze_and_cache(&uri, text);

        let lsp_diagnostics: Vec<Diagnostic> = diagnostics
            .iter()
            .map(|d| to_lsp_diagnostic(text, &uri, d))
            .collect();

        self.client
            .publish_diagnostics(uri, lsp_diagnostics, None)
            .await;
    }

    /// Analyze source and return diagnostics.
    ///
    /// Runs the same pipeline stages as the CLI (via rask-compiler types),
    /// fixing previous divergences: now includes desugar_with_diagnostics,
    /// desugar_default_args, comptime cfg elimination, correct stdlib decls,
    /// and effect analysis.
    pub fn analyze_and_cache(&self, uri: &Url, source: &str) -> Vec<rask_diagnostics::Diagnostic> {
        let mut diags = Vec::new();

        // --- Lex (collect all errors, deduplicate by line) ---
        let mut lexer = rask_lexer::Lexer::new(source);
        let lex_result = lexer.tokenize();

        let mut last_lex_line: Option<u32> = None;
        for error in &lex_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_lex_line != Some(line) {
                diags.push(error.to_diagnostic());
                last_lex_line = Some(line);
            }
        }

        // --- Parse (continue even with lex errors) ---
        let mut parser = rask_parser::Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();

        let mut last_parse_line: Option<u32> = None;
        for error in &parse_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_parse_line != Some(line) {
                diags.push(error.to_diagnostic());
                last_parse_line = Some(line);
            }
        }

        if !parse_result.is_ok() {
            return diags;
        }

        // --- Comptime cfg elimination (CC1) — previously missing from LSP ---
        let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
        rask_comptime::eliminate_comptime_if(&mut parse_result.decls, &cfg);

        // --- Desugar — now uses desugar_with_diagnostics + default args ---
        let desugar_errors = rask_desugar::desugar_with_diagnostics(&mut parse_result.decls);
        rask_desugar::desugar_default_args(&mut parse_result.decls);
        for e in &desugar_errors {
            diags.push(
                rask_diagnostics::Diagnostic::error(e.message.clone())
                    .with_code("E0338")
                    .with_primary(e.span, "variant needs @message(\"...\") annotation"),
            );
        }

        // Record current file's decl span ranges for diagnostic filtering.
        let current_file_spans: Vec<Span> = parse_result.decls.iter().map(|d| d.span).collect();

        // Detect package context using rask-compiler (shared implementation).
        let file_path_str = uri.to_file_path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let pkg_ctx = rask_compiler::detect_package(&file_path_str);
        let sibling_decl_names = if let Some(ref ctx) = pkg_ctx {
            build_sibling_names(uri, ctx)
        } else {
            HashMap::new()
        };

        let is_stdlib = rask_stdlib::StubRegistry::is_stdlib_path(uri.path());
        let resolved = if is_stdlib {
            match rask_resolve::resolve_stdlib(&parse_result.decls) {
                Ok(r) => r,
                Err(errors) => {
                    for error in &errors {
                        diags.push(error.to_diagnostic());
                    }
                    return diags;
                }
            }
        } else if let Some(ref ctx) = pkg_ctx {
            // Multi-file package: use sibling decls from the package registry but
            // replace the current file's decls with our freshly parsed version
            // (editor buffer may differ from disk).
            let file_path = uri.to_file_path().unwrap_or_default();
            let mut sibling_decls: Vec<Decl> = ctx.registry.get(ctx.root_id)
                .map(|pkg| {
                    pkg.files.iter()
                        .filter(|f| f.path != file_path)
                        .flat_map(|f| f.decls.clone())
                        .collect()
                })
                .unwrap_or_default();
            // Desugar siblings with full pipeline too
            rask_desugar::desugar_with_diagnostics(&mut sibling_decls);
            rask_desugar::desugar_default_args(&mut sibling_decls);
            parse_result.decls.extend(sibling_decls);

            match rask_resolve::resolve_package_with_cfg(
                &parse_result.decls,
                &ctx.registry,
                ctx.root_id,
                cfg.to_cfg_values(),
            ) {
                Ok(r) => r,
                Err(errors) => {
                    for error in &errors {
                        let diag = error.to_diagnostic();
                        if is_current_file_diagnostic(&diag, &current_file_spans) {
                            diags.push(diag);
                        }
                    }
                    return diags;
                }
            }
        } else {
            // Single-file mode — use resolve_with_cfg for consistency with CLI
            match rask_resolve::resolve_with_cfg(&parse_result.decls, cfg.to_cfg_values()) {
                Ok(r) => r,
                Err(errors) => {
                    for error in &errors {
                        diags.push(error.to_diagnostic());
                    }
                    return diags;
                }
            }
        };

        // Stdlib stubs are signatures, not real code — skip semantic analysis
        if is_stdlib {
            return diags;
        }

        // --- Typecheck (lenient — returns partial TypedProgram + errors
        //     so ownership/effects still run for full diagnostic coverage) ---
        let stdlib_decls = rask_stdlib::StubRegistry::typecheck_decls();
        let (typed, type_errors) =
            rask_types::typecheck_with_stdlib_lenient(resolved, &parse_result.decls, &stdlib_decls);
        for error in &type_errors {
            let diag = error.to_diagnostic();
            if is_current_file_diagnostic(&diag, &current_file_spans) {
                diags.push(diag);
            }
        }

        // --- Ownership (non-blocking — accumulate and continue) ---
        let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
        for error in &ownership_result.errors {
            let diag = error.to_diagnostic();
            if is_current_file_diagnostic(&diag, &current_file_spans) {
                diags.push(diag);
            }
        }

        // --- Effect analysis — previously missing from LSP ---
        let (effects, effect_warnings) = rask_effects::infer_effects(&parse_result.decls);
        for w in &effect_warnings {
            let d = rask_diagnostics::Diagnostic::warning(&w.message)
                .with_code(w.code)
                .with_primary(w.span, format!("`{}` has IO effect", w.callee_name));
            if is_current_file_diagnostic(&d, &current_file_spans) {
                diags.push(d);
            }
        }

        let frozen_diagnostics = rask_effects::frozen::check(&parse_result.decls, &effects);
        for fd in &frozen_diagnostics {
            let d = if fd.is_error {
                rask_diagnostics::Diagnostic::error(&fd.message)
            } else {
                rask_diagnostics::Diagnostic::warning(&fd.message)
            };
            let d = d.with_code(fd.code).with_primary(fd.span, "");
            if is_current_file_diagnostic(&d, &current_file_spans) {
                diags.push(d);
            }
        }

        // Build position index for fast lookups
        let mut position_index = build_position_index(&parse_result.decls);
        position_index.finalize();

        let result = CompilationResult {
            source: source.to_string(),
            _decls: parse_result.decls,
            typed,
            diagnostics: diags.clone(),
            position_index,
            sibling_decl_names,
        };

        let mut compiled = self.compiled.write().unwrap();
        compiled.insert(uri.clone(), result);

        diags
    }
}

/// Build sibling declaration name mapping for cross-file navigation.
fn build_sibling_names(uri: &Url, ctx: &rask_compiler::PackageContext) -> HashMap<String, SiblingFile> {
    let file_path = match uri.to_file_path() {
        Ok(p) => p,
        Err(_) => return HashMap::new(),
    };
    let pkg = match ctx.registry.get(ctx.root_id) {
        Some(p) => p,
        None => return HashMap::new(),
    };

    let mut names = HashMap::new();
    for file in &pkg.files {
        if file.path == file_path {
            continue;
        }
        for decl in &file.decls {
            let name = match &decl.kind {
                DeclKind::Fn(f) => Some(f.name.clone()),
                DeclKind::Struct(s) => Some(s.name.clone()),
                DeclKind::Enum(e) => Some(e.name.clone()),
                DeclKind::Trait(t) => Some(t.name.clone()),
                DeclKind::Const(c) => Some(c.name.clone()),
                DeclKind::Union(u) => Some(u.name.clone()),
                _ => None,
            };
            if let Some(name) = name {
                names.entry(name).or_insert_with(|| SiblingFile {
                    path: file.path.clone(),
                    source: file.source.clone(),
                });
            }
        }
    }
    names
}

/// Check if a diagnostic's primary span falls within one of the current file's decl spans.
fn is_current_file_diagnostic(diag: &rask_diagnostics::Diagnostic, current_file_spans: &[Span]) -> bool {
    let primary_span = match diag.primary_span() {
        Some(s) => s,
        None => return true,
    };
    if current_file_spans.is_empty() {
        return true;
    }
    current_file_spans.iter().any(|decl_span| {
        primary_span.start >= decl_span.start && primary_span.end <= decl_span.end
    })
}

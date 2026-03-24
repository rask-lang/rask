// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Core backend struct and compilation pipeline.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use tower_lsp::lsp_types::*;
use tower_lsp::Client;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::Span;
use rask_diagnostics::ToDiagnostic;
use rask_lexer::Lexer;
use rask_parser::Parser;
use rask_resolve::PackageRegistry;
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

        // Record current file's decl span ranges for diagnostic filtering.
        let current_file_spans: Vec<Span> = parse_result.decls.iter().map(|d| d.span).collect();

        // Detect package context and resolve accordingly.
        let pkg_ctx = detect_package_context(uri);
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
                        rask_diagnostics.push(error.to_diagnostic());
                    }
                    return rask_diagnostics;
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
            rask_desugar::desugar(&mut sibling_decls);
            parse_result.decls.extend(sibling_decls);

            match rask_resolve::resolve_package(&parse_result.decls, &ctx.registry, ctx.root_id) {
                Ok(r) => r,
                Err(errors) => {
                    for error in &errors {
                        let diag = error.to_diagnostic();
                        if is_current_file_diagnostic(&diag, &current_file_spans) {
                            rask_diagnostics.push(diag);
                        }
                    }
                    return rask_diagnostics;
                }
            }
        } else {
            // Single-file mode
            match rask_resolve::resolve(&parse_result.decls) {
                Ok(r) => r,
                Err(errors) => {
                    for error in &errors {
                        rask_diagnostics.push(error.to_diagnostic());
                    }
                    return rask_diagnostics;
                }
            }
        };

        // Stdlib stubs are signatures, not real code — skip semantic analysis
        if is_stdlib {
            return rask_diagnostics;
        }

        // Run type checking (register stdlib types so methods like Request.path() resolve)
        let stdlib_decls = rask_stdlib::StubRegistry::compilable_decls();
        let typed = match rask_types::typecheck_with_stdlib(resolved, &parse_result.decls, &stdlib_decls) {
            Ok(t) => t,
            Err(errors) => {
                for error in &errors {
                    let diag = error.to_diagnostic();
                    if is_current_file_diagnostic(&diag, &current_file_spans) {
                        rask_diagnostics.push(diag);
                    }
                }
                return rask_diagnostics;
            }
        };

        // Run ownership analysis
        let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
        for error in &ownership_result.errors {
            let diag = error.to_diagnostic();
            if is_current_file_diagnostic(&diag, &current_file_spans) {
                rask_diagnostics.push(diag);
            }
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
            sibling_decl_names,
        };

        // Cache the result (only if successful compilation)
        let mut compiled = self.compiled.write().unwrap();
        compiled.insert(uri.clone(), result);

        rask_diagnostics
    }
}

/// Package context discovered from the file system.
struct PackageContext {
    registry: PackageRegistry,
    root_id: rask_resolve::PackageId,
}

/// Detect whether a URI belongs to a multi-file package.
/// Walks up from the file looking for `build.rk`, then uses PackageRegistry::discover.
fn detect_package_context(uri: &Url) -> Option<PackageContext> {
    let file_path = uri.to_file_path().ok()?;
    let dir = file_path.parent()?;

    // Walk up looking for build.rk
    let mut search_dir = dir.to_path_buf();
    loop {
        if search_dir.join("build.rk").is_file() {
            let mut registry = PackageRegistry::new();
            let root_id = registry.discover(&search_dir).ok()?;
            return Some(PackageContext { registry, root_id });
        }
        if search_dir.join(".git").exists() {
            return None;
        }
        match search_dir.parent() {
            Some(parent) if parent != search_dir => {
                search_dir = parent.to_path_buf();
            }
            _ => return None,
        }
    }
}

/// Build sibling declaration name mapping for cross-file navigation.
fn build_sibling_names(uri: &Url, ctx: &PackageContext) -> HashMap<String, SiblingFile> {
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
/// Sibling files are parsed independently (spans start from 0), so we use the current file's
/// known decl ranges to filter out diagnostics that originated from sibling code.
fn is_current_file_diagnostic(diag: &rask_diagnostics::Diagnostic, current_file_spans: &[Span]) -> bool {
    let primary_span = match diag.primary_span() {
        Some(s) => s,
        None => return true, // No span — keep it
    };
    // If no decls were parsed from the current file, keep all diagnostics
    if current_file_spans.is_empty() {
        return true;
    }
    current_file_spans.iter().any(|decl_span| {
        primary_span.start >= decl_span.start && primary_span.end <= decl_span.end
    })
}

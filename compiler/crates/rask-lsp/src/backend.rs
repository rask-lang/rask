// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Backend state and the analysis pipeline.
//!
//! The backend owns:
//!   - open documents (source text keyed by URI)
//!   - the most recent compilation result per document
//!   - the workspace root (for stub-file goto)
//!
//! All locks are `tokio::sync::RwLock` — blocking in an async handler would
//! stall the shared tokio executor. The CPU-heavy analysis pass runs via
//! `spawn_blocking` and is wrapped in `catch_unwind` so a compiler panic
//! reports as a diagnostic instead of killing the server.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::RwLock;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::Span;
use rask_diagnostics::ToDiagnostic;
use rask_types::TypedProgram;

use crate::convert::{to_lsp_diagnostic, LineIndex};
use crate::position_index::{build_position_index, PositionIndex};

/// How long we wait for further keystrokes before re-analyzing.
///
/// 120 ms is enough that a fast typist (~8 chars/sec) gets at most one
/// analysis per word, but short enough that the diagnostics feel live.
const DEBOUNCE_MS: u64 = 120;

/// Sibling file info for cross-file navigation.
#[derive(Debug, Clone)]
pub struct SiblingFile {
    pub path: PathBuf,
    pub source: String,
}

/// Cached compilation result for a document.
pub struct CompilationResult {
    /// Source text. Held so later lookups (hover, completion) see the same
    /// bytes we analyzed, even if the document has moved on.
    pub source: String,
    /// UTF-16 line index for this source snapshot.
    pub line_index: LineIndex,
    /// Parsed and desugared declarations (current file + siblings appended).
    pub decls: Vec<Decl>,
    /// How many of the leading `decls` belong to the current file.
    /// Sibling decls are appended after this index.
    pub current_file_decl_count: usize,
    /// Span range of each *current-file* declaration, used to filter
    /// diagnostics produced by the whole-package pipeline down to just
    /// the ones the editor buffer is responsible for.
    pub current_file_spans: Vec<Span>,
    pub typed: TypedProgram,
    pub diagnostics: Vec<rask_diagnostics::Diagnostic>,
    pub position_index: PositionIndex,
    pub sibling_decl_names: HashMap<String, SiblingFile>,
}

pub struct Backend {
    pub client: Client,
    pub documents: RwLock<HashMap<Url, DocState>>,
    pub compiled: RwLock<HashMap<Url, Arc<CompilationResult>>>,
    pub root_uri: RwLock<Option<Url>>,
    /// Monotonic analysis generation counter — used to cancel outdated
    /// debounced analyses when a newer keystroke arrives.
    pub generation: AtomicU64,
}

/// Live document state (most recent text + client's version number).
#[derive(Debug, Clone)]
pub struct DocState {
    pub text: String,
    pub version: i32,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            compiled: RwLock::new(HashMap::new()),
            root_uri: RwLock::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Schedules diagnostics publication after a short debounce window.
    /// Returns immediately; analysis runs on a blocking worker.
    pub async fn schedule_analysis(self: Arc<Self>, uri: Url) {
        let gen_at_call = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let backend = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)).await;
            // Bail if another keystroke arrived during the sleep.
            if backend.generation.load(Ordering::SeqCst) != gen_at_call {
                return;
            }
            backend.analyze_now(uri).await;
        });
    }

    /// Runs analysis immediately (used by did_save and initial open).
    pub async fn analyze_now(self: Arc<Self>, uri: Url) {
        let Some(doc) = self.documents.read().await.get(&uri).cloned() else {
            return;
        };
        let backend = self.clone();
        let uri_for_task = uri.clone();
        let doc_clone = doc.clone();

        // Heavy work on a blocking thread so we don't stall the async runtime.
        let analysis = tokio::task::spawn_blocking(move || {
            let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
                run_pipeline(&uri_for_task, &doc_clone.text, doc_clone.version)
            }));
            match res {
                Ok(result) => result,
                Err(_) => PipelineOutput::panic(&doc_clone.text, doc_clone.version),
            }
        })
        .await;

        let output = match analysis {
            Ok(o) => o,
            Err(_) => PipelineOutput::panic(&doc.text, doc.version),
        };

        // Publish diagnostics regardless of whether the pipeline had errors.
        // Cap to avoid flooding the editor — root causes come first,
        // residual cascades are noise.
        let lsp_diags: Vec<Diagnostic> = output
            .diagnostics
            .iter()
            .take(MAX_LSP_DIAGNOSTICS)
            .map(|d| to_lsp_diagnostic(&output.line_index, &output.source, &uri, d))
            .collect();
        self.client
            .publish_diagnostics(uri.clone(), lsp_diags, Some(output.version))
            .await;

        // Only replace the cache if the analysis succeeded fully.
        if let Some(result) = output.result {
            backend.compiled.write().await.insert(uri, Arc::new(result));
        }
    }

    /// Read the cached compilation for `uri` (shared Arc — cheap to clone).
    pub async fn get_compiled(&self, uri: &Url) -> Option<Arc<CompilationResult>> {
        self.compiled.read().await.get(uri).cloned()
    }

    /// Read the current live text (most recent keystrokes).
    pub async fn get_text(&self, uri: &Url) -> Option<String> {
        self.documents.read().await.get(uri).map(|d| d.text.clone())
    }
}

/// All outputs of one analysis pass.
struct PipelineOutput {
    version: i32,
    source: String,
    line_index: LineIndex,
    diagnostics: Vec<rask_diagnostics::Diagnostic>,
    /// Full CompilationResult when every stage succeeded far enough to produce
    /// one. None means resolve failed — we still report diagnostics but leave
    /// the last good cache in place.
    result: Option<CompilationResult>,
}

impl PipelineOutput {
    fn panic(source: &str, version: i32) -> Self {
        let line_index = LineIndex::new(source);
        let diag = rask_diagnostics::Diagnostic::error(
            "rask-lsp: analyzer panicked (this is a compiler bug)",
        )
        .with_code("E9999")
        .with_primary(Span::new(0, 0), "");
        Self {
            version,
            source: source.to_string(),
            line_index,
            diagnostics: vec![diag],
            result: None,
        }
    }
}

/// Run lex → parse → desugar → resolve → typecheck → ownership → effects.
///
/// Never panics on its own input as long as the compiler doesn't; the caller
/// wraps this in `catch_unwind` as a backstop.
fn run_pipeline(uri: &Url, source: &str, version: i32) -> PipelineOutput {
    let line_index = LineIndex::new(source);
    let mut diags = Vec::new();

    // --- Lex ---
    let mut lexer = rask_lexer::Lexer::new(source);
    let lex_result = lexer.tokenize();
    dedupe_by_line(&lex_result.errors, |e| e.span.start, &line_index, &mut diags, |e| e.to_diagnostic());

    // --- Parse ---
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();
    dedupe_by_line(&parse_result.errors, |e| e.span.start, &line_index, &mut diags, |e| e.to_diagnostic());

    // Don't bail on parse errors — the parser recovers and the recovered
    // decls flow through resolve/typecheck so hover still works.

    // --- Comptime cfg elimination ---
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    rask_comptime::eliminate_comptime_if(&mut parse_result.decls, &cfg);

    // --- Desugar (operators + default/named args) ---
    let desugar_errors = rask_desugar::desugar_with_diagnostics(&mut parse_result.decls);
    rask_desugar::desugar_default_args(&mut parse_result.decls);
    for e in &desugar_errors {
        diags.push(
            rask_diagnostics::Diagnostic::error(e.message.clone())
                .with_code("E0338")
                .with_primary(e.span, "variant needs @message(\"...\") annotation"),
        );
    }

    let current_file_spans: Vec<Span> = parse_result.decls.iter().map(|d| d.span).collect();
    let current_file_decl_count = parse_result.decls.len();

    // --- Package context (if this file lives under a build.rk tree) ---
    let file_path_str = uri.to_file_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let pkg_ctx = rask_compiler::detect_package(&file_path_str);
    let sibling_decl_names = if let Some(ref ctx) = pkg_ctx {
        build_sibling_names(uri, ctx)
    } else {
        HashMap::new()
    };

    // --- Resolve ---
    let is_stdlib = rask_stdlib::StubRegistry::is_stdlib_path(uri.path());
    let resolved = if is_stdlib {
        match rask_resolve::resolve_stdlib(&parse_result.decls) {
            Ok(r) => r,
            Err(errors) => {
                for error in &errors {
                    diags.push(error.to_diagnostic());
                }
                return PipelineOutput {
                    version,
                    source: source.to_string(),
                    line_index,
                    diagnostics: diags,
                    result: None,
                };
            }
        }
    } else if let Some(ref ctx) = pkg_ctx {
        let file_path = uri.to_file_path().unwrap_or_default();
        let mut sibling_decls: Vec<Decl> = ctx.registry.get(ctx.root_id)
            .map(|pkg| {
                pkg.files.iter()
                    .filter(|f| f.path != file_path)
                    .flat_map(|f| f.decls.clone())
                    .collect()
            })
            .unwrap_or_default();
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
                return PipelineOutput {
                    version,
                    source: source.to_string(),
                    line_index,
                    diagnostics: diags,
                    result: None,
                };
            }
        }
    } else {
        match rask_resolve::resolve_with_cfg(&parse_result.decls, cfg.to_cfg_values()) {
            Ok(r) => r,
            Err(errors) => {
                for error in &errors {
                    diags.push(error.to_diagnostic());
                }
                return PipelineOutput {
                    version,
                    source: source.to_string(),
                    line_index,
                    diagnostics: diags,
                    result: None,
                };
            }
        }
    };

    if is_stdlib {
        // Stdlib stubs are signatures only — no further analysis.
        return PipelineOutput {
            version,
            source: source.to_string(),
            line_index,
            diagnostics: diags,
            result: None,
        };
    }

    // --- Typecheck (lenient so ownership/effects still run) ---
    let stdlib_decls = rask_stdlib::StubRegistry::typecheck_decls();
    let (mut typed, type_errors) =
        rask_types::typecheck_with_stdlib_lenient(resolved, &parse_result.decls, &stdlib_decls);
    for error in &type_errors {
        let diag = error.to_diagnostic();
        if is_current_file_diagnostic(&diag, &current_file_spans) {
            diags.push(diag);
        }
    }

    // --- Ownership ---
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    for error in &ownership_result.errors {
        let diag = error.to_diagnostic();
        if is_current_file_diagnostic(&diag, &current_file_spans) {
            diags.push(diag);
        }
    }

    // --- Effects (IO propagation + frozen check) ---
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

    // Only index current-file decls — sibling byte offsets would collide.
    let mut position_index = build_position_index(&parse_result.decls[..current_file_decl_count]);
    position_index.finalize();

    // Strip sibling entries from span_types — file_id 0 is the current file
    // in LSP context (parsed fresh by Parser::new which defaults to file_id 0).
    typed.span_types.retain(|&(_, _, file_id), _| file_id == 0);

    let result = CompilationResult {
        source: source.to_string(),
        line_index: line_index.clone(),
        decls: parse_result.decls,
        current_file_decl_count,
        current_file_spans,
        typed,
        diagnostics: diags.clone(),
        position_index,
        sibling_decl_names,
    };

    PipelineOutput {
        version,
        source: source.to_string(),
        line_index,
        diagnostics: diags,
        result: Some(result),
    }
}

/// Cap diagnostics to avoid flooding the editor. Root-cause errors come
/// first; cascading errors (from poison propagation) are already filtered
/// by the type checker, but residual noise can still exceed what's useful.
const MAX_LSP_DIAGNOSTICS: usize = 20;

/// Deduplicate errors that land on the same line — they tend to cascade
/// after a single real problem, and a wall of them is noise.
fn dedupe_by_line<E, F, G>(
    errors: &[E],
    span_start: F,
    line_index: &LineIndex,
    out: &mut Vec<rask_diagnostics::Diagnostic>,
    to_diag: G,
)
where
    F: Fn(&E) -> usize,
    G: Fn(&E) -> rask_diagnostics::Diagnostic,
{
    let mut last_line: Option<u32> = None;
    for e in errors {
        let line = line_index.line_of_offset(span_start(e));
        if last_line != Some(line) {
            out.push(to_diag(e));
            last_line = Some(line);
        }
    }
}

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

fn is_current_file_diagnostic(diag: &rask_diagnostics::Diagnostic, _current_file_spans: &[Span]) -> bool {
    let primary_span = match diag.primary_span() {
        Some(s) => s,
        None => return true,
    };
    // Current file is always file_id 0 in LSP context (parsed by Parser::new
    // which defaults to file_id 0). Sibling decls have file_id > 0 from
    // package parsing.
    primary_span.file_id == 0
}

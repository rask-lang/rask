// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask Language Server
//!
//! Provides diagnostics for all compilation errors:
//! lexer, parser, resolve, type check, and ownership.

use std::collections::HashMap;
use std::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use rask_diagnostics::{LabelStyle, Severity, ToDiagnostic};
use rask_lexer::Lexer;
use rask_parser::Parser;

#[derive(Debug)]
struct Backend {
    client: Client,
    documents: RwLock<HashMap<Url, String>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
        }
    }

    async fn publish_diagnostics(&self, uri: Url, text: &str) {
        let diagnostics = self.analyze(text, &uri);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    fn analyze(&self, source: &str, uri: &Url) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        // Run lexer - collect all errors
        let mut lexer = Lexer::new(source);
        let lex_result = lexer.tokenize();

        // Deduplicate adjacent lex errors (e.g., ### becomes one error, not three)
        let mut last_lex_line: Option<u32> = None;
        for error in &lex_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_lex_line != Some(line) {
                diagnostics.push(to_lsp_diagnostic(source, uri, &error.to_diagnostic()));
                last_lex_line = Some(line);
            }
        }

        // Run parser even if lexer had errors - it may still produce useful results
        let mut parser = Parser::new(lex_result.tokens);
        let mut parse_result = parser.parse();

        // Deduplicate parse errors - only first error per line
        let mut last_parse_line: Option<u32> = None;
        for error in &parse_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_parse_line != Some(line) {
                diagnostics.push(to_lsp_diagnostic(source, uri, &error.to_diagnostic()));
                last_parse_line = Some(line);
            }
        }

        // Only continue with semantic analysis if parsing succeeded
        if !parse_result.is_ok() {
            return diagnostics;
        }

        // Desugar operators (a + b → a.add(b))
        rask_desugar::desugar(&mut parse_result.decls);

        // Run name resolution
        let resolved = match rask_resolve::resolve(&parse_result.decls) {
            Ok(r) => r,
            Err(errors) => {
                for error in &errors {
                    diagnostics.push(to_lsp_diagnostic(source, uri, &error.to_diagnostic()));
                }
                return diagnostics;
            }
        };

        // Run type checking
        let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
            Ok(t) => t,
            Err(errors) => {
                for error in &errors {
                    diagnostics.push(to_lsp_diagnostic(source, uri, &error.to_diagnostic()));
                }
                return diagnostics;
            }
        };

        // Run ownership analysis
        let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
        for error in &ownership_result.errors {
            diagnostics.push(to_lsp_diagnostic(source, uri, &error.to_diagnostic()));
        }

        diagnostics
    }
}

/// Convert a rask diagnostic to an LSP diagnostic.
fn to_lsp_diagnostic(
    source: &str,
    uri: &Url,
    diag: &rask_diagnostics::Diagnostic,
) -> Diagnostic {
    // Primary span determines the main range
    let primary = diag
        .labels
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .or(diag.labels.first());

    let range = if let Some(label) = primary {
        let start = byte_offset_to_position(source, label.span.start);
        let end = byte_offset_to_position(source, label.span.end);
        Range::new(start, end)
    } else {
        Range::new(Position::new(0, 0), Position::new(0, 0))
    };

    let severity = Some(match diag.severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Note => DiagnosticSeverity::INFORMATION,
    });

    let code = diag
        .code
        .as_ref()
        .map(|c| NumberOrString::String(c.0.clone()));

    // Build message: main message + primary label + notes + help
    let mut message = diag.message.clone();

    if let Some(label) = primary {
        if let Some(ref msg) = label.message {
            message = format!("{}: {}", message, msg);
        }
    }

    for note in &diag.notes {
        message = format!("{}\n\nnote: {}", message, note);
    }

    if let Some(ref help) = diag.help {
        message = format!("{}\n\nhelp: {}", message, help.message);
    }

    // Secondary labels become related information
    let related_information: Vec<DiagnosticRelatedInformation> = diag
        .labels
        .iter()
        .filter(|l| l.style == LabelStyle::Secondary)
        .map(|l| {
            let start = byte_offset_to_position(source, l.span.start);
            let end = byte_offset_to_position(source, l.span.end);
            DiagnosticRelatedInformation {
                location: Location {
                    uri: uri.clone(),
                    range: Range::new(start, end),
                },
                message: l.message.clone().unwrap_or_default(),
            }
        })
        .collect();

    Diagnostic {
        range,
        severity,
        code,
        code_description: None,
        source: Some("rask".to_string()),
        message,
        related_information: if related_information.is_empty() {
            None
        } else {
            Some(related_information)
        },
        tags: None,
        data: None,
    }
}

fn byte_offset_to_position(source: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    Position::new(line, col)
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "rask-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Rask language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        {
            let mut docs = self.documents.write().unwrap();
            docs.insert(uri.clone(), text.clone());
        }

        self.publish_diagnostics(uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;

        // With FULL sync, we get the entire document
        if let Some(change) = params.content_changes.into_iter().last() {
            let text = change.text;
            {
                let mut docs = self.documents.write().unwrap();
                docs.insert(uri.clone(), text.clone());
            }
            self.publish_diagnostics(uri, &text).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = {
            let docs = self.documents.read().unwrap();
            docs.get(&uri).cloned()
        };
        if let Some(text) = text {
            self.publish_diagnostics(uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut docs = self.documents.write().unwrap();
            docs.remove(&uri);
        }
        // Clear diagnostics
        self.client.publish_diagnostics(uri, vec![], None).await;
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

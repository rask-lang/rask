//! Rask Language Server
//!
//! Provides diagnostics for all compilation errors:
//! lexer, parser, resolve, type check, and ownership.

use std::collections::HashMap;
use std::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use rask_ast::Span;
use rask_lexer::{LexError, Lexer};
use rask_ownership::{OwnershipError, OwnershipErrorKind};
use rask_parser::{ParseError, Parser};
use rask_resolve::ResolveError;
use rask_types::TypeError;

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
            // Only report first error per line for lexer errors
            if last_lex_line != Some(line) {
                diagnostics.push(lex_error_to_diagnostic(source, error));
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
                diagnostics.push(parse_error_to_diagnostic(source, error));
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
                    diagnostics.push(resolve_error_to_diagnostic(source, uri, error));
                }
                return diagnostics;
            }
        };

        // Run type checking
        let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
            Ok(t) => t,
            Err(errors) => {
                for error in &errors {
                    diagnostics.push(type_error_to_diagnostic(source, error));
                }
                return diagnostics;
            }
        };

        // Run ownership analysis
        let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
        for error in &ownership_result.errors {
            diagnostics.push(ownership_error_to_diagnostic(source, uri, error));
        }

        diagnostics
    }
}

fn lex_error_to_diagnostic(source: &str, error: &LexError) -> Diagnostic {
    let start = byte_offset_to_position(source, error.span.start);
    let end = byte_offset_to_position(source, error.span.end);

    let mut message = error.message.clone();
    if let Some(hint) = &error.hint {
        message = format!("{}\n\nHint: {}", message, hint);
    }

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("rask".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn parse_error_to_diagnostic(source: &str, error: &ParseError) -> Diagnostic {
    let start = byte_offset_to_position(source, error.span.start);
    let end = byte_offset_to_position(source, error.span.end);

    let mut message = error.message.clone();
    if let Some(hint) = &error.hint {
        message = format!("{}\n\nHint: {}", message, hint);
    }

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("rask".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

fn resolve_error_to_diagnostic(source: &str, uri: &Url, error: &ResolveError) -> Diagnostic {
    let start = byte_offset_to_position(source, error.span.start);
    let end = byte_offset_to_position(source, error.span.end);

    // Check for related location (duplicate definition has previous span)
    let related_information = match &error.kind {
        rask_resolve::ResolveErrorKind::DuplicateDefinition { previous, .. } => {
            let prev_start = byte_offset_to_position(source, previous.start);
            let prev_end = byte_offset_to_position(source, previous.end);
            Some(vec![DiagnosticRelatedInformation {
                location: Location {
                    uri: uri.clone(),
                    range: Range::new(prev_start, prev_end),
                },
                message: "previously defined here".to_string(),
            }])
        }
        _ => None,
    };

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("rask".to_string()),
        message: error.kind.to_string(),
        related_information,
        tags: None,
        data: None,
    }
}

fn type_error_to_diagnostic(source: &str, error: &TypeError) -> Diagnostic {
    let span = get_type_error_span(error);
    let start = byte_offset_to_position(source, span.start);
    let end = byte_offset_to_position(source, span.end);

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("rask".to_string()),
        message: error.to_string(),
        related_information: None,
        tags: None,
        data: None,
    }
}

fn get_type_error_span(error: &TypeError) -> Span {
    match error {
        TypeError::Mismatch { span, .. } => *span,
        TypeError::ArityMismatch { span, .. } => *span,
        TypeError::NotCallable { span, .. } => *span,
        TypeError::NoSuchField { span, .. } => *span,
        TypeError::NoSuchMethod { span, .. } => *span,
        TypeError::InfiniteType { span, .. } => *span,
        TypeError::CannotInfer { span } => *span,
        _ => Span::new(0, 0),
    }
}

fn ownership_error_to_diagnostic(source: &str, uri: &Url, error: &OwnershipError) -> Diagnostic {
    let start = byte_offset_to_position(source, error.span.start);
    let end = byte_offset_to_position(source, error.span.end);

    // Build related information for errors with secondary locations
    let related_information = match &error.kind {
        OwnershipErrorKind::UseAfterMove { moved_at, .. } => {
            let mov_start = byte_offset_to_position(source, moved_at.start);
            let mov_end = byte_offset_to_position(source, moved_at.end);
            Some(vec![DiagnosticRelatedInformation {
                location: Location {
                    uri: uri.clone(),
                    range: Range::new(mov_start, mov_end),
                },
                message: "value was moved here".to_string(),
            }])
        }
        OwnershipErrorKind::BorrowConflict { existing_span, .. } => {
            let ex_start = byte_offset_to_position(source, existing_span.start);
            let ex_end = byte_offset_to_position(source, existing_span.end);
            Some(vec![DiagnosticRelatedInformation {
                location: Location {
                    uri: uri.clone(),
                    range: Range::new(ex_start, ex_end),
                },
                message: "conflicting access here".to_string(),
            }])
        }
        OwnershipErrorKind::MutateWhileBorrowed { borrow_span, .. } => {
            let br_start = byte_offset_to_position(source, borrow_span.start);
            let br_end = byte_offset_to_position(source, borrow_span.end);
            Some(vec![DiagnosticRelatedInformation {
                location: Location {
                    uri: uri.clone(),
                    range: Range::new(br_start, br_end),
                },
                message: "borrowed here".to_string(),
            }])
        }
        _ => None,
    };

    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("rask".to_string()),
        message: error.kind.to_string(),
        related_information,
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

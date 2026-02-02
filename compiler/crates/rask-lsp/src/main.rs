//! Rask Language Server
//!
//! Provides diagnostics for lexer and parser errors.

use std::collections::HashMap;
use std::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use rask_lexer::{LexError, Lexer};
use rask_parser::{ParseError, Parser};

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
        let diagnostics = self.analyze(text);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    fn analyze(&self, source: &str) -> Vec<Diagnostic> {
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
        let parse_result = parser.parse();

        // Deduplicate parse errors - only first error per line
        let mut last_parse_line: Option<u32> = None;
        for error in &parse_result.errors {
            let line = byte_offset_to_position(source, error.span.start).line;
            if last_parse_line != Some(line) {
                diagnostics.push(parse_error_to_diagnostic(source, error));
                last_parse_line = Some(line);
            }
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

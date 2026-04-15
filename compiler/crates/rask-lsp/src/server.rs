// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! LanguageServer trait implementation.
//!
//! Handler methods run on the tokio executor. Anything that might panic
//! (cursor on a char boundary, a compiler bug walking the AST) is caught
//! in `catch_unwind` via `safe_handler` — dropping a single request is
//! fine; taking the server down is not.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::LanguageServer;

use crate::backend::{Backend, CompilationResult, DocState};
use crate::convert::{ranges_overlap, to_lsp_diagnostic};
use crate::incremental::apply_change;
use crate::{hover, inlay_hints, references, semantic_tokens, signature_help, symbols};

/// Run `f` inside `catch_unwind`, returning `Ok(None)` on panic. Intended
/// for query handlers whose failure should not bring down the server.
fn safe_handler<T, F: FnOnce() -> T>(f: F) -> std::result::Result<T, ()>
where
    T: Default,
{
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|_| ())
}

#[tower_lsp::async_trait]
impl LanguageServer for BackendHandle {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(root_uri) = params.root_uri {
            *self.inner.root_uri.write().await = Some(root_uri);
        }

        // Prefer UTF-16 — it's the default VS Code negotiates, but we also
        // accept UTF-8 if the client requests it. The compiler gets whatever
        // encoding was accepted via the negotiated capability below.
        let negotiated_encoding = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .and_then(|encs| {
                if encs.iter().any(|e| e == &PositionEncodingKind::UTF16) {
                    Some(PositionEncodingKind::UTF16)
                } else {
                    encs.first().cloned()
                }
            })
            .unwrap_or(PositionEncodingKind::UTF16);

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(negotiated_encoding),
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(
                        [".", ":", "("].iter().map(|s| s.to_string()).collect(),
                    ),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: Default::default(),
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Left(true)),
                inlay_hint_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: semantic_tokens::legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: Some(false),
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "rask-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.inner
            .client
            .log_message(MessageType::INFO, "Rask language server initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        self.inner
            .documents
            .write()
            .await
            .insert(uri.clone(), DocState { text, version });

        // Open happens once per file — analyze immediately rather than
        // waiting for debounce.
        self.inner.clone().analyze_now(uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        let mut docs = self.inner.documents.write().await;
        let Some(state) = docs.get_mut(&uri) else {
            // The client sent change before open — apply as a full replace
            // if we got one content change with no range.
            if let [single] = params.content_changes.as_slice() {
                if single.range.is_none() {
                    docs.insert(
                        uri.clone(),
                        DocState { text: single.text.clone(), version },
                    );
                }
            }
            drop(docs);
            self.inner.clone().schedule_analysis(uri).await;
            return;
        };

        for change in params.content_changes {
            apply_change(&mut state.text, change);
        }
        state.version = version;
        drop(docs);

        self.inner.clone().schedule_analysis(uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.inner.clone().analyze_now(params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.inner.documents.write().await.remove(&uri);
        self.inner.compiled.write().await.remove(&uri);
        self.inner
            .client
            .publish_diagnostics(uri, vec![], None)
            .await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };
        let root = self.inner.root_uri.read().await.clone();

        Ok(safe_handler(|| {
            crate::goto::goto_definition(&uri, position, &cached, root.as_ref())
        })
        .unwrap_or(None))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };

        Ok(safe_handler(|| hover::hover(position, &cached)).unwrap_or(None))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let range = params.range;
        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };

        let mut actions = Vec::new();
        for diag in &cached.diagnostics {
            use rask_diagnostics::LabelStyle;
            let primary = diag.labels.iter()
                .find(|l| l.style == LabelStyle::Primary)
                .or(diag.labels.first());
            let Some(primary_label) = primary else { continue };

            let diag_range = cached.line_index.span_to_range(&cached.source, primary_label.span);
            if !ranges_overlap(diag_range, range) {
                continue;
            }
            let Some(help) = &diag.help else { continue };
            let Some(suggestion) = &help.suggestion else { continue };

            let edit_range = cached.line_index.span_to_range(&cached.source, suggestion.span);
            let text_edit = TextEdit::new(edit_range, suggestion.replacement.clone());

            let mut changes = HashMap::new();
            changes.insert(uri.clone(), vec![text_edit]);
            let workspace_edit = WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            };

            let lsp_diagnostic = to_lsp_diagnostic(&cached.line_index, &cached.source, &uri, diag);
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: help.message.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![lsp_diagnostic]),
                edit: Some(workspace_edit),
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: None,
            }));
        }

        Ok(if actions.is_empty() { None } else { Some(actions) })
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };
        let live_text = self.inner.get_text(&uri).await;

        let is_dot = params
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref())
            == Some(".");

        Ok(safe_handler(|| {
            crate::completion::completion(position, &cached, live_text.as_deref(), is_dot)
        })
        .unwrap_or(None))
    }

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };

        Ok(safe_handler(|| signature_help::signature_help(position, &cached)).unwrap_or(None))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let Some(cached) = self.inner.get_compiled(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(safe_handler(|| symbols::document_symbols(&cached)).unwrap_or(None))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let query = params.query;
        let compiled = self.inner.compiled.read().await;
        let all: Vec<(Url, Arc<CompilationResult>)> = compiled
            .iter()
            .map(|(u, c)| (u.clone(), c.clone()))
            .collect();
        drop(compiled);
        Ok(safe_handler(|| symbols::workspace_symbols(&query, &all)).unwrap_or(None))
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let Some(text) = self.inner.get_text(&uri).await else {
            return Ok(None);
        };
        Ok(safe_handler(|| crate::format::format_document(&text)).unwrap_or(None))
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_decl = params.context.include_declaration;
        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };
        Ok(safe_handler(|| references::references(&uri, position, &cached, include_decl))
            .unwrap_or(None))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;
        let Some(cached) = self.inner.get_compiled(&uri).await else {
            return Ok(None);
        };
        Ok(safe_handler(|| references::rename(&uri, position, &new_name, &cached))
            .unwrap_or(None))
    }

    async fn inlay_hint(
        &self,
        params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        let Some(cached) = self.inner.get_compiled(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(safe_handler(|| Some(inlay_hints::inlay_hints(&cached, params.range))).unwrap_or(None))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let Some(cached) = self.inner.get_compiled(&params.text_document.uri).await else {
            return Ok(None);
        };
        Ok(safe_handler(|| Some(semantic_tokens::tokens(&cached))).unwrap_or(None))
    }
}

/// Handle that implements `LanguageServer` by delegating to an inner `Backend`.
///
/// `tower_lsp` expects the impl to own the state, but we want to pass `Arc`
/// clones into tokio tasks. A light wrapper sidesteps that.
pub struct BackendHandle {
    pub inner: Arc<Backend>,
}

impl BackendHandle {
    pub fn new(inner: Arc<Backend>) -> Self {
        Self { inner }
    }
}

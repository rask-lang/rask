// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! LanguageServer trait implementation.

use std::collections::HashMap;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::LanguageServer;

use rask_diagnostics::LabelStyle;

use crate::backend::Backend;
use crate::convert::{byte_offset_to_position, position_to_offset, ranges_overlap, to_lsp_diagnostic};
use crate::type_format::TypeFormatter;

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    resolve_provider: Some(false),
                    ..Default::default()
                }),
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
        {
            let mut compiled = self.compiled.write().unwrap();
            compiled.remove(&uri);
        }
        // Clear diagnostics
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Use last good compilation
        let compiled = self.compiled.read().unwrap();
        let Some(cached) = compiled.get(uri) else {
            return Ok(None);
        };

        let offset = position_to_offset(&cached.source, position);

        // Find identifier at cursor
        let Some((node_id, _name)) = cached.position_index.ident_at_position(offset) else {
            return Ok(None);
        };

        // Look up symbol for this node
        let Some(&symbol_id) = cached.typed.resolutions.get(&node_id) else {
            return Ok(None);
        };

        // Get symbol definition location
        let Some(symbol) = cached.typed.symbols.get(symbol_id) else {
            return Ok(None);
        };

        // Skip built-in symbols (span = 0..0)
        if symbol.span.start == 0 && symbol.span.end == 0 {
            return Ok(None);
        }

        let def_range = Range::new(
            byte_offset_to_position(&cached.source, symbol.span.start),
            byte_offset_to_position(&cached.source, symbol.span.end),
        );

        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range: def_range,
        })))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Use last good compilation
        let compiled = self.compiled.read().unwrap();
        let Some(cached) = compiled.get(uri) else {
            return Ok(None);
        };

        let offset = position_to_offset(&cached.source, position);

        // Find node at cursor
        let Some(node_id) = cached.position_index.node_at_position(offset) else {
            return Ok(None);
        };

        // Get type for this node
        let Some(ty) = cached.typed.node_types.get(&node_id) else {
            return Ok(None);
        };

        // Format type for display
        let formatter = TypeFormatter::new(&cached.typed.types);
        let type_str = formatter.format(ty);

        // Build hover content
        let mut contents = format!("**Type:** `{}`", type_str);

        // For identifiers, add symbol info
        if let Some((ident_node_id, name)) = cached.position_index.ident_at_position(offset) {
            if ident_node_id == node_id {
                if let Some(&symbol_id) = cached.typed.resolutions.get(&node_id) {
                    if let Some(symbol) = cached.typed.symbols.get(symbol_id) {
                        let kind_str = match symbol.kind {
                            rask_resolve::SymbolKind::Variable { mutable } => {
                                if mutable {
                                    "Variable (mutable)"
                                } else {
                                    "Variable"
                                }
                            }
                            rask_resolve::SymbolKind::Parameter { .. } => "Parameter",
                            rask_resolve::SymbolKind::Function { .. } => "Function",
                            rask_resolve::SymbolKind::Struct { .. } => "Struct",
                            rask_resolve::SymbolKind::Enum { .. } => "Enum",
                            rask_resolve::SymbolKind::Field { .. } => "Field",
                            rask_resolve::SymbolKind::Trait { .. } => "Trait",
                            rask_resolve::SymbolKind::EnumVariant { .. } => "Enum Variant",
                            rask_resolve::SymbolKind::BuiltinType { .. } => "Built-in Type",
                            rask_resolve::SymbolKind::BuiltinFunction { .. } => "Built-in Function",
                            rask_resolve::SymbolKind::BuiltinModule { .. } => "Built-in Module",
                            rask_resolve::SymbolKind::ExternFunction { .. } => "Extern Function",
                            rask_resolve::SymbolKind::ExternalPackage { .. } => "Package",
                        };
                        contents = format!("**{}:** `{}`\n\n**Type:** `{}`", kind_str, name, type_str);
                    }
                }
            }
        }

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: contents,
            }),
            range: None,
        }))
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let range = params.range;

        // Use last good compilation
        let compiled = self.compiled.read().unwrap();
        let Some(cached) = compiled.get(uri) else {
            return Ok(None);
        };
        let source = &cached.source;

        let mut actions = Vec::new();

        // Find diagnostics with suggestions that overlap the requested range
        for diag in &cached.diagnostics {
            // Get diagnostic range
            let diag_primary = diag
                .labels
                .iter()
                .find(|l| l.style == LabelStyle::Primary)
                .or(diag.labels.first());

            let Some(primary_label) = diag_primary else {
                continue;
            };

            let diag_range = Range::new(
                byte_offset_to_position(&source, primary_label.span.start),
                byte_offset_to_position(&source, primary_label.span.end),
            );

            // Check if diagnostic overlaps with requested range
            if !ranges_overlap(diag_range, range) {
                continue;
            }

            // Check if diagnostic has a suggestion
            if let Some(ref help) = diag.help {
                if let Some(ref suggestion) = help.suggestion {
                    // Convert rask Span to LSP Range
                    let edit_range = Range::new(
                        byte_offset_to_position(&source, suggestion.span.start),
                        byte_offset_to_position(&source, suggestion.span.end),
                    );

                    // Create text edit
                    let text_edit = TextEdit::new(edit_range, suggestion.replacement.clone());

                    // Create workspace edit
                    let mut changes = HashMap::new();
                    changes.insert(uri.clone(), vec![text_edit]);

                    let workspace_edit = WorkspaceEdit {
                        changes: Some(changes),
                        document_changes: None,
                        change_annotations: None,
                    };

                    // Convert rask diagnostic to LSP diagnostic
                    let lsp_diagnostic = to_lsp_diagnostic(&source, uri, diag);

                    // Create code action
                    let action = CodeAction {
                        title: help.message.clone(),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![lsp_diagnostic]),
                        edit: Some(workspace_edit),
                        command: None,
                        is_preferred: Some(true),
                        disabled: None,
                        data: None,
                    };

                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        // Get current source text
        let source = {
            let docs = self.documents.read().unwrap();
            docs.get(uri).cloned()
        };
        let Some(source) = source else {
            return Ok(None);
        };

        let offset = position_to_offset(&source, position);

        // Use last good compilation (code is likely broken while typing)
        let compiled = self.compiled.read().unwrap();
        let cached = match compiled.get(uri) {
            Some(c) => c,
            None => return Ok(None),
        };

        // Dot-completion vs identifier completion
        let is_dot = params
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref())
            == Some(".");

        if is_dot {
            Ok(self.dot_completion(&source, offset, cached))
        } else {
            Ok(self.identifier_completion(&source, offset, cached))
        }
    }
}

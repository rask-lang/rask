// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! LanguageServer trait implementation.

use std::collections::HashMap;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::LanguageServer;

use rask_diagnostics::LabelStyle;

use crate::backend::{Backend, CompilationResult};
use crate::convert::{byte_offset_to_position, position_to_offset, ranges_overlap, to_lsp_diagnostic};
use crate::type_format::TypeFormatter;

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Capture workspace root for resolving stdlib stub file paths
        if let Some(root_uri) = params.root_uri {
            *self.root_uri.write().unwrap() = Some(root_uri);
        }
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
        let Some((node_id, name)) = cached.position_index.ident_at_position(offset) else {
            return Ok(None);
        };

        // Look up symbol for this node
        let symbol = cached.typed.resolutions.get(&node_id)
            .and_then(|&sid| cached.typed.symbols.get(sid));

        if let Some(symbol) = symbol {
            // For built-in symbols, navigate to the stub file
            if symbol.span.start == 0 && symbol.span.end == 0 {
                return Ok(self.resolve_builtin_location(&symbol.name, None));
            }

            // Check if this symbol was defined in a sibling file
            if let Some(sibling) = cached.sibling_decl_names.get(&symbol.name) {
                // Validate the span falls within the sibling source
                if symbol.span.end <= sibling.source.len() {
                    let def_range = Range::new(
                        byte_offset_to_position(&sibling.source, symbol.span.start),
                        byte_offset_to_position(&sibling.source, symbol.span.end),
                    );
                    let sibling_uri = Url::from_file_path(&sibling.path)
                        .unwrap_or_else(|_| uri.clone());
                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: sibling_uri,
                        range: def_range,
                    })));
                }
            }

            let def_range = Range::new(
                byte_offset_to_position(&cached.source, symbol.span.start),
                byte_offset_to_position(&cached.source, symbol.span.end),
            );

            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: def_range,
            })));
        }

        // No symbol resolution — try method-level go-to-def for builtins
        if let Some(response) = self.try_method_goto_definition(&cached.source, offset, &name, cached) {
            return Ok(Some(response));
        }

        Ok(None)
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
        let formatter = TypeFormatter::new(&cached.typed.types);
        let ty_opt = cached.typed.node_types.get(&node_id);

        // If no type info, try StubRegistry fallback for stdlib types
        if ty_opt.is_none() {
            if let Some((_, ident_name)) = cached.position_index.ident_at_position(offset) {
                let reg = rask_stdlib::StubRegistry::load();
                // Check if it's a known stdlib type (e.g., Response, Request, HttpServer)
                if let Some(ts) = reg.get_type(&ident_name) {
                    let mut contents = format!("**Stdlib Type:** `{}`", ident_name);
                    if let Some(doc) = &ts.doc {
                        contents.push_str(&format!("\n\n---\n\n{}", doc));
                    }
                    if !ts.methods.is_empty() {
                        contents.push_str("\n\n**Methods:**\n");
                        for m in &ts.methods {
                            let params_str = m.params.iter()
                                .map(|(n, t)| format!("{}: {}", n, t))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let self_prefix = if m.takes_self { "self, " } else { "" };
                            contents.push_str(&format!(
                                "\n- `{}({}{}) -> {}`",
                                m.name, self_prefix, params_str, m.ret_ty
                            ));
                        }
                    }
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: contents,
                        }),
                        range: None,
                    }));
                }
                // Check if it's a method on a known type
                if let Some(doc) = self.try_method_hover(&cached.source, offset, &ident_name, cached) {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: doc,
                        }),
                        range: None,
                    }));
                }
            }
            return Ok(None);
        }

        let ty = ty_opt.unwrap();

        // Format type for display
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
                            rask_resolve::SymbolKind::TypeAlias { .. } => "Type Alias",
                        };
                        contents = format!("**{}:** `{}`\n\n**Type:** `{}`", kind_str, name, type_str);

                        // Add doc comment and method signatures from stubs for builtins
                        match &symbol.kind {
                            rask_resolve::SymbolKind::BuiltinType { .. }
                            | rask_resolve::SymbolKind::BuiltinFunction { .. }
                            | rask_resolve::SymbolKind::BuiltinModule { .. } => {
                                let reg = rask_stdlib::StubRegistry::load();
                                if let Some(ts) = reg.get_type(&name) {
                                    if let Some(doc) = &ts.doc {
                                        contents.push_str(&format!("\n\n---\n\n{}", doc));
                                    }
                                    if !ts.methods.is_empty() {
                                        contents.push_str("\n\n**Methods:**\n");
                                        for m in &ts.methods {
                                            let params_str = m.params.iter()
                                                .map(|(n, t)| format!("{}: {}", n, t))
                                                .collect::<Vec<_>>()
                                                .join(", ");
                                            let self_prefix = if m.takes_self { "self, " } else { "" };
                                            contents.push_str(&format!(
                                                "\n- `{}({}{}) -> {}`",
                                                m.name, self_prefix, params_str, m.ret_ty
                                            ));
                                        }
                                    }
                                } else {
                                    let doc = reg.functions().iter()
                                        .find(|f| f.name == name)
                                        .and_then(|f| f.doc.as_deref());
                                    if let Some(doc) = doc {
                                        contents.push_str(&format!("\n\n---\n\n{}", doc));
                                    }
                                }
                            }
                            rask_resolve::SymbolKind::Struct { .. }
                            | rask_resolve::SymbolKind::Enum { .. } => {
                                // Show fields/variants and methods for user-defined types
                                if let Some(type_id) = cached.typed.types.get_type_id(&name) {
                                    if let Some(def) = cached.typed.types.get(type_id) {
                                        match def {
                                            rask_types::TypeDef::Struct { fields, methods, .. } => {
                                                if !fields.is_empty() {
                                                    contents.push_str("\n\n**Fields:**\n");
                                                    for (fname, fty) in fields {
                                                        contents.push_str(&format!(
                                                            "\n- `{}: {}`", fname, formatter.format(fty)
                                                        ));
                                                    }
                                                }
                                                if !methods.is_empty() {
                                                    contents.push_str("\n\n**Methods:**\n");
                                                    for m in methods {
                                                        contents.push_str(&format!(
                                                            "\n- `{}`", m.name
                                                        ));
                                                    }
                                                }
                                            }
                                            rask_types::TypeDef::Enum { variants, methods, .. } => {
                                                if !variants.is_empty() {
                                                    contents.push_str("\n\n**Variants:**\n");
                                                    for (vname, fields) in variants {
                                                        if fields.is_empty() {
                                                            contents.push_str(&format!("\n- `{}`", vname));
                                                        } else {
                                                            let fields_str = fields.iter()
                                                                .map(|t| formatter.format(t))
                                                                .collect::<Vec<_>>()
                                                                .join(", ");
                                                            contents.push_str(&format!("\n- `{}({})`", vname, fields_str));
                                                        }
                                                    }
                                                }
                                                if !methods.is_empty() {
                                                    contents.push_str("\n\n**Methods:**\n");
                                                    for m in methods {
                                                        contents.push_str(&format!(
                                                            "\n- `{}`", m.name
                                                        ));
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    // No symbol — try method hover for builtin types
                    if let Some(doc) = self.try_method_hover(&cached.source, offset, &name, cached) {
                        contents.push_str(&format!("\n\n---\n\n{}", doc));
                    }
                }
            }
        }

        // Enrich UnresolvedNamed types with StubRegistry info
        if let rask_types::Type::UnresolvedNamed(name) = ty {
            let reg = rask_stdlib::StubRegistry::load();
            if let Some(ts) = reg.get_type(name) {
                if let Some(doc) = &ts.doc {
                    contents.push_str(&format!("\n\n---\n\n{}", doc));
                }
                if !ts.methods.is_empty() {
                    contents.push_str("\n\n**Methods:**\n");
                    for m in &ts.methods {
                        let params_str = m.params.iter()
                            .map(|(n, t)| format!("{}: {}", n, t))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let self_prefix = if m.takes_self { "self, " } else { "" };
                        contents.push_str(&format!(
                            "\n- `{}({}{}) -> {}`",
                            m.name, self_prefix, params_str, m.ret_ty
                        ));
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

impl Backend {
    /// Resolve a builtin type, function, or method to its stub file location.
    fn resolve_builtin_location(
        &self,
        name: &str,
        method: Option<&str>,
    ) -> Option<GotoDefinitionResponse> {
        let reg = rask_stdlib::StubRegistry::load();

        let (source_file, span) = if let Some(method_name) = method {
            let normalized = match name {
                "String" => "string",
                _ => name,
            };
            let m = reg.lookup_method(normalized, method_name)?;
            (&m.source_file, m.span)
        } else if let Some(ts) = reg.get_type(name) {
            (&ts.source_file, ts.span)
        } else {
            let f = reg.functions().iter().find(|f| f.name == name)?;
            (&f.source_file, f.span)
        };

        let (start_line, start_col) = reg.offset_to_lsp_position(source_file, span.start)?;
        let (end_line, end_col) = reg.offset_to_lsp_position(source_file, span.end)?;

        let root = self.root_uri.read().unwrap();
        let root_uri = root.as_ref()?;

        let stub_path = format!("{}/{}", root_uri.as_str().trim_end_matches('/'), source_file);
        let stub_uri = Url::parse(&stub_path).ok()?;

        Some(GotoDefinitionResponse::Scalar(Location {
            uri: stub_uri,
            range: Range::new(
                Position::new(start_line, start_col),
                Position::new(end_line, end_col),
            ),
        }))
    }

    /// Try to resolve a method call to its definition in a stub file.
    fn try_method_goto_definition(
        &self,
        source: &str,
        offset: usize,
        method_name: &str,
        cached: &CompilationResult,
    ) -> Option<GotoDefinitionResponse> {
        // Find the span of this ident to check for a preceding dot
        let (ident_span, _, _) = cached.position_index.idents.iter()
            .find(|(span, _, name)| span.start <= offset && offset <= span.end && name == method_name)?;

        if ident_span.start == 0 {
            return None;
        }
        if *source.as_bytes().get(ident_span.start - 1)? != b'.' {
            return None;
        }

        let type_name = self.resolve_receiver_type_name(source, ident_span.start - 1, cached)?;
        self.resolve_builtin_location(&type_name, Some(method_name))
    }

    /// Try to get doc comment for a method call on a builtin type.
    fn try_method_hover(
        &self,
        source: &str,
        offset: usize,
        method_name: &str,
        cached: &CompilationResult,
    ) -> Option<String> {
        let (ident_span, _, _) = cached.position_index.idents.iter()
            .find(|(span, _, name)| span.start <= offset && offset <= span.end && name == method_name)?;

        if ident_span.start == 0 {
            return None;
        }
        if *source.as_bytes().get(ident_span.start - 1)? != b'.' {
            return None;
        }

        let type_name = self.resolve_receiver_type_name(source, ident_span.start - 1, cached)?;
        let reg = rask_stdlib::StubRegistry::load();
        let normalized = match type_name.as_str() {
            "String" => "string",
            _ => &type_name,
        };
        let method = reg.lookup_method(normalized, method_name)?;

        // Build a richer hover with signature + doc
        let params_str = method.params.iter()
            .map(|(n, t)| format!("{}: {}", n, t))
            .collect::<Vec<_>>()
            .join(", ");
        let self_prefix = if method.takes_self { "self, " } else { "" };
        let mut result = format!(
            "**Method:** `{}.{}({}{}) -> {}`",
            normalized, method.name, self_prefix, params_str, method.ret_ty
        );
        if let Some(doc) = &method.doc {
            result.push_str(&format!("\n\n---\n\n{}", doc));
        }
        Some(result)
    }

    /// Given a dot position in source, determine the receiver's type name for stub lookup.
    fn resolve_receiver_type_name(
        &self,
        source: &str,
        dot_pos: usize,
        cached: &CompilationResult,
    ) -> Option<String> {
        let text_before = &source[..dot_pos];
        let receiver_end = text_before.len();
        let receiver_start = text_before
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let receiver_name = &text_before[receiver_start..receiver_end];

        if receiver_name.is_empty() {
            return None;
        }

        // Check if it's a known stub type/module directly
        let reg = rask_stdlib::StubRegistry::load();
        if reg.get_type(receiver_name).is_some() {
            return Some(receiver_name.to_string());
        }

        // Look up the receiver's type from the typed program
        for (_span, node_id, name) in &cached.position_index.idents {
            if name == receiver_name {
                if let Some(ty) = cached.typed.node_types.get(node_id) {
                    return type_to_stub_name(ty, cached);
                }
            }
        }

        None
    }
}

/// Map a type checker Type to the stub registry type name.
fn type_to_stub_name(
    ty: &rask_types::Type,
    cached: &CompilationResult,
) -> Option<String> {
    match ty {
        rask_types::Type::String => Some("string".to_string()),
        rask_types::Type::Named(id) => {
            Some(cached.typed.types.type_name(*id))
        }
        rask_types::Type::Generic { base, .. } => {
            Some(cached.typed.types.type_name(*base))
        }
        rask_types::Type::UnresolvedNamed(name) => Some(name.clone()),
        rask_types::Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
        rask_types::Type::Option(_) => Some("Option".to_string()),
        rask_types::Type::Result { .. } => Some("Result".to_string()),
        rask_types::Type::Array { .. } | rask_types::Type::Slice(_) => Some("Vec".to_string()),
        _ => None,
    }
}

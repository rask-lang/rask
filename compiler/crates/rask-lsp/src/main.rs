// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask Language Server
//!
//! Provides diagnostics for all compilation errors:
//! lexer, parser, resolve, type check, and ownership.
//!
//! Also provides IDE features:
//! - Go to Definition
//! - Hover (type information)
//! - Code Actions (quick fixes)

use std::collections::HashMap;
use std::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_diagnostics::{LabelStyle, Severity, ToDiagnostic};
use rask_lexer::Lexer;
use rask_parser::Parser;
use rask_types::{GenericArg, Type, TypeTable, TypedProgram};

/// Cached compilation result for a file.
#[derive(Debug)]
struct CompilationResult {
    /// Source text (for cache validation)
    source: String,
    /// Parsed AST declarations
    decls: Vec<Decl>,
    /// Type-checked program
    typed: TypedProgram,
    /// Original diagnostics (before LSP conversion)
    diagnostics: Vec<rask_diagnostics::Diagnostic>,
    /// Position index for fast lookups
    position_index: PositionIndex,
}

/// Maps source positions to AST nodes for fast lookup.
#[derive(Debug, Clone)]
struct PositionIndex {
    /// All expressions with their spans and node IDs
    exprs: Vec<(Span, NodeId)>,
    /// Identifiers specifically (for go-to-definition)
    idents: Vec<(Span, NodeId, String)>,
}

impl PositionIndex {
    fn new() -> Self {
        Self {
            exprs: Vec::new(),
            idents: Vec::new(),
        }
    }

    /// Find the innermost node containing the given byte offset.
    fn node_at_position(&self, offset: usize) -> Option<NodeId> {
        self.exprs
            .iter()
            .filter(|(span, _)| span.start <= offset && offset <= span.end)
            .min_by_key(|(span, _)| span.end - span.start) // Smallest span
            .map(|(_, node_id)| *node_id)
    }

    /// Find identifier at the given byte offset.
    fn ident_at_position(&self, offset: usize) -> Option<(NodeId, String)> {
        self.idents
            .iter()
            .find(|(span, _, _)| span.start <= offset && offset <= span.end)
            .map(|(_, node_id, name)| (*node_id, name.clone()))
    }

    /// Sort spans for efficient lookup (call after building).
    fn finalize(&mut self) {
        self.exprs.sort_by_key(|(span, _)| span.start);
        self.idents.sort_by_key(|(span, _, _)| span.start);
    }
}

/// Formats types for human-readable display in hover tooltips.
struct TypeFormatter<'a> {
    types: &'a TypeTable,
}

impl<'a> TypeFormatter<'a> {
    fn new(types: &'a TypeTable) -> Self {
        Self { types }
    }

    fn format(&self, ty: &Type) -> String {
        match ty {
            Type::Unit => "()".to_string(),
            Type::Never => "!".to_string(),
            Type::Bool => "bool".to_string(),
            Type::I32 => "i32".to_string(),
            Type::I64 => "i64".to_string(),
            Type::F32 => "f32".to_string(),
            Type::F64 => "f64".to_string(),
            Type::String => "string".to_string(),
            Type::Char => "char".to_string(),

            Type::Named(id) => {
                self.types.type_name(*id)
            }

            Type::Generic { base, args } => {
                let base_name = self.types.type_name(*base);
                let args_str = args.iter()
                    .map(|t| self.format_generic_arg(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}<{}>", base_name, args_str)
            }

            Type::UnresolvedGeneric { name, args } => {
                if args.is_empty() {
                    name.clone()
                } else {
                    let args_str = args.iter()
                        .map(|t| self.format_generic_arg(t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}<{}>", name, args_str)
                }
            }

            Type::Fn { params, ret } => {
                let params_str = params.iter()
                    .map(|p| self.format(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("func({}) -> {}", params_str, self.format(ret))
            }

            Type::Option(inner) => format!("{}?", self.format(inner)),

            Type::Result { ok, err } => {
                format!("{} or {}", self.format(ok), self.format(err))
            }

            Type::Tuple(elements) => {
                let elems_str = elements.iter()
                    .map(|e| self.format(e))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", elems_str)
            }

            Type::UnresolvedNamed(name) => name.clone(),
            Type::Error => "<error>".to_string(),
            _ => format!("{:?}", ty),
        }
    }

    fn format_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.format(ty),
            GenericArg::ConstUsize(n) => n.to_string(),
        }
    }
}

#[derive(Debug)]
struct Backend {
    client: Client,
    documents: RwLock<HashMap<Url, String>>,
    /// Cached compilation results
    compiled: RwLock<HashMap<Url, CompilationResult>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            compiled: RwLock::new(HashMap::new()),
        }
    }

    /// Check if we have a valid cached compilation for this URI/source.
    fn has_compiled(&self, uri: &Url, source: &str) -> bool {
        let compiled = self.compiled.read().unwrap();
        compiled.get(uri)
            .map(|cached| cached.source == source)
            .unwrap_or(false)
    }

    async fn publish_diagnostics(&self, uri: Url, text: &str) {
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
    fn analyze_and_cache(&self, uri: &Url, source: &str) -> Vec<rask_diagnostics::Diagnostic> {
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
            decls: parse_result.decls,
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

/// Build position index by traversing the AST.
fn build_position_index(decls: &[Decl]) -> PositionIndex {
    let mut index = PositionIndex::new();
    for decl in decls {
        visit_decl(decl, &mut index);
    }
    index
}

fn visit_decl(decl: &Decl, index: &mut PositionIndex) {
    match &decl.kind {
        DeclKind::Fn(fn_decl) => {
            for stmt in &fn_decl.body {
                visit_stmt(stmt, index);
            }
        }
        DeclKind::Const(const_decl) => {
            visit_expr(&const_decl.init, index);
        }
        DeclKind::Test(test_decl) => {
            for stmt in &test_decl.body {
                visit_stmt(stmt, index);
            }
        }
        DeclKind::Benchmark(bench_decl) => {
            for stmt in &bench_decl.body {
                visit_stmt(stmt, index);
            }
        }
        DeclKind::Impl(impl_decl) => {
            for method in &impl_decl.methods {
                for stmt in &method.body {
                    visit_stmt(stmt, index);
                }
            }
        }
        DeclKind::Trait(trait_decl) => {
            for method in &trait_decl.methods {
                for stmt in &method.body {
                    visit_stmt(stmt, index);
                }
            }
        }
        _ => {}
    }
}

fn visit_stmt(stmt: &Stmt, index: &mut PositionIndex) {
    match &stmt.kind {
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => {
            visit_expr(e, index);
        }
        StmtKind::Deliver { value, .. } => {
            visit_expr(value, index);
        }
        StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
            visit_expr(init, index);
        }
        StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
            visit_expr(init, index);
        }
        StmtKind::Assign { target, value } => {
            visit_expr(target, index);
            visit_expr(value, index);
        }
        StmtKind::While { cond, body } => {
            visit_expr(cond, index);
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::WhileLet { expr, body, .. } => {
            visit_expr(expr, index);
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::For { iter, body, .. } => {
            visit_expr(iter, index);
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::Loop { body, .. } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        StmtKind::Ensure { body, catch } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
            if let Some((_, handler)) = catch {
                for stmt in handler {
                    visit_stmt(stmt, index);
                }
            }
        }
        StmtKind::Comptime(stmts) => {
            for stmt in stmts {
                visit_stmt(stmt, index);
            }
        }
        _ => {}
    }
}

fn visit_expr(expr: &Expr, index: &mut PositionIndex) {
    // Record this expression
    index.exprs.push((expr.span, expr.id));

    // Record identifiers separately
    if let ExprKind::Ident(name) = &expr.kind {
        index.idents.push((expr.span, expr.id, name.clone()));
    }

    // Recursively visit child expressions
    match &expr.kind {
        ExprKind::Binary { left, right, .. } => {
            visit_expr(left, index);
            visit_expr(right, index);
        }
        ExprKind::Unary { operand, .. } => {
            visit_expr(operand, index);
        }
        ExprKind::Call { func, args } => {
            visit_expr(func, index);
            for arg in args {
                visit_expr(arg, index);
            }
        }
        ExprKind::MethodCall { object, args, .. } => {
            visit_expr(object, index);
            for arg in args {
                visit_expr(arg, index);
            }
        }
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            visit_expr(object, index);
        }
        ExprKind::Index { object, index: idx } => {
            visit_expr(object, index);
            visit_expr(idx, index);
        }
        ExprKind::Block(stmts) => {
            for stmt in stmts {
                visit_stmt(stmt, index);
            }
        }
        ExprKind::If { cond, then_branch, else_branch } => {
            visit_expr(cond, index);
            visit_expr(then_branch, index);
            if let Some(else_br) = else_branch {
                visit_expr(else_br, index);
            }
        }
        ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
            visit_expr(expr, index);
            visit_expr(then_branch, index);
            if let Some(else_br) = else_branch {
                visit_expr(else_br, index);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            visit_expr(scrutinee, index);
            for arm in arms {
                if let Some(ref guard) = arm.guard {
                    visit_expr(guard, index);
                }
                visit_expr(&arm.body, index);
            }
        }
        ExprKind::Try(e) => {
            visit_expr(e, index);
        }
        ExprKind::NullCoalesce { value, default } => {
            visit_expr(value, index);
            visit_expr(default, index);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                visit_expr(s, index);
            }
            if let Some(e) = end {
                visit_expr(e, index);
            }
        }
        ExprKind::StructLit { fields, .. } => {
            for field_init in fields {
                visit_expr(&field_init.value, index);
            }
        }
        ExprKind::Tuple(exprs) => {
            for e in exprs {
                visit_expr(e, index);
            }
        }
        ExprKind::Array(exprs) => {
            for e in exprs {
                visit_expr(e, index);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            visit_expr(value, index);
            visit_expr(count, index);
        }
        ExprKind::Closure { body, .. } => {
            visit_expr(body, index);
        }
        ExprKind::Spawn { body } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        ExprKind::WithBlock { body, .. } => {
            for stmt in body {
                visit_stmt(stmt, index);
            }
        }
        _ => {}
    }
}

/// Convert LSP Position (line/col) to byte offset.
fn position_to_offset(source: &str, pos: Position) -> usize {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in source.char_indices() {
        if line == pos.line && col == pos.character {
            return i;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    source.len()
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
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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

            // Invalidate compilation cache
            {
                let mut compiled = self.compiled.write().unwrap();
                compiled.remove(&uri);
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

        // Get source
        let source = {
            let docs = self.documents.read().unwrap();
            docs.get(uri).cloned()
        };

        let Some(source) = source else {
            return Ok(None);
        };

        // Check cache and extract data we need
        let compiled = self.compiled.read().unwrap();
        let Some(cached) = compiled.get(uri) else {
            return Ok(None);
        };

        if cached.source != source {
            return Ok(None);
        }

        // Convert position to byte offset
        let offset = position_to_offset(&source, position);

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

        // Convert span to LSP location
        let def_range = Range::new(
            byte_offset_to_position(&source, symbol.span.start),
            byte_offset_to_position(&source, symbol.span.end),
        );

        let location = Location {
            uri: uri.clone(),
            range: def_range,
        };

        Ok(Some(GotoDefinitionResponse::Scalar(location)))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        // Get source
        let source = {
            let docs = self.documents.read().unwrap();
            docs.get(uri).cloned()
        };

        let Some(source) = source else {
            return Ok(None);
        };

        // Check cache
        let compiled = self.compiled.read().unwrap();
        let Some(cached) = compiled.get(uri) else {
            return Ok(None);
        };

        if cached.source != source {
            return Ok(None);
        }

        // Convert position to offset
        let offset = position_to_offset(&source, position);

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

        // Get source
        let source = {
            let docs = self.documents.read().unwrap();
            docs.get(uri).cloned()
        };

        let Some(source) = source else {
            return Ok(None);
        };

        // Check cache
        let compiled = self.compiled.read().unwrap();
        let Some(cached) = compiled.get(uri) else {
            return Ok(None);
        };

        if cached.source != source {
            return Ok(None);
        }

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
}

/// Check if two ranges overlap.
fn ranges_overlap(r1: Range, r2: Range) -> bool {
    !(r1.end < r2.start || r2.end < r1.start)
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

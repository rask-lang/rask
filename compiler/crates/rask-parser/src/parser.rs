// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The parser implementation using Pratt parsing for expressions.

use rask_ast::decl::{BenchmarkDecl, CImportDecl, ConstDecl, ContextClause, Decl, DeclKind, DepDecl, EnumDecl, ExternDecl, FeatureDecl, FeatureOption, Field, FieldVisibility, FnDecl, ImplDecl, ImportDecl, PackageDecl, Param, ProfileDecl, StructDecl, TestDecl, TraitDecl, TypeAliasDecl, TypeParam, UnionDecl, Variant};
use rask_ast::expr::{ArgMode, BinOp, CallArg, ClosureParam, ConvertKind, Expr, ExprKind, FieldInit, MatchArm, Pattern, SelectArm, SelectArmKind, StringSegment, UnaryOp, WithBinding};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::token::{IntSuffix, Token, TokenKind};
use rask_ast::{NodeId, Span};

/// Maximum number of errors to collect before stopping.
const MAX_ERRORS: usize = 20;

/// The parser for Rask source code.
pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Track pending `>` from splitting `>>` in generics
    pending_gt: bool,
    /// Controls whether `{` can start struct literals (false in control flow conditions)
    allow_brace_expr: bool,
    /// Set while parsing an element of a comma list — call arguments, struct
    /// literal fields, array/tuple elements. A diverging `??`/`catch` right
    /// side there needs parens (ER45a).
    in_comma_list: bool,
    /// Collected errors during parsing
    errors: Vec<ParseError>,
    /// Counter for generating unique NodeIds
    next_node_id: u32,
    /// Pending declarations from expanded grouped imports
    pending_decls: Vec<Decl>,
    /// Buffer for doc comments collected during skip_newlines
    doc_buffer: Vec<String>,
    /// File index for multi-file packages (0 for single-file).
    file_id: u16,
    /// Stub files declare `func assert(...)` and friends so the checker knows
    /// their signatures. Real source can't — the call would parse as the
    /// keyword form — so only stub parsing sets this.
    allow_keyword_fn_names: bool,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self::new_with_start_id(tokens, 0)
    }

    /// Create a parser with a custom starting NodeId.
    /// Used by multi-file parsing to ensure unique NodeIds across files.
    pub fn new_with_start_id(tokens: Vec<Token>, start_id: u32) -> Self {
        Self::new_with_file_id(tokens, start_id, 0)
    }

    /// Create a parser with a custom starting NodeId and file index.
    pub fn new_with_file_id(tokens: Vec<Token>, start_id: u32, file_id: u16) -> Self {
        Self { tokens, pos: 0, pending_gt: false, allow_brace_expr: true, in_comma_list: false, errors: Vec::new(), next_node_id: start_id, pending_decls: Vec::new(), doc_buffer: Vec::new(), file_id, allow_keyword_fn_names: false }
    }

    /// Let top-level `func` declarations use keyword names. Only stub files
    /// need this — see `allow_keyword_fn_names`.
    pub fn allow_keyword_fn_names(mut self) -> Self {
        self.allow_keyword_fn_names = true;
        self
    }

    /// Return the next available NodeId (for chaining across files).
    pub fn next_node_id(&self) -> u32 {
        self.next_node_id
    }

    fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// Create a span tagged with this parser's file_id.
    fn span(&self, start: usize, end: usize) -> Span {
        Span::with_file(start, end, self.file_id)
    }

    /// Record error, return if should continue.
    fn record_error(&mut self, error: ParseError) -> bool {
        self.errors.push(error);
        self.errors.len() < MAX_ERRORS
    }

    /// Skip to next declaration after error.
    fn synchronize(&mut self) {
        let mut brace_depth = 0;

        while !self.at_end() {
            match self.current_kind() {
                TokenKind::LBrace => {
                    brace_depth += 1;
                    self.advance();
                }
                TokenKind::RBrace => {
                    if brace_depth > 0 {
                        brace_depth -= 1;
                        self.advance();
                        if brace_depth == 0 {
                            self.skip_newlines();
                            return;
                        }
                    } else {
                        self.advance();
                    }
                }
                TokenKind::Func | TokenKind::Struct | TokenKind::Enum |
                TokenKind::Trait | TokenKind::Extend | TokenKind::Import |
                TokenKind::Extern | TokenKind::Public | TokenKind::Private | TokenKind::Package if brace_depth == 0 => {
                    return;
                }
                _ => { self.advance(); }
            }
        }
    }

    // =========================================================================
    // Token Navigation
    // =========================================================================

    fn current(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn current_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    fn peek(&self, n: usize) -> &TokenKind {
        self.tokens.get(self.pos + n).map(|t| &t.kind).unwrap_or(&TokenKind::Eof)
    }

    fn at_end(&self) -> bool {
        matches!(self.current_kind(), TokenKind::Eof)
    }

    /// True if the token `n` ahead is the contextual identifier `word`.
    /// Conversion verbs (`truncate`, `saturate`, `float`, `convert`, `to`, `int`,
    /// `saturating`) aren't reserved keywords — they're matched positionally.
    fn peek_is_word(&self, n: usize, word: &str) -> bool {
        matches!(self.peek(n), TokenKind::Ident(s) if s == word)
    }

    fn advance(&mut self) -> &Token {
        if !self.at_end() {
            self.pos += 1;
        }
        self.tokens.get(self.pos - 1).unwrap()
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.current_kind()) == std::mem::discriminant(kind)
    }

    fn match_token(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<&Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(ParseError::expected(
                kind.display_name(),
                self.current_kind(),
                self.current().span,
            ))
        }
    }

    fn skip_newlines(&mut self) {
        loop {
            if self.check(&TokenKind::Newline) {
                self.advance();
            } else if let TokenKind::DocComment(text) = self.current_kind().clone() {
                self.doc_buffer.push(text);
                self.advance();
            } else {
                break;
            }
        }
    }

    fn take_doc(&mut self) -> Option<String> {
        if self.doc_buffer.is_empty() {
            None
        } else {
            let doc = self.doc_buffer.join("\n");
            self.doc_buffer.clear();
            Some(doc)
        }
    }

    fn expect_terminator(&mut self) -> Result<(), ParseError> {
        if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) {
            self.advance();
            self.skip_newlines();
            Ok(())
        } else if self.check(&TokenKind::Eof) || self.check(&TokenKind::RBrace) {
            Ok(())
        } else {
            Err(ParseError::expected(
                "newline or ';'",
                self.current_kind(),
                self.current().span,
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.current_kind().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            // `read` is reserved by the lexer but has no structural role in the grammar.
            // Allow it anywhere a plain identifier is expected.
            TokenKind::ReadKw => {
                self.advance();
                Ok("read".to_string())
            }
            _ => Err(ParseError::expected(
                "a name",
                self.current_kind(),
                self.current().span,
            )),
        }
    }

    fn expect_string(&mut self) -> Result<String, ParseError> {
        match self.current_kind().clone() {
            TokenKind::String(s) => {
                self.advance();
                Ok(s)
            }
            _ => Err(ParseError::expected(
                "a string",
                self.current_kind(),
                self.current().span,
            )),
        }
    }

    /// Allow keywords as field/method names.
    /// After `.` or `?.`, any keyword can be used as an identifier.
    fn expect_ident_or_keyword(&mut self) -> Result<String, ParseError> {
        let name = match self.current_kind() {
            TokenKind::Ident(name) => name.clone(),
            other => match keyword_spelling(other) {
                Some(kw) => kw.to_string(),
                None => return Err(ParseError::expected(
                    "a name",
                    self.current_kind(),
                    self.current().span,
                ).with_hint("Names start with a letter or '_'")),
            },
        };
        self.advance();
        Ok(name)
    }

    fn is_type_name(name: &str) -> bool {
        name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    }

    /// Check for postfix operator after newlines (method chaining).
    fn peek_past_newlines_is_postfix(&self) -> bool {
        let mut pos = self.pos + 1;
        while pos < self.tokens.len() {
            match &self.tokens[pos].kind {
                TokenKind::Newline => pos += 1,
                TokenKind::Dot | TokenKind::QuestionDot | TokenKind::Question | TokenKind::LBracket => return true,
                _ => return false,
            }
        }
        false
    }

    /// Check for unambiguous infix operator after newlines (expression continuation).
    /// Excludes `+`, `-` (prefix ambiguity), `*` (deref ambiguity), `<`/`>` (generics ambiguity).
    fn peek_past_newlines_is_infix(&self) -> bool {
        let mut pos = self.pos + 1;
        while pos < self.tokens.len() {
            match &self.tokens[pos].kind {
                TokenKind::Newline => pos += 1,
                TokenKind::AmpAmp | TokenKind::PipePipe
                | TokenKind::EqEq | TokenKind::BangEq
                | TokenKind::LtEq | TokenKind::GtEq
                | TokenKind::QuestionQuestion | TokenKind::Catch
                | TokenKind::Pipe | TokenKind::Caret | TokenKind::Amp
                | TokenKind::LtLt | TokenKind::GtGt
                | TokenKind::Slash | TokenKind::Percent
                | TokenKind::DotDot | TokenKind::DotDotEq => return true,
                _ => return false,
            }
        }
        false
    }

    /// Check for `else` after newlines (if-else continuation).
    fn peek_past_newlines_is_else(&self) -> bool {
        let mut pos = self.pos + 1;
        while pos < self.tokens.len() {
            match &self.tokens[pos].kind {
                TokenKind::Newline => pos += 1,
                TokenKind::Else => return true,
                _ => return false,
            }
        }
        false
    }

    /// Check if `<` starts generic method call: `<T>(`.
    fn looks_like_generic_method_call(&self) -> bool {
        self.looks_like_generic_followed_by(&TokenKind::LParen)
    }

    /// Check if `<` starts generic type with static method: `<T>.`.
    fn looks_like_generic_type_with_static_method(&self) -> bool {
        self.looks_like_generic_followed_by(&TokenKind::Dot)
    }

    fn looks_like_generic_followed_by(&self, expected: &TokenKind) -> bool {
        let mut pos = self.pos + 1;
        let mut depth: i32 = 1;
        let mut paren_depth: i32 = 0;
        let mut bracket_depth: i32 = 0;

        while pos < self.tokens.len() && depth > 0 {
            match &self.tokens[pos].kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => depth -= 1,
                TokenKind::GtGt => {
                    depth -= 2;
                    if depth < 0 {
                        if pos + 1 < self.tokens.len() {
                            return &self.tokens[pos + 1].kind == expected;
                        }
                        return false;
                    }
                }
                // Track parens/brackets so we don't scan past enclosing groups.
                // f(g(x < y)) — the `)` closing g() should stop the scan.
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen => {
                    paren_depth -= 1;
                    if paren_depth < 0 { return false; }
                }
                TokenKind::LBracket => bracket_depth += 1,
                TokenKind::RBracket => {
                    bracket_depth -= 1;
                    if bracket_depth < 0 { return false; }
                }
                TokenKind::Eof | TokenKind::Newline | TokenKind::Semi => {
                    return false;
                }
                _ => {}
            }
            pos += 1;
        }

        if depth == 0 && pos < self.tokens.len() {
            return &self.tokens[pos].kind == expected;
        }
        false
    }

    /// Handle `>>` splitting in generic contexts.
    fn expect_gt_in_generic(&mut self) -> Result<(), ParseError> {
        if self.pending_gt {
            self.pending_gt = false;
            return Ok(());
        }

        match self.current_kind() {
            TokenKind::Gt => {
                self.advance();
                Ok(())
            }
            TokenKind::GtGt => {
                self.advance();
                self.pending_gt = true;
                Ok(())
            }
            TokenKind::GtGtEq => {
                Err(ParseError::expected(
                    "'>'",
                    self.current_kind(),
                    self.current().span,
                ))
            }
            _ => Err(ParseError::expected(
                "'>'",
                self.current_kind(),
                self.current().span,
            )),
        }
    }

    // =========================================================================
    // Top-Level Parsing
    // =========================================================================

    pub fn parse(&mut self) -> ParseResult {
        let mut decls = Vec::new();
        let mut top_level_stmts: Vec<Stmt> = Vec::new();
        self.skip_newlines();

        while !self.at_end() || !self.pending_decls.is_empty() {
            // Save position to retry as statement if declaration parse fails
            let saved_pos = self.pos;
            let saved_errors = self.errors.len();

            match self.parse_decl() {
                Ok(decl) => decls.push(decl),
                Err(decl_err) => {
                    // Reset to saved position and try parsing as a statement
                    self.pos = saved_pos;
                    self.errors.truncate(saved_errors);

                    // Declaration-only keywords can never start a statement.
                    // Use the original decl error (more specific) and synchronize.
                    if matches!(self.current_kind(),
                        TokenKind::Func | TokenKind::Struct | TokenKind::Enum |
                        TokenKind::Union | TokenKind::Trait | TokenKind::Extend |
                        TokenKind::Import | TokenKind::Export | TokenKind::Extern |
                        TokenKind::Test | TokenKind::Benchmark | TokenKind::Package |
                        TokenKind::Public | TokenKind::Private
                    ) {
                        if !self.record_error(decl_err) { break; }
                        self.synchronize();
                        // These keywords are in the synchronize recovery set,
                        // so synchronize stops AT them without advancing.
                        // Force progress to prevent infinite loops.
                        if self.pos == saved_pos && !self.at_end() {
                            self.advance();
                            self.synchronize();
                        }
                    // Reject rebindable bindings at top level
                    } else if matches!(self.current_kind(), TokenKind::Mut) {
                        let err = ParseError {
                            span: self.current().span,
                            message: "rebindable 'mut' bindings are not allowed at the top level".to_string(),
                            hint: Some("use 'const' for module-level constants, or move into a function".to_string()),
                            why: None,
                        };
                        if !self.record_error(err) { break; }
                        self.synchronize();
                    } else if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "pub") {
                        let err = ParseError {
                            span: self.current().span,
                            message: "unknown keyword 'pub'".to_string(),
                            hint: Some("use 'public' instead of 'pub'".to_string()),
                            why: None,
                        };
                        if !self.record_error(err) { break; }
                        self.synchronize();
                    } else if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "fn") {
                        let err = ParseError {
                            span: self.current().span,
                            message: "unknown keyword 'fn'".to_string(),
                            hint: Some("use 'func' instead of 'fn'".to_string()),
                            why: None,
                        };
                        if !self.record_error(err) { break; }
                        self.synchronize();
                    } else {
                        match self.parse_stmt() {
                            Ok(stmt) => top_level_stmts.push(stmt),
                            Err(stmt_err) => {
                                if !self.record_error(stmt_err) {
                                    break;
                                }
                                self.synchronize();
                            }
                        }
                    }
                }
            }
            self.skip_newlines();
        }

        // Script mode: when there's no explicit main(), top-level const
        // declarations become statements so everything executes in source
        // order. Without this, consts are evaluated at registration time
        // (before any statements run), which is surprising when consts
        // read from sync primitives modified by earlier statements.
        let has_main = decls.iter().any(|d| matches!(&d.kind,
            DeclKind::Fn(f) if f.name == "main" || f.attrs.contains(&"entry".to_string())));

        if !has_main && !top_level_stmts.is_empty() {
            // Move const decls into the statement list
            let mut const_stmts: Vec<Stmt> = Vec::new();
            decls.retain(|d| {
                if let DeclKind::Const(c) = &d.kind {
                    const_stmts.push(Stmt {
                        id: d.id,
                        kind: StmtKind::Let {
                            name: c.name.clone(),
                            name_span: d.span,
                            ty: c.ty.clone(),
                            init: c.init.clone(),
                        },
                        span: d.span,
                    });
                    false
                } else {
                    true
                }
            });
            top_level_stmts.extend(const_stmts);
            top_level_stmts.sort_by_key(|s| s.span.start);
        }

        // Wrap top-level statements in a synthetic main function
        if !top_level_stmts.is_empty() {
            if has_main {
                self.errors.push(ParseError {
                    span: top_level_stmts[0].span,
                    message: "top-level statements cannot coexist with an explicit main function".to_string(),
                    hint: Some("move statements into main() or remove the main function".to_string()),
                    why: None,
                });
            } else {
                let span = self.span(
                    top_level_stmts.first().map(|s| s.span.start).unwrap_or(0),
                    top_level_stmts.last().map(|s| s.span.end).unwrap_or(0),
                );
                decls.push(Decl {
                    id: self.next_id(),
                    kind: DeclKind::Fn(FnDecl {
                        name: "main".to_string(),
                        type_params: vec![],
                        params: vec![],
                        ret_ty: None,
                        context_clauses: vec![],
                        body: top_level_stmts,
                        is_pub: false, is_private: false,
                        is_comptime: false,
                        is_unsafe: false,
                        abi: None,
                        attrs: vec!["entry".to_string()],
                        doc: None,
                        span,
                    }),
                    span,
                });
            }
        }

        ParseResult {
            decls,
            errors: std::mem::take(&mut self.errors),
        }
    }

    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        if let Some(decl) = self.pending_decls.pop() {
            return Ok(decl);
        }

        let start = self.current().span.start;

        let mut attrs = Vec::new();
        while self.check(&TokenKind::At) {
            attrs.push(self.parse_attribute()?);
            self.skip_newlines();
        }

        let is_pub = self.match_token(&TokenKind::Public);
        let is_comptime = self.match_token(&TokenKind::Comptime);
        let is_unsafe = if !is_comptime { self.match_token(&TokenKind::Unsafe) } else { false };

        let doc = self.take_doc();

        // Contextual modifiers: `duck trait` (G1 shape-matched) and
        // `scoped extend` (MN4). Both are plain identifiers followed by the
        // real keyword, so no lexer keyword is needed.
        let is_duck = matches!(self.current_kind(), TokenKind::Ident(s) if s == "duck")
            && matches!(self.peek(1), TokenKind::Trait);
        if is_duck {
            self.advance();
        }
        let is_scoped = matches!(self.current_kind(), TokenKind::Ident(s) if s == "scoped")
            && matches!(self.peek(1), TokenKind::Extend);
        if is_scoped {
            self.advance();
        }

        // Detect common Rust keywords
        if let TokenKind::Ident(s) = self.current_kind() {
            if s == "pub" {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unknown keyword 'pub'".to_string(),
                    hint: Some("use 'public' instead of 'pub'".to_string()),
                    why: None,
                });
            } else if s == "fn" {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unknown keyword 'fn'".to_string(),
                    hint: Some("use 'func' instead of 'fn'".to_string()),
                    why: None,
                });
            }
        }

        let kind = match self.current_kind() {
            TokenKind::Func => {
                self.reject_keyword_fn_name()?;
                self.parse_fn_decl(is_pub, false, is_comptime, is_unsafe, attrs, doc)?
            }
            TokenKind::Struct => self.parse_struct_decl(is_pub, attrs, doc)?,
            TokenKind::Enum => self.parse_enum_decl(is_pub, attrs, doc)?,
            TokenKind::Union => self.parse_union_decl(is_pub, doc)?,
            TokenKind::Trait => self.parse_trait_decl(is_pub, is_unsafe, is_duck, attrs, doc)?,
            TokenKind::Extend => self.parse_impl_decl(is_unsafe, is_scoped, doc)?,
            TokenKind::Import => self.parse_import_decl()?,
            TokenKind::Export => self.parse_export_decl()?,
            TokenKind::Const => self.parse_const_decl(is_pub, doc)?,
            TokenKind::Type => self.parse_type_alias_decl(is_pub)?,
            TokenKind::Test => self.parse_test_decl(is_comptime)?,
            TokenKind::Benchmark => self.parse_benchmark_decl()?,
            TokenKind::Extern => {
                let mut kinds = self.parse_extern_decls(doc)?;
                let first = kinds.remove(0);
                let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);
                let pending: Vec<Decl> = kinds.into_iter().map(|kind| {
                    let id = self.next_id();
                    Decl { id, kind, span: self.span(start, end) }
                }).collect();
                self.pending_decls.extend(pending);
                first
            }
            TokenKind::Package => {
                if is_pub || is_comptime || is_unsafe || !attrs.is_empty() {
                    return Err(ParseError {
                        span: self.current().span,
                        message: "package declarations cannot have modifiers".to_string(),
                        hint: Some("remove 'public', 'comptime', 'unsafe', or attributes".to_string()),
                        why: None,
                    });
                }
                self.parse_package_decl()?
            }
            _ => {
                return Err(ParseError::expected(
                    "declaration (func, struct, enum, union, trait, extend, import, export, const, type, test, benchmark, extern, package)",
                    self.current_kind(),
                    self.current().span,
                ));
            }
        };

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);
        Ok(Decl { id: self.next_id(), kind, span: self.span(start, end) })
    }

    fn parse_attribute(&mut self) -> Result<String, ParseError> {
        self.expect(&TokenKind::At)?;
        // Use expect_ident_or_keyword so @test, @benchmark etc. work
        let mut attr = self.expect_ident_or_keyword()?;

        if self.match_token(&TokenKind::LParen) {
            attr.push('(');
            let mut depth = 1;
            while depth > 0 && !self.at_end() {
                match self.current_kind() {
                    TokenKind::LParen => depth += 1,
                    TokenKind::RParen => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    // Preserve original token text for strings, idents, etc.
                    match self.current_kind() {
                        TokenKind::String(s) => {
                            attr.push('"');
                            attr.push_str(s);
                            attr.push('"');
                        }
                        TokenKind::Ident(s) => attr.push_str(s),
                        TokenKind::Int(n, _) => attr.push_str(&n.to_string()),
                        TokenKind::Comma => attr.push_str(", "),
                        // Keywords, operators, and delimiters keep their source
                        // text. Debug output here would mangle anything with
                        // punctuation — a lint rule id like
                        // `@allow(idiom/duck-trait)` came out as
                        // `allow(idiomSlashduck-Trait)` and matched nothing,
                        // breaking per-rule suppression (tool.lint/SU1).
                        k => attr.push_str(k.display_name().trim_matches('\'')),
                    }
                }
                self.advance();
            }
            attr.push(')');
        }

        Ok(attr)
    }

    // =========================================================================
    // Declaration Parsing
    // =========================================================================


    /// A method's own type parameters stay in `type_params`; they don't belong
    /// in its name. `parse_fn_decl` folds them into the name for display, which
    /// is what a free function wants — but a method is looked up by the name the
    /// call site writes, and `w.tag(7)` writes `tag`, not `tag<E>`.
    fn as_method(mut fn_decl: FnDecl) -> FnDecl {
        if let Some(base) = fn_decl.name.split('<').next() {
            if base.len() != fn_decl.name.len() {
                fn_decl.name = base.to_string();
            }
        }
        fn_decl
    }

    /// A free function named with a keyword can be declared but never called
    /// (#500). Say so at the declaration, where the name is, instead of leaving
    /// a type error pointing at some argument. Methods are exempt: `x.check()`
    /// is unambiguous, and that's how `Option.or` is spelled.
    ///
    /// There are two ways it goes wrong and the reason has to match, or the
    /// message claims something that isn't true. `check(r)` parses as the
    /// check-expression — the call is read as something else. `func struct()`
    /// doesn't get that far: `struct` opens a declaration, so the name is never
    /// read as a name at all.
    fn reject_keyword_fn_name(&mut self) -> Result<(), ParseError> {
        if self.allow_keyword_fn_names {
            return Ok(());
        }
        let name_tok = &self.tokens[(self.pos + 1).min(self.tokens.len() - 1)];
        let keyword = match &name_tok.kind {
            TokenKind::Ident(_) => return Ok(()),
            other => keyword_spelling(other),
        };
        let Some(keyword) = keyword else { return Ok(()) };
        let (why, hint) = if starts_an_expression(&name_tok.kind) {
            (
                format!(
                    "`{0}(…)` at a call site parses as the `{0}` expression, so this function could never be called",
                    keyword
                ),
                format!("pick another name, or make it a method so the call reads `x.{}()`", keyword),
            )
        } else {
            (
                format!(
                    "`{0}` only ever starts a declaration, so the parser never reads it as a name",
                    keyword
                ),
                "pick another name".to_string(),
            )
        };
        Err(ParseError {
            span: name_tok.span,
            message: format!("`{}` is a keyword, so it can't name a function", keyword),
            hint: Some(hint),
            why: None,
        }
        .with_why(why))
    }

    fn parse_fn_decl(&mut self, is_pub: bool, is_private: bool, is_comptime: bool, is_unsafe: bool, attrs: Vec<String>, doc: Option<String>) -> Result<DeclKind, ParseError> {
        let fn_start = self.current().span.start;
        self.expect(&TokenKind::Func)?;
        // Allow keywords as function names (e.g., `or` for Option.or)
        let mut name = self.expect_ident_or_keyword()?;

        let mut type_params = if self.match_token(&TokenKind::Lt) {
            let (params, suffix) = self.parse_type_params()?;
            name.push_str(&suffix);
            params
        } else {
            vec![]
        };

        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;

        // Parse `using` clauses and `->` return type (either order accepted)
        let mut context_clauses = self.parse_using_clauses()?;

        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            // Elided return type: `func foo()` means returns void.
            // `or E` must attach to an explicit return type — a bare `or` here
            // would mean "void or E", which the spec forbids (type.errors, SYNTAX.md).
            if self.check(&TokenKind::Or) {
                let or_span = self.current().span;
                return Err(ParseError {
                    span: or_span,
                    message: "`or` must follow an explicit return type".to_string(),
                    hint: Some("write `-> void or E` (or pick the concrete success type)".to_string()),
                    why: None,
                });
            }
            None
        };

        // Also accept `using` after return type
        if context_clauses.is_empty() {
            context_clauses = self.parse_using_clauses()?;
        }

        // `where` closes the signature (after return type + using clauses).
        self.parse_where_clause(&mut type_params)?;

        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else if self.check(&TokenKind::Newline) {
            self.skip_newlines();
            if self.check(&TokenKind::LBrace) {
                self.parse_block_body()?
            } else {
                Vec::new()
            }
        } else if self.check(&TokenKind::Semi) {
            self.advance();
            self.skip_newlines();
            Vec::new()
        } else if self.check(&TokenKind::Eof) || self.check(&TokenKind::RBrace) {
            Vec::new()
        } else {
            return Err(ParseError::expected(
                "'{' or newline",
                self.current_kind(),
                self.current().span,
            ));
        };

        let fn_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        let fn_span = self.span(fn_start, fn_end);
        Ok(DeclKind::Fn(FnDecl { name, type_params, params, ret_ty, context_clauses, body, is_pub, is_private, is_comptime, is_unsafe, abi: None, attrs, doc, span: fn_span }))
    }

    fn parse_using_clauses(&mut self) -> Result<Vec<ContextClause>, ParseError> {
        // Skip newlines that might appear between return type and `using`
        if self.check(&TokenKind::Newline) {
            // Peek past newlines to see if `using` follows
            let saved = self.pos;
            self.skip_newlines();
            if !self.check(&TokenKind::Using) {
                self.pos = saved;
                return Ok(vec![]);
            }
        }

        if !self.match_token(&TokenKind::Using) {
            return Ok(vec![]);
        }

        let mut clauses = Vec::new();
        loop {
            self.skip_newlines();

            // Check for `frozen` modifier
            let is_frozen = if let TokenKind::Ident(ref name) = self.current_kind().clone() {
                if name == "frozen" {
                    self.advance();
                    true
                } else {
                    false
                }
            } else {
                false
            };

            // Peek: Ident Colon means named context, otherwise unnamed
            let name = if let TokenKind::Ident(ref ident) = self.current_kind().clone() {
                // Look ahead for `name:`
                if self.pos + 1 < self.tokens.len() && self.tokens[self.pos + 1].kind == TokenKind::Colon {
                    let n = ident.clone();
                    self.advance(); // consume name
                    self.advance(); // consume colon
                    Some(n)
                } else {
                    None
                }
            } else {
                None
            };

            let ty = self.parse_type_name()?;

            clauses.push(ContextClause { name, ty, is_frozen });

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }

        Ok(clauses)
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();

        self.skip_newlines();
        if self.check(&TokenKind::RParen) {
            return Ok(params);
        }

        loop {
            let is_take = self.match_token(&TokenKind::Take);
            let is_mutate = if !is_take { self.match_token(&TokenKind::MutateKw) } else { false };
            let name_span = self.current().span;
            let name = self.expect_ident_or_keyword()?;

            let ty = if self.match_token(&TokenKind::Colon) {
                self.parse_type_name()?
            } else if name == "self" {
                "Self".to_string()
            } else {
                // GC1: Allow omitting parameter type — inferred from body
                String::new()
            };

            let default = if self.match_token(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            params.push(Param { name, name_span, ty, is_take, is_mutate, default });

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
            // Trailing comma before closing paren
            if self.check(&TokenKind::RParen) {
                break;
            }
        }

        // Validate: optional params (with defaults) must come after all required params
        let mut saw_default = false;
        for param in &params {
            if param.name == "self" { continue; }
            if param.default.is_some() {
                saw_default = true;
            } else if saw_default {
                return Err(ParseError::expected(
                    "default value",
                    &TokenKind::Eq,
                    param.name_span,
                ).with_hint(
                    "Required parameters must come before optional ones. Add a default value with `= expr`"
                ));
            }
        }

        Ok(params)
    }

    /// Parse one parameter inside a function type: `T`, `name: T`, or `mutate name: T`.
    /// In type position, names and modifiers are noise — only the type part is kept.
    fn parse_func_type_param(&mut self) -> Result<String, ParseError> {
        // Skip optional `mutate` modifier
        if matches!(self.current_kind(), TokenKind::MutateKw) {
            self.advance();
        }

        // If `name :` precedes the type, skip the name and colon
        if let TokenKind::Ident(_) = self.current_kind() {
            if matches!(self.peek(1), TokenKind::Colon) {
                self.advance(); // name
                self.advance(); // :
            }
        }

        self.parse_type_name()
    }

    fn parse_type_name(&mut self) -> Result<String, ParseError> {
        let base = self.parse_base_type()?;

        if self.check(&TokenKind::Or) {
            self.advance();
            let error_ty = self.parse_error_type()?;
            return Ok(format!("Result<{}, {}>", base, error_ty));
        }

        Ok(base)
    }

    /// Parse an error type, which may be a union: `E` or `(E1 | E2 | E3)` or `E1 | E2`.
    fn parse_error_type(&mut self) -> Result<String, ParseError> {
        // Check for parenthesized union: (E1 | E2)
        if self.check(&TokenKind::LParen) {
            self.advance();
            let mut types = vec![self.parse_base_type()?];
            while self.match_token(&TokenKind::Pipe) {
                types.push(self.parse_base_type()?);
            }
            self.expect(&TokenKind::RParen)?;
            if types.len() == 1 {
                return Ok(types.into_iter().next().unwrap());
            }
            return Ok(types.join("|"));
        }

        // Single error type, possibly followed by | for bare union
        let first = self.parse_base_type()?;
        if self.check(&TokenKind::Pipe) {
            let mut types = vec![first];
            while self.match_token(&TokenKind::Pipe) {
                types.push(self.parse_base_type()?);
            }
            return Ok(types.join("|"));
        }
        Ok(first)
    }

    fn parse_base_type(&mut self) -> Result<String, ParseError> {
        // Reference types are not yet implemented
        if self.check(&TokenKind::Amp) {
            let span = self.current().span;
            return Err(ParseError::not_implemented(
                "reference types",
                "remove the '&' - Rask currently uses owned values",
                span,
            ));
        }

        // Handle raw pointer types: *T
        if self.check(&TokenKind::Star) {
            self.advance();
            let pointee_ty = self.parse_type_name()?;
            return Ok(format!("*{}", pointee_ty));
        }

        if self.check(&TokenKind::LParen) {
            let lparen_span = self.current().span;
            self.advance();
            if self.check(&TokenKind::RParen) {
                // type.primitives/P6: () is no longer the unit type — use `void`.
                let rparen_span = self.current().span;
                let span = Span::with_file(lparen_span.start, rparen_span.end, lparen_span.file_id);
                return Err(ParseError {
                    span,
                    message: "`()` is not a type".to_string(),
                    hint: Some("use `void` for the zero-sized type".to_string()),
                    why: None,
                });
            }
            let first_ty = self.parse_type_name()?;
            if self.match_token(&TokenKind::Comma) {
                // Tuple type: (T, U, ...) — arity >= 2 (type.tuples/TU1)
                let mut types = vec![first_ty];
                while !self.check(&TokenKind::RParen) && !self.at_end() {
                    types.push(self.parse_type_name()?);
                    if !self.match_token(&TokenKind::Comma) { break; }
                }
                let rparen_span = self.current().span;
                self.expect(&TokenKind::RParen)?;
                if types.len() == 1 {
                    // type.tuples/TU3: 1-tuples are not a thing
                    let span = Span::with_file(lparen_span.start, rparen_span.end, lparen_span.file_id);
                    return Err(ParseError {
                        span,
                        message: "1-tuples are not supported".to_string(),
                        hint: Some(format!("tuples have arity >= 2; use `{}` directly", types[0])),
                        why: None,
                    });
                }
                return Ok(format!("({})", types.join(", ")));
            }
            // Parenthesized type: (T) — not a tuple
            self.expect(&TokenKind::RParen)?;
            return Ok(first_ty);
        }

        // `void` keyword: zero-sized unit type (type.primitives/P6)
        if self.check(&TokenKind::Void) {
            self.advance();
            return Ok("()".to_string());
        }

        // `none` keyword: zero-sized absent-sentinel type (type.primitives/P7).
        // Allowed in type position only; the same token is also the absent
        // literal in expression position.
        if self.check(&TokenKind::None) {
            self.advance();
            return Ok("none".to_string());
        }

        if self.check(&TokenKind::LBracket) {
            self.advance();

            if self.check(&TokenKind::RBracket) {
                self.advance();
                let elem_ty = self.parse_type_name()?;
                return Ok(format!("[]{}", elem_ty));
            }

            // [N]T — fixed-count type (used by @binary byte arrays)
            if let TokenKind::Int(n, _) = self.current_kind().clone() {
                if matches!(self.peek(1), TokenKind::RBracket) {
                    let count = n;
                    self.advance(); // consume N
                    self.advance(); // consume ]
                    let elem_ty = self.parse_base_type()?;
                    return Ok(format!("[{}]{}", count, elem_ty));
                }
            }

            let elem_ty = self.parse_type_name()?;
            self.expect(&TokenKind::Semi)?;
            let size = match self.current_kind().clone() {
                TokenKind::Int(n, _) => {
                    self.advance();
                    n.to_string()
                }
                TokenKind::Ident(name) => {
                    self.advance();
                    name
                }
                _ => return Err(ParseError::expected(
                    "array size (number or name)",
                    self.current_kind(),
                    self.current().span,
                )),
            };
            self.expect(&TokenKind::RBracket)?;
            return Ok(format!("[{}; {}]", elem_ty, size));
        }

        if let TokenKind::Int(n, _) = self.current_kind().clone() {
            self.advance();
            return Ok(n.to_string());
        }

        // Closure type: |T1, T2| -> R, or with named/modified params:
        //   |name: T|, |mutate name: T| (names are noise in type position)
        if self.check(&TokenKind::Pipe) {
            self.advance();
            let mut params = Vec::new();
            if !self.check(&TokenKind::Pipe) {
                loop {
                    params.push(self.parse_func_type_param()?);
                    if !self.match_token(&TokenKind::Comma) { break; }
                }
            }
            self.expect(&TokenKind::Pipe)?;

            let ret_ty = if self.match_token(&TokenKind::Arrow) {
                self.parse_type_name()?
            } else {
                "()".to_string()
            };

            return Ok(format!("func({}) -> {}", params.join(", "), ret_ty));
        }

        if self.check(&TokenKind::Func) {
            self.advance();
            self.expect(&TokenKind::LParen)?;

            let mut params = Vec::new();
            if !self.check(&TokenKind::RParen) {
                loop {
                    params.push(self.parse_func_type_param()?);
                    if !self.match_token(&TokenKind::Comma) { break; }
                }
            }
            self.expect(&TokenKind::RParen)?;

            let ret_ty = if self.match_token(&TokenKind::Arrow) {
                self.parse_type_name()?
            } else {
                "()".to_string()
            };

            return Ok(format!("func({}) -> {}", params.join(", "), ret_ty));
        }

        let mut name = self.expect_ident()?;

        if name == "any" {
            if let TokenKind::Ident(_) = self.current_kind() {
                let mut trait_name = self.expect_ident()?;
                if self.match_token(&TokenKind::Lt) {
                    trait_name.push('<');
                    loop {
                        if let TokenKind::Int(n, _) = self.current_kind().clone() {
                            self.advance();
                            trait_name.push_str(&n.to_string());
                        } else {
                            trait_name.push_str(&self.parse_type_name()?);
                        }
                        if self.pending_gt {
                            break;
                        }
                        if self.match_token(&TokenKind::Comma) {
                            trait_name.push_str(", ");
                        } else {
                            break;
                        }
                    }
                    self.expect_gt_in_generic()?;
                    trait_name.push('>');
                }
                return Ok(format!("any {}", trait_name));
            }
        }

        while self.check(&TokenKind::Dot) && !matches!(self.peek(1), TokenKind::LBrace) {
            self.advance();
            name.push('.');
            name.push_str(&self.expect_ident()?);
        }

        if self.match_token(&TokenKind::Lt) {
            name.push('<');
            loop {
                if let TokenKind::Int(n, _) = self.current_kind().clone() {
                    self.advance();
                    name.push_str(&n.to_string());
                } else {
                    name.push_str(&self.parse_type_name()?);
                }
                // If >> was split, pending_gt means the next > belongs to this
                // generic's closing bracket — don't consume a comma.
                if self.pending_gt {
                    break;
                }
                if self.match_token(&TokenKind::Comma) {
                    name.push_str(", ");
                } else {
                    break;
                }
            }
            self.expect_gt_in_generic()?;
            name.push('>');
        }

        // Optional markers stack: `T?`, `T??`, … Each `?` adds a `none` layer
        // (type.optionals/OPT28, OPT31). The lexer hands back `??` as one token
        // — in type position there is no coalescing operator, so it is just two
        // markers.
        loop {
            if self.match_token(&TokenKind::QuestionQuestion) {
                name.push_str("??");
            } else if self.match_token(&TokenKind::Question) {
                name.push('?');
            } else {
                break;
            }
        }

        Ok(name)
    }

    /// Parse type parameters like `<T, comptime N: usize>`.
    /// Returns (type_params, name_suffix) where name_suffix is the string representation for display.
    fn parse_type_params(&mut self) -> Result<(Vec<TypeParam>, String), ParseError> {
        let mut type_params = Vec::new();
        let mut name_suffix = String::from("<");

        loop {
            let is_comptime = self.match_token(&TokenKind::Comptime);
            let param_name = self.expect_ident()?;

            if is_comptime {
                // Const generic: `comptime N: usize`
                self.expect(&TokenKind::Colon)?;
                let comptime_type = self.parse_type_name()?;

                type_params.push(TypeParam {
                    name: param_name.clone(),
                    is_comptime: true,
                    comptime_type: Some(comptime_type.clone()),
                    bounds: vec![],
                });

                name_suffix.push_str("comptime ");
                name_suffix.push_str(&param_name);
                name_suffix.push_str(": ");
                name_suffix.push_str(&comptime_type);
            } else {
                // Regular type parameter: `T` or `T: Trait` or `T: A + B`
                let mut bounds = vec![];
                if self.match_token(&TokenKind::Colon) {
                    bounds = self.parse_trait_bounds()?;
                }

                type_params.push(TypeParam {
                    name: param_name.clone(),
                    is_comptime: false,
                    comptime_type: None,
                    bounds: bounds.clone(),
                });

                name_suffix.push_str(&param_name);
                if !bounds.is_empty() {
                    name_suffix.push_str(": ");
                    name_suffix.push_str(&bounds.join(" + "));
                }
            }

            // If >> was split in a nested generic bound, pending_gt is our closing >
            if self.pending_gt {
                break;
            }
            if self.match_token(&TokenKind::Comma) {
                name_suffix.push_str(", ");
            } else {
                break;
            }
        }

        self.expect_gt_in_generic()?;
        name_suffix.push('>');

        Ok((type_params, name_suffix))
    }

    /// Parse a single trait bound, e.g. `Comparable` or `Iterator<Item>`.
    fn parse_one_bound(&mut self) -> Result<String, ParseError> {
        let mut bound = self.expect_ident()?;
        // Generic trait bound: `Iterator<Item>`
        if self.match_token(&TokenKind::Lt) {
            bound.push('<');
            bound.push_str(&self.parse_type_name()?);
            while self.match_token(&TokenKind::Comma) {
                bound.push_str(", ");
                bound.push_str(&self.parse_type_name()?);
            }
            self.expect_gt_in_generic()?;
            bound.push('>');
        }
        Ok(bound)
    }

    /// Parse `+`-separated trait bounds: `A + B<X> + C`.
    fn parse_trait_bounds(&mut self) -> Result<Vec<String>, ParseError> {
        let mut bounds = vec![self.parse_one_bound()?];
        while self.match_token(&TokenKind::Plus) {
            bounds.push(self.parse_one_bound()?);
        }
        Ok(bounds)
    }

    /// Parse an optional `where` clause and fold its bounds into `type_params`.
    ///
    /// `where T: A + B, U: C` — bounds attach to the named parameter. A name
    /// that isn't an explicitly-declared param (an implicit single-letter
    /// generic like `T` in `func sort(items: Vec<T>) where T: Comparable`)
    /// gets a fresh entry, so the clause is equivalent to writing `<T: A + B>`.
    ///
    /// Signature grammar order is `generics → params → return → using → where`,
    /// so this runs last, after the return type and `using` clauses.
    fn parse_where_clause(&mut self, type_params: &mut Vec<TypeParam>) -> Result<(), ParseError> {
        // `where` may sit on its own line below the signature.
        if self.check(&TokenKind::Newline) {
            let saved = self.pos;
            self.skip_newlines();
            if !self.check(&TokenKind::Where) {
                self.pos = saved;
                return Ok(());
            }
        }

        if !self.match_token(&TokenKind::Where) {
            return Ok(());
        }

        loop {
            self.skip_newlines();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let bounds = self.parse_trait_bounds()?;

            match type_params.iter_mut().find(|tp| tp.name == name) {
                Some(tp) => tp.bounds.extend(bounds),
                None => type_params.push(TypeParam {
                    name,
                    is_comptime: false,
                    comptime_type: None,
                    bounds,
                }),
            }

            // Constraints are comma-separated; a newline ends the clause.
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }

        Ok(())
    }

    fn parse_struct_decl(&mut self, is_pub: bool, attrs: Vec<String>, doc: Option<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Struct)?;
        let mut name = self.expect_ident()?;

        let type_params = if self.match_token(&TokenKind::Lt) {
            let (params, suffix) = self.parse_type_params()?;
            name.push_str(&suffix);
            params
        } else {
            vec![]
        };

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut methods = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            if self.check(&TokenKind::DotDot) {
                self.advance();
                if self.check(&TokenKind::Dot) {
                    self.advance();
                }
                self.skip_newlines();
                continue;
            }

            let method_doc = self.take_doc();

            // Field annotations: @rename("..."), @skip, @default(expr).
            let mut field_attrs = Vec::new();
            while self.check(&TokenKind::At) {
                field_attrs.push(self.parse_attribute()?);
                self.skip_newlines();
            }

            let field_private = self.match_token(&TokenKind::Private);
            let field_pub = if !field_private { self.match_token(&TokenKind::Public) } else { false };

            if self.check(&TokenKind::Func) {
                if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(field_pub, field_private, false, false, field_attrs, method_doc)? {
                    methods.push(Self::as_method(fn_decl));
                }
            } else {
                let visibility = if field_private {
                    FieldVisibility::Private
                } else if field_pub {
                    FieldVisibility::Public
                } else {
                    FieldVisibility::Package
                };
                let name_span = self.current().span;
                let field_name = self.expect_ident_or_keyword()?;
                self.expect(&TokenKind::Colon)?;
                let ty = self.parse_type_name()?;
                // FD1: declared default — `port: i32 = 8080`.
                // Comptime-const validation happens in desugar (same as param defaults).
                let default = if self.match_token(&TokenKind::Eq) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                fields.push(Field { name: field_name, name_span, ty, visibility, attrs: field_attrs, default });
            }

            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Struct(StructDecl {
            name,
            type_params,
            fields,
            methods,
            is_pub,
            attrs,
            doc,
        }))
    }

    fn parse_union_decl(&mut self, is_pub: bool, doc: Option<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Union)?;
        let name = self.expect_ident()?;

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut fields = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let _field_doc = self.take_doc();
            let field_private = self.match_token(&TokenKind::Private);
            let field_pub = if !field_private { self.match_token(&TokenKind::Public) } else { false };

            if self.check(&TokenKind::Func) {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unions cannot have methods".to_string(),
                    hint: Some("define methods separately with extend".to_string()),
                    why: None,
                });
            }

            let visibility = if field_private {
                FieldVisibility::Private
            } else if field_pub {
                FieldVisibility::Public
            } else {
                FieldVisibility::Package
            };
            let name_span = self.current().span;
            let field_name = self.expect_ident_or_keyword()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type_name()?;
            fields.push(Field { name: field_name, name_span, ty, visibility, attrs: vec![], default: None });

            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Union(UnionDecl {
            name,
            fields,
            is_pub,
            doc,
        }))
    }

    fn parse_enum_decl(&mut self, is_pub: bool, attrs: Vec<String>, doc: Option<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Enum)?;
        let mut name = self.expect_ident()?;

        let type_params = if self.match_token(&TokenKind::Lt) {
            let (params, suffix) = self.parse_type_params()?;
            name.push_str(&suffix);
            params
        } else {
            vec![]
        };

        // E14: Optional backing type (e.g., enum Foo: u8 { ... })
        let backing_type = if self.match_token(&TokenKind::Colon) {
            Some(self.expect_ident()?)
        } else {
            None
        };

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut variants = Vec::new();
        let mut methods = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            // Parse variant-level attributes (e.g., @message("template"))
            let mut variant_attrs = Vec::new();
            while self.check(&TokenKind::At) {
                variant_attrs.push(self.parse_attribute()?);
                self.skip_newlines();
            }

            let item_doc = self.take_doc();
            if self.check(&TokenKind::Func) || (self.check(&TokenKind::Public) && matches!(self.peek(1), TokenKind::Func)) || (self.check(&TokenKind::Private) && matches!(self.peek(1), TokenKind::Func)) {
                let m_private = self.match_token(&TokenKind::Private);
                let m_pub = if !m_private { self.match_token(&TokenKind::Public) } else { false };
                if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(m_pub, m_private, false, false, vec![], item_doc)? {
                    methods.push(Self::as_method(fn_decl));
                }
            } else {
                let _variant_doc = item_doc;
                let variant_name = self.expect_ident()?;
                let mut fields = Vec::new();

                if self.match_token(&TokenKind::LParen) {
                    let mut idx = 0;
                    while !self.check(&TokenKind::RParen) && !self.at_end() {
                        let (field_name, name_span, ty) = if self.check(&TokenKind::Ident(String::new())) {
                            if self.peek(1) == &TokenKind::Colon {
                                let span = self.current().span;
                                let name = self.expect_ident()?;
                                self.advance();
                                let ty = self.parse_type_name()?;
                                (name, span, ty)
                            } else {
                                let type_span = self.current().span;
                                let ty = self.parse_type_name()?;
                                (format!("_{}", idx), type_span, ty)
                            }
                        } else {
                            let type_span = self.current().span;
                            let ty = self.parse_type_name()?;
                            (format!("_{}", idx), type_span, ty)
                        };

                        fields.push(Field { name: field_name, name_span, ty, visibility: FieldVisibility::Package, attrs: vec![], default: None });
                        idx += 1;

                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen)?;
                } else if self.check(&TokenKind::LBrace) {
                    // Struct-style variant: Move { x: i32, y: i32 }
                    self.advance();
                    self.skip_newlines();
                    while !self.check(&TokenKind::RBrace) && !self.at_end() {
                        let name_span = self.current().span;
                        let field_name = self.expect_ident()?;
                        self.expect(&TokenKind::Colon)?;
                        let ty = self.parse_type_name()?;
                        fields.push(Field { name: field_name, name_span, ty, visibility: FieldVisibility::Package, attrs: vec![], default: None });
                        if !self.match_token(&TokenKind::Comma) {
                            self.skip_newlines();
                            if !self.check(&TokenKind::RBrace) { continue; }
                        } else {
                            self.skip_newlines();
                        }
                    }
                    self.expect(&TokenKind::RBrace)?;
                }

                // E15: Optional explicit discriminant value (e.g., = 42)
                let discriminant = if self.match_token(&TokenKind::Eq) {
                    let negative = self.match_token(&TokenKind::Minus);
                    if let TokenKind::Int(n, _) = &self.current().kind {
                        let val = *n as i128;
                        self.advance();
                        Some(if negative { -val } else { val })
                    } else {
                        return Err(ParseError::expected(
                        "integer literal",
                        self.current_kind(),
                        self.current().span,
                    ));
                    }
                } else {
                    None
                };

                variants.push(Variant { name: variant_name, fields, attrs: variant_attrs, discriminant });
            }

            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Enum(EnumDecl {
            name,
            type_params,
            variants,
            methods,
            is_pub,
            attrs,
            doc,
            backing_type,
        }))
    }

    fn parse_trait_decl(&mut self, is_pub: bool, is_unsafe: bool, is_duck: bool, attrs: Vec<String>, doc: Option<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Trait)?;
        let name = self.expect_ident()?;

        if self.match_token(&TokenKind::Lt) {
            while !self.check(&TokenKind::Gt) && !self.at_end() {
                self.advance();
            }
            self.expect(&TokenKind::Gt)?;
        }

        // Super-traits: trait Display: ToString, Debug { ... }
        let mut super_traits = Vec::new();
        if self.match_token(&TokenKind::Colon) {
            loop {
                self.skip_newlines();
                super_traits.push(self.parse_type_name()?);
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
        }

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let method_doc = self.take_doc();
            if self.check(&TokenKind::Func) {
                if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(false, false, false, false, vec![], method_doc)? {
                    methods.push(Self::as_method(fn_decl));
                }
            } else if let TokenKind::Ident(_) = self.current_kind() {
                let mut fn_decl = self.parse_trait_method_shorthand()?;
                fn_decl.doc = method_doc;
                methods.push(fn_decl);
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Trait(TraitDecl { name, super_traits, methods, is_pub, is_unsafe, is_duck, attrs, doc }))
    }

    fn parse_trait_method_shorthand(&mut self) -> Result<FnDecl, ParseError> {
        let fn_start = self.current().span.start;
        let mut name = self.expect_ident()?;

        let mut type_params = if self.match_token(&TokenKind::Lt) {
            let (params, suffix) = self.parse_type_params()?;
            name.push_str(&suffix);
            params
        } else {
            vec![]
        };

        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;

        let mut context_clauses = self.parse_using_clauses()?;

        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        if context_clauses.is_empty() {
            context_clauses = self.parse_using_clauses()?;
        }

        self.parse_where_clause(&mut type_params)?;

        if self.check(&TokenKind::Newline) {
            self.skip_newlines();
        }
        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            Vec::new()
        };

        let fn_end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(FnDecl {
            name,
            type_params,
            params,
            ret_ty,
            context_clauses,
            body,
            is_pub: false, is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: self.span(fn_start, fn_end),
        })
    }

    fn parse_impl_decl(&mut self, is_unsafe: bool, is_scoped: bool, doc: Option<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Extend)?;
        let target_ty = self.parse_type_name()?;

        // CD1: `extend T with A, B, C` — comma-separated conformance list.
        let mut trait_names = Vec::new();
        if self.match_token(&TokenKind::With) {
            loop {
                self.skip_newlines();
                trait_names.push(self.parse_type_name()?);
                if !self.match_token(&TokenKind::Comma) {
                    break;
                }
            }
        }

        // CC2: conditional conformance condition — `where T: Displayable`.
        let mut where_bounds = Vec::new();
        self.parse_where_clause(&mut where_bounds)?;

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let saved_pos = self.pos;
            let mut method_attrs = Vec::new();
            while self.check(&TokenKind::At) {
                match self.parse_attribute() {
                    Ok(attr) => method_attrs.push(attr),
                    Err(e) => {
                        self.record_error(e);
                        self.synchronize_to_next_method();
                        continue;
                    }
                }
                self.skip_newlines();
            }
            let method_doc = self.take_doc();
            let m_private = self.match_token(&TokenKind::Private);
            let m_pub = if !m_private { self.match_token(&TokenKind::Public) } else { false };
            let m_comptime = self.match_token(&TokenKind::Comptime);
            let m_unsafe = if !m_comptime { self.match_token(&TokenKind::Unsafe) } else { false };
            match self.parse_fn_decl(m_pub, m_private, m_comptime, m_unsafe, method_attrs, method_doc) {
                Ok(DeclKind::Fn(fn_decl)) => methods.push(Self::as_method(fn_decl)),
                Ok(_) => {}
                Err(e) => {
                    self.record_error(e);
                    self.synchronize_to_next_method();
                    if self.pos == saved_pos && !self.at_end() {
                        self.advance();
                    }
                    self.skip_newlines();
                    continue;
                }
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Impl(ImplDecl { trait_names, target_ty, methods, is_unsafe, is_scoped, where_bounds, doc }))
    }

    fn parse_import_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Import)?;

        // CI1: Check for `import c "header.h"` syntax
        if matches!(self.current_kind(), TokenKind::Ident(s) if s == "c") {
            return self.parse_c_import();
        }

        let is_lazy = self.match_token(&TokenKind::Lazy);

        let mut path = Vec::new();
        let mut is_glob = false;

        path.push(self.expect_ident()?);

        while self.match_token(&TokenKind::Dot) {
            if self.match_token(&TokenKind::Star) {
                is_glob = true;
                break;
            }
            if self.check(&TokenKind::LBrace) {
                return self.parse_grouped_imports(path, is_lazy);
            }
            path.push(self.expect_ident()?);
        }

        let alias = if self.match_token(&TokenKind::As) {
            Some(self.expect_ident()?)
        } else {
            None
        };

        self.expect_terminator()?;
        Ok(DeclKind::Import(ImportDecl { path, alias, is_glob, is_lazy }))
    }

    /// Parse `import c "header.h"` (CI1).
    /// Variants:
    /// - `import c "header.h"` — single header, `c` namespace
    /// - `import c "header.h" as name` — aliased namespace
    /// - `import c { "a.h", "b.h" }` — multiple headers
    /// - `import c "header.h" hiding { symbol1, symbol2 }` — suppress symbols
    fn parse_c_import(&mut self) -> Result<DeclKind, ParseError> {
        self.expect_ident()?; // consume "c"

        let mut headers = Vec::new();

        if self.match_token(&TokenKind::LBrace) {
            // Multiple headers: import c { "a.h", "b.h" }
            self.skip_newlines();
            loop {
                if self.check(&TokenKind::RBrace) { break; }
                headers.push(self.expect_string()?);
                if !self.match_token(&TokenKind::Comma) { break; }
                self.skip_newlines();
            }
            self.expect(&TokenKind::RBrace)?;
        } else {
            headers.push(self.expect_string()?);
        }

        // Optional alias: `as name`
        let alias = if self.match_token(&TokenKind::As) {
            self.expect_ident()?
        } else {
            "c".to_string()
        };

        // Optional hiding: `hiding { symbol1, symbol2 }`
        let mut hiding = Vec::new();
        if matches!(self.current_kind(), TokenKind::Ident(s) if s == "hiding") {
            self.advance();
            self.expect(&TokenKind::LBrace)?;
            self.skip_newlines();
            loop {
                if self.check(&TokenKind::RBrace) { break; }
                hiding.push(self.expect_ident()?);
                if !self.match_token(&TokenKind::Comma) { break; }
                self.skip_newlines();
            }
            self.expect(&TokenKind::RBrace)?;
        }

        self.expect_terminator()?;
        Ok(DeclKind::CImport(CImportDecl { headers, alias, hiding }))
    }

    /// Expand grouped imports into individual decls.
    fn parse_grouped_imports(&mut self, base_path: Vec<String>, is_lazy: bool) -> Result<DeclKind, ParseError> {
        let start = self.tokens.get(self.pos.saturating_sub(base_path.len() + 2))
            .map(|t| t.span.start)
            .unwrap_or(0);

        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut items: Vec<(String, Option<String>)> = Vec::new();

        loop {
            if self.check(&TokenKind::RBrace) {
                if items.is_empty() {
                    return Err(ParseError::expected("identifier", self.current_kind(), self.current().span));
                }
                break;
            }

            let name = self.expect_ident()?;
            let alias = if self.match_token(&TokenKind::As) {
                Some(self.expect_ident()?)
            } else {
                None
            };
            items.push((name, alias));

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }

        self.skip_newlines();
        self.expect(&TokenKind::RBrace)?;
        self.expect_terminator()?;

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);

        for i in (1..items.len()).rev() {
            let (ref name, ref alias) = items[i];
            let mut path = base_path.clone();
            path.push(name.clone());
            let decl = Decl {
                id: self.next_id(),
                kind: DeclKind::Import(ImportDecl {
                    path,
                    alias: alias.clone(),
                    is_glob: false,
                    is_lazy,
                }),
                span: self.span(start, end),
            };
            self.pending_decls.push(decl);
        }

        let (name, alias) = items.into_iter().next().unwrap();
        let mut path = base_path;
        path.push(name);
        Ok(DeclKind::Import(ImportDecl { path, alias, is_glob: false, is_lazy }))
    }

    fn parse_export_decl(&mut self) -> Result<DeclKind, ParseError> {
        use rask_ast::decl::{ExportDecl, ExportItem};

        self.expect(&TokenKind::Export)?;

        let mut items = Vec::new();

        loop {
            let mut path = Vec::new();
            path.push(self.expect_ident()?);
            while self.match_token(&TokenKind::Dot) {
                path.push(self.expect_ident()?);
            }

            let alias = if self.match_token(&TokenKind::As) {
                Some(self.expect_ident()?)
            } else {
                None
            };

            items.push(ExportItem { path, alias });

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }

        self.expect_terminator()?;
        Ok(DeclKind::Export(ExportDecl { items }))
    }

    /// Parse a top-level const declaration.
    fn parse_const_decl(&mut self, is_pub: bool, doc: Option<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Const)?;
        let name = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) {
            Some(self.parse_type_name()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq)?;
        let init = self.parse_expr()?;
        self.expect_terminator()?;
        Ok(DeclKind::Const(ConstDecl { name, ty, init, is_pub, doc }))
    }

    /// Parse type declaration:
    /// - `type Name = TargetType` (nominal, default)
    /// - `type Name = TargetType with (Trait1, Trait2)` (nominal with traits)
    /// - `type alias Name = TargetType` (transparent)
    fn parse_type_alias_decl(&mut self, is_pub: bool) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Type)?;

        // Check for `type alias` (transparent)
        let is_transparent = if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "alias") {
            self.advance();
            true
        } else {
            false
        };

        let name = self.expect_ident()?;
        let type_params = if self.check(&TokenKind::Lt) {
            self.advance();
            let (params, _) = self.parse_type_params()?;
            params
        } else {
            Vec::new()
        };
        self.expect(&TokenKind::Eq)?;
        let target = self.parse_type_name()?;

        // Parse optional `with (Trait1, Trait2)` clause (nominal types only)
        let with_traits = if !is_transparent && self.check(&TokenKind::With) {
            self.advance();
            self.expect(&TokenKind::LParen)?;
            let mut traits = Vec::new();
            loop {
                if self.check(&TokenKind::RParen) { break; }
                traits.push(self.expect_ident()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            traits
        } else {
            Vec::new()
        };

        self.expect_terminator()?;
        Ok(DeclKind::TypeAlias(TypeAliasDecl {
            name, type_params, target, is_pub, is_transparent, with_traits,
        }))
    }

    /// Parse a test block: `test "name" { body }` or `comptime test "name" { body }`
    fn parse_test_decl(&mut self, is_comptime: bool) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Test)?;
        let name = self.expect_string()?;
        self.skip_newlines();
        let body = self.parse_block_body()?;
        Ok(DeclKind::Test(TestDecl { name, body, is_comptime }))
    }

    /// Parse a benchmark block: `benchmark "name" { body }`
    fn parse_benchmark_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Benchmark)?;
        let name = self.expect_string()?;
        self.skip_newlines();
        let body = self.parse_block_body()?;
        Ok(DeclKind::Benchmark(BenchmarkDecl { name, body }))
    }

    /// Parse `extern "C" func name(...)` or `extern "C" { func ...; func ... }`.
    /// Returns one or more extern declarations.
    fn parse_extern_decls(&mut self, doc: Option<String>) -> Result<Vec<DeclKind>, ParseError> {
        self.expect(&TokenKind::Extern)?;
        let abi = self.expect_string()?;

        // Block form: extern "C" { func ...; func ... }
        self.skip_newlines();
        if self.check(&TokenKind::LBrace) {
            self.advance();
            self.skip_newlines();
            let mut decls = Vec::new();
            while !self.check(&TokenKind::RBrace) && !self.at_end() {
                let func_doc = self.take_doc();
                self.expect(&TokenKind::Func)?;
                decls.push(self.parse_extern_func(&abi, func_doc)?);
                self.skip_newlines();
            }
            self.expect(&TokenKind::RBrace)?;
            return Ok(decls);
        }

        // Single form: extern "C" func name(...)
        self.expect(&TokenKind::Func)?;
        Ok(vec![self.parse_extern_func(&abi, doc)?])
    }

    /// Parse a single extern function — signature-only (import) or with body (export).
    fn parse_extern_func(&mut self, abi: &str, doc: Option<String>) -> Result<DeclKind, ParseError> {
        let fn_start = self.current().span.start;
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;
        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        // If followed by a body, this is an exported function definition
        self.skip_newlines();
        if self.check(&TokenKind::LBrace) {
            let body = self.parse_block_body()?;
            return Ok(DeclKind::Fn(FnDecl {
                name,
                type_params: vec![],
                params,
                ret_ty,
                context_clauses: vec![],
                body,
                is_pub: true,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: Some(abi.to_string()),
                attrs: vec![],
                doc,
                span: self.span(fn_start, self.tokens[self.pos.saturating_sub(1)].span.end),
            }));
        }

        Ok(DeclKind::Extern(ExternDecl { abi: abi.to_string(), name, params, ret_ty, doc }))
    }

    /// Parse a package block (struct.build/PK1-PK5).
    ///
    /// ```rask
    /// package "my-app" "1.0.0" {
    ///     dep "http" "^2.0"
    ///     dep "shared" { path: "../shared" }
    /// }
    /// ```
    fn parse_package_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Package)?;

        let name = self.expect_string()?;
        let version = self.expect_string()?;

        let mut deps = Vec::new();
        let mut features = Vec::new();
        let mut metadata = Vec::new();
        let mut list_metadata = Vec::new();
        let mut profiles = Vec::new();

        self.skip_newlines();
        if self.match_token(&TokenKind::LBrace) {
            self.skip_newlines();

            while !self.check(&TokenKind::RBrace) && !self.at_end() {
                match self.current_kind() {
                    TokenKind::Ident(ref s) if s == "dep" => {
                        deps.push(self.parse_dep_item()?);
                    }
                    TokenKind::Scope => {
                        // scope "dev" { dep ... }
                        self.advance();
                        let _scope_name = self.expect_string()?;
                        self.skip_newlines();
                        self.expect(&TokenKind::LBrace)?;
                        self.skip_newlines();
                        while !self.check(&TokenKind::RBrace) && !self.at_end() {
                            if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "dep") {
                                deps.push(self.parse_dep_item()?);
                            } else {
                                self.advance();
                            }
                            self.skip_newlines();
                        }
                        self.expect(&TokenKind::RBrace)?;
                    }
                    TokenKind::Feature => {
                        features.push(self.parse_feature_decl()?);
                    }
                    TokenKind::Profile => {
                        // profile "embedded" { key: "value", ... }
                        self.advance();
                        let profile_name = self.expect_string()?;
                        let mut settings = Vec::new();
                        self.skip_newlines();
                        if self.match_token(&TokenKind::LBrace) {
                            self.skip_newlines();
                            while !self.check(&TokenKind::RBrace) && !self.at_end() {
                                if matches!(self.current_kind(), TokenKind::Ident(_)) {
                                    let key = self.expect_ident()?;
                                    self.expect(&TokenKind::Colon)?;
                                    let value = self.expect_string()?;
                                    settings.push((key, value));
                                } else {
                                    self.advance();
                                }
                                self.skip_newlines();
                            }
                            self.expect(&TokenKind::RBrace)?;
                        }
                        profiles.push(ProfileDecl { name: profile_name, settings });
                    }
                    TokenKind::Ident(_) => {
                        let key = self.expect_ident()?;
                        self.expect(&TokenKind::Colon)?;
                        if self.check(&TokenKind::LBracket) {
                            // list metadata: key: ["a", "b"]
                            self.advance(); // consume [
                            let mut values = Vec::new();
                            self.skip_newlines();
                            while !self.check(&TokenKind::RBracket) && !self.at_end() {
                                values.push(self.expect_string()?);
                                self.skip_newlines();
                                if !self.match_token(&TokenKind::Comma) {
                                    self.skip_newlines();
                                    break;
                                }
                                self.skip_newlines();
                            }
                            self.expect(&TokenKind::RBracket)?;
                            list_metadata.push((key, values));
                        } else {
                            // scalar metadata: key: "value"
                            let value = self.expect_string()?;
                            metadata.push((key, value));
                        }
                    }
                    _ => {
                        self.advance();
                    }
                }
                self.skip_newlines();
            }
            self.expect(&TokenKind::RBrace)?;
        }

        Ok(DeclKind::Package(PackageDecl { name, version, deps, features, metadata, list_metadata, profiles }))
    }

    /// Parse a feature declaration inside a package block.
    ///
    /// Additive: `feature "ssl" { dep "openssl" "^3.0" }`
    /// Exclusive: `feature "runtime" exclusive { "tokio" { dep "tokio" "^1.0" } default: "tokio" }`
    fn parse_feature_decl(&mut self) -> Result<FeatureDecl, ParseError> {
        self.expect(&TokenKind::Feature)?;
        let name = self.expect_string()?;

        let exclusive = self.match_token(&TokenKind::Exclusive);

        let mut feature_deps = Vec::new();
        let mut options = Vec::new();
        let mut default = None;

        self.skip_newlines();
        if self.match_token(&TokenKind::LBrace) {
            self.skip_newlines();
            while !self.check(&TokenKind::RBrace) && !self.at_end() {
                if exclusive && matches!(self.current_kind(), TokenKind::String(_)) {
                    // String-named option block: "tokio" { dep ... }
                    let opt_name = self.expect_string()?;
                    let mut opt_deps = Vec::new();
                    self.skip_newlines();
                    if self.match_token(&TokenKind::LBrace) {
                        self.skip_newlines();
                        while !self.check(&TokenKind::RBrace) && !self.at_end() {
                            if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "dep") {
                                opt_deps.push(self.parse_dep_item()?);
                            } else {
                                self.advance();
                            }
                            self.skip_newlines();
                        }
                        self.expect(&TokenKind::RBrace)?;
                    }
                    options.push(FeatureOption { name: opt_name, deps: opt_deps });
                } else if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "dep") {
                    feature_deps.push(self.parse_dep_item()?);
                } else if matches!(self.current_kind(), TokenKind::Ident(_)) {
                    // default: "tokio"
                    let key = self.expect_ident()?;
                    if key == "default" {
                        self.expect(&TokenKind::Colon)?;
                        default = Some(self.expect_string()?);
                    } else {
                        // skip unknown key
                        if self.match_token(&TokenKind::Colon) {
                            self.advance();
                        }
                    }
                } else {
                    self.advance();
                }
                self.skip_newlines();
            }
            self.expect(&TokenKind::RBrace)?;
        }

        Ok(FeatureDecl { name, exclusive, deps: feature_deps, options, default })
    }

    /// Parse a single dep item inside a package block.
    ///
    /// ```rask
    /// dep "http" "^2.0"
    /// dep "shared" { path: "../shared" }
    /// dep "tokio" "^1.0" { with: ["rt-multi-thread", "net"] }
    /// ```
    fn parse_dep_item(&mut self) -> Result<DepDecl, ParseError> {
        // `dep` is a contextual keyword — only recognized inside package blocks
        if matches!(self.current_kind(), TokenKind::Ident(ref s) if s == "dep") {
            self.advance();
        } else {
            return Err(ParseError::expected("'dep'", self.current_kind(), self.current().span));
        }

        let name = self.expect_string()?;

        // Optional version string
        let version = if matches!(self.current_kind(), TokenKind::String(_)) {
            Some(self.expect_string()?)
        } else {
            None
        };

        let mut path = None;
        let mut git = None;
        let mut branch = None;
        let mut with_features = Vec::new();
        let mut target = None;
        let mut allow = Vec::new();
        let mut exclusive_selections = Vec::new();

        // Optional sub-block { key: value, ... }
        self.skip_newlines();
        if self.match_token(&TokenKind::LBrace) {
            self.skip_newlines();
            while !self.check(&TokenKind::RBrace) && !self.at_end() {
                // Keys may be keywords (e.g. `with`, `target`)
                let key = self.expect_ident_or_keyword()?;
                self.expect(&TokenKind::Colon)?;

                match key.as_str() {
                    "path" => { path = Some(self.expect_string()?); }
                    "git" => { git = Some(self.expect_string()?); }
                    "branch" => { branch = Some(self.expect_string()?); }
                    "target" => { target = Some(self.expect_string()?); }
                    "with" => {
                        self.expect(&TokenKind::LBracket)?;
                        while !self.check(&TokenKind::RBracket) && !self.at_end() {
                            with_features.push(self.expect_string()?);
                            if !self.match_token(&TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RBracket)?;
                    }
                    "allow" => {
                        // allow: ["net", "read"]
                        self.expect(&TokenKind::LBracket)?;
                        while !self.check(&TokenKind::RBracket) && !self.at_end() {
                            allow.push(self.expect_string()?);
                            if !self.match_token(&TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(&TokenKind::RBracket)?;
                    }
                    other => {
                        // Could be an exclusive feature selection: runtime: "tokio"
                        if matches!(self.current_kind(), TokenKind::String(_)) {
                            let selection = self.expect_string()?;
                            exclusive_selections.push((other.to_string(), selection));
                        } else if self.check(&TokenKind::LBrace) {
                            // Skip unknown block value
                            let mut depth = 1;
                            self.advance();
                            while depth > 0 && !self.at_end() {
                                match self.current_kind() {
                                    TokenKind::LBrace => depth += 1,
                                    TokenKind::RBrace => depth -= 1,
                                    _ => {}
                                }
                                if depth > 0 { self.advance(); }
                            }
                            if self.check(&TokenKind::RBrace) {
                                self.advance();
                            }
                        } else {
                            self.advance();
                        }
                    }
                }

                // Optional comma or newline separator
                let _ = self.match_token(&TokenKind::Comma);
                self.skip_newlines();
            }
            self.expect(&TokenKind::RBrace)?;
        }

        Ok(DepDecl {
            name, version, path, git, branch, with_features, target,
            allow, exclusive_selections,
        })
    }

    // =========================================================================
    // Statement Parsing
    // =========================================================================

    /// Parse a block body (statements inside braces), with error recovery.
    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        // Statements inside a block are not elements of any enclosing comma list.
        let outer_list = std::mem::replace(&mut self.in_comma_list, false);

        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            match self.parse_stmt() {
                Ok(stmt) => stmts.push(stmt),
                Err(e) => {
                    // Record error but stay within the block
                    if !self.record_error(e) {
                        // Too many errors - skip to closing brace
                        self.skip_to_closing_brace();
                        break;
                    }
                    // Synchronize within the block - skip to next statement
                    self.synchronize_in_block();
                }
            }
            self.skip_newlines();
        }

        self.in_comma_list = outer_list;
        self.expect(&TokenKind::RBrace)?;
        Ok(stmts)
    }

    /// Synchronize within a block - skip to the next statement boundary.
    fn synchronize_in_block(&mut self) {
        while !self.at_end() {
            // Stop at block end
            if self.check(&TokenKind::RBrace) {
                return;
            }
            // Stop at statement boundaries
            if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) {
                self.advance();
                self.skip_newlines();
                return;
            }
            // Stop before statement-starting keywords
            match self.current_kind() {
                TokenKind::Mut | TokenKind::Let | TokenKind::Const | TokenKind::Return |
                TokenKind::If | TokenKind::While | TokenKind::For |
                TokenKind::Loop | TokenKind::Match | TokenKind::Break |
                TokenKind::Continue | TokenKind::Ensure |
                TokenKind::Assert | TokenKind::Check => return,
                _ => { self.advance(); }
            }
        }
    }

    /// Skip to the next method boundary inside an extend/impl block.
    /// Tracks brace depth so nested blocks are skipped properly. Stops
    /// before `func` at depth 0 or before the closing `}` at depth 0,
    /// so the caller's loop can continue or exit normally.
    fn synchronize_to_next_method(&mut self) {
        let mut brace_depth: i32 = 0;
        while !self.at_end() {
            match self.current_kind() {
                TokenKind::LBrace => {
                    brace_depth += 1;
                    self.advance();
                }
                TokenKind::RBrace if brace_depth > 0 => {
                    brace_depth -= 1;
                    self.advance();
                }
                TokenKind::RBrace => return,
                TokenKind::Func | TokenKind::Public | TokenKind::Private
                | TokenKind::Comptime | TokenKind::Unsafe | TokenKind::At
                    if brace_depth == 0 => return,
                _ => { self.advance(); }
            }
        }
    }

    /// Skip to the closing brace of a block.
    fn skip_to_closing_brace(&mut self) {
        let mut depth = 1;
        while !self.at_end() && depth > 0 {
            match self.current_kind() {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                self.advance();
            }
        }
    }

    /// Parse a statement.
    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.current().span.start;

        // Check for labeled statement: `label: loop { }`
        let label = if let TokenKind::Ident(name) = self.current_kind().clone() {
            if matches!(self.peek(1), TokenKind::Colon) {
                // Check if this is actually a label (followed by loop/for/while)
                if matches!(self.peek(2), TokenKind::Loop | TokenKind::For | TokenKind::While) {
                    self.advance(); // consume identifier
                    self.advance(); // consume colon
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let kind = match self.current_kind() {
            TokenKind::Mut => self.parse_mut_stmt()?,
            TokenKind::Let => self.parse_let_stmt()?,
            TokenKind::Const => {
                let err = ParseError {
                    span: self.current().span,
                    message: "'const' is only for module-level constants".to_string(),
                    hint: Some("use 'let' for local bindings ('mut' if it needs to change)".to_string()),
                    why: None,
                };
                self.advance(); // consume 'const' so recovery doesn't loop
                return Err(err);
            }
            TokenKind::Return => self.parse_return_stmt()?,
            TokenKind::Break => self.parse_break_stmt()?,
            TokenKind::Continue => self.parse_continue_stmt()?,
            TokenKind::While => self.parse_while_stmt(label)?,
            TokenKind::Loop => self.parse_loop_stmt(label)?,
            TokenKind::For => self.parse_for_stmt(label)?,
            TokenKind::Ensure => self.parse_ensure_stmt()?,
            TokenKind::Comptime => self.parse_comptime_stmt()?,
            TokenKind::Discard => self.parse_discard_stmt()?,
            TokenKind::If => {
                let expr = self.parse_if_expr()?;
                self.expect_terminator()?;
                StmtKind::Expr(expr)
            }
            TokenKind::Match => {
                let expr = self.parse_match_expr()?;
                self.expect_terminator()?;
                StmtKind::Expr(expr)
            }
            TokenKind::Using => {
                let expr = self.parse_using_block()?;
                self.expect_terminator()?;
                StmtKind::Expr(expr)
            }
            TokenKind::With => {
                let expr = self.parse_with_binding()?;
                self.expect_terminator()?;
                StmtKind::Expr(expr)
            }
            _ => {
                let expr = self.parse_expr()?;

                if self.match_token(&TokenKind::Eq) {
                    let value = self.parse_expr()?;
                    self.expect_terminator()?;
                    StmtKind::Assign { target: expr, value }
                } else if let Some(op) = self.match_compound_assign() {
                    let rhs = self.parse_expr()?;
                    let value = Expr {
                        id: self.next_id(),
                        kind: ExprKind::Binary {
                            op,
                            left: Box::new(expr.clone()),
                            right: Box::new(rhs),
                        },
                        span: expr.span.clone(),
                    };
                    self.expect_terminator()?;
                    StmtKind::Assign { target: expr, value }
                } else {
                    self.expect_terminator()?;
                    StmtKind::Expr(expr)
                }
            }
        };

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);
        Ok(Stmt { id: self.next_id(), kind, span: self.span(start, end) })
    }

    fn match_compound_assign(&mut self) -> Option<BinOp> {
        let op = match self.current_kind() {
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Mod),
            TokenKind::AmpEq => Some(BinOp::BitAnd),
            TokenKind::PipeEq => Some(BinOp::BitOr),
            TokenKind::CaretEq => Some(BinOp::BitXor),
            TokenKind::LtLtEq => Some(BinOp::Shl),
            TokenKind::GtGtEq => Some(BinOp::Shr),
            _ => None,
        };
        if op.is_some() {
            self.advance();
        }
        op
    }

    /// Parse a single element in a tuple destructuring pattern.
    /// Supports names, wildcards `_`, and nested tuples `(a, b)`.
    fn parse_tuple_pat_element(&mut self) -> Result<rask_ast::stmt::TuplePat, ParseError> {
        use rask_ast::stmt::TuplePat;
        if self.match_token(&TokenKind::LParen) {
            let mut pats = Vec::new();
            loop {
                pats.push(self.parse_tuple_pat_element()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            Ok(TuplePat::Nested(pats))
        } else if matches!(self.current_kind(), TokenKind::Ident(s) if s == "_") {
            self.advance();
            Ok(TuplePat::Wildcard)
        } else {
            Ok(TuplePat::Name(self.expect_ident()?))
        }
    }

    fn parse_mut_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Mut)?;

        if self.match_token(&TokenKind::LParen) {
            let mut patterns = Vec::new();
            loop {
                patterns.push(self.parse_tuple_pat_element()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            self.expect(&TokenKind::Eq)?;
            let init = self.parse_expr()?;
            self.expect_terminator()?;
            return Ok(StmtKind::MutTuple { patterns, init });
        }

        let name_span = self.current().span;
        let name = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) { Some(self.parse_type_name()?) } else { None };
        self.expect(&TokenKind::Eq)?;
        let mut init = self.parse_expr()?;

        // Check for guard pattern: mut v = expr is Pattern else { ... }
        if matches!(init.kind, ExprKind::IsPattern { .. }) && self.check(&TokenKind::Else) {
            let ExprKind::IsPattern { expr, pattern } = init.kind else { unreachable!() };
            let guard_start = expr.span.start;
            self.expect(&TokenKind::Else)?;

            let else_branch = if self.match_token(&TokenKind::Colon) {
                self.parse_inline_block(guard_start)?
            } else {
                self.skip_newlines();
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(guard_start, end) }
            };

            let end = else_branch.span.end;
            init = Expr {
                id: self.next_id(),
                kind: ExprKind::GuardPattern {
                    expr,
                    pattern,
                    else_branch: Box::new(else_branch),
                },
                span: self.span(guard_start, end),
            };
        }

        self.expect_terminator()?;
        Ok(StmtKind::Mut { name, name_span, ty, init })
    }

    fn parse_let_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Let)?;

        // Catch Rust muscle memory: `let mut x = ...`
        if self.check(&TokenKind::Mut) {
            let err = ParseError {
                span: self.current().span,
                message: "'let mut' is not Rask".to_string(),
                hint: Some("'mut x = ...' declares a rebindable binding — drop the 'let'".to_string()),
                why: None,
            };
            self.advance(); // consume 'mut' so recovery doesn't loop
            return Err(err);
        }

        if self.match_token(&TokenKind::LParen) {
            let mut patterns = Vec::new();
            loop {
                patterns.push(self.parse_tuple_pat_element()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            self.expect(&TokenKind::Eq)?;
            let init = self.parse_expr()?;
            self.expect_terminator()?;
            return Ok(StmtKind::LetTuple { patterns, init });
        }

        let name_span = self.current().span;
        let name = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) { Some(self.parse_type_name()?) } else { None };
        self.expect(&TokenKind::Eq)?;
        let mut init = self.parse_expr()?;

        // Check for guard pattern: let v = expr is Pattern else { ... }
        if matches!(init.kind, ExprKind::IsPattern { .. }) && self.check(&TokenKind::Else) {
            let ExprKind::IsPattern { expr, pattern } = init.kind else { unreachable!() };
            let guard_start = expr.span.start;
            self.expect(&TokenKind::Else)?;

            let else_branch = if self.match_token(&TokenKind::Colon) {
                self.parse_inline_block(guard_start)?
            } else {
                self.skip_newlines();
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(guard_start, end) }
            };

            let end = else_branch.span.end;
            init = Expr {
                id: self.next_id(),
                kind: ExprKind::GuardPattern {
                    expr,
                    pattern,
                    else_branch: Box::new(else_branch),
                },
                span: self.span(guard_start, end),
            };
        }

        self.expect_terminator()?;
        Ok(StmtKind::Let { name, name_span, ty, init })
    }

    fn parse_discard_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Discard)?;
        let name_start = self.current().span.start;
        let name = self.expect_ident()?;
        let name_end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(name_start);
        self.expect_terminator()?;
        Ok(StmtKind::Discard {
            name,
            name_span: self.span(name_start, name_end),
        })
    }

    fn parse_return_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Return)?;
        let value = if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) || self.check(&TokenKind::RBrace) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect_terminator()?;
        Ok(StmtKind::Return(value))
    }

    fn parse_break_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Break)?;

        // break              → no label, no value
        // break label        → label, no value
        // break expr         → no label, value
        // break label expr   → label, value
        if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) || self.at_end() {
            self.expect_terminator()?;
            return Ok(StmtKind::Break { label: None, value: None });
        }

        if let TokenKind::Ident(name) = self.current_kind().clone() {
            // Disambiguate: `break label expr` vs `break value_expr`
            // If the ident is followed by a clear infix operator, it's a value expr.
            // Note: Minus excluded — `break label -1` is label + negative value.
            let next_is_infix = matches!(self.peek(1),
                TokenKind::Plus | TokenKind::Star | TokenKind::Slash |
                TokenKind::Percent | TokenKind::EqEq | TokenKind::BangEq |
                TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq |
                TokenKind::AmpAmp | TokenKind::PipePipe | TokenKind::Dot | TokenKind::LParen |
                TokenKind::LBracket | TokenKind::DotDot
            );
            if next_is_infix {
                // `break expr` — parse whole thing as value expression
                let value = self.parse_expr()?;
                self.expect_terminator()?;
                return Ok(StmtKind::Break { label: None, value: Some(value) });
            }

            self.advance();
            if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) || self.at_end()
                || self.check(&TokenKind::RBrace) || self.check(&TokenKind::Comma)
            {
                // `break ident` at end of statement — treat as label
                self.expect_terminator()?;
                Ok(StmtKind::Break { label: Some(name), value: None })
            } else if self.is_expr_start() {
                // `break ident expr` — ident is label, expr is value
                let value = self.parse_expr()?;
                self.expect_terminator()?;
                Ok(StmtKind::Break { label: Some(name), value: Some(value) })
            } else {
                // `break ident <non-expr>` — treat ident as label
                self.expect_terminator()?;
                Ok(StmtKind::Break { label: Some(name), value: None })
            }
        } else if self.is_expr_start() {
            // `break expr` — no label, just a value
            let value = self.parse_expr()?;
            self.expect_terminator()?;
            Ok(StmtKind::Break { label: None, value: Some(value) })
        } else {
            // `break` followed by non-expr token (e.g., comma in match arms)
            Ok(StmtKind::Break { label: None, value: None })
        }
    }

    fn parse_continue_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Continue)?;
        let label = if let TokenKind::Ident(name) = self.current_kind().clone() {
            self.advance();
            Some(name)
        } else {
            None
        };
        self.expect_terminator()?;
        Ok(StmtKind::Continue(label))
    }

    fn is_expr_start(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::Int(_, _) | TokenKind::Float(_, _) | TokenKind::String(_) | TokenKind::Bool(_)
                | TokenKind::Ident(_) | TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket
                | TokenKind::If | TokenKind::Match | TokenKind::With
                | TokenKind::Select | TokenKind::SelectPriority
                | TokenKind::Minus | TokenKind::Bang | TokenKind::Pipe | TokenKind::Try
                | TokenKind::Take
                | TokenKind::Amp | TokenKind::Star | TokenKind::Tilde
                | TokenKind::None | TokenKind::Null | TokenKind::ReadKw
        )
    }

    fn parse_while_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::While)?;

        let cond = self.parse_expr_no_braces()?;

        if let ExprKind::IsPattern { expr: scrutinee, pattern } = cond.kind {
            let body = if self.match_token(&TokenKind::Colon) {
                vec![self.parse_stmt()?]
            } else {
                self.skip_newlines();
                self.parse_block_body()?
            };
            return Ok(StmtKind::WhileLet { label, pattern, expr: *scrutinee, body });
        }

        let body = if self.match_token(&TokenKind::Colon) {
            vec![self.parse_stmt()?]
        } else {
            self.skip_newlines();
            self.parse_block_body()?
        };
        Ok(StmtKind::While { label, cond, body })
    }

    fn parse_loop_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Loop)?;
        self.skip_newlines();
        let body = self.parse_block_body()?;
        Ok(StmtKind::Loop { label, body })
    }

    fn parse_for_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::For)?;

        let mutate = self.match_token(&TokenKind::MutateKw);

        let binding = if self.match_token(&TokenKind::LParen) {
            let mut names = Vec::new();
            loop {
                names.push(self.expect_ident()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            ForBinding::Tuple(names)
        } else {
            ForBinding::Single(self.expect_ident()?)
        };

        self.expect(&TokenKind::In)?;
        let iter = self.parse_expr_no_braces()?;
        let body = if self.match_token(&TokenKind::Colon) {
            vec![self.parse_stmt()?]
        } else {
            self.skip_newlines();
            self.parse_block_body()?
        };
        Ok(StmtKind::For { label, binding, mutate, iter, body })
    }

    fn parse_ensure_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Ensure)?;
        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            let expr = self.parse_expr()?;
            let span = expr.span.clone();
            vec![Stmt { id: self.next_id(), kind: StmtKind::Expr(expr), span }]
        };

        let else_handler = if self.check(&TokenKind::Else) {
            self.advance();
            self.expect(&TokenKind::Pipe)?;
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Pipe)?;
            let handler = if self.check(&TokenKind::LBrace) {
                self.parse_block_body()?
            } else {
                let expr = self.parse_expr()?;
                let span = expr.span.clone();
                vec![Stmt { id: self.next_id(), kind: StmtKind::Expr(expr), span }]
            };
            Some((name, handler))
        } else {
            None
        };

        self.expect_terminator()?;
        Ok(StmtKind::Ensure { body, else_handler })
    }

    fn parse_comptime_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Comptime)?;

        // CT48: `comptime for` — compile-time loop unrolling
        if self.check(&TokenKind::For) {
            self.advance();
            let binding = if self.match_token(&TokenKind::LParen) {
                let mut names = Vec::new();
                loop {
                    names.push(self.expect_ident()?);
                    if !self.match_token(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RParen)?;
                ForBinding::Tuple(names)
            } else {
                ForBinding::Single(self.expect_ident()?)
            };
            self.expect(&TokenKind::In)?;
            let iter = self.parse_expr_no_braces()?;
            let body = if self.match_token(&TokenKind::Colon) {
                vec![self.parse_stmt()?]
            } else {
                self.skip_newlines();
                self.parse_block_body()?
            };
            return Ok(StmtKind::ComptimeFor { binding, iter, body });
        }

        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            vec![self.parse_stmt()?]
        };
        Ok(StmtKind::Comptime(body))
    }

    // =========================================================================
    // Expression Parsing (Pratt Parser)
    // =========================================================================

    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_bp(0)
    }

    /// Disallow brace-started constructs in control flow conditions.
    fn parse_expr_no_braces(&mut self) -> Result<Expr, ParseError> {
        let old = self.allow_brace_expr;
        self.allow_brace_expr = false;
        let result = self.parse_expr_bp(0);
        self.allow_brace_expr = old;
        result
    }

    /// Parse a conversion target type — a single primitive type name.
    fn parse_convert_target(&mut self) -> Result<String, ParseError> {
        self.expect_ident()
    }

    /// The right side of `??` or the body of `catch` — a value or a divergence
    /// (`return` / `break` / `continue`; `panic(…)` is an ordinary call whose
    /// type is Never). `min_bp` is 0 for a greedy `catch` body, the operator's
    /// right binding power for `??`.
    ///
    /// ER45a: a *diverging* right side inside a comma list reads ambiguously —
    /// the comma could end the exit or continue the list — so it needs parens.
    fn parse_fallback_body(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        let diverges = matches!(
            self.current_kind(),
            TokenKind::Return | TokenKind::Break | TokenKind::Continue
        );
        if diverges && self.in_comma_list {
            return Err(ParseError {
                span: self.current().span,
                message: "a `return`/`break`/`continue` fallback inside a comma list needs parens"
                    .to_string(),
                hint: Some(
                    "wrap the whole fallback: `f((x ?? return e), y)` — otherwise the comma \
                     could belong to either the exit or the list"
                        .to_string(),
                ),
                why: None,
            });
        }
        if diverges {
            // Statement keywords aren't expressions; wrap in a block, whose type is Never.
            return self.parse_inline_block(start);
        }
        let saved = std::mem::replace(&mut self.in_comma_list, false);
        let body = self.parse_expr_bp(min_bp);
        self.in_comma_list = saved;
        body
    }

    /// The `<binder> => <body>` of a `catch`. The binder is mandatory (ER14);
    /// `catch <expr>` names both spellings instead of silently swallowing.
    fn parse_catch_clause(
        &mut self,
        catch_span: rask_ast::Span,
    ) -> Result<rask_ast::expr::CatchClause, ParseError> {
        let binder = match self.current_kind().clone() {
            TokenKind::Ident(name) if self.peek(1) == &TokenKind::FatArrow => {
                self.advance();
                name
            }
            _ => {
                return Err(ParseError {
                    span: catch_span,
                    message: "`catch` needs a binder".to_string(),
                    hint: Some(
                        "write `catch e => value` to use the error, or `catch _ => value` to \
                         drop it — there is no bare-value form, so an error is never swallowed \
                         silently"
                            .to_string(),
                    ),
                    why: None,
                });
            }
        };
        self.expect(&TokenKind::FatArrow)?;
        // Greedy, like a match-arm body: `a catch _ => b catch _ => c` right-nests.
        let body = self.parse_fallback_body(0)?;
        Ok(rask_ast::expr::CatchClause { binder, body: Box::new(body) })
    }

    /// Detect and parse a non-`try` lossy conversion suffix on `lhs`
    /// (type.primitives CV5/CV6/CV8/CV9): `truncate to T`, `saturate to T`,
    /// `float to int T`, `float to int T (saturating)`. Returns None if the
    /// following tokens aren't a conversion suffix.
    fn try_parse_convert_suffix(
        &mut self,
        lhs: Expr,
        start: usize,
    ) -> Result<Result<Expr, Expr>, ParseError> {
        let kind = if self.peek_is_word(0, "truncate") && self.peek_is_word(1, "to") {
            self.advance(); // truncate
            self.advance(); // to
            ConvertKind::Truncate
        } else if self.peek_is_word(0, "saturate") && self.peek_is_word(1, "to") {
            self.advance(); // saturate
            self.advance(); // to
            ConvertKind::Saturate
        } else if self.peek_is_word(0, "float")
            && self.peek_is_word(1, "to")
            && self.peek_is_word(2, "int")
        {
            self.advance(); // float
            self.advance(); // to
            self.advance(); // int
            ConvertKind::FloatToInt
        } else {
            // Not a conversion suffix — hand the lhs back untouched.
            return Ok(Err(lhs));
        };

        let target = self.parse_convert_target()?;
        // CV9: optional `(saturating)` marker on float-to-int.
        let kind = if kind == ConvertKind::FloatToInt
            && self.check(&TokenKind::LParen)
            && self.peek_is_word(1, "saturating")
        {
            self.advance(); // (
            self.advance(); // saturating
            self.expect(&TokenKind::RParen)?;
            ConvertKind::FloatToIntSat
        } else {
            kind
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Convert {
                expr: Box::new(lhs),
                target,
                kind,
            },
            span: self.span(start, end),
        }))
    }

    fn parse_expr_bp(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        let mut lhs = self.parse_prefix()?;

        loop {
            if self.check(&TokenKind::Newline)
                && (self.peek_past_newlines_is_postfix() || self.peek_past_newlines_is_infix())
            {
                self.skip_newlines();
            }

            if let Some(bp) = self.postfix_bp() {
                if bp < min_bp { break; }
                lhs = self.parse_postfix(lhs)?;
                continue;
            }

            if self.check(&TokenKind::As) {
                let bp = 21;
                if bp < min_bp { break; }
                self.advance();
                let ty = self.parse_type_name()?;
                let end = self.tokens[self.pos - 1].span.end;
                lhs = Expr {
                    id: self.next_id(),
                    kind: ExprKind::Cast { expr: Box::new(lhs), ty },
                    span: self.span(start, end),
                };
                continue;
            }

            // Lossy conversion suffixes (CV5/CV6/CV8/CV9) bind like `as` (bp 21).
            if (self.peek_is_word(0, "truncate")
                || self.peek_is_word(0, "saturate")
                || self.peek_is_word(0, "float"))
                && 21 >= min_bp
            {
                match self.try_parse_convert_suffix(lhs, start)? {
                    Ok(converted) => {
                        lhs = converted;
                        continue;
                    }
                    Err(orig) => {
                        // `float`/`truncate`/`saturate` used as a plain identifier —
                        // not a conversion. Restore and fall through.
                        lhs = orig;
                    }
                }
            }

            // Pattern test: expr is Pattern (evaluates to bool)
            if self.check(&TokenKind::Is) {
                let bp = 5;
                if bp < min_bp { break; }
                self.advance();
                let pattern = self.parse_pattern()?;
                let end = self.tokens[self.pos - 1].span.end;
                lhs = Expr {
                    id: self.next_id(),
                    kind: ExprKind::IsPattern { expr: Box::new(lhs), pattern },
                    span: self.span(start, end),
                };
                continue;
            }

            if let Some((l_bp, r_bp)) = self.infix_bp() {
                if l_bp < min_bp { break; }

                if self.check(&TokenKind::QuestionQuestion) {
                    self.advance();
                    // OPT11: the right side is a value or any divergence —
                    // `x ?? return y`, `?? break`, `?? continue`, `?? panic(…)`.
                    let default = self.parse_fallback_body(r_bp)?;
                    let end = default.span.end;
                    lhs = Expr {
                        id: self.next_id(),
                        kind: ExprKind::NullCoalesce {
                            value: Box::new(lhs),
                            default: Box::new(default),
                        },
                        span: self.span(start, end),
                    };
                    continue;
                }

                // ER14: `r catch e => body` / `r catch _ => body`. Left side
                // groups at level 7 like `??`; the body is greedy to the right,
                // so a `catch` chain right-nests without parens.
                if self.check(&TokenKind::Catch) {
                    let catch_span = self.current().span;
                    self.advance();
                    let clause = self.parse_catch_clause(catch_span)?;
                    let end = clause.body.span.end;
                    lhs = Expr {
                        id: self.next_id(),
                        kind: ExprKind::Catch { value: Box::new(lhs), clause },
                        span: self.span(start, end),
                    };
                    continue;
                }

                if self.check(&TokenKind::DotDot) || self.check(&TokenKind::DotDotEq) {
                    let inclusive = self.check(&TokenKind::DotDotEq);
                    self.advance();
                    let end_expr = if self.is_expr_start() {
                        Some(Box::new(self.parse_expr_bp(r_bp)?))
                    } else {
                        None
                    };
                    let end = end_expr.as_ref().map(|e| e.span.end).unwrap_or(self.tokens[self.pos - 1].span.end);
                    lhs = Expr {
                        id: self.next_id(),
                        kind: ExprKind::Range {
                            start: Some(Box::new(lhs)),
                            end: end_expr,
                            inclusive,
                        },
                        span: self.span(start, end),
                    };
                    continue;
                }

                let op = self.parse_binop()?;
                self.skip_newlines();
                let rhs = self.parse_expr_bp(r_bp)?;
                let end = rhs.span.end;
                lhs = Expr {
                    id: self.next_id(),
                    kind: ExprKind::Binary { op, left: Box::new(lhs), right: Box::new(rhs) },
                    span: self.span(start, end),
                };
                continue;
            }

            break;
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;

        match self.current_kind().clone() {
            TokenKind::Int(n, suffix) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Int(n, suffix.clone()), span: self.span(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::Float(n, suffix) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Float(n, suffix.clone()), span: self.span(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::String(s) => {
                self.advance();
                let str_span = self.span(start, self.tokens[self.pos - 1].span.end);
                // `}` alone matters too: `"}}"` is an escaped brace with no
                // `{` anywhere in it (fmt/F4).
                if s.contains('{') || s.contains('}') {
                    match self.parse_string_interpolation(&s, str_span) {
                        Some(segments) => Ok(Expr { id: self.next_id(), kind: ExprKind::StringInterp(segments), span: str_span }),
                        None => Ok(Expr { id: self.next_id(), kind: ExprKind::String(s), span: str_span }),
                    }
                } else {
                    Ok(Expr { id: self.next_id(), kind: ExprKind::String(s), span: str_span })
                }
            }
            TokenKind::Char(c) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Char(c), span: self.span(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Bool(b), span: self.span(start, self.tokens[self.pos - 1].span.end) })
            }

            TokenKind::None => {
                self.advance();
                let end = self.tokens[self.pos - 1].span.end;
                // OPT3: dedicated absent literal, not the `None` enum variant.
                Ok(Expr { id: self.next_id(), kind: ExprKind::None, span: self.span(start, end) })
            }
            TokenKind::Null => {
                self.advance();
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Null, span: self.span(start, end) })
            }

            TokenKind::Ident(name) => {
                self.advance();

                // Labeled loop/for/while expression: `label: loop { ... }`
                if self.check(&TokenKind::Colon)
                    && matches!(self.peek(1), TokenKind::Loop | TokenKind::For | TokenKind::While)
                {
                    self.advance(); // consume ':'
                    let label = Some(name);
                    match self.current_kind() {
                        TokenKind::Loop => {
                            self.advance();
                            self.skip_newlines();
                            let body = self.parse_block_body()?;
                            let end = self.tokens[self.pos - 1].span.end;
                            return Ok(Expr {
                                id: self.next_id(),
                                kind: ExprKind::Loop { label, body },
                                span: self.span(start, end),
                            });
                        }
                        _ => {
                            return Err(ParseError::not_implemented(
                                "labeled for/while expressions",
                                "only labeled `loop` expressions are supported",
                                self.span(start, self.current().span.end),
                            ));
                        }
                    }
                }

                let mut full_name = name.clone();

                // Parse generic arguments: ident<T>(...), Type<T>.method(), Type<T> { ... }
                if self.check(&TokenKind::Lt) {
                    // Any identifier can have generic args when followed by `(`
                    let is_generic_call = self.looks_like_generic_method_call();
                    // Only type names (uppercase) can have static methods or struct literals
                    let is_static_method = Self::is_type_name(&name)
                        && self.looks_like_generic_type_with_static_method();
                    let is_struct_literal = Self::is_type_name(&name) && {
                        // Look ahead: Name<Args> {
                        let mut lookahead_pos = self.pos + 1;
                        let mut depth = 1;
                        let mut found_brace = false;

                        while lookahead_pos < self.tokens.len() && depth > 0 {
                            match &self.tokens[lookahead_pos].kind {
                                TokenKind::Lt => depth += 1,
                                TokenKind::Gt => {
                                    depth -= 1;
                                    if depth == 0 {
                                        if lookahead_pos + 1 < self.tokens.len() {
                                            found_brace = matches!(self.tokens[lookahead_pos + 1].kind, TokenKind::LBrace);
                                        }
                                    }
                                }
                                _ => {}
                            }
                            lookahead_pos += 1;
                        }
                        found_brace
                    };

                    if is_generic_call || is_static_method || is_struct_literal {
                        self.advance(); // consume '<'
                        full_name.push('<');
                        loop {
                            full_name.push_str(&self.parse_type_name()?);
                            if self.match_token(&TokenKind::Comma) {
                                full_name.push_str(", ");
                            } else {
                                break;
                            }
                        }
                        self.expect_gt_in_generic()?;
                        full_name.push('>');
                    }
                }

                let end = self.tokens[self.pos - 1].span.end;

                if Self::is_type_name(&full_name) && self.allow_brace_expr && self.check(&TokenKind::LBrace) {
                    self.parse_struct_literal(full_name, start)
                } else {
                    Ok(Expr { id: self.next_id(), kind: ExprKind::Ident(full_name), span: self.span(start, end) })
                }
            }

            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                // `-N` is one literal, not a negation of `N`. Folding the sign
                // in here is what lets `i64::MIN` be written: the lexer sees
                // 9223372036854775808 on its own, which only fits `u64`, so
                // leaving the `-` as an operator asked for `neg` on a `u64`.
                if let ExprKind::Int(v, suffix) = operand.kind {
                    match suffix {
                        None => {
                            let kind = ExprKind::Int(-v, None);
                            return Ok(Expr { id: self.next_id(), kind, span: self.span(start, end) });
                        }
                        // Only the lexer's "too big for i64" marker folds; an
                        // explicitly written `u64` keeps meaning `u64`.
                        Some(IntSuffix::U64ByMagnitude) => {
                            if v == i64::MIN {
                                let kind = ExprKind::Int(i64::MIN, None);
                                return Ok(Expr { id: self.next_id(), kind, span: self.span(start, end) });
                            }
                            return Err(ParseError {
                                span: self.span(start, end),
                                message: format!("integer literal `-{}` is too small for `i64`", v as u64),
                                hint: Some(format!("the smallest `i64` is {}", i64::MIN)),
                                why: Some(
                                    "integer literals are `i64` unless a suffix says otherwise, and \
                                     there is no wider signed type to hold this one"
                                        .to_string(),
                                ),
                            });
                        }
                        Some(_) => {}
                    }
                    let operand = Expr { id: self.next_id(), kind: ExprKind::Int(v, suffix), span: operand.span };
                    let kind = ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(operand) };
                    return Ok(Expr { id: self.next_id(), kind, span: self.span(start, end) });
                }
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(operand) }, span: self.span(start, end) })
            }
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                // OPT17/ER26: `!x?` is forbidden — prefix `!` with suffix `?`
                // fights the parse. Suggest `x == none` (Option) or `x is E`
                // (Result) instead. `!` on a plain bool is still fine.
                if matches!(operand.kind, ExprKind::IsPresent { .. }) {
                    return Err(ParseError {
                        span: self.span(start, end),
                        message: "cannot negate `?` with prefix `!`".to_string(),
                        // OPT16: absence has its own spelling; `r is E` tests a result.
                        hint: Some("use `x is none` for an optional, `r is E` for a result".to_string()),
                        why: None,
                    });
                }
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(operand) }, span: self.span(start, end) })
            }
            TokenKind::Tilde => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::BitNot, operand: Box::new(operand) }, span: self.span(start, end) })
            }
            TokenKind::Amp => {
                return Err(ParseError::not_implemented(
                    "reference expressions",
                    "remove the '&' - Rask currently uses owned values",
                    self.current().span,
                ));
            }
            TokenKind::Star => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Deref, operand: Box::new(operand) }, span: self.span(start, end) })
            }

            // `own` as prefix: either an owned closure (`own |...| body`) or a
            // struct-field / call-site mode marker (captured by parse_args()).
            TokenKind::Own => {
                self.advance();
                match self.current().kind {
                    TokenKind::Pipe => self.parse_closure(true),
                    TokenKind::PipePipe => {
                        self.advance();
                        let body = self.parse_closure_body()?;
                        let end = body.span.end;
                        Ok(Expr {
                            id: self.next_id(),
                            kind: ExprKind::Closure {
                                params: vec![],
                                ret_ty: None,
                                body: Box::new(body),
                                is_own: true,
                            },
                            span: self.span(start, end),
                        })
                    }
                    _ => self.parse_expr_bp(Self::PREFIX_BP),
                }
            }

            // `read` is reserved as a parameter mode keyword but has no syntactic
            // role in expressions. Allow it as a plain identifier so user-defined
            // functions and variables named `read` work correctly.
            TokenKind::ReadKw => {
                self.advance();
                let name = "read".to_string();
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Ident(name), span: self.span(start, end) })
            }

            TokenKind::LParen => self.parse_paren_or_tuple(),

            TokenKind::LBracket => self.parse_array_literal(),

            TokenKind::LBrace => {
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(start, end) })
            }

            TokenKind::PipePipe => {
                self.advance();
                let body = self.parse_closure_body()?;
                let end = body.span.end;
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Closure { params: vec![], ret_ty: None, body: Box::new(body), is_own: false },
                    span: self.span(start, end),
                })
            }

            TokenKind::Pipe => self.parse_closure(false),

            TokenKind::If => self.parse_if_expr(),

            TokenKind::Loop => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Loop { label: None, body },
                    span: self.span(start, end),
                })
            }

            TokenKind::Match => self.parse_match_expr(),

            TokenKind::Using => self.parse_using_block(),

            TokenKind::With => self.parse_with_binding(),

            TokenKind::Select => self.parse_select_expr(false),

            TokenKind::SelectPriority => self.parse_select_expr(true),

            TokenKind::Unsafe => {
                self.advance();
                self.skip_newlines();
                let body = if self.check(&TokenKind::LBrace) {
                    self.parse_block_body()?
                } else {
                    let expr = self.parse_expr()?;
                    vec![Stmt { id: self.next_id(), kind: StmtKind::Expr(expr.clone()), span: expr.span }]
                };
                let end = body.last().map(|s| s.span.end).unwrap_or(start);
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unsafe { body }, span: self.span(start, end) })
            }

            TokenKind::Comptime => {
                self.advance();
                self.skip_newlines();
                let body = if self.check(&TokenKind::LBrace) {
                    self.parse_block_body()?
                } else {
                    let expr = self.parse_expr()?;
                    vec![Stmt { id: self.next_id(), kind: StmtKind::Expr(expr.clone()), span: expr.span }]
                };
                let end = body.last().map(|s| s.span.end).unwrap_or(start);
                Ok(Expr { id: self.next_id(), kind: ExprKind::Comptime { body }, span: self.span(start, end) })
            }

            TokenKind::Assert => self.parse_assert_expr(),

            TokenKind::Check => self.parse_check_expr(),

            // A range with no start — `s[..5]`, `buf[..]`. The open-end form
            // falls out of the infix table; this one has nothing to its left.
            TokenKind::DotDot | TokenKind::DotDotEq => {
                let inclusive = self.check(&TokenKind::DotDotEq);
                self.advance();
                let end = if self.is_expr_start() {
                    Some(Box::new(self.parse_expr_bp(4)?))
                } else {
                    None
                };
                let end_span = end
                    .as_ref()
                    .map(|e| e.span.end)
                    .unwrap_or(self.tokens[self.pos - 1].span.end);
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Range { start: None, end, inclusive },
                    span: self.span(start, end_span),
                })
            }

            // OPT32: `take <place>` moves the payload out of a mutable
            // optional slot and leaves `none`. Binds like the other prefixes,
            // so `take conn.pending` covers the whole field path.
            TokenKind::Take => {
                self.advance();
                let place = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = place.span.end;
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Take { place: Box::new(place) },
                    span: self.span(start, end),
                })
            }

            TokenKind::Try => {
                self.advance();
                let inner = self.parse_expr_bp(Self::PREFIX_BP)?;

                // CV7 `try x convert to T` / CV10 `try x float to int T`:
                // `try` here forms an optional-producing conversion, not error
                // propagation. Detect the conversion tail before treating `try`
                // as propagation.
                let convert_kind = if self.peek_is_word(0, "convert") && self.peek_is_word(1, "to") {
                    self.advance(); // convert
                    self.advance(); // to
                    Some(ConvertKind::TryConvert)
                } else if self.peek_is_word(0, "float")
                    && self.peek_is_word(1, "to")
                    && self.peek_is_word(2, "int")
                {
                    self.advance(); // float
                    self.advance(); // to
                    self.advance(); // int
                    Some(ConvertKind::TryFloatToInt)
                } else {
                    None
                };
                if let Some(kind) = convert_kind {
                    let target = self.parse_convert_target()?;
                    let end = self.tokens[self.pos - 1].span.end;
                    return Ok(Expr {
                        id: self.next_id(),
                        kind: ExprKind::Convert {
                            expr: Box::new(inner),
                            target,
                            kind,
                        },
                        span: self.span(start, end),
                    });
                }

                // `try … else …` is gone (ER45/ER46/ER48 deleted). Point at the
                // replacement rather than failing on a stray `else`.
                if self.check(&TokenKind::Else)
                    || (self.check(&TokenKind::Newline) && self.peek_past_newlines_is_else())
                {
                    if self.check(&TokenKind::Newline) {
                        self.skip_newlines();
                    }
                    let else_span = self.current().span;
                    self.advance();
                    return Err(ParseError {
                        span: else_span,
                        message: "`try` has no `else` clause".to_string(),
                        hint: Some(
                            "handle a failure with `catch e => …` (or `catch _ => …` to drop it), \
                             an absence with `?? …`; bare `try` only propagates"
                                .to_string(),
                        ),
                        why: None,
                    });
                }

                let end = inner.span.end;
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Try { expr: Box::new(inner) },
                    span: self.span(start, end),
                })
            }

            _ => Err(ParseError::expected(
                "expression",
                self.current_kind(),
                self.current().span,
            )),
        }
    }

    fn parse_struct_literal(&mut self, name: String, start: usize) -> Result<Expr, ParseError> {
        let outer_list = std::mem::replace(&mut self.in_comma_list, true);
        let result = self.parse_struct_literal_inner(name, start);
        self.in_comma_list = outer_list;
        result
    }

    fn parse_struct_literal_inner(&mut self, name: String, start: usize) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut spread = None;

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            if self.match_token(&TokenKind::DotDot) {
                spread = Some(Box::new(self.parse_expr()?));
                self.skip_newlines();
                break;
            }

            let field_name = self.expect_ident_or_keyword()?;

            let value = if self.match_token(&TokenKind::Colon) {
                self.parse_expr()?
            } else {
                Expr {
                    id: self.next_id(),
                    kind: ExprKind::Ident(field_name.clone()),
                    span: self.tokens[self.pos - 1].span.clone(),
                }
            };

            fields.push(FieldInit { name: field_name, value });

            if !self.match_token(&TokenKind::Comma) {
                self.skip_newlines();
                break;
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        let end = self.tokens[self.pos - 1].span.end;

        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::StructLit { name, fields, spread },
            span: self.span(start, end),
        })
    }

    fn parse_assert_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Assert)?;
        let condition = Box::new(self.parse_expr()?);
        let message = if self.match_token(&TokenKind::Comma) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Assert { condition, message },
            span: self.span(start, end),
        })
    }

    fn parse_check_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Check)?;
        let condition = Box::new(self.parse_expr()?);
        let message = if self.match_token(&TokenKind::Comma) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Check { condition, message },
            span: self.span(start, end),
        })
    }

    fn parse_paren_or_tuple(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::LParen)?;

        // Parens close the ambiguity a condition opens: inside them a `{` can
        // only start a struct literal, never the body of the `if`. So this is
        // the way to write one there — `if (c == Shape.Circle { r: 4 }) { … }`.
        let outer_braces = self.allow_brace_expr;
        self.allow_brace_expr = true;
        // Parens end the enclosing comma list — that's the ER45a escape hatch.
        let outer_list = std::mem::replace(&mut self.in_comma_list, false);
        let result = self.parse_paren_or_tuple_inner(start);
        self.in_comma_list = outer_list;
        self.allow_brace_expr = outer_braces;
        result
    }

    fn parse_paren_or_tuple_inner(&mut self, start: usize) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::RParen) {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr { id: self.next_id(), kind: ExprKind::Tuple(Vec::new()), span: self.span(start, end) });
        }

        let first = self.parse_expr()?;

        if self.match_token(&TokenKind::Comma) {
            let mut elements = vec![first];
            self.in_comma_list = true;
            while !self.check(&TokenKind::RParen) && !self.at_end() {
                elements.push(self.parse_expr()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.in_comma_list = false;
            self.expect(&TokenKind::RParen)?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Expr { id: self.next_id(), kind: ExprKind::Tuple(elements), span: self.span(start, end) })
        } else {
            self.expect(&TokenKind::RParen)?;
            Ok(first)
        }
    }

    fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
        let outer_list = std::mem::replace(&mut self.in_comma_list, true);
        let result = self.parse_array_literal_inner();
        self.in_comma_list = outer_list;
        result
    }

    fn parse_array_literal_inner(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::LBracket)?;
        self.skip_newlines();

        if self.check(&TokenKind::RBracket) {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr { id: self.next_id(), kind: ExprKind::Array(Vec::new()), span: self.span(start, end) });
        }

        let first = self.parse_expr()?;
        self.skip_newlines();

        if self.match_token(&TokenKind::Semi) {
            let count = self.parse_expr()?;
            self.skip_newlines();
            self.expect(&TokenKind::RBracket)?;
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr {
                id: self.next_id(),
                kind: ExprKind::ArrayRepeat { value: Box::new(first), count: Box::new(count) },
                span: self.span(start, end),
            });
        }

        let mut elements = vec![first];
        if self.match_token(&TokenKind::Comma) {
            self.skip_newlines();
            while !self.check(&TokenKind::RBracket) && !self.at_end() {
                elements.push(self.parse_expr()?);
                self.skip_newlines();
                if !self.match_token(&TokenKind::Comma) { break; }
                self.skip_newlines();
            }
        }

        self.expect(&TokenKind::RBracket)?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr { id: self.next_id(), kind: ExprKind::Array(elements), span: self.span(start, end) })
    }

    fn parse_closure(&mut self, is_own: bool) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Pipe)?;

        let mut params = Vec::new();
        while !self.check(&TokenKind::Pipe) && !self.at_end() {
            // Typed mutable parameter: |mutate x: T|. Explicit type is required
            // (mem.closures/CP2). Untyped `|mutate x|` is mutable-capture syntax
            // (CP3), not a parameter — and is not handled by this loop.
            let mutate_span = self.current().span;
            let is_mutate = self.match_token(&TokenKind::MutateKw);
            let name = self.expect_ident()?;
            let ty = if self.match_token(&TokenKind::Colon) {
                Some(self.parse_type_name()?)
            } else if is_mutate {
                return Err(ParseError {
                    span: mutate_span,
                    message: format!(
                        "closure parameter '{}' with 'mutate' requires an explicit type",
                        name
                    ),
                    hint: Some(format!("write it as |mutate {}: T|", name)),
                    why: None,
                });
            } else {
                None
            };
            params.push(ClosureParam { name, ty, is_mutate, is_take: false });
            if !self.match_token(&TokenKind::Comma) { break; }
        }

        self.expect(&TokenKind::Pipe)?;

        // Optional return type annotation
        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        let body = self.parse_closure_body()?;
        let end = body.span.end;

        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Closure { params, ret_ty, body: Box::new(body), is_own },
            span: self.span(start, end),
        })
    }

    /// Parse a closure body, handling assignment in braceless bodies.
    /// Supports `|c| c = 42` and `|c| c += 1` without requiring braces.
    fn parse_closure_body(&mut self) -> Result<Expr, ParseError> {
        let body = self.parse_expr()?;

        if self.match_token(&TokenKind::Eq) {
            let value = self.parse_expr()?;
            let span = self.span(body.span.start, value.span.end);
            let assign_stmt = Stmt {
                id: self.next_id(),
                kind: StmtKind::Assign { target: body, value },
                span: span.clone(),
            };
            Ok(Expr { id: self.next_id(), kind: ExprKind::Block(vec![assign_stmt]), span })
        } else if let Some(op) = self.match_compound_assign() {
            let rhs = self.parse_expr()?;
            let value = Expr {
                id: self.next_id(),
                kind: ExprKind::Binary {
                    op,
                    left: Box::new(body.clone()),
                    right: Box::new(rhs),
                },
                span: body.span.clone(),
            };
            let span = self.span(body.span.start, value.span.end);
            let assign_stmt = Stmt {
                id: self.next_id(),
                kind: StmtKind::Assign { target: body, value },
                span: span.clone(),
            };
            Ok(Expr { id: self.next_id(), kind: ExprKind::Block(vec![assign_stmt]), span })
        } else {
            Ok(body)
        }
    }

    fn parse_postfix(&mut self, lhs: Expr) -> Result<Expr, ParseError> {
        let start = lhs.span.start;

        match self.current_kind() {
            TokenKind::LParen => {
                self.advance();
                // std.fmt/CM2: the desugar pass parses `format`'s template at
                // compile time, so the literal has to reach it whole. Splitting
                // it here read `{0}` as the integer zero and turned `{{x}}`
                // back into a placeholder.
                let raw_template = matches!(&lhs.kind, ExprKind::Ident(n) if n == "format")
                    && matches!(self.current_kind(), TokenKind::String(_));
                let args = self.parse_args_with(raw_template)?;
                self.expect(&TokenKind::RParen)?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Call { func: Box::new(lhs), args }, span: self.span(start, end) })
            }

            TokenKind::Dot => {
                self.advance();

                // Dynamic field access: value.(expr) — comptime field name
                if self.check(&TokenKind::LParen) {
                    self.advance();
                    let field_expr = self.parse_expr()?;
                    self.expect(&TokenKind::RParen)?;
                    let end = self.tokens[self.pos - 1].span.end;
                    return Ok(Expr {
                        id: self.next_id(),
                        kind: ExprKind::DynamicField { object: Box::new(lhs), field_expr: Box::new(field_expr) },
                        span: self.span(start, end),
                    });
                }

                // Tuple field access: expr.0, expr.1, ...
                if let TokenKind::Int(n, None) = self.current_kind().clone() {
                    if n < 0 {
                        return Err(ParseError::expected(
                            "a non-negative index",
                            self.current_kind(),
                            self.current().span,
                        ));
                    }
                    let field = n.to_string();
                    self.advance();
                    let end = self.tokens[self.pos - 1].span.end;
                    return Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: self.span(start, end) });
                }

                let field = self.expect_ident_or_keyword()?;

                let type_args = if self.check(&TokenKind::Lt) && self.looks_like_generic_method_call() {
                    self.advance();
                    let mut args = Vec::new();
                    loop {
                        args.push(self.parse_type_name()?);
                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect_gt_in_generic()?;
                    Some(args)
                } else {
                    None
                };

                if self.check(&TokenKind::LParen) {
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(&TokenKind::RParen)?;
                    let end = self.tokens[self.pos - 1].span.end;
                    Ok(Expr {
                        id: self.next_id(),
                        kind: ExprKind::MethodCall { object: Box::new(lhs), method: field, type_args, args },
                        span: self.span(start, end),
                    })
                } else if type_args.is_some() {
                    // Had generic args but no parens - error
                    Err(ParseError::expected(
                        "'('",
                        self.current_kind(),
                        self.current().span,
                    ).with_hint("Generic type arguments must be followed by ()"))
                } else if self.check(&TokenKind::LBrace) && self.allow_brace_expr {
                    // Struct variant constructor: Enum.Variant { field: value }
                    // Only when base is a type name (uppercase) to avoid ambiguity
                    // with blocks — and never in a condition, where the brace
                    // starts the body. Without that second guard,
                    // `if m == Mode.On { … }` read `Mode.On { … }` as a struct
                    // literal and swallowed the if-block (#342).
                    if let ExprKind::Ident(base) = &lhs.kind {
                        if base.starts_with(|c: char| c.is_uppercase()) && field.starts_with(|c: char| c.is_uppercase()) {
                            let full_name = format!("{}.{}", base, field);
                            self.parse_struct_literal(full_name, start)
                        } else {
                            let end = self.tokens[self.pos - 1].span.end;
                            Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: self.span(start, end) })
                        }
                    } else {
                        let end = self.tokens[self.pos - 1].span.end;
                        Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: self.span(start, end) })
                    }
                } else {
                    let end = self.tokens[self.pos - 1].span.end;
                    Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: self.span(start, end) })
                }
            }

            // Optional chaining
            TokenKind::QuestionDot => {
                self.advance();
                let field = if let TokenKind::Int(n, None) = self.current_kind().clone() {
                    if n < 0 {
                        return Err(ParseError::expected(
                            "a non-negative index",
                            self.current_kind(),
                            self.current().span,
                        ));
                    }
                    self.advance();
                    n.to_string()
                } else {
                    self.expect_ident_or_keyword()?
                };
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::OptionalField { object: Box::new(lhs), field }, span: self.span(start, end) })
            }

            // Index access
            TokenKind::LBracket => {
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Index { object: Box::new(lhs), index: Box::new(index) }, span: self.span(start, end) })
            }

            // Presence predicate (postfix ?) — evaluates to bool (OPT10/ER12).
            // OPT20/ER20: `expr? as v` binds the payload as a fresh let in
            // the then-branch. Consuming `as <ident>` here avoids the `as` cast
            // infix operator swallowing `v` as a type name. To cast a bool from
            // `?`, wrap in parens: `(x?) as i32`.
            TokenKind::Question => {
                self.advance();
                let mut end = self.tokens[self.pos - 1].span.end;
                let binding = if self.check(&TokenKind::As)
                    && matches!(self.peek(1), TokenKind::Ident(_))
                {
                    self.advance();
                    let name = self.expect_ident()?;
                    end = self.tokens[self.pos - 1].span.end;
                    Some(name)
                } else {
                    None
                };
                Ok(Expr { id: self.next_id(), kind: ExprKind::IsPresent { expr: Box::new(lhs), binding }, span: self.span(start, end) })
            }

            // Unwrap operator (!) - panics if None/Err
            TokenKind::Bang => {
                self.advance();
                let mut end = self.tokens[self.pos - 1].span.end;

                // Check for optional custom message: x! "message"
                let message = if matches!(self.peek(0), TokenKind::String(_)) {
                    let msg_token = self.advance();
                    end = msg_token.span.end;
                    if let TokenKind::String(s) = &msg_token.kind {
                        Some(s.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };

                Ok(Expr { id: self.next_id(), kind: ExprKind::Unwrap { expr: Box::new(lhs), message }, span: self.span(start, end) })
            }

            // Detect :: path separator (Rust syntax)
            TokenKind::ColonColon => {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unexpected '::'".to_string(),
                    hint: Some("use '.' for paths (e.g., Result.Ok) instead of '::'".to_string()),
                    why: None,
                });
            }

            _ => Ok(lhs),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<CallArg>, ParseError> {
        self.parse_args_with(false)
    }

    /// `raw_first_string`: take the first argument's string literal verbatim,
    /// without splitting it into interpolation segments. Only `format` wants
    /// this — its template is a compile-time input, not a string to render.
    fn parse_args_with(&mut self, raw_first_string: bool) -> Result<Vec<CallArg>, ParseError> {
        let mut args = Vec::new();
        self.skip_newlines();
        if self.check(&TokenKind::RParen) { return Ok(args); }

        let outer_list = std::mem::replace(&mut self.in_comma_list, true);
        let result = self.parse_args_loop(raw_first_string, &mut args);
        self.in_comma_list = outer_list;
        result?;
        Ok(args)
    }

    fn parse_args_loop(
        &mut self,
        raw_first_string: bool,
        args: &mut Vec<CallArg>,
    ) -> Result<(), ParseError> {
        loop {
            if raw_first_string && args.is_empty() {
                if let TokenKind::String(s) = self.current_kind().clone() {
                    let start = self.current().span.start;
                    self.advance();
                    let span = self.span(start, self.tokens[self.pos - 1].span.end);
                    args.push(CallArg {
                        name: None,
                        mode: ArgMode::Default,
                        expr: Expr { id: self.next_id(), kind: ExprKind::String(s), span },
                    });
                    self.skip_newlines();
                    if !self.match_token(&TokenKind::Comma) { break; }
                    self.skip_newlines();
                    if self.check(&TokenKind::RParen) { break; }
                    continue;
                }
            }
            // Capture named argument labels (name: expr)
            let arg_name = if let TokenKind::Ident(_) = self.current_kind().clone() {
                if self.peek(1) == &TokenKind::Colon {
                    let name = if let TokenKind::Ident(n) = self.current_kind().clone() {
                        n.clone()
                    } else {
                        unreachable!()
                    };
                    self.advance();
                    self.advance();
                    Some(name)
                } else {
                    None
                }
            } else {
                None
            };

            // Capture call-site mode keywords. `own` is overloaded — it also
            // prefixes an owned-closure literal (`own || body` / `own |x| body`).
            // When the next token is `|` or `||`, treat `own` as part of the
            // expression so parse_expr sees an owned closure, not ArgMode::Own.
            let mode = if self.check(&TokenKind::MutateKw) {
                self.advance();
                ArgMode::Mutate
            } else if self.check(&TokenKind::Own)
                && !matches!(self.peek(1), TokenKind::Pipe | TokenKind::PipePipe)
            {
                self.advance();
                ArgMode::Own
            } else {
                ArgMode::Default
            };

            let expr = self.parse_expr()?;
            args.push(CallArg { name: arg_name, mode, expr });
            self.skip_newlines();
            if !self.match_token(&TokenKind::Comma) { break; }
            self.skip_newlines();
            if self.check(&TokenKind::RParen) { break; }
        }

        Ok(())
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::If)?;

        let cond = self.parse_expr_no_braces()?;

        if let ExprKind::IsPattern { expr: scrutinee, pattern } = cond.kind {
            let then_branch = if self.match_token(&TokenKind::Colon) {
                self.parse_inline_block(start)?
            } else {
                self.skip_newlines();
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(start, end) }
            };

            let else_branch = if self.check(&TokenKind::Else) ||
                (self.check(&TokenKind::Newline) && self.peek_past_newlines_is_else()) {
                if self.check(&TokenKind::Newline) {
                    self.skip_newlines();
                }
                self.expect(&TokenKind::Else)?;
                if self.check(&TokenKind::If) {
                    Some(Box::new(self.parse_if_expr()?))
                } else if self.match_token(&TokenKind::Colon) {
                    Some(Box::new(self.parse_inline_block(start)?))
                } else {
                    self.skip_newlines();
                    let stmts = self.parse_block_body()?;
                    let end = self.tokens[self.pos - 1].span.end;
                    Some(Box::new(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(start, end) }))
                }
            } else {
                None
            };

            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr {
                id: self.next_id(),
                kind: ExprKind::IfLet { expr: scrutinee, pattern, then_branch: Box::new(then_branch), else_branch },
                span: self.span(start, end),
            });
        }

        let then_branch = if self.match_token(&TokenKind::Colon) {
            self.parse_inline_block(start)?
        } else {
            self.skip_newlines();
            let stmts = self.parse_block_body()?;
            let end = self.tokens[self.pos - 1].span.end;
            Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(start, end) }
        };

        let (else_branch, else_binding) = if self.check(&TokenKind::Else) ||
            (self.check(&TokenKind::Newline) && self.peek_past_newlines_is_else()) {
            if self.check(&TokenKind::Newline) {
                self.skip_newlines();
            }
            self.expect(&TokenKind::Else)?;
            // ER22: `else as e { … }` binds the error from a Result cond.
            let binding = if self.check(&TokenKind::As)
                && matches!(self.peek(1), TokenKind::Ident(_))
            {
                self.advance();
                Some(self.expect_ident()?)
            } else {
                None
            };
            let body = if self.check(&TokenKind::If) {
                Box::new(self.parse_if_expr()?)
            } else if self.match_token(&TokenKind::Colon) {
                Box::new(self.parse_inline_block(start)?)
            } else {
                self.skip_newlines();
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Box::new(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(start, end) })
            };
            (Some(body), binding)
        } else {
            (None, None)
        };

        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::If { cond: Box::new(cond), then_branch: Box::new(then_branch), else_branch, else_binding },
            span: self.span(start, end),
        })
    }

    /// Parse inline block after colon (doesn't consume terminator).
    fn parse_inline_block(&mut self, start: usize) -> Result<Expr, ParseError> {
        let stmt_start = self.current().span.start;

        let kind = match self.current_kind().clone() {
            TokenKind::Return => {
                self.advance();
                let value = if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                StmtKind::Return(value)
            }
            TokenKind::Break => {
                self.advance();
                if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) || self.at_end() {
                    StmtKind::Break { label: None, value: None }
                } else if let TokenKind::Ident(name) = self.current_kind().clone() {
                    // Disambiguate label vs value: if followed by infix op, it's a value
                    let next_is_infix = matches!(self.peek(1),
                        TokenKind::Plus | TokenKind::Star | TokenKind::Slash |
                        TokenKind::Percent | TokenKind::EqEq | TokenKind::BangEq |
                        TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq |
                        TokenKind::AmpAmp | TokenKind::PipePipe | TokenKind::Dot | TokenKind::LParen |
                        TokenKind::LBracket | TokenKind::DotDot
                    );
                    if next_is_infix {
                        StmtKind::Break { label: None, value: Some(self.parse_expr()?) }
                    } else {
                        self.advance();
                        if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) || self.at_end() {
                            StmtKind::Break { label: Some(name), value: None }
                        } else if self.is_expr_start() {
                            StmtKind::Break { label: Some(name), value: Some(self.parse_expr()?) }
                        } else {
                            StmtKind::Break { label: Some(name), value: None }
                        }
                    }
                } else if self.is_expr_start() {
                    StmtKind::Break { label: None, value: Some(self.parse_expr()?) }
                } else {
                    StmtKind::Break { label: None, value: None }
                }
            }
            TokenKind::Continue => {
                self.advance();
                let label = if let TokenKind::Ident(name) = self.current_kind().clone() {
                    if !self.check(&TokenKind::Newline) && !self.check(&TokenKind::Semi) {
                        self.advance();
                        Some(name)
                    } else { None }
                } else { None };
                StmtKind::Continue(label)
            }
            _ => {
                let expr = self.parse_expr()?;
                if self.match_token(&TokenKind::Eq) {
                    let value = self.parse_expr()?;
                    StmtKind::Assign { target: expr, value }
                } else if let Some(op) = self.match_compound_assign() {
                    let rhs = self.parse_expr()?;
                    let value = Expr {
                        id: self.next_id(),
                        kind: ExprKind::Binary {
                            op,
                            left: Box::new(expr.clone()),
                            right: Box::new(rhs),
                        },
                        span: expr.span.clone(),
                    };
                    StmtKind::Assign { target: expr, value }
                } else {
                    StmtKind::Expr(expr)
                }
            }
        };

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(stmt_start);
        let stmt = Stmt { id: self.next_id(), kind, span: self.span(stmt_start, end) };
        Ok(Expr { id: self.next_id(), kind: ExprKind::Block(vec![stmt]), span: self.span(start, end) })
    }

    fn parse_match_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Match)?;

        let scrutinee = self.parse_expr()?;
        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let pattern = self.parse_pattern()?;
            let guard = if self.match_token(&TokenKind::If) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };

            self.expect(&TokenKind::FatArrow)?;
            self.skip_newlines();

            let body = if self.check(&TokenKind::LBrace) {
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: self.span(start, end) }
            } else {
                self.parse_inline_block(start)?
            };

            arms.push(MatchArm { pattern, guard, body: Box::new(body) });
            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr { id: self.next_id(), kind: ExprKind::Match { scrutinee: Box::new(scrutinee), arms }, span: self.span(start, end) })
    }

    /// Parse `using Name { }` or `using A, B(args) { }`.
    /// Multi-context desugars to nested blocks:
    /// `using A, B { body }` → `using A { using B { body } }`
    fn parse_using_block(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Using)?;

        let mut contexts: Vec<(String, Vec<CallArg>)> = Vec::new();
        loop {
            let mut name = self.expect_ident()?;
            // Generic args on context types: `using Pool<Entity>, Multitasking { ... }`
            // (mem.context-clauses/CC4). Mirrors the loop in parse_base_type.
            if self.match_token(&TokenKind::Lt) {
                name.push('<');
                loop {
                    if let TokenKind::Int(n, _) = self.current_kind().clone() {
                        self.advance();
                        name.push_str(&n.to_string());
                    } else {
                        name.push_str(&self.parse_type_name()?);
                    }
                    if self.pending_gt {
                        break;
                    }
                    if self.match_token(&TokenKind::Comma) {
                        name.push_str(", ");
                    } else {
                        break;
                    }
                }
                self.expect_gt_in_generic()?;
                name.push('>');
            }
            let args = if self.match_token(&TokenKind::LParen) {
                let args = self.parse_args()?;
                self.expect(&TokenKind::RParen)?;
                args
            } else {
                Vec::new()
            };
            contexts.push((name, args));
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }

        self.skip_newlines();
        let body = self.parse_block_body()?;
        let end = self.tokens[self.pos - 1].span.end;
        let span = self.span(start, end);

        // Build nested UsingBlock from innermost (last) outward
        let (innermost_name, innermost_args) = contexts.pop().unwrap();
        let mut expr = Expr {
            id: self.next_id(),
            kind: ExprKind::UsingBlock { name: innermost_name, args: innermost_args, body },
            span,
        };

        while let Some((name, args)) = contexts.pop() {
            let wrapper_body = vec![Stmt {
                id: self.next_id(),
                kind: StmtKind::Expr(expr),
                span,
            }];
            expr = Expr {
                id: self.next_id(),
                kind: ExprKind::UsingBlock { name, args, body: wrapper_body },
                span,
            };
        }

        Ok(expr)
    }

    /// Reject a stray keyword after `as` in a with-binding. Bindings are
    /// always mutable — read-only access comes from the source (`.read()`,
    /// frozen pools) — so there is nothing to annotate here.
    fn reject_with_binding_keyword(&mut self) -> Result<(), ParseError> {
        if matches!(self.current_kind(), TokenKind::Mut | TokenKind::Let | TokenKind::Const) {
            let err = ParseError {
                span: self.current().span,
                message: "with-bindings take a bare name".to_string(),
                hint: Some("bindings are mutable; read-only access comes from the source (`.read()`, frozen pools) — write `as name`".to_string()),
                why: None,
            };
            self.advance();
            return Err(err);
        }
        Ok(())
    }

    fn parse_with_binding(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::With)?;
        let first_ident = self.expect_ident()?;

        let mut bindings = Vec::new();

        // Parse first binding (ident already consumed)
        let first_expr = self.build_with_as_expr(start, first_ident)?;
        self.expect(&TokenKind::As)?;
        self.reject_with_binding_keyword()?;
        let first_name = self.expect_ident()?;
        bindings.push(WithBinding {
            source: first_expr,
            name: first_name,
        });

        // Parse additional comma-separated bindings
        while self.match_token(&TokenKind::Comma) {
            // Use bp=22 to stop before consuming 'as' (which has bp=21)
            let expr = self.parse_expr_bp(22)?;
            self.expect(&TokenKind::As)?;
            self.reject_with_binding_keyword()?;
            let name = self.expect_ident()?;
            bindings.push(WithBinding {
                source: expr,
                name,
            });
        }

        let body = if self.match_token(&TokenKind::Colon) {
            let inline = self.parse_inline_block(start)?;
            match inline.kind {
                ExprKind::Block(stmts) => stmts,
                _ => vec![Stmt {
                    id: self.next_id(),
                    kind: StmtKind::Expr(inline.clone()),
                    span: inline.span,
                }],
            }
        } else {
            self.skip_newlines();
            self.parse_block_body()?
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::WithAs { bindings, body },
            span: self.span(start, end),
        })
    }


    /// Build an expression from an already-consumed ident, parsing postfix
    /// [index], .field, and .method() until we reach the `as` keyword.
    fn build_with_as_expr(&mut self, start: usize, ident: String) -> Result<Expr, ParseError> {
        let ident_end = self.current().span.start;
        let mut expr = Expr {
            id: self.next_id(),
            kind: ExprKind::Ident(ident),
            span: self.span(start, ident_end),
        };

        loop {
            if self.check(&TokenKind::LBracket) {
                self.advance();
                let index = self.parse_expr_bp(0)?;
                let end = self.current().span.end;
                self.expect(&TokenKind::RBracket)?;
                expr = Expr {
                    id: self.next_id(),
                    kind: ExprKind::Index { object: Box::new(expr), index: Box::new(index) },
                    span: self.span(start, end),
                };
            } else if self.check(&TokenKind::Dot) {
                self.advance();

                // Tuple field access: expr.0, expr.1, ...
                if let TokenKind::Int(n, None) = self.current_kind().clone() {
                    if n < 0 {
                        return Err(ParseError::expected(
                            "a non-negative index",
                            self.current_kind(),
                            self.current().span,
                        ));
                    }
                    let field = n.to_string();
                    self.advance();
                    let end = self.tokens[self.pos - 1].span.end;
                    expr = Expr {
                        id: self.next_id(),
                        kind: ExprKind::Field { object: Box::new(expr), field },
                        span: self.span(start, end),
                    };
                    continue;
                }

                let field = self.expect_ident_or_keyword()?;
                // Method call: .field(args...)
                if self.check(&TokenKind::LParen) {
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(&TokenKind::RParen)?;
                    let end = self.tokens[self.pos - 1].span.end;
                    expr = Expr {
                        id: self.next_id(),
                        kind: ExprKind::MethodCall {
                            object: Box::new(expr),
                            method: field,
                            type_args: None,
                            args,
                        },
                        span: self.span(start, end),
                    };
                } else {
                    let end = self.tokens[self.pos - 1].span.end;
                    expr = Expr {
                        id: self.next_id(),
                        kind: ExprKind::Field { object: Box::new(expr), field },
                        span: self.span(start, end),
                    };
                }
            } else {
                break;
            }
        }

        Ok(expr)
    }

    fn parse_select_expr(&mut self, is_priority: bool) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        if is_priority {
            self.expect(&TokenKind::SelectPriority)?;
        } else {
            self.expect(&TokenKind::Select)?;
        }
        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut arms = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.check(&TokenKind::Eof) {
            let arm = self.parse_select_arm()?;
            arms.push(arm);

            // Arms separated by commas or newlines
            if self.match_token(&TokenKind::Comma) {
                self.skip_newlines();
            } else {
                self.skip_newlines();
            }
        }

        self.expect(&TokenKind::RBrace)?;
        let end = self.tokens[self.pos - 1].span.end;

        if arms.is_empty() {
            return Err(ParseError {
                span: self.span(start, end),
                message: "select requires at least one arm".to_string(),
                hint: None,
                why: None,
            });
        }

        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Select { arms, is_priority },
            span: self.span(start, end),
        })
    }

    fn parse_select_arm(&mut self) -> Result<SelectArm, ParseError> {
        // Default arm: `_: body`
        if let TokenKind::Ident(ref name) = self.current_kind().clone() {
            if name == "_" {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                self.skip_newlines();
                let body = self.parse_expr()?;
                return Ok(SelectArm {
                    kind: SelectArmKind::Default,
                    body: Box::new(body),
                });
            }
        }

        // Parse channel expression at bp 9 (above comparison, so `<` isn't consumed)
        let old = self.allow_brace_expr;
        self.allow_brace_expr = false;
        let channel = self.parse_expr_bp(9)?;
        self.allow_brace_expr = old;

        if self.match_token(&TokenKind::Arrow) {
            // Recv arm: `channel -> binding: body`
            let binding = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            self.skip_newlines();
            let body = self.parse_expr()?;
            Ok(SelectArm {
                kind: SelectArmKind::Recv { channel, binding },
                body: Box::new(body),
            })
        } else if self.check(&TokenKind::Lt) {
            // Send arm: `channel <- value: body`
            // `<` then `-` as two tokens to avoid breaking `x < -y` elsewhere
            self.advance(); // consume `<`
            self.expect(&TokenKind::Minus)?;
            let value = self.parse_expr_no_braces()?;
            self.expect(&TokenKind::Colon)?;
            self.skip_newlines();
            let body = self.parse_expr()?;
            Ok(SelectArm {
                kind: SelectArmKind::Send { channel, value },
                body: Box::new(body),
            })
        } else {
            Err(ParseError::expected(
                "'->' or '<-'",
                self.current_kind(),
                self.current().span,
            ))
        }
    }

    /// Parse string interpolation segments from a string like "hello {name}, age {age}".
    /// Returns None if the string has no valid interpolation (e.g., escaped braces only).
    fn parse_string_interpolation(&mut self, s: &str, str_span: Span) -> Option<Vec<StringSegment>> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let chars: Vec<char> = s.chars().collect();
        let mut i = 0;
        // A string with escapes but no expressions still has to come back as
        // segments — returning None handed the raw text on with the `{{` still
        // in it, and the next scanner down read `{braces}` as an expression
        // (#521, fmt/F4).
        let mut escaped_any = false;

        while i < chars.len() {
            if chars[i] == '{' {
                if i + 1 < chars.len() && chars[i + 1] == '{' {
                    // Escaped brace: {{ → {
                    literal.push('{');
                    escaped_any = true;
                    i += 2;
                    continue;
                }
                // Start of interpolation expression
                if !literal.is_empty() {
                    segments.push(StringSegment::Literal(std::mem::take(&mut literal)));
                }
                i += 1; // skip '{'
                let expr_start = i;
                let mut depth = 1;
                while i < chars.len() && depth > 0 {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    if depth > 0 { i += 1; }
                }
                if depth != 0 {
                    return None; // Unclosed brace
                }
                let expr_str: String = chars[expr_start..i].iter().collect();
                i += 1; // skip '}'

                // Calculate byte offset of this expression within the string content
                let abs_offset = str_span.start + 1 + s.char_indices()
                    .nth(expr_start)
                    .map(|(pos, _)| pos)
                    .unwrap_or(0);
                // `{}` and `{:spec}` are placeholders the runtime formatter
                // fills in — nothing to parse here.
                if expr_str.is_empty() || expr_str.starts_with(':') {
                    literal.push('{');
                    literal.push_str(&expr_str);
                    literal.push('}');
                    continue;
                }

                // `{expr}` or `{expr:spec}` (fmt/F1–F4). Split the same way the
                // formatter does, then sanity-check the spec: a real one is a
                // short run of spec characters. Anything with quotes or commas
                // in it means these braces were never an interpolation —
                // `"{\"x\":1,\"y\":2}"` used to parse as the expression `"x"`
                // with `1,"y":2` as its "format spec", and the rest of the JSON
                // vanished without a word (#506).
                let (expr_part, spec) = match rask_ast::fmt_spec::split_spec(&expr_str) {
                    Some(pos) => (&expr_str[..pos], Some(&expr_str[pos + 1..])),
                    None => (expr_str.as_str(), None),
                };
                // The spec has to be one the formatters actually understand.
                // A character-class guess accepted ` 1`, so a one-pair JSON
                // body `{"k": 1}` parsed as the expression `"k"` with ` 1` as
                // its spec and printed `k` (#506).
                let parsed_spec = spec.map(rask_ast::fmt_spec::parse_spec);
                let spec_is_plausible = !matches!(parsed_spec, Some(None));
                let parsed_spec = parsed_spec.flatten();

                let bad_expr = |parser: &mut Self, detail: &str| {
                    parser.errors.push(ParseError {
                        span: Span::new(abs_offset, abs_offset + expr_str.len()),
                        message: format!("`{{{}}}` is not a valid interpolation: {}", expr_str, detail),
                        hint: Some("write `{{` for a literal `{` — a lone `{` starts an interpolation".to_string()),
                        why: None,
                    }.with_why("a `{` in a string always starts an interpolation, so what follows has to be an expression"));
                };

                if !spec_is_plausible {
                    bad_expr(self, "there's more here than an expression and a format spec");
                    return None;
                }

                // Parse the expression using the lexer/parser with correct context
                let lex = rask_lexer::Lexer::new(expr_part).tokenize();
                if !lex.errors.is_empty() {
                    bad_expr(self, "the text inside doesn't lex");
                    return None;
                }
                // Reuse this parser's file_id and get sequential NodeIds
                let saved_tokens = std::mem::replace(&mut self.tokens, lex.tokens);
                let saved_pos = std::mem::replace(&mut self.pos, 0);

                let result = self.parse_expr();
                // The whole expression part belongs to the interpolation.
                let leftover = !self.at_end()
                    && !matches!(self.current_kind(), TokenKind::Newline);

                self.tokens = saved_tokens;
                self.pos = saved_pos;

                let mut parsed = match result {
                    Ok(expr) => expr,
                    Err(e) => {
                        bad_expr(self, &e.message);
                        return None;
                    }
                };
                if leftover {
                    bad_expr(self, "there's more here than one expression");
                    return None;
                }
                // `{"x"}` renders the literal it already is, so nobody writes
                // one on purpose — but a JSON body starts with exactly that
                // shape. `"{\"x\":1}"` splits into the expression `"x"` with
                // `1` as a width spec, and prints `x` (#506). The spec grammar
                // can't tell those apart; the string literal can.
                if matches!(parsed.kind, ExprKind::String(_)) {
                    bad_expr(self, "a string literal on its own isn't something to interpolate");
                    return None;
                }

                // Remap spans from 0-based (within expr_str) to absolute file position.
                // str_span.start is the opening quote, +1 for content start, +byte_offset for position.
                Self::offset_spans(&mut parsed, abs_offset);

                segments.push(StringSegment::Expr(Box::new(parsed), parsed_spec));
            } else if chars[i] == '}' && i + 1 < chars.len() && chars[i + 1] == '}' {
                // Escaped brace: }} → }
                literal.push('}');
                escaped_any = true;
                i += 2;
            } else {
                literal.push(chars[i]);
                i += 1;
            }
        }

        if !literal.is_empty() {
            segments.push(StringSegment::Literal(literal));
        }

        // Segments are worth returning when there's an expression to evaluate,
        // or an escape whose unescaping would otherwise be lost.
        if escaped_any || segments.iter().any(|s| matches!(s, StringSegment::Expr(..))) {
            Some(segments)
        } else {
            None
        }
    }

    /// Offset all spans in an expression tree by a byte amount.
    fn offset_spans(expr: &mut Expr, offset: usize) {
        expr.span.start += offset;
        expr.span.end += offset;
        match &mut expr.kind {
            ExprKind::Binary { left, right, .. } => {
                Self::offset_spans(left, offset);
                Self::offset_spans(right, offset);
            }
            ExprKind::Unary { operand, .. } => Self::offset_spans(operand, offset),
            ExprKind::Call { func, args } => {
                Self::offset_spans(func, offset);
                for arg in args { Self::offset_spans(&mut arg.expr, offset); }
            }
            ExprKind::MethodCall { object, args, .. } => {
                Self::offset_spans(object, offset);
                for arg in args { Self::offset_spans(&mut arg.expr, offset); }
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                Self::offset_spans(object, offset);
            }
            ExprKind::Index { object, index } => {
                Self::offset_spans(object, offset);
                Self::offset_spans(index, offset);
            }
            ExprKind::Try { expr } => Self::offset_spans(expr, offset),
            ExprKind::Take { place } => Self::offset_spans(place, offset),
            ExprKind::Catch { value, clause } => {
                Self::offset_spans(value, offset);
                Self::offset_spans(&mut clause.body, offset);
            }
            ExprKind::Unwrap { expr, .. } => Self::offset_spans(expr, offset),
            ExprKind::Cast { expr, .. } => Self::offset_spans(expr, offset),
            ExprKind::Convert { expr, .. } => Self::offset_spans(expr, offset),
            ExprKind::NullCoalesce { value, default } => {
                Self::offset_spans(value, offset);
                Self::offset_spans(default, offset);
            }
            ExprKind::Array(exprs) | ExprKind::Tuple(exprs) => {
                for e in exprs { Self::offset_spans(e, offset); }
            }
            _ => {}
        }
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let first = self.parse_single_pattern()?;

        if self.check(&TokenKind::Pipe) {
            let mut patterns = vec![first];
            while self.match_token(&TokenKind::Pipe) {
                patterns.push(self.parse_single_pattern()?);
            }
            Ok(Pattern::Or(patterns))
        } else {
            Ok(first)
        }
    }

    fn parse_single_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.current_kind().clone() {
            TokenKind::LParen => {
                self.advance();
                let mut patterns = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.at_end() {
                    patterns.push(self.parse_pattern()?);
                    if !self.match_token(&TokenKind::Comma) { break; }
                }
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::Tuple(patterns))
            }
            TokenKind::Ident(name) if name == "_" => {
                self.advance();
                Ok(Pattern::Wildcard)
            }
            // OPT15: `x is none` — the absent branch named as a type. `none`
            // is a keyword, so it never reaches the identifier arm. No `as`
            // form: there's no payload to bind.
            TokenKind::None => {
                self.advance();
                Ok(Pattern::TypePat { ty_name: "none".to_string(), binding: None })
            }
            TokenKind::Ident(name) => {
                self.advance();

                // Handle qualified paths: Enum.Variant or Enum.Variant(args) or Enum.Variant { fields }
                let mut name = if self.match_token(&TokenKind::Dot) {
                    let variant = self.expect_ident()?;
                    format!("{}.{}", name, variant)
                } else {
                    name
                };

                // rask#217: generic type patterns — `is Vec<i32>`, `is Map<K, V>`.
                // After `is`, `<` can't be a comparison, so consume generic args.
                if self.check(&TokenKind::Lt) {
                    self.advance();
                    name.push('<');
                    loop {
                        if let TokenKind::Int(n, _) = self.current_kind().clone() {
                            self.advance();
                            name.push_str(&n.to_string());
                        } else {
                            name.push_str(&self.parse_type_name()?);
                        }
                        if self.pending_gt { break; }
                        if self.match_token(&TokenKind::Comma) {
                            name.push_str(", ");
                        } else {
                            break;
                        }
                    }
                    self.expect_gt_in_generic()?;
                    name.push('>');
                }

                if self.match_token(&TokenKind::LParen) {
                    // Constructor pattern: Name(patterns...) or Enum.Variant(patterns...)
                    let mut fields = Vec::new();
                    while !self.check(&TokenKind::RParen) && !self.at_end() {
                        fields.push(self.parse_pattern()?);
                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Pattern::Constructor { name, fields })
                } else if self.check(&TokenKind::As)
                    && matches!(self.peek(1), TokenKind::Ident(_))
                {
                    // ER23: type pattern `Type as binding`. Unqualified name
                    // without a constructor — interpret as a type match.
                    self.advance();
                    let binding = self.expect_ident()?;
                    Ok(Pattern::TypePat { ty_name: name, binding: Some(binding) })
                } else if self.check(&TokenKind::LBrace) && name.contains('.') && self.allow_brace_expr {
                    // Struct variant pattern: Enum.Variant { field1, field2 }
                    // Only for qualified names, and only when braces are allowed (not in while/if conditions)
                    self.advance();
                    self.skip_newlines();
                    let mut fields = Vec::new();
                    while !self.check(&TokenKind::RBrace) && !self.at_end() {
                        let field_name = self.expect_ident()?;
                        let pattern = if self.match_token(&TokenKind::Colon) {
                            self.parse_pattern()?
                        } else {
                            // Shorthand: { field } means { field: field }
                            Pattern::Ident(field_name.clone())
                        };
                        fields.push((field_name, pattern));
                        if !self.match_token(&TokenKind::Comma) {
                            self.skip_newlines();
                            if !self.check(&TokenKind::RBrace) { continue; }
                        } else {
                            self.skip_newlines();
                        }
                    }
                    self.expect(&TokenKind::RBrace)?;
                    Ok(Pattern::Struct { name, fields, rest: false })
                } else {
                    Ok(Pattern::Ident(name))
                }
            }
            TokenKind::Int(n, suffix) => {
                self.advance();
                let span = self.tokens[self.pos - 1].span.clone();
                let start = Box::new(Expr { id: self.next_id(), kind: ExprKind::Int(n, suffix.clone()), span });
                if self.match_token(&TokenKind::DotDotEq) {
                    let end = self.parse_single_pattern()?;
                    if let Pattern::Literal(end_expr) = end {
                        return Ok(Pattern::Range { start, end: end_expr });
                    }
                    return Err(ParseError::expected("literal", self.current_kind(), self.current().span));
                }
                Ok(Pattern::Literal(start))
            }
            TokenKind::String(s) => {
                self.advance();
                let span = self.tokens[self.pos - 1].span.clone();
                Ok(Pattern::Literal(Box::new(Expr { id: self.next_id(), kind: ExprKind::String(s), span })))
            }
            TokenKind::Bool(b) => {
                self.advance();
                let span = self.tokens[self.pos - 1].span.clone();
                Ok(Pattern::Literal(Box::new(Expr { id: self.next_id(), kind: ExprKind::Bool(b), span })))
            }
            TokenKind::Char(c) => {
                self.advance();
                let span = self.tokens[self.pos - 1].span.clone();
                let start = Box::new(Expr { id: self.next_id(), kind: ExprKind::Char(c), span });
                if self.match_token(&TokenKind::DotDotEq) {
                    let end = self.parse_single_pattern()?;
                    if let Pattern::Literal(end_expr) = end {
                        return Ok(Pattern::Range { start, end: end_expr });
                    }
                    return Err(ParseError::expected("literal", self.current_kind(), self.current().span));
                }
                Ok(Pattern::Literal(start))
            }
            _ => Err(ParseError::expected(
                "pattern",
                self.current_kind(),
                self.current().span,
            )),
        }
    }

    // =========================================================================
    // Operator Precedence
    // =========================================================================

    const PREFIX_BP: u8 = 23;

    fn postfix_bp(&self) -> Option<u8> {
        match self.current_kind() {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot | TokenKind::QuestionDot => Some(25),
            TokenKind::Question | TokenKind::Bang => Some(24),
            TokenKind::ColonColon => Some(25), // Same precedence as dot for better error messages
            _ => None,
        }
    }

    fn infix_bp(&self) -> Option<(u8, u8)> {
        match self.current_kind() {
            TokenKind::PipePipe => Some((1, 2)),
            TokenKind::AmpAmp => Some((3, 4)),
            TokenKind::EqEq | TokenKind::BangEq => Some((5, 6)),
            TokenKind::Lt | TokenKind::Gt | TokenKind::LtEq | TokenKind::GtEq => Some((7, 8)),
            TokenKind::QuestionQuestion | TokenKind::Catch => Some((9, 10)),
            TokenKind::Pipe => Some((11, 12)),
            TokenKind::Caret => Some((13, 14)),
            TokenKind::Amp => Some((15, 16)),
            TokenKind::LtLt | TokenKind::GtGt => Some((17, 18)),
            TokenKind::Plus | TokenKind::Minus => Some((19, 20)),
            TokenKind::Star | TokenKind::Slash | TokenKind::Percent => Some((21, 22)),
            TokenKind::DotDot | TokenKind::DotDotEq => Some((3, 4)), // Low precedence for ranges
            _ => None,
        }
    }

    fn parse_binop(&mut self) -> Result<BinOp, ParseError> {
        let op = match self.current_kind() {
            TokenKind::Plus => BinOp::Add,
            TokenKind::Minus => BinOp::Sub,
            TokenKind::Star => BinOp::Mul,
            TokenKind::Slash => BinOp::Div,
            TokenKind::Percent => BinOp::Mod,
            TokenKind::EqEq => BinOp::Eq,
            TokenKind::BangEq => BinOp::Ne,
            TokenKind::Lt => BinOp::Lt,
            TokenKind::Gt => BinOp::Gt,
            TokenKind::LtEq => BinOp::Le,
            TokenKind::GtEq => BinOp::Ge,
            TokenKind::AmpAmp => BinOp::And,
            TokenKind::PipePipe => BinOp::Or,
            TokenKind::Amp => BinOp::BitAnd,
            TokenKind::Pipe => BinOp::BitOr,
            TokenKind::Caret => BinOp::BitXor,
            TokenKind::LtLt => BinOp::Shl,
            TokenKind::GtGt => BinOp::Shr,
            _ => return Err(ParseError::expected(
                "operator like '+' or '-'",
                self.current_kind(),
                self.current().span,
            )),
        };
        self.advance();
        Ok(op)
    }
}

/// Result of parsing: declarations plus any errors found.
#[derive(Debug)]
pub struct ParseResult {
    pub decls: Vec<Decl>,
    pub errors: Vec<ParseError>,
}

impl ParseResult {
    /// Returns true if parsing completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// A parser error with location and friendly message.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
    pub hint: Option<String>,
    /// Why this is a rule, in the reader's terms. Most parse errors share the
    /// generic "expected valid syntax" line; set this when there's something
    /// more useful to say.
    pub why: Option<String>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

impl ParseError {
    fn expected(expected: &str, found: &TokenKind, span: Span) -> Self {
        let message = format_expected_message(expected, found);
        let hint = crate::hints::for_expected(expected, found).map(String::from);
        Self { span, message, hint, why: None }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Replace the generic "expected valid syntax" explanation with one that
    /// says what the actual rule is.
    fn with_why(mut self, why: impl Into<String>) -> Self {
        self.why = Some(why.into());
        self
    }

    fn not_implemented(feature: &str, hint: &str, span: Span) -> Self {
        Self {
            span,
            message: format!("{} are not yet implemented", feature),
            hint: Some(hint.to_string()),
            why: None,
        }
    }
}

/// Format a user-friendly "expected X, found Y" message.
fn format_expected_message(expected: &str, found: &TokenKind) -> String {
    // Handle common cases with specific messages
    match expected {
        "';'" | "newline or ';'" => "Expected ';' or newline after statement".to_string(),
        "':'" => format!("Expected ':', found {}", found.display_name()),
        "'{'" => format!("Expected '{{' to start block, found {}", found.display_name()),
        "'}'" => format!("Expected '}}' to close block, found {}", found.display_name()),
        "'('" => format!("Expected '(', found {}", found.display_name()),
        "')'" => {
            if matches!(found, TokenKind::Eof) {
                "Unclosed '(' - missing ')'".to_string()
            } else {
                format!("Expected ')', found {}", found.display_name())
            }
        }
        "'['" => format!("Expected '[', found {}", found.display_name()),
        "']'" => {
            if matches!(found, TokenKind::Eof) {
                "Unclosed '[' - missing ']'".to_string()
            } else {
                format!("Expected ']', found {}", found.display_name())
            }
        }
        "'>'" => format!("Expected '>', found {}", found.display_name()),
        "'='" => format!("Expected '=', found {}", found.display_name()),
        "a name" | "identifier" => format!("Expected name, found {}", found.display_name()),
        "expression" => format!("Expected expression, found {}", found.display_name()),
        "type" => format!("Expected type, found {}", found.display_name()),
        "pattern" => format!("Expected pattern, found {}", found.display_name()),
        "declaration (func, struct, enum, trait, extend, import, const)" => {
            format!("Expected declaration, found {}", found.display_name())
        }
        _ => format!("Expected {}, found {}", expected, found.display_name()),
    }
}

/// How a keyword token is spelled in source. `None` for anything that isn't a
/// keyword. Used where keywords are allowed as names (field and method
/// Keywords the expression parser accepts in prefix position. A function named
/// with one of these is shadowed at every call site; a function named with any
/// other keyword doesn't parse at all. The two need different explanations.
fn starts_an_expression(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Assert
            | TokenKind::Check
            | TokenKind::Comptime
            | TokenKind::If
            | TokenKind::Loop
            | TokenKind::Match
            | TokenKind::None
            | TokenKind::Null
            | TokenKind::Own
            | TokenKind::ReadKw
            | TokenKind::Select
            | TokenKind::SelectPriority
            | TokenKind::Try
            | TokenKind::Take
            | TokenKind::Unsafe
            | TokenKind::Using
            | TokenKind::With
            | TokenKind::For
            | TokenKind::While
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue
    )
}

/// positions) and to name one in a diagnostic.
fn keyword_spelling(kind: &TokenKind) -> Option<&'static str> {
    Some(match kind {
        // Control flow
        TokenKind::If => "if",
        TokenKind::Else => "else",
        TokenKind::Match => "match",
        TokenKind::For => "for",
        TokenKind::In => "in",
        TokenKind::While => "while",
        TokenKind::Loop => "loop",
        TokenKind::Break => "break",
        TokenKind::Continue => "continue",
        TokenKind::Return => "return",
        // Declarations
        TokenKind::Func => "func",
        TokenKind::Let => "let",
        TokenKind::Mut => "mut",
        TokenKind::Const => "const",
        TokenKind::Struct => "struct",
        TokenKind::Enum => "enum",
        TokenKind::Trait => "trait",
        TokenKind::Extend => "extend",
        TokenKind::Import => "import",
        TokenKind::Type => "type",
        // Modifiers
        TokenKind::Public => "public",
        TokenKind::Private => "private",
        TokenKind::Take => "take",
        TokenKind::Own => "own",
        TokenKind::ReadKw => "read",
        TokenKind::MutateKw => "mutate",
        TokenKind::Unsafe => "unsafe",
        TokenKind::Comptime => "comptime",
        TokenKind::Native => "native",
        TokenKind::Export => "export",
        TokenKind::Using => "using",
        TokenKind::Lazy => "lazy",
        // Concurrency
        TokenKind::Select => "select",
        TokenKind::With => "with",
        // Error handling
        TokenKind::Ensure => "ensure",
        TokenKind::Try => "try",
        TokenKind::Catch => "catch",
        // Testing
        TokenKind::Test => "test",
        TokenKind::Benchmark => "benchmark",
        TokenKind::Assert => "assert",
        TokenKind::Check => "check",
        // Operators/keywords
        TokenKind::As => "as",
        TokenKind::Is => "is",
        TokenKind::Where => "where",
        TokenKind::Or => "or",
        // Literals/constants
        TokenKind::Bool(true) => "true",
        TokenKind::Bool(false) => "false",
        TokenKind::None => "none",
        TokenKind::Null => "null",
        // Other
        TokenKind::Extern => "extern",
        TokenKind::Asm => "asm",
        TokenKind::Discard => "discard",
        // Build system
        TokenKind::Package => "package",
        TokenKind::Scope => "scope",
        TokenKind::Feature => "feature",
        TokenKind::Profile => "profile",
        _ => return None,
    })
}


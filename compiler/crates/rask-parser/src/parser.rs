// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The parser implementation using Pratt parsing for expressions.

use rask_ast::decl::{BenchmarkDecl, ConstDecl, Decl, DeclKind, EnumDecl, ExternDecl, Field, FnDecl, ImplDecl, ImportDecl, Param, StructDecl, TestDecl, TraitDecl, TypeParam, Variant};
use rask_ast::expr::{BinOp, ClosureParam, Expr, ExprKind, FieldInit, MatchArm, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::token::{Token, TokenKind};
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
    /// Collected errors during parsing
    errors: Vec<ParseError>,
    /// Counter for generating unique NodeIds
    next_node_id: u32,
    /// Pending declarations from expanded grouped imports
    pending_decls: Vec<Decl>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0, pending_gt: false, allow_brace_expr: true, errors: Vec::new(), next_node_id: 0, pending_decls: Vec::new() }
    }

    fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
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
                TokenKind::Extern | TokenKind::Public if brace_depth == 0 => {
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
        while self.check(&TokenKind::Newline) {
            self.advance();
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
        let name = match self.current_kind().clone() {
            TokenKind::Ident(name) => name,
            // Control flow
            TokenKind::If => "if".to_string(),
            TokenKind::Else => "else".to_string(),
            TokenKind::Match => "match".to_string(),
            TokenKind::For => "for".to_string(),
            TokenKind::In => "in".to_string(),
            TokenKind::While => "while".to_string(),
            TokenKind::Loop => "loop".to_string(),
            TokenKind::Break => "break".to_string(),
            TokenKind::Continue => "continue".to_string(),
            TokenKind::Return => "return".to_string(),
            TokenKind::Deliver => "deliver".to_string(),
            // Declarations
            TokenKind::Func => "func".to_string(),
            TokenKind::Let => "let".to_string(),
            TokenKind::Const => "const".to_string(),
            TokenKind::Struct => "struct".to_string(),
            TokenKind::Enum => "enum".to_string(),
            TokenKind::Trait => "trait".to_string(),
            TokenKind::Extend => "extend".to_string(),
            TokenKind::Import => "import".to_string(),
            TokenKind::Type => "type".to_string(),
            // Modifiers
            TokenKind::Public => "public".to_string(),
            TokenKind::Take => "take".to_string(),
            TokenKind::Own => "own".to_string(),
            TokenKind::ReadKw => "read".to_string(),
            TokenKind::MutateKw => "mutate".to_string(),
            TokenKind::Unsafe => "unsafe".to_string(),
            TokenKind::Comptime => "comptime".to_string(),
            TokenKind::Native => "native".to_string(),
            TokenKind::Export => "export".to_string(),
            TokenKind::Using => "using".to_string(),
            TokenKind::Lazy => "lazy".to_string(),
            // Concurrency
            TokenKind::Spawn => "spawn".to_string(),
            TokenKind::SpawnThread => "spawn_thread".to_string(),
            TokenKind::SpawnRaw => "spawn_raw".to_string(),
            TokenKind::Select => "select".to_string(),
            TokenKind::With => "with".to_string(),
            // Error handling
            TokenKind::Ensure => "ensure".to_string(),
            TokenKind::Catch => "catch".to_string(),
            TokenKind::Try => "try".to_string(),
            // Testing
            TokenKind::Test => "test".to_string(),
            TokenKind::Benchmark => "benchmark".to_string(),
            TokenKind::Assert => "assert".to_string(),
            TokenKind::Check => "check".to_string(),
            // Operators/keywords
            TokenKind::As => "as".to_string(),
            TokenKind::Is => "is".to_string(),
            TokenKind::Where => "where".to_string(),
            TokenKind::Step => "step".to_string(),
            TokenKind::Or => "or".to_string(),
            // Literals/constants
            TokenKind::Bool(true) => "true".to_string(),
            TokenKind::Bool(false) => "false".to_string(),
            TokenKind::None => "none".to_string(),
            TokenKind::Null => "null".to_string(),
            // Other
            TokenKind::Extern => "extern".to_string(),
            TokenKind::Asm => "asm".to_string(),
            _ => return Err(ParseError::expected(
                "a name",
                self.current_kind(),
                self.current().span,
            ).with_hint("Names start with a letter or '_'")),
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
        let mut depth = 1;

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
        self.skip_newlines();

        while !self.at_end() || !self.pending_decls.is_empty() {
            match self.parse_decl() {
                Ok(decl) => decls.push(decl),
                Err(e) => {
                    if !self.record_error(e) {
                        break;
                    }
                    self.synchronize();
                }
            }
            self.skip_newlines();
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

        // Detect common Rust keywords
        if let TokenKind::Ident(s) = self.current_kind() {
            if s == "pub" {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unknown keyword 'pub'".to_string(),
                    hint: Some("use 'public' instead of 'pub'".to_string()),
                });
            } else if s == "fn" {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unknown keyword 'fn'".to_string(),
                    hint: Some("use 'func' instead of 'fn'".to_string()),
                });
            }
        }

        let kind = match self.current_kind() {
            TokenKind::Func => self.parse_fn_decl(is_pub, is_comptime, is_unsafe, attrs)?,
            TokenKind::Struct => self.parse_struct_decl(is_pub, attrs)?,
            TokenKind::Enum => self.parse_enum_decl(is_pub)?,
            TokenKind::Trait => self.parse_trait_decl(is_pub)?,
            TokenKind::Extend => self.parse_impl_decl()?,
            TokenKind::Import => self.parse_import_decl()?,
            TokenKind::Export => self.parse_export_decl()?,
            TokenKind::Const => self.parse_const_decl(is_pub)?,
            TokenKind::Test => self.parse_test_decl(is_comptime)?,
            TokenKind::Benchmark => self.parse_benchmark_decl()?,
            TokenKind::Extern => self.parse_extern_decl()?,
            _ => {
                return Err(ParseError::expected(
                    "declaration (func, struct, enum, trait, extend, import, export, const, test, benchmark, extern)",
                    self.current_kind(),
                    self.current().span,
                ));
            }
        };

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);
        Ok(Decl { id: self.next_id(), kind, span: Span::new(start, end) })
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
                    attr.push_str(&format!("{:?}", self.current_kind()));
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

    fn parse_fn_decl(&mut self, is_pub: bool, is_comptime: bool, is_unsafe: bool, attrs: Vec<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Func)?;
        let mut name = self.expect_ident()?;

        let type_params = if self.match_token(&TokenKind::Lt) {
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

        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

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

        Ok(DeclKind::Fn(FnDecl { name, type_params, params, ret_ty, body, is_pub, is_comptime, is_unsafe, attrs }))
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
            let name = self.expect_ident_or_keyword()?;

            let ty = if self.match_token(&TokenKind::Colon) {
                self.parse_type_name()?
            } else if name == "self" {
                "Self".to_string()
            } else {
                return Err(ParseError::expected(
                    "':'",
                    self.current_kind(),
                    self.current().span,
                ).with_hint("Parameters need a type, like: name: Type"));
            };

            let default = if self.match_token(&TokenKind::Eq) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            params.push(Param { name, ty, is_take, is_mutate, default });

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }

        Ok(params)
    }

    fn parse_type_name(&mut self) -> Result<String, ParseError> {
        let base = self.parse_base_type()?;

        if self.check(&TokenKind::Or) {
            self.advance();
            let error_ty = self.parse_type_name()?;
            return Ok(format!("Result<{}, {}>", base, error_ty));
        }

        Ok(base)
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

        // Handle raw pointer types: *const T, *mut T
        if self.check(&TokenKind::Star) {
            self.advance();
            let mutability = if self.check(&TokenKind::Const) {
                self.advance();
                "const"
            } else if matches!(self.current_kind(), TokenKind::Ident(s) if s == "mut") {
                self.advance();
                "mut"
            } else {
                return Err(ParseError::expected(
                    "'const' or 'mut' after '*' in pointer type",
                    self.current_kind(),
                    self.current().span,
                ));
            };
            let pointee_ty = self.parse_type_name()?;
            return Ok(format!("*{} {}", mutability, pointee_ty));
        }

        if self.check(&TokenKind::LParen) {
            self.advance();
            if self.check(&TokenKind::RParen) {
                self.advance();
                return Ok("()".to_string());
            }
            let mut types = Vec::new();
            loop {
                types.push(self.parse_type_name()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            return Ok(format!("({})", types.join(", ")));
        }

        if self.check(&TokenKind::LBracket) {
            self.advance();

            if self.check(&TokenKind::RBracket) {
                self.advance();
                let elem_ty = self.parse_type_name()?;
                return Ok(format!("[]{}", elem_ty));
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

        // Closure type: |T1, T2| -> R
        if self.check(&TokenKind::Pipe) {
            self.advance();
            let mut params = Vec::new();
            if !self.check(&TokenKind::Pipe) {
                loop {
                    params.push(self.parse_type_name()?);
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
                    params.push(self.parse_type_name()?);
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
                if self.match_token(&TokenKind::Comma) {
                    name.push_str(", ");
                } else {
                    break;
                }
            }
            self.expect_gt_in_generic()?;
            name.push('>');
        }

        if self.match_token(&TokenKind::Question) {
            name.push('?');
        }

        if self.check(&TokenKind::Dot) && matches!(self.peek(1), TokenKind::LBrace) {
            self.advance();
            self.advance();
            name.push_str(".{");
            let mut first = true;
            while !self.check(&TokenKind::RBrace) && !self.at_end() {
                if !first { name.push_str(", "); }
                first = false;
                name.push_str(&self.expect_ident()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RBrace)?;
            name.push('}');
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
                // Regular type parameter: `T` or `T: Trait`
                let mut bounds = vec![];
                if self.match_token(&TokenKind::Colon) {
                    // Parse trait bounds (simplified - just one for now)
                    bounds.push(self.expect_ident()?);
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

            if self.match_token(&TokenKind::Comma) {
                name_suffix.push_str(", ");
            } else {
                break;
            }
        }

        self.expect(&TokenKind::Gt)?;
        name_suffix.push('>');

        Ok((type_params, name_suffix))
    }

    fn parse_struct_decl(&mut self, is_pub: bool, attrs: Vec<String>) -> Result<DeclKind, ParseError> {
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

            let field_pub = self.match_token(&TokenKind::Public);

            // Detect trailing/separator comma (Rust syntax)
            if self.check(&TokenKind::Comma) {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unexpected ',' in struct definition".to_string(),
                    hint: Some("struct fields are separated by newlines, not commas".to_string()),
                });
            }

            if self.check(&TokenKind::Func) {
                if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(field_pub, false, false, vec![])? {
                    methods.push(fn_decl);
                }
            } else {
                let field_name = self.expect_ident_or_keyword()?;
                self.expect(&TokenKind::Colon)?;
                let ty = self.parse_type_name()?;
                fields.push(Field { name: field_name, ty, is_pub: field_pub });
            }

            self.skip_newlines();
        }

        // Detect trailing comma (Rust syntax)
        if self.check(&TokenKind::Comma) {
            return Err(ParseError {
                span: self.current().span,
                message: "unexpected ',' in struct definition".to_string(),
                hint: Some("struct fields are separated by newlines, not commas".to_string()),
            });
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Struct(StructDecl {
            name,
            type_params,
            fields,
            methods,
            is_pub,
            attrs,
        }))
    }

    fn parse_enum_decl(&mut self, is_pub: bool) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Enum)?;
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

        let mut variants = Vec::new();
        let mut methods = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            if self.check(&TokenKind::Func) || (self.check(&TokenKind::Public) && matches!(self.peek(1), TokenKind::Func)) {
                let m_pub = self.match_token(&TokenKind::Public);
                if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(m_pub, false, false, vec![])? {
                    methods.push(fn_decl);
                }
            } else {
                let variant_name = self.expect_ident()?;
                let mut fields = Vec::new();

                if self.match_token(&TokenKind::LParen) {
                    let mut idx = 0;
                    while !self.check(&TokenKind::RParen) && !self.at_end() {
                        let (field_name, ty) = if self.check(&TokenKind::Ident(String::new())) {
                            if self.peek(1) == &TokenKind::Colon {
                                let name = self.expect_ident()?;
                                self.advance();
                                let ty = self.parse_type_name()?;
                                (name, ty)
                            } else {
                                let ty = self.parse_type_name()?;
                                (format!("_{}", idx), ty)
                            }
                        } else {
                            let ty = self.parse_type_name()?;
                            (format!("_{}", idx), ty)
                        };

                        fields.push(Field { name: field_name, ty, is_pub: false });
                        idx += 1;

                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen)?;
                } else if self.check(&TokenKind::LBrace) {
                    // Struct-style variant: Move { x: i32, y: i32 }
                    self.advance();
                    self.skip_newlines();
                    while !self.check(&TokenKind::RBrace) && !self.at_end() {
                        let field_name = self.expect_ident()?;
                        self.expect(&TokenKind::Colon)?;
                        let ty = self.parse_type_name()?;
                        fields.push(Field { name: field_name, ty, is_pub: false });
                        if !self.match_token(&TokenKind::Comma) {
                            self.skip_newlines();
                            if !self.check(&TokenKind::RBrace) { continue; }
                        } else {
                            self.skip_newlines();
                        }
                    }
                    self.expect(&TokenKind::RBrace)?;
                }

                variants.push(Variant { name: variant_name, fields });
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
        }))
    }

    fn parse_trait_decl(&mut self, is_pub: bool) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Trait)?;
        let name = self.expect_ident()?;

        if self.match_token(&TokenKind::Lt) {
            while !self.check(&TokenKind::Gt) && !self.at_end() {
                self.advance();
            }
            self.expect(&TokenKind::Gt)?;
        }

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            if self.check(&TokenKind::Func) {
                if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(false, false, false, vec![])? {
                    methods.push(fn_decl);
                }
            } else if let TokenKind::Ident(_) = self.current_kind() {
                let fn_decl = self.parse_trait_method_shorthand()?;
                methods.push(fn_decl);
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Trait(TraitDecl { name, methods, is_pub }))
    }

    fn parse_trait_method_shorthand(&mut self) -> Result<FnDecl, ParseError> {
        let mut name = self.expect_ident()?;

        let type_params = if self.match_token(&TokenKind::Lt) {
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

        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        if self.check(&TokenKind::Newline) {
            self.skip_newlines();
        }
        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            Vec::new()
        };

        Ok(FnDecl {
            name,
            type_params,
            params,
            ret_ty,
            body,
            is_pub: false,
            is_comptime: false,
            is_unsafe: false,
            attrs: vec![],
        })
    }

    fn parse_impl_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Extend)?;
        let target_ty = self.parse_type_name()?;

        let trait_name = if self.match_token(&TokenKind::With) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            let mut method_attrs = Vec::new();
            while self.check(&TokenKind::At) {
                method_attrs.push(self.parse_attribute()?);
                self.skip_newlines();
            }
            let m_pub = self.match_token(&TokenKind::Public);
            if let DeclKind::Fn(fn_decl) = self.parse_fn_decl(m_pub, false, false, method_attrs)? {
                methods.push(fn_decl);
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Impl(ImplDecl { trait_name, target_ty, methods }))
    }

    fn parse_import_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Import)?;

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
                span: Span::new(start, end),
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
    fn parse_const_decl(&mut self, is_pub: bool) -> Result<DeclKind, ParseError> {
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
        Ok(DeclKind::Const(ConstDecl { name, ty, init, is_pub }))
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

    fn parse_extern_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Extern)?;

        // Parse ABI string (e.g., "C", "system")
        let abi = self.expect_string()?;

        // Expect func keyword
        self.expect(&TokenKind::Func)?;

        // Parse function name
        let name = self.expect_ident()?;

        // Parse parameters
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;

        // Parse optional return type
        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        Ok(DeclKind::Extern(ExternDecl { abi, name, params, ret_ty }))
    }

    // =========================================================================
    // Statement Parsing
    // =========================================================================

    /// Parse a block body (statements inside braces), with error recovery.
    fn parse_block_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

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
                TokenKind::Let | TokenKind::Const | TokenKind::Return |
                TokenKind::If | TokenKind::While | TokenKind::For |
                TokenKind::Loop | TokenKind::Match | TokenKind::Break |
                TokenKind::Continue | TokenKind::Ensure |
                TokenKind::Assert | TokenKind::Check => return,
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
            TokenKind::Let => self.parse_let_stmt()?,
            TokenKind::Const => self.parse_const_stmt()?,
            TokenKind::Return => self.parse_return_stmt()?,
            TokenKind::Break => self.parse_break_stmt()?,
            TokenKind::Continue => self.parse_continue_stmt()?,
            TokenKind::Deliver => self.parse_deliver_stmt()?,
            TokenKind::While => self.parse_while_stmt(label)?,
            TokenKind::Loop => self.parse_loop_stmt(label)?,
            TokenKind::For => self.parse_for_stmt(label)?,
            TokenKind::Ensure => self.parse_ensure_stmt()?,
            TokenKind::Comptime => self.parse_comptime_stmt()?,
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
        Ok(Stmt { id: self.next_id(), kind, span: Span::new(start, end) })
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

    fn parse_let_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Let)?;

        // Detect 'mut' keyword after 'let' (Rust syntax)
        if let TokenKind::Ident(s) = self.current_kind() {
            if s == "mut" {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unexpected 'mut' keyword".to_string(),
                    hint: Some("'let' is already mutable in Rask. Use 'const' for immutable bindings".to_string()),
                });
            }
        }

        if self.match_token(&TokenKind::LParen) {
            let mut names = Vec::new();
            loop {
                names.push(self.expect_ident()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            self.expect(&TokenKind::Eq)?;
            let init = self.parse_expr()?;
            self.expect_terminator()?;
            return Ok(StmtKind::LetTuple { names, init });
        }

        let name = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) { Some(self.parse_type_name()?) } else { None };
        self.expect(&TokenKind::Eq)?;
        let init = self.parse_expr()?;
        self.expect_terminator()?;
        Ok(StmtKind::Let { name, ty, init })
    }

    fn parse_const_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Const)?;

        if self.match_token(&TokenKind::LParen) {
            let mut names = Vec::new();
            loop {
                names.push(self.expect_ident()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            self.expect(&TokenKind::Eq)?;
            let init = self.parse_expr()?;
            self.expect_terminator()?;
            return Ok(StmtKind::ConstTuple { names, init });
        }

        let name = self.expect_ident()?;
        let ty = if self.match_token(&TokenKind::Colon) { Some(self.parse_type_name()?) } else { None };
        self.expect(&TokenKind::Eq)?;
        let init = self.parse_expr()?;
        self.expect_terminator()?;
        Ok(StmtKind::Const { name, ty, init })
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
        let label = if let TokenKind::Ident(name) = self.current_kind().clone() {
            self.advance();
            Some(name)
        } else {
            None
        };
        self.expect_terminator()?;
        Ok(StmtKind::Break(label))
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

    fn parse_deliver_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Deliver)?;

        let (label, value) = if let TokenKind::Ident(name) = self.current_kind().clone() {
            self.advance();
            if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) {
                (None, Expr { id: self.next_id(), kind: ExprKind::Ident(name), span: self.tokens[self.pos - 1].span.clone() })
            } else if self.is_expr_start() {
                (Some(name), self.parse_expr()?)
            } else {
                (None, Expr { id: self.next_id(), kind: ExprKind::Ident(name), span: self.tokens[self.pos - 1].span.clone() })
            }
        } else {
            (None, self.parse_expr()?)
        };

        self.expect_terminator()?;
        Ok(StmtKind::Deliver { label, value })
    }

    fn is_expr_start(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::Int(_, _) | TokenKind::Float(_, _) | TokenKind::String(_) | TokenKind::Bool(_)
                | TokenKind::Ident(_) | TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket
                | TokenKind::If | TokenKind::Match | TokenKind::With | TokenKind::Spawn
                | TokenKind::Minus | TokenKind::Bang | TokenKind::Pipe | TokenKind::Try
                | TokenKind::Amp | TokenKind::Star | TokenKind::Tilde
                | TokenKind::None | TokenKind::Null
        )
    }

    fn parse_while_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::While)?;

        let cond = self.parse_expr_no_braces()?;

        if self.match_token(&TokenKind::Is) {
            let pattern = self.parse_pattern()?;
            let body = if self.match_token(&TokenKind::Colon) {
                vec![self.parse_stmt()?]
            } else {
                self.skip_newlines();
                self.parse_block_body()?
            };
            let _ = label;
            return Ok(StmtKind::WhileLet { pattern, expr: cond, body });
        }

        let body = if self.match_token(&TokenKind::Colon) {
            vec![self.parse_stmt()?]
        } else {
            self.skip_newlines();
            self.parse_block_body()?
        };
        let _ = label;
        Ok(StmtKind::While { cond, body })
    }

    fn parse_loop_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Loop)?;
        self.skip_newlines();
        let body = self.parse_block_body()?;
        Ok(StmtKind::Loop { label, body })
    }

    fn parse_for_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::For)?;
        let binding = self.expect_ident()?;
        self.expect(&TokenKind::In)?;
        let iter = self.parse_expr_no_braces()?;
        let body = if self.match_token(&TokenKind::Colon) {
            vec![self.parse_stmt()?]
        } else {
            self.skip_newlines();
            self.parse_block_body()?
        };
        Ok(StmtKind::For { label, binding, iter, body })
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

        let catch = if self.check(&TokenKind::Catch) {
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
        Ok(StmtKind::Ensure { body, catch })
    }

    fn parse_comptime_stmt(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::Comptime)?;
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

    fn parse_expr_bp(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        let mut lhs = self.parse_prefix()?;

        loop {
            if self.check(&TokenKind::Newline) && self.peek_past_newlines_is_postfix() {
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
                    span: Span::new(start, end),
                };
                continue;
            }

            if let Some((l_bp, r_bp)) = self.infix_bp() {
                if l_bp < min_bp { break; }

                if self.check(&TokenKind::QuestionQuestion) {
                    self.advance();
                    let default = self.parse_expr_bp(r_bp)?;
                    let end = default.span.end;
                    lhs = Expr {
                        id: self.next_id(),
                        kind: ExprKind::NullCoalesce {
                            value: Box::new(lhs),
                            default: Box::new(default),
                        },
                        span: Span::new(start, end),
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
                        span: Span::new(start, end),
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
                    span: Span::new(start, end),
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
                Ok(Expr { id: self.next_id(), kind: ExprKind::Int(n, suffix.clone()), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::Float(n, suffix) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Float(n, suffix.clone()), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::String(s) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::String(s), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::Char(c) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Char(c), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Bool(b), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
            }

            TokenKind::None => {
                self.advance();
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Ident("None".to_string()), span: Span::new(start, end) })
            }
            TokenKind::Null => {
                self.advance();
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Ident("null".to_string()), span: Span::new(start, end) })
            }

            TokenKind::Ident(name) => {
                self.advance();
                let mut full_name = name.clone();

                // Parse generic arguments for type names (for static methods or struct literals)
                if Self::is_type_name(&name) && self.check(&TokenKind::Lt) {
                    // Check if this looks like a generic instantiation
                    let is_static_method = self.looks_like_generic_type_with_static_method();
                    let is_struct_literal = {
                        // Look ahead to see if this could be a struct literal: Name<Args> {
                        let mut lookahead_pos = self.pos + 1; // Skip the '<'
                        let mut depth = 1;
                        let mut found_brace = false;

                        // Scan through the generic args to find the closing '>'
                        while lookahead_pos < self.tokens.len() && depth > 0 {
                            match &self.tokens[lookahead_pos].kind {
                                TokenKind::Lt => depth += 1,
                                TokenKind::Gt => {
                                    depth -= 1;
                                    if depth == 0 {
                                        // Check if the next token after '>' is '{'
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

                    if is_static_method || is_struct_literal {
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
                    Ok(Expr { id: self.next_id(), kind: ExprKind::Ident(full_name), span: Span::new(start, end) })
                }
            }

            TokenKind::Minus => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(operand) }, span: Span::new(start, end) })
            }
            TokenKind::Bang => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(operand) }, span: Span::new(start, end) })
            }
            TokenKind::Tilde => {
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::BitNot, operand: Box::new(operand) }, span: Span::new(start, end) })
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
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Deref, operand: Box::new(operand) }, span: Span::new(start, end) })
            }

            TokenKind::Own => {
                self.advance();
                self.parse_expr_bp(Self::PREFIX_BP)
            }

            TokenKind::LParen => self.parse_paren_or_tuple(),

            TokenKind::LBracket => self.parse_array_literal(),

            TokenKind::LBrace => {
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) })
            }

            TokenKind::PipePipe => {
                self.advance();
                let body = self.parse_expr()?;
                let end = body.span.end;
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Closure { params: vec![], body: Box::new(body) },
                    span: Span::new(start, end),
                })
            }

            TokenKind::Pipe => self.parse_closure(),

            TokenKind::If => self.parse_if_expr(),

            TokenKind::Match => self.parse_match_expr(),

            TokenKind::With => self.parse_with_block(),

            TokenKind::Spawn => self.parse_spawn_expr(),

            TokenKind::SpawnThread => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::BlockCall { name: "spawn_thread".to_string(), body }, span: Span::new(start, end) })
            }

            TokenKind::SpawnRaw => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::BlockCall { name: "spawn_raw".to_string(), body }, span: Span::new(start, end) })
            }

            TokenKind::Unsafe => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unsafe { body }, span: Span::new(start, end) })
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
                Ok(Expr { id: self.next_id(), kind: ExprKind::Comptime { body }, span: Span::new(start, end) })
            }

            TokenKind::Assert => self.parse_assert_expr(),

            TokenKind::Check => self.parse_check_expr(),

            TokenKind::Try => {
                self.advance();
                let inner = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = inner.span.end;
                Ok(Expr {
                    id: self.next_id(),
                    kind: ExprKind::Try(Box::new(inner)),
                    span: Span::new(start, end),
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
            span: Span::new(start, end),
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
            span: Span::new(start, end),
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
            span: Span::new(start, end),
        })
    }

    fn parse_paren_or_tuple(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::LParen)?;

        if self.check(&TokenKind::RParen) {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr { id: self.next_id(), kind: ExprKind::Tuple(Vec::new()), span: Span::new(start, end) });
        }

        let first = self.parse_expr()?;

        if self.match_token(&TokenKind::Comma) {
            let mut elements = vec![first];
            while !self.check(&TokenKind::RParen) && !self.at_end() {
                elements.push(self.parse_expr()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Expr { id: self.next_id(), kind: ExprKind::Tuple(elements), span: Span::new(start, end) })
        } else {
            self.expect(&TokenKind::RParen)?;
            Ok(first)
        }
    }

    fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::LBracket)?;
        self.skip_newlines();

        if self.check(&TokenKind::RBracket) {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr { id: self.next_id(), kind: ExprKind::Array(Vec::new()), span: Span::new(start, end) });
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
                span: Span::new(start, end),
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
        Ok(Expr { id: self.next_id(), kind: ExprKind::Array(elements), span: Span::new(start, end) })
    }

    fn parse_closure(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Pipe)?;

        let mut params = Vec::new();
        while !self.check(&TokenKind::Pipe) && !self.at_end() {
            let name = self.expect_ident()?;
            let ty = if self.match_token(&TokenKind::Colon) {
                Some(self.parse_type_name()?)
            } else {
                None
            };
            params.push(ClosureParam { name, ty });
            if !self.match_token(&TokenKind::Comma) { break; }
        }

        self.expect(&TokenKind::Pipe)?;

        // Optional return type annotation
        let _return_type = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        let body = self.parse_expr()?;
        let end = body.span.end;

        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Closure { params, body: Box::new(body) },
            span: Span::new(start, end),
        })
    }

    fn parse_postfix(&mut self, lhs: Expr) -> Result<Expr, ParseError> {
        let start = lhs.span.start;

        match self.current_kind() {
            TokenKind::LParen => {
                self.advance();
                let args = self.parse_args()?;
                self.expect(&TokenKind::RParen)?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Call { func: Box::new(lhs), args }, span: Span::new(start, end) })
            }

            TokenKind::Dot => {
                self.advance();
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
                        span: Span::new(start, end),
                    })
                } else if type_args.is_some() {
                    // Had generic args but no parens - error
                    Err(ParseError::expected(
                        "'('",
                        self.current_kind(),
                        self.current().span,
                    ).with_hint("Generic type arguments must be followed by ()"))
                } else if self.check(&TokenKind::LBrace) {
                    // Struct variant constructor: Enum.Variant { field: value }
                    // Only when base is a type name (uppercase) to avoid ambiguity with blocks
                    if let ExprKind::Ident(base) = &lhs.kind {
                        if base.starts_with(|c: char| c.is_uppercase()) && field.starts_with(|c: char| c.is_uppercase()) {
                            let full_name = format!("{}.{}", base, field);
                            self.parse_struct_literal(full_name, start)
                        } else {
                            let end = self.tokens[self.pos - 1].span.end;
                            Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: Span::new(start, end) })
                        }
                    } else {
                        let end = self.tokens[self.pos - 1].span.end;
                        Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: Span::new(start, end) })
                    }
                } else {
                    let end = self.tokens[self.pos - 1].span.end;
                    Ok(Expr { id: self.next_id(), kind: ExprKind::Field { object: Box::new(lhs), field }, span: Span::new(start, end) })
                }
            }

            // Optional chaining
            TokenKind::QuestionDot => {
                self.advance();
                let field = self.expect_ident_or_keyword()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::OptionalField { object: Box::new(lhs), field }, span: Span::new(start, end) })
            }

            // Index access
            TokenKind::LBracket => {
                self.advance();
                let index = self.parse_expr()?;
                self.expect(&TokenKind::RBracket)?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Index { object: Box::new(lhs), index: Box::new(index) }, span: Span::new(start, end) })
            }

            // Try operator (?)
            // Note: Postfix ? is for optional chaining (T?).
            // For Result error propagation, use prefix 'try expr' instead.
            TokenKind::Question => {
                self.advance();
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Try(Box::new(lhs)), span: Span::new(start, end) })
            }

            // Detect :: path separator (Rust syntax)
            TokenKind::ColonColon => {
                return Err(ParseError {
                    span: self.current().span,
                    message: "unexpected '::'".to_string(),
                    hint: Some("use '.' for paths (e.g., Result.Ok) instead of '::'".to_string()),
                });
            }

            _ => Ok(lhs),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.check(&TokenKind::RParen) { return Ok(args); }

        loop {
            if let TokenKind::Ident(_) = self.current_kind().clone() {
                if self.peek(1) == &TokenKind::Colon {
                    self.advance();
                    self.advance();
                }
            }
            args.push(self.parse_expr()?);
            if !self.match_token(&TokenKind::Comma) { break; }
        }

        Ok(args)
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::If)?;

        let cond = self.parse_expr_no_braces()?;

        if self.match_token(&TokenKind::Is) {
            let pattern = self.parse_pattern()?;

            let then_branch = if self.match_token(&TokenKind::Colon) {
                self.parse_inline_block(start)?
            } else {
                self.skip_newlines();
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }
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
                    Some(Box::new(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }))
                }
            } else {
                None
            };

            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr {
                id: self.next_id(),
                kind: ExprKind::IfLet { expr: Box::new(cond), pattern, then_branch: Box::new(then_branch), else_branch },
                span: Span::new(start, end),
            });
        }

        let then_branch = if self.match_token(&TokenKind::Colon) {
            self.parse_inline_block(start)?
        } else {
            self.skip_newlines();
            let stmts = self.parse_block_body()?;
            let end = self.tokens[self.pos - 1].span.end;
            Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }
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
                Some(Box::new(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }))
            }
        } else {
            None
        };

        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::If { cond: Box::new(cond), then_branch: Box::new(then_branch), else_branch },
            span: Span::new(start, end),
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
                let label = if let TokenKind::Ident(name) = self.current_kind().clone() {
                    if !self.check(&TokenKind::Newline) && !self.check(&TokenKind::Semi) {
                        self.advance();
                        Some(name)
                    } else { None }
                } else { None };
                StmtKind::Break(label)
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
            TokenKind::Deliver => {
                self.advance();
                let (label, value) = if let TokenKind::Ident(name) = self.current_kind().clone() {
                    self.advance();
                    if self.check(&TokenKind::Newline) || self.check(&TokenKind::Semi) {
                        (None, Expr { id: self.next_id(), kind: ExprKind::Ident(name), span: self.tokens[self.pos - 1].span.clone() })
                    } else if self.is_expr_start() {
                        (Some(name), self.parse_expr()?)
                    } else {
                        (None, Expr { id: self.next_id(), kind: ExprKind::Ident(name), span: self.tokens[self.pos - 1].span.clone() })
                    }
                } else {
                    (None, self.parse_expr()?)
                };
                StmtKind::Deliver { label, value }
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
        let stmt = Stmt { id: self.next_id(), kind, span: Span::new(stmt_start, end) };
        Ok(Expr { id: self.next_id(), kind: ExprKind::Block(vec![stmt]), span: Span::new(start, end) })
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
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }
            } else {
                self.parse_inline_block(start)?
            };

            arms.push(MatchArm { pattern, guard, body: Box::new(body) });
            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr { id: self.next_id(), kind: ExprKind::Match { scrutinee: Box::new(scrutinee), arms }, span: Span::new(start, end) })
    }

    fn parse_with_block(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::With)?;
        let name = self.expect_ident()?;

        // Disambiguate: with...as (element binding) vs with...{ } (resource scoping)
        // If ident is followed by [ or ., it's an expression → with...as mode
        if self.check(&TokenKind::LBracket) || self.check(&TokenKind::Dot) {
            return self.parse_with_as(start, name);
        }

        let args = if self.match_token(&TokenKind::LParen) {
            let args = self.parse_args()?;
            self.expect(&TokenKind::RParen)?;
            args
        } else {
            Vec::new()
        };

        self.skip_newlines();
        let body = self.parse_block_body()?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::WithBlock { name, args, body },
            span: Span::new(start, end),
        })
    }

    /// Parse with...as element binding: with expr as name, ... { body }
    fn parse_with_as(&mut self, start: usize, first_ident: String) -> Result<Expr, ParseError> {
        let mut bindings = Vec::new();

        // Parse first binding (ident already consumed)
        let first_expr = self.build_with_as_expr(start, first_ident)?;
        self.expect(&TokenKind::As)?;
        let first_name = self.expect_ident()?;
        bindings.push((first_expr, first_name));

        // Parse additional comma-separated bindings
        while self.match_token(&TokenKind::Comma) {
            // Use bp=22 to stop before consuming 'as' (which has bp=21)
            let expr = self.parse_expr_bp(22)?;
            self.expect(&TokenKind::As)?;
            let name = self.expect_ident()?;
            bindings.push((expr, name));
        }

        self.skip_newlines();
        let body = self.parse_block_body()?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::WithAs { bindings, body },
            span: Span::new(start, end),
        })
    }

    /// Build an expression from an already-consumed ident, parsing postfix [index] and .field
    /// until we reach the `as` keyword.
    fn build_with_as_expr(&mut self, start: usize, ident: String) -> Result<Expr, ParseError> {
        let ident_end = self.current().span.start;
        let mut expr = Expr {
            id: self.next_id(),
            kind: ExprKind::Ident(ident),
            span: Span::new(start, ident_end),
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
                    span: Span::new(start, end),
                };
            } else if self.check(&TokenKind::Dot) {
                self.advance();
                let field = self.expect_ident()?;
                let end = self.tokens[self.pos - 1].span.end;
                expr = Expr {
                    id: self.next_id(),
                    kind: ExprKind::Field { object: Box::new(expr), field },
                    span: Span::new(start, end),
                };
            } else {
                break;
            }
        }

        Ok(expr)
    }

    fn parse_spawn_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::Spawn)?;
        self.skip_newlines();
        let body = self.parse_block_body()?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Spawn { body },
            span: Span::new(start, end),
        })
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
            TokenKind::Ident(name) => {
                self.advance();

                // Handle qualified paths: Enum.Variant or Enum.Variant(args) or Enum.Variant { fields }
                let name = if self.match_token(&TokenKind::Dot) {
                    let variant = self.expect_ident()?;
                    format!("{}.{}", name, variant)
                } else {
                    name
                };

                if self.match_token(&TokenKind::LParen) {
                    // Constructor pattern: Name(patterns...) or Enum.Variant(patterns...)
                    let mut fields = Vec::new();
                    while !self.check(&TokenKind::RParen) && !self.at_end() {
                        fields.push(self.parse_pattern()?);
                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Pattern::Constructor { name, fields })
                } else if self.check(&TokenKind::LBrace) && name.contains('.') {
                    // Struct variant pattern: Enum.Variant { field1, field2 }
                    // Only for qualified names to avoid ambiguity with blocks
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
                Ok(Pattern::Literal(Box::new(Expr { id: self.next_id(), kind: ExprKind::Int(n, suffix.clone()), span })))
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
                Ok(Pattern::Literal(Box::new(Expr { id: self.next_id(), kind: ExprKind::Char(c), span })))
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
            TokenKind::Question => Some(24),
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
            TokenKind::QuestionQuestion => Some((9, 10)),
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
        Self { span, message, hint }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn not_implemented(feature: &str, hint: &str, span: Span) -> Self {
        Self {
            span,
            message: format!("{} are not yet implemented", feature),
            hint: Some(hint.to_string()),
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

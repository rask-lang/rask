//! The parser implementation using Pratt parsing for expressions.

use rask_ast::decl::{BenchmarkDecl, ConstDecl, Decl, DeclKind, EnumDecl, Field, FnDecl, ImplDecl, ImportDecl, Param, StructDecl, TestDecl, TraitDecl, Variant};
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
    /// Create a new parser for the given tokens.
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0, pending_gt: false, allow_brace_expr: true, errors: Vec::new(), next_node_id: 0, pending_decls: Vec::new() }
    }

    /// Generate a new unique NodeId.
    fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// Record an error and return whether we should continue parsing.
    fn record_error(&mut self, error: ParseError) -> bool {
        self.errors.push(error);
        self.errors.len() < MAX_ERRORS
    }

    /// Synchronize after an error by skipping to the next declaration.
    fn synchronize(&mut self) {
        // Skip to the next top-level declaration, properly handling brace blocks
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
                        // After closing a top-level block, we're at declaration level
                        if brace_depth == 0 {
                            self.skip_newlines();
                            return;
                        }
                    } else {
                        // Unmatched } - skip it
                        self.advance();
                    }
                }
                // Only check for declaration keywords when we're not inside braces
                TokenKind::Func | TokenKind::Struct | TokenKind::Enum |
                TokenKind::Trait | TokenKind::Extend | TokenKind::Import |
                TokenKind::Public if brace_depth == 0 => {
                    return;
                }
                _ => { self.advance(); }
            }
        }
    }

    // =========================================================================
    // Token Navigation
    // =========================================================================

    /// Get the current token.
    fn current(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or_else(|| self.tokens.last().unwrap())
    }

    /// Get the current token kind.
    fn current_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    /// Peek ahead n tokens.
    fn peek(&self, n: usize) -> &TokenKind {
        self.tokens.get(self.pos + n).map(|t| &t.kind).unwrap_or(&TokenKind::Eof)
    }

    /// Check if we're at the end of the token stream.
    fn at_end(&self) -> bool {
        matches!(self.current_kind(), TokenKind::Eof)
    }

    /// Advance to the next token and return the previous one.
    fn advance(&mut self) -> &Token {
        if !self.at_end() {
            self.pos += 1;
        }
        self.tokens.get(self.pos - 1).unwrap()
    }

    /// Check if the current token matches the given kind.
    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.current_kind()) == std::mem::discriminant(kind)
    }

    /// If the current token matches, advance and return true.
    fn match_token(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Expect a specific token kind, or return an error.
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

    /// Skip newline tokens.
    fn skip_newlines(&mut self) {
        while self.check(&TokenKind::Newline) {
            self.advance();
        }
    }

    /// Expect a newline or semicolon (statement terminator).
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

    /// Get an identifier string from the current token.
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

    /// Get a string literal from the current token.
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

    /// Get an identifier string, allowing keywords to be used as identifiers.
    /// This is used for field/method names where keywords are contextually valid.
    fn expect_ident_or_keyword(&mut self) -> Result<String, ParseError> {
        let name = match self.current_kind().clone() {
            TokenKind::Ident(name) => name,
            // Allow keywords to be used as field/method names
            TokenKind::Spawn => "spawn".to_string(),
            TokenKind::Match => "match".to_string(),
            TokenKind::If => "if".to_string(),
            TokenKind::Else => "else".to_string(),
            TokenKind::For => "for".to_string(),
            TokenKind::While => "while".to_string(),
            TokenKind::Loop => "loop".to_string(),
            TokenKind::Return => "return".to_string(),
            TokenKind::Break => "break".to_string(),
            TokenKind::Continue => "continue".to_string(),
            TokenKind::With => "with".to_string(),
            TokenKind::In => "in".to_string(),
            TokenKind::As => "as".to_string(),
            TokenKind::Is => "is".to_string(),
            TokenKind::Step => "step".to_string(),
            _ => return Err(ParseError::expected(
                "a name",
                self.current_kind(),
                self.current().span,
            )),
        };
        self.advance();
        Ok(name)
    }

    /// Check if the current identifier looks like a type name (PascalCase).
    fn is_type_name(name: &str) -> bool {
        name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
    }

    /// Look ahead past newlines to check if there's a postfix operator (for method chaining).
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

    /// Look ahead past newlines to check if there's an `else` keyword (for if-else continuation).
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

    /// Look ahead to check if < is the start of generic type args for a method call.
    /// Returns true if we see `<` followed by balanced `<>` and then `(`.
    fn looks_like_generic_method_call(&self) -> bool {
        // We're at `<`, look ahead to find matching `>` followed by `(`
        self.looks_like_generic_followed_by(&TokenKind::LParen)
    }

    fn looks_like_generic_type_with_static_method(&self) -> bool {
        // We're at `<`, look ahead to find matching `>` followed by `.`
        self.looks_like_generic_followed_by(&TokenKind::Dot)
    }

    fn looks_like_generic_followed_by(&self, expected: &TokenKind) -> bool {
        let mut pos = self.pos + 1; // skip the <
        let mut depth = 1;

        while pos < self.tokens.len() && depth > 0 {
            match &self.tokens[pos].kind {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt => depth -= 1,
                TokenKind::GtGt => {
                    // >> counts as two >'s
                    depth -= 2;
                    if depth < 0 {
                        // This would mean unbalanced, but >>  could mean we closed and have extra >
                        // For safety, just check if next matches expected
                        if pos + 1 < self.tokens.len() {
                            return &self.tokens[pos + 1].kind == expected;
                        }
                        return false;
                    }
                }
                TokenKind::Eof | TokenKind::Newline | TokenKind::Semi => {
                    // Hit end of statement without finding >
                    return false;
                }
                _ => {}
            }
            pos += 1;
        }

        // After balanced <>, check if next token matches expected
        if depth == 0 && pos < self.tokens.len() {
            return &self.tokens[pos].kind == expected;
        }
        false
    }

    /// Expect `>` in generic context, handling `>>` by splitting it.
    fn expect_gt_in_generic(&mut self) -> Result<(), ParseError> {
        // Check for pending > from a previous >> split
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
                // Split >> into two >'s - consume one, mark other as pending
                self.advance();
                self.pending_gt = true;
                Ok(())
            }
            TokenKind::GtGtEq => {
                // >>= case (rare in generics but handle it)
                // This would need more complex handling, for now error
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

    /// Parse the tokens into a list of declarations, collecting errors.
    pub fn parse(&mut self) -> ParseResult {
        let mut decls = Vec::new();
        self.skip_newlines();

        // Continue while there are tokens OR pending decls (from grouped imports)
        while !self.at_end() || !self.pending_decls.is_empty() {
            match self.parse_decl() {
                Ok(decl) => decls.push(decl),
                Err(e) => {
                    if !self.record_error(e) {
                        break; // Too many errors
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

    /// Parse a declaration.
    fn parse_decl(&mut self) -> Result<Decl, ParseError> {
        // Return pending decl if any (from expanded grouped imports)
        if let Some(decl) = self.pending_decls.pop() {
            return Ok(decl);
        }

        let start = self.current().span.start;

        // Check for attributes
        let mut attrs = Vec::new();
        while self.check(&TokenKind::At) {
            attrs.push(self.parse_attribute()?);
            self.skip_newlines();
        }

        // Check for visibility
        let is_pub = self.match_token(&TokenKind::Public);

        // Check for comptime/unsafe modifiers (before func)
        let is_comptime = self.match_token(&TokenKind::Comptime);
        let is_unsafe = if !is_comptime { self.match_token(&TokenKind::Unsafe) } else { false };

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
            _ => {
                return Err(ParseError::expected(
                    "declaration (func, struct, enum, trait, extend, import, export, const, test, benchmark)",
                    self.current_kind(),
                    self.current().span,
                ));
            }
        };

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);
        Ok(Decl { id: self.next_id(), kind, span: Span::new(start, end) })
    }

    /// Parse an attribute like `@resource` or `@deprecated("message")`.
    fn parse_attribute(&mut self) -> Result<String, ParseError> {
        self.expect(&TokenKind::At)?;
        let mut attr = self.expect_ident()?;

        // Handle attribute arguments like @deprecated("message")
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

    /// Parse a function declaration.
    fn parse_fn_decl(&mut self, is_pub: bool, is_comptime: bool, is_unsafe: bool, attrs: Vec<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Func)?;
        let mut name = self.expect_ident()?;

        // Handle generic parameters: func foo<T, U, comptime N: usize>()
        if self.match_token(&TokenKind::Lt) {
            name.push('<');
            loop {
                // Check for const generic: `const N: Type`
                if self.match_token(&TokenKind::Const) {
                    name.push_str("const ");
                    name.push_str(&self.expect_ident()?);
                    self.expect(&TokenKind::Colon)?;
                    name.push_str(": ");
                    name.push_str(&self.parse_type_name()?);
                // Check for comptime generic: `comptime N: Type`
                } else if self.match_token(&TokenKind::Comptime) {
                    name.push_str("comptime ");
                    name.push_str(&self.expect_ident()?);
                    self.expect(&TokenKind::Colon)?;
                    name.push_str(": ");
                    name.push_str(&self.parse_type_name()?);
                } else {
                    // Regular type parameter (may have bounds later)
                    name.push_str(&self.expect_ident()?);
                    // Handle type bounds: T: Trait
                    if self.match_token(&TokenKind::Colon) {
                        name.push_str(": ");
                        name.push_str(&self.parse_type_name()?);
                    }
                }
                if self.match_token(&TokenKind::Comma) {
                    name.push_str(", ");
                } else {
                    break;
                }
            }
            self.expect(&TokenKind::Gt)?;
            name.push('>');
        }

        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;

        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        // Body is optional for trait method signatures
        // Check for body without consuming newlines that might be terminators
        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else if self.check(&TokenKind::Newline) {
            // Skip newlines and check if body follows
            self.skip_newlines();
            if self.check(&TokenKind::LBrace) {
                self.parse_block_body()?
            } else {
                // No body - just a signature (newline already consumed)
                Vec::new()
            }
        } else if self.check(&TokenKind::Semi) {
            self.advance(); // consume semicolon
            self.skip_newlines();
            Vec::new()
        } else if self.check(&TokenKind::Eof) || self.check(&TokenKind::RBrace) {
            // End of file or block - no terminator needed
            Vec::new()
        } else {
            return Err(ParseError::expected(
                "'{' or newline",
                self.current_kind(),
                self.current().span,
            ));
        };

        Ok(DeclKind::Fn(FnDecl { name, params, ret_ty, body, is_pub, is_comptime, is_unsafe, attrs }))
    }

    /// Parse function parameters.
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut params = Vec::new();

        self.skip_newlines();
        if self.check(&TokenKind::RParen) {
            return Ok(params);
        }

        loop {
            let is_take = self.match_token(&TokenKind::Take);
            let name = self.expect_ident_or_keyword()?;

            // Type is optional for `self` parameter
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

            params.push(Param { name, ty, is_take, default });

            if !self.match_token(&TokenKind::Comma) {
                break;
            }
            self.skip_newlines();
        }

        Ok(params)
    }

    /// Parse a type name.
    fn parse_type_name(&mut self) -> Result<String, ParseError> {
        // Handle unit type () and tuple types (T1, T2, ...)
        if self.check(&TokenKind::LParen) {
            self.advance();
            if self.check(&TokenKind::RParen) {
                self.advance();
                return Ok("()".to_string());
            }
            // Tuple type
            let mut types = Vec::new();
            loop {
                types.push(self.parse_type_name()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            return Ok(format!("({})", types.join(", ")));
        }

        // Handle slice type []T or fixed-size array type [T; N]
        if self.check(&TokenKind::LBracket) {
            self.advance();

            // Check for slice type: []T
            if self.check(&TokenKind::RBracket) {
                self.advance(); // consume ]
                let elem_ty = self.parse_type_name()?;
                return Ok(format!("[]{}", elem_ty));
            }

            // Otherwise it's a fixed-size array: [T; N]
            let elem_ty = self.parse_type_name()?;
            self.expect(&TokenKind::Semi)?;
            // Parse the size (can be a literal or identifier like BUFFER_SIZE)
            let size = match self.current_kind().clone() {
                TokenKind::Int(n) => {
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

        // Handle const generic arguments (plain integers like 256)
        if let TokenKind::Int(n) = self.current_kind().clone() {
            self.advance();
            return Ok(n.to_string());
        }

        // Handle function types: func(Args) -> Ret
        if self.check(&TokenKind::Func) {
            self.advance(); // consume func
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

        // Handle 'any Trait' or 'any Trait<T>' opaque/trait object syntax
        if name == "any" {
            if let TokenKind::Ident(_) = self.current_kind() {
                let mut trait_name = self.expect_ident()?;
                // Handle generics like any Iterator<T>
                if self.match_token(&TokenKind::Lt) {
                    trait_name.push('<');
                    loop {
                        if let TokenKind::Int(n) = self.current_kind().clone() {
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

        // Handle qualified names like fs.IoError or std.io.File
        // But stop if we see .{ which is a projection
        while self.check(&TokenKind::Dot) && !matches!(self.peek(1), TokenKind::LBrace) {
            self.advance(); // consume dot
            name.push('.');
            name.push_str(&self.expect_ident()?);
        }

        // Handle generics like Option<T> or Result<T, E> or Array<T, 256>
        if self.match_token(&TokenKind::Lt) {
            name.push('<');
            loop {
                // Handle const generic arguments (integers)
                if let TokenKind::Int(n) = self.current_kind().clone() {
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
            // Handle >> case: Vec<Vec<i32>> - the >> is one token
            self.expect_gt_in_generic()?;
            name.push('>');
        }

        // Handle optional type T?
        if self.match_token(&TokenKind::Question) {
            name.push('?');
        }

        // Handle projections (partial borrows): Type.{field1, field2}
        if self.check(&TokenKind::Dot) && matches!(self.peek(1), TokenKind::LBrace) {
            self.advance(); // consume dot
            self.advance(); // consume {
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

    /// Parse a struct declaration.
    fn parse_struct_decl(&mut self, is_pub: bool, attrs: Vec<String>) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Struct)?;
        let mut name = self.expect_ident()?;

        // Handle generic parameters <T>, <T, U>, <const N: usize>, <comptime N: usize>, etc.
        if self.match_token(&TokenKind::Lt) {
            name.push('<');
            loop {
                // Check for const generic: `const N: Type`
                if self.match_token(&TokenKind::Const) {
                    name.push_str("const ");
                    name.push_str(&self.expect_ident()?);
                    self.expect(&TokenKind::Colon)?;
                    name.push_str(": ");
                    name.push_str(&self.parse_type_name()?);
                // Check for comptime generic: `comptime N: Type`
                } else if self.match_token(&TokenKind::Comptime) {
                    name.push_str("comptime ");
                    name.push_str(&self.expect_ident()?);
                    self.expect(&TokenKind::Colon)?;
                    name.push_str(": ");
                    name.push_str(&self.parse_type_name()?);
                } else {
                    name.push_str(&self.expect_ident()?);
                }
                if self.match_token(&TokenKind::Comma) {
                    name.push_str(", ");
                } else {
                    break;
                }
            }
            self.expect(&TokenKind::Gt)?;
            name.push('>');
        }

        self.skip_newlines();
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut methods = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            // Handle ellipsis placeholder `...` (for documentation)
            if self.check(&TokenKind::DotDot) {
                self.advance(); // consume ..
                if self.check(&TokenKind::Dot) {
                    self.advance(); // consume the third dot
                }
                self.skip_newlines();
                continue;
            }

            let field_pub = self.match_token(&TokenKind::Public);

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

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Struct(StructDecl { name, fields, methods, is_pub, attrs }))
    }

    /// Parse an enum declaration.
    fn parse_enum_decl(&mut self, is_pub: bool) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Enum)?;
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
                        // Check if this is `name: Type` or just `Type`
                        let (field_name, ty) = if self.check(&TokenKind::Ident(String::new())) {
                            // Peek ahead: if ident followed by `:`, it's named
                            if self.peek(1) == &TokenKind::Colon {
                                let name = self.expect_ident()?;
                                self.advance(); // consume colon
                                let ty = self.parse_type_name()?;
                                (name, ty)
                            } else {
                                // Just a type (possibly qualified)
                                let ty = self.parse_type_name()?;
                                (format!("_{}", idx), ty)
                            }
                        } else {
                            // Type starting with non-ident (like `()`)
                            let ty = self.parse_type_name()?;
                            (format!("_{}", idx), ty)
                        };

                        fields.push(Field { name: field_name, ty, is_pub: false });
                        idx += 1;

                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen)?;
                }

                variants.push(Variant { name: variant_name, fields });
            }

            // Skip optional comma and newlines between variants
            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Enum(EnumDecl { name, variants, methods, is_pub }))
    }

    /// Parse a trait declaration.
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
                // Shorthand method: name(params) -> RetType (without func keyword)
                let fn_decl = self.parse_trait_method_shorthand()?;
                methods.push(fn_decl);
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::RBrace)?;
        Ok(DeclKind::Trait(TraitDecl { name, methods, is_pub }))
    }

    /// Parse trait method shorthand: `name(params) -> RetType` or `name<T>(params) -> T` without `func` keyword.
    fn parse_trait_method_shorthand(&mut self) -> Result<FnDecl, ParseError> {
        let mut name = self.expect_ident()?;

        // Handle generic parameters: name<T, U>()
        if self.match_token(&TokenKind::Lt) {
            name.push('<');
            loop {
                // Check for const generic: `const N: Type`
                if self.match_token(&TokenKind::Const) {
                    name.push_str("const ");
                    name.push_str(&self.expect_ident()?);
                    self.expect(&TokenKind::Colon)?;
                    name.push_str(": ");
                    name.push_str(&self.parse_type_name()?);
                // Check for comptime generic: `comptime N: Type`
                } else if self.match_token(&TokenKind::Comptime) {
                    name.push_str("comptime ");
                    name.push_str(&self.expect_ident()?);
                    self.expect(&TokenKind::Colon)?;
                    name.push_str(": ");
                    name.push_str(&self.parse_type_name()?);
                } else {
                    // Regular type parameter
                    name.push_str(&self.expect_ident()?);
                    // Handle type bounds: T: Trait
                    if self.match_token(&TokenKind::Colon) {
                        name.push_str(": ");
                        name.push_str(&self.parse_type_name()?);
                    }
                }
                if self.match_token(&TokenKind::Comma) {
                    name.push_str(", ");
                } else {
                    break;
                }
            }
            self.expect(&TokenKind::Gt)?;
            name.push('>');
        }

        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.skip_newlines();
        self.expect(&TokenKind::RParen)?;

        let ret_ty = if self.match_token(&TokenKind::Arrow) {
            Some(self.parse_type_name()?)
        } else {
            None
        };

        // Body is optional for trait method signatures
        // Only skip newlines if we see a brace (body follows)
        if self.check(&TokenKind::Newline) {
            self.skip_newlines();
        }
        let body = if self.check(&TokenKind::LBrace) {
            self.parse_block_body()?
        } else {
            // No body - this is a signature-only declaration
            Vec::new()
        };

        Ok(FnDecl {
            name,
            params,
            ret_ty,
            body,
            is_pub: false,
            is_comptime: false,
            is_unsafe: false,
            attrs: vec![],
        })
    }

    /// Parse an extend (impl) block.
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
            // Skip attributes on methods for now
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

    /// Parse an import declaration.
    ///
    /// Syntax:
    /// - `import pkg` - qualified access
    /// - `import pkg as p` - aliased
    /// - `import pkg.Name` - unqualified access to Name
    /// - `import pkg.Name as N` - renamed
    /// - `import lazy pkg` - lazy initialization
    /// - `import pkg.*` - glob import (with warning)
    /// - `import pkg.{A, B}` - grouped imports (expands to multiple ImportDecl)
    fn parse_import_decl(&mut self) -> Result<DeclKind, ParseError> {
        self.expect(&TokenKind::Import)?;

        // Check for lazy import
        let is_lazy = self.match_token(&TokenKind::Lazy);

        let mut path = Vec::new();
        let mut is_glob = false;

        path.push(self.expect_ident()?);

        // Parse dotted path: pkg.sub.Name, pkg.*, or pkg.{A, B}
        while self.match_token(&TokenKind::Dot) {
            if self.match_token(&TokenKind::Star) {
                is_glob = true;
                break;
            }
            // Check for grouped import syntax: import pkg.{A, B}
            if self.check(&TokenKind::LBrace) {
                return self.parse_grouped_imports(path, is_lazy);
            }
            path.push(self.expect_ident()?);
        }

        // Parse optional alias
        let alias = if self.match_token(&TokenKind::As) {
            Some(self.expect_ident()?)
        } else {
            None
        };

        self.expect_terminator()?;
        Ok(DeclKind::Import(ImportDecl { path, alias, is_glob, is_lazy }))
    }

    /// Parse grouped imports: `import pkg.{A, B as C, D}`
    ///
    /// Called after consuming `import pkg.` when `{` is detected.
    /// Returns the first import and pushes the rest to pending_decls.
    fn parse_grouped_imports(&mut self, base_path: Vec<String>, is_lazy: bool) -> Result<DeclKind, ParseError> {
        let start = self.tokens.get(self.pos.saturating_sub(base_path.len() + 2))
            .map(|t| t.span.start)
            .unwrap_or(0);

        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut items: Vec<(String, Option<String>)> = Vec::new();

        // Parse comma-separated list of identifiers with optional aliases
        loop {
            // Check for empty braces
            if self.check(&TokenKind::RBrace) {
                if items.is_empty() {
                    return Err(ParseError::expected("identifier", self.current_kind(), self.current().span));
                }
                break; // Trailing comma case
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
            self.skip_newlines(); // Allow newlines after comma
        }

        self.skip_newlines();
        self.expect(&TokenKind::RBrace)?;
        self.expect_terminator()?;

        let end = self.tokens.get(self.pos.saturating_sub(1)).map(|t| t.span.end).unwrap_or(start);

        // Push all items except first to pending (reversed so first pending is second item)
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

        // Return first import
        let (name, alias) = items.into_iter().next().unwrap();
        let mut path = base_path;
        path.push(name);
        Ok(DeclKind::Import(ImportDecl { path, alias, is_glob: false, is_lazy }))
    }

    /// Parse an export declaration (re-exports).
    ///
    /// Syntax:
    /// - `export internal.Name` - re-export as mylib.Name
    /// - `export internal.Name as Alias` - re-export with rename
    /// - `export internal.Name, other.Thing` - multiple re-exports
    fn parse_export_decl(&mut self) -> Result<DeclKind, ParseError> {
        use rask_ast::decl::{ExportDecl, ExportItem};

        self.expect(&TokenKind::Export)?;

        let mut items = Vec::new();

        loop {
            // Parse dotted path: internal.parser.Parser
            let mut path = Vec::new();
            path.push(self.expect_ident()?);
            while self.match_token(&TokenKind::Dot) {
                path.push(self.expect_ident()?);
            }

            // Parse optional alias
            let alias = if self.match_token(&TokenKind::As) {
                Some(self.expect_ident()?)
            } else {
                None
            };

            items.push(ExportItem { path, alias });

            // Check for comma (multiple exports)
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

    /// Check for and consume a compound assignment operator.
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

        // Check for tuple destructuring: let (a, b) = ...
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

        // Check for tuple destructuring: const (a, b) = ...
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
            TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::String(_) | TokenKind::Bool(_)
                | TokenKind::Ident(_) | TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket
                | TokenKind::If | TokenKind::Match | TokenKind::With | TokenKind::Spawn
                | TokenKind::Minus | TokenKind::Bang | TokenKind::Pipe
        )
    }

    fn parse_while_stmt(&mut self, label: Option<String>) -> Result<StmtKind, ParseError> {
        self.expect(&TokenKind::While)?;

        // Parse condition expression (no braces - they start the body)
        let cond = self.parse_expr_no_braces()?;

        // Check for `is` pattern matching: `while expr is Pattern { }`
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
        // While doesn't use label in this AST, but we parsed it
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
        let iter = self.parse_expr_no_braces()?;  // No braces - they start the body
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
            vec![self.parse_stmt()?]
        };
        Ok(StmtKind::Ensure(body))
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

    /// Parse an expression.
    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_expr_bp(0)
    }

    /// Parse an expression without allowing brace-started constructs (struct literals).
    /// Used in control flow conditions where `{` should start the body, not a struct literal.
    fn parse_expr_no_braces(&mut self) -> Result<Expr, ParseError> {
        let old = self.allow_brace_expr;
        self.allow_brace_expr = false;
        let result = self.parse_expr_bp(0);
        self.allow_brace_expr = old;
        result
    }

    /// Parse an expression with the given minimum binding power.
    fn parse_expr_bp(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        let mut lhs = self.parse_prefix()?;

        loop {
            // Handle method chaining across newlines: if we see newline followed by . or ?,
            // continue parsing as a postfix operation
            if self.check(&TokenKind::Newline) && self.peek_past_newlines_is_postfix() {
                self.skip_newlines();
            }

            // Postfix operators
            if let Some(bp) = self.postfix_bp() {
                if bp < min_bp { break; }
                lhs = self.parse_postfix(lhs)?;
                continue;
            }

            // `as` cast (special infix)
            if self.check(&TokenKind::As) {
                let bp = 21; // High precedence
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

            // Infix operators
            if let Some((l_bp, r_bp)) = self.infix_bp() {
                if l_bp < min_bp { break; }

                // Special handling for ??
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

                // Special handling for range operators
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
                self.skip_newlines(); // Allow continuation after binary operator
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

    /// Parse a prefix expression (atoms and unary operators).
    fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;

        match self.current_kind().clone() {
            // Literals
            TokenKind::Int(n) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Int(n), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
            }
            TokenKind::Float(n) => {
                self.advance();
                Ok(Expr { id: self.next_id(), kind: ExprKind::Float(n), span: Span::new(start, self.tokens[self.pos - 1].span.end) })
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

            // Identifier (may be followed by struct literal or generic type)
            TokenKind::Ident(name) => {
                self.advance();
                let mut full_name = name.clone();

                // Handle generic type with static method: `Channel<LogEntry>.buffered()`
                if Self::is_type_name(&name) && self.check(&TokenKind::Lt) && self.looks_like_generic_type_with_static_method() {
                    self.advance(); // consume <
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

                let end = self.tokens[self.pos - 1].span.end;

                // Check for struct literal: `Point { x: 1, y: 2 }`
                if Self::is_type_name(&full_name) && self.allow_brace_expr && self.check(&TokenKind::LBrace) {
                    self.parse_struct_literal(full_name, start)
                } else {
                    Ok(Expr { id: self.next_id(), kind: ExprKind::Ident(full_name), span: Span::new(start, end) })
                }
            }

            // Unary operators
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
                self.advance();
                let operand = self.parse_expr_bp(Self::PREFIX_BP)?;
                let end = operand.span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unary { op: UnaryOp::Ref, operand: Box::new(operand) }, span: Span::new(start, end) })
            }

            // Ownership transfer: `own x`
            TokenKind::Own => {
                self.advance();
                // For now, just parse the inner expression - ownership is semantic
                self.parse_expr_bp(Self::PREFIX_BP)
            }

            // Parenthesized expression or tuple
            TokenKind::LParen => self.parse_paren_or_tuple(),

            // Array literal
            TokenKind::LBracket => self.parse_array_literal(),

            // Block expression
            TokenKind::LBrace => {
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) })
            }

            // Zero-param closure: || expr
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

            // Closure: |x, y| expr
            TokenKind::Pipe => self.parse_closure(),

            // If expression
            TokenKind::If => self.parse_if_expr(),

            // Match expression
            TokenKind::Match => self.parse_match_expr(),

            // With block: `with name { body }`
            TokenKind::With => self.parse_with_block(),

            // Spawn expression: `spawn { body }`
            TokenKind::Spawn => self.parse_spawn_expr(),

            // Spawn thread: `spawn_thread { body }`
            TokenKind::SpawnThread => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::BlockCall { name: "spawn_thread".to_string(), body }, span: Span::new(start, end) })
            }

            // Spawn raw: `spawn_raw { body }`
            TokenKind::SpawnRaw => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::BlockCall { name: "spawn_raw".to_string(), body }, span: Span::new(start, end) })
            }

            // Unsafe block: `unsafe { body }`
            TokenKind::Unsafe => {
                self.advance();
                self.skip_newlines();
                let body = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Unsafe { body }, span: Span::new(start, end) })
            }

            // Comptime expression: `comptime { body }` or `comptime expr`
            TokenKind::Comptime => {
                self.advance();
                self.skip_newlines();
                let body = if self.check(&TokenKind::LBrace) {
                    self.parse_block_body()?
                } else {
                    // Single expression form: wrap as expression statement
                    let expr = self.parse_expr()?;
                    vec![Stmt { id: self.next_id(), kind: StmtKind::Expr(expr.clone()), span: expr.span }]
                };
                let end = body.last().map(|s| s.span.end).unwrap_or(start);
                Ok(Expr { id: self.next_id(), kind: ExprKind::Comptime { body }, span: Span::new(start, end) })
            }

            // Assert expression: `assert condition` or `assert condition, "message"`
            TokenKind::Assert => self.parse_assert_expr(),

            // Check expression: `check condition` or `check condition, "message"`
            TokenKind::Check => self.parse_check_expr(),

            _ => Err(ParseError::expected(
                "expression",
                self.current_kind(),
                self.current().span,
            )),
        }
    }

    /// Parse struct literal: `Point { x: 1, y: 2, ..other }`
    fn parse_struct_literal(&mut self, name: String, start: usize) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();

        let mut fields = Vec::new();
        let mut spread = None;

        while !self.check(&TokenKind::RBrace) && !self.at_end() {
            // Check for spread: `..other`
            if self.match_token(&TokenKind::DotDot) {
                spread = Some(Box::new(self.parse_expr()?));
                self.skip_newlines();
                break;
            }

            let field_name = self.expect_ident_or_keyword()?;

            // Shorthand: `{ x }` is same as `{ x: x }`
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

    /// Parse assert expression: `assert condition` or `assert condition, "message"`
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

    /// Parse check expression: `check condition` or `check condition, "message"`
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

    /// Parse parenthesized expression or tuple.
    fn parse_paren_or_tuple(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::LParen)?;

        if self.check(&TokenKind::RParen) {
            // Empty tuple: ()
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr { id: self.next_id(), kind: ExprKind::Tuple(Vec::new()), span: Span::new(start, end) });
        }

        let first = self.parse_expr()?;

        if self.match_token(&TokenKind::Comma) {
            // It's a tuple
            let mut elements = vec![first];
            while !self.check(&TokenKind::RParen) && !self.at_end() {
                elements.push(self.parse_expr()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
            self.expect(&TokenKind::RParen)?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Expr { id: self.next_id(), kind: ExprKind::Tuple(elements), span: Span::new(start, end) })
        } else {
            // Just parenthesized expression
            self.expect(&TokenKind::RParen)?;
            Ok(first)
        }
    }

    /// Parse array literal: `[1, 2, 3]` or array repeat: `[0; 64]`
    fn parse_array_literal(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::LBracket)?;

        // Empty array
        if self.check(&TokenKind::RBracket) {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr { id: self.next_id(), kind: ExprKind::Array(Vec::new()), span: Span::new(start, end) });
        }

        // Parse first element
        let first = self.parse_expr()?;

        // Check for array repeat syntax: [value; count]
        if self.match_token(&TokenKind::Semi) {
            let count = self.parse_expr()?;
            self.expect(&TokenKind::RBracket)?;
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Expr {
                id: self.next_id(),
                kind: ExprKind::ArrayRepeat { value: Box::new(first), count: Box::new(count) },
                span: Span::new(start, end),
            });
        }

        // Regular array literal: [elem, elem, ...]
        let mut elements = vec![first];
        if self.match_token(&TokenKind::Comma) {
            while !self.check(&TokenKind::RBracket) && !self.at_end() {
                elements.push(self.parse_expr()?);
                if !self.match_token(&TokenKind::Comma) { break; }
            }
        }

        self.expect(&TokenKind::RBracket)?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Expr { id: self.next_id(), kind: ExprKind::Array(elements), span: Span::new(start, end) })
    }

    /// Parse closure: `|x, y| expr` or `|x, y| { stmts }`
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

        let body = self.parse_expr()?;
        let end = body.span.end;

        Ok(Expr {
            id: self.next_id(),
            kind: ExprKind::Closure { params, body: Box::new(body) },
            span: Span::new(start, end),
        })
    }

    /// Parse postfix operators.
    fn parse_postfix(&mut self, lhs: Expr) -> Result<Expr, ParseError> {
        let start = lhs.span.start;

        match self.current_kind() {
            // Function call
            TokenKind::LParen => {
                self.advance();
                let args = self.parse_args()?;
                self.expect(&TokenKind::RParen)?;
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Call { func: Box::new(lhs), args }, span: Span::new(start, end) })
            }

            // Field access (may become method call)
            TokenKind::Dot => {
                self.advance();
                let field = self.expect_ident_or_keyword()?;

                // Check for generic type arguments: .method<T>()
                // Only parse < as generics if we can confirm it's followed by types, >, and (
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

                // Check if it's a method call
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
            TokenKind::Question => {
                self.advance();
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Expr { id: self.next_id(), kind: ExprKind::Try(Box::new(lhs)), span: Span::new(start, end) })
            }

            _ => Ok(lhs),
        }
    }

    /// Parse function call arguments (supports named arguments like `name: value`).
    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.check(&TokenKind::RParen) { return Ok(args); }

        loop {
            // Check for named argument: `name: value`
            if let TokenKind::Ident(_) = self.current_kind().clone() {
                if self.peek(1) == &TokenKind::Colon {
                    // Named argument - skip name and colon, just parse value
                    self.advance(); // skip name
                    self.advance(); // skip colon
                }
            }
            args.push(self.parse_expr()?);
            if !self.match_token(&TokenKind::Comma) { break; }
        }

        Ok(args)
    }

    /// Parse an if expression.
    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::If)?;

        let cond = self.parse_expr_no_braces()?;  // No braces - they start the body

        // Check for `is` pattern matching: `if expr is Pattern { }`
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

            // Check for else branch (may be on next line after the then-branch)
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
            // Inline block: parse a single statement and wrap it
            self.parse_inline_block(start)?
        } else {
            self.skip_newlines();
            let stmts = self.parse_block_body()?;
            let end = self.tokens[self.pos - 1].span.end;
            Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }
        };

        // Check for else branch (may be on next line after the then-branch)
        // Use peek to check if else follows without consuming the newline
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

    /// Parse an inline block (after colon).
    /// Parses statement content WITHOUT consuming terminator - caller handles that.
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
                // deliver [label] value - if ident followed by expr, it's label + value
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
                // Check for assignment
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

    /// Parse a match expression.
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

            let body = if self.check(&TokenKind::LBrace) {
                let stmts = self.parse_block_body()?;
                let end = self.tokens[self.pos - 1].span.end;
                Expr { id: self.next_id(), kind: ExprKind::Block(stmts), span: Span::new(start, end) }
            } else {
                // Match arm body can be expression or statement (like assignment)
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

    /// Parse a with block expression: `with name { body }`
    fn parse_with_block(&mut self) -> Result<Expr, ParseError> {
        let start = self.current().span.start;
        self.expect(&TokenKind::With)?;
        let name = self.expect_ident()?;

        // Optional arguments: with threading(4) { }
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

    /// Parse a spawn expression: `spawn { body }`
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

    /// Parse a pattern (may be an or-pattern with `|`).
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let first = self.parse_single_pattern()?;

        // Check for or-pattern: pattern | pattern | ...
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

    /// Parse a single pattern (without or).
    fn parse_single_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.current_kind().clone() {
            // Tuple pattern: (a, b, c)
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
                if self.match_token(&TokenKind::LParen) {
                    let mut fields = Vec::new();
                    while !self.check(&TokenKind::RParen) && !self.at_end() {
                        fields.push(self.parse_pattern()?);
                        if !self.match_token(&TokenKind::Comma) { break; }
                    }
                    self.expect(&TokenKind::RParen)?;
                    Ok(Pattern::Constructor { name, fields })
                } else {
                    Ok(Pattern::Ident(name))
                }
            }
            TokenKind::Int(n) => {
                self.advance();
                let span = self.tokens[self.pos - 1].span.clone();
                Ok(Pattern::Literal(Box::new(Expr { id: self.next_id(), kind: ExprKind::Int(n), span })))
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

    #[allow(dead_code)]
    fn unexpected(span: Span) -> Self {
        Self {
            span,
            message: "Unexpected syntax".to_string(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
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

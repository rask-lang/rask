// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! C header lexer — tokenizes preprocessed C source.

use crate::parser::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub struct CToken {
    pub kind: CTokenKind,
    pub line: usize,
    /// True if whitespace preceded this token (used for #define disambiguation).
    pub space_before: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CTokenKind {
    // Literals
    Ident(String),
    IntLit(i64),
    UIntLit(u64),
    FloatLit(f64),
    StringLit(String),
    CharLit(char),

    // Keywords
    Void,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    Signed,
    Unsigned,
    Struct,
    Union,
    Enum,
    Typedef,
    Const,
    Volatile,
    Restrict,
    Static,
    Extern,
    Inline,
    Bool,       // _Bool / bool
    Sizeof,
    Alignof,    // _Alignof

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semi,
    Comma,
    Star,
    Ampersand,
    Eq,
    Plus,
    Minus,
    Slash,
    Percent,
    Dot,
    Arrow,      // ->
    Ellipsis,   // ...
    Hash,       // #
    DoubleHash, // ##
    Pipe,       // |
    Caret,      // ^
    Tilde,      // ~
    Bang,       // !
    Lt,
    Gt,
    LtEq,      // <=
    GtEq,      // >=
    EqEq,      // ==
    BangEq,    // !=
    LShift,    // <<
    RShift,    // >>
    Question,
    Colon,

    // Preprocessor (kept as tokens for #define parsing)
    PPDefine,
    PPInclude,
    PPIfdef,
    PPIfndef,
    PPIf,
    PPElse,
    PPElif,
    PPEndif,
    PPUndef,
    PPPragma,
    PPError,
    PPLine,

    Newline,
    Eof,
}

pub struct CLexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> CLexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
            line: 1,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<CToken>, ParseError> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok.kind == CTokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn peek(&self) -> u8 {
        if self.pos < self.source.len() {
            self.source[self.pos]
        } else {
            0
        }
    }

    fn peek_at(&self, offset: usize) -> u8 {
        let idx = self.pos + offset;
        if idx < self.source.len() {
            self.source[idx]
        } else {
            0
        }
    }

    fn advance(&mut self) -> u8 {
        let ch = self.peek();
        if ch == b'\n' {
            self.line += 1;
        }
        self.pos += 1;
        ch
    }

    fn skip_whitespace_no_newline(&mut self) {
        while self.pos < self.source.len() {
            match self.peek() {
                b' ' | b'\t' | b'\r' => { self.advance(); }
                _ => break,
            }
        }
    }

    fn next_token(&mut self) -> Result<CToken, ParseError> {
        // Skip whitespace and track whether there was any (for #define disambiguation).
        let pos_before_ws = self.pos;
        self.skip_whitespace_no_newline();
        let had_space = self.pos > pos_before_ws;

        let mut tok = self.next_token_raw()?;
        tok.space_before = had_space;
        Ok(tok)
    }

    fn next_token_raw(&mut self) -> Result<CToken, ParseError> {
        if self.pos >= self.source.len() {
            return Ok(CToken { kind: CTokenKind::Eof, line: self.line, space_before: false });
        }

        let line = self.line;
        let ch = self.peek();

        // Newline
        if ch == b'\n' {
            self.advance();
            return Ok(CToken { kind: CTokenKind::Newline, line, space_before: false });
        }

        // Line continuation
        if ch == b'\\' && self.peek_at(1) == b'\n' {
            self.advance(); // backslash
            self.advance(); // newline
            return self.next_token();
        }

        // Preprocessor directive
        if ch == b'#' {
            self.advance();
            // ## ?
            if self.peek() == b'#' {
                self.advance();
                return Ok(CToken { kind: CTokenKind::DoubleHash, line, space_before: false });
            }
            self.skip_whitespace_no_newline();
            let directive = self.read_ident();
            let kind = match directive.as_str() {
                "define" => CTokenKind::PPDefine,
                "include" => CTokenKind::PPInclude,
                "ifdef" => CTokenKind::PPIfdef,
                "ifndef" => CTokenKind::PPIfndef,
                "if" => CTokenKind::PPIf,
                "else" => CTokenKind::PPElse,
                "elif" => CTokenKind::PPElif,
                "endif" => CTokenKind::PPEndif,
                "undef" => CTokenKind::PPUndef,
                "pragma" => CTokenKind::PPPragma,
                "error" => CTokenKind::PPError,
                "line" => CTokenKind::PPLine,
                _ => CTokenKind::Hash, // unknown directive, treat as hash
            };
            return Ok(CToken { kind, line, space_before: false });
        }

        // Numbers
        if ch.is_ascii_digit() || (ch == b'.' && self.peek_at(1).is_ascii_digit()) {
            return self.read_number(line);
        }

        // String literal
        if ch == b'"' {
            return self.read_string_lit(line);
        }

        // Char literal
        if ch == b'\'' {
            return self.read_char_lit(line);
        }

        // Identifiers and keywords
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let ident = self.read_ident();
            let kind = match ident.as_str() {
                "void" => CTokenKind::Void,
                "char" => CTokenKind::Char,
                "short" => CTokenKind::Short,
                "int" => CTokenKind::Int,
                "long" => CTokenKind::Long,
                "float" => CTokenKind::Float,
                "double" => CTokenKind::Double,
                "signed" => CTokenKind::Signed,
                "unsigned" => CTokenKind::Unsigned,
                "struct" => CTokenKind::Struct,
                "union" => CTokenKind::Union,
                "enum" => CTokenKind::Enum,
                "typedef" => CTokenKind::Typedef,
                "const" => CTokenKind::Const,
                "volatile" => CTokenKind::Volatile,
                "restrict" | "__restrict" | "__restrict__" => CTokenKind::Restrict,
                "static" => CTokenKind::Static,
                "extern" => CTokenKind::Extern,
                "inline" | "__inline" | "__inline__" | "__forceinline" => CTokenKind::Inline,
                "_Bool" | "bool" => CTokenKind::Bool,
                "sizeof" => CTokenKind::Sizeof,
                "_Alignof" | "alignof" => CTokenKind::Alignof,
                // GCC/Clang attributes and glibc macros — skip entirely
                "__attribute__" | "__attribute" => {
                    self.skip_attribute();
                    return self.next_token();
                }
                "__declspec" => {
                    self.skip_attribute();
                    return self.next_token();
                }
                "__extension__" => return self.next_token(),
                "__asm__" | "__asm" | "asm" => {
                    self.skip_attribute();
                    return self.next_token();
                }
                // glibc function attributes — skip ident + optional parens
                "__THROW" | "__THROWNL" | "__nonnull" | "__wur"
                | "__attribute_pure__" | "__attribute_const__"
                | "__attribute_malloc__" | "__attribute_format_strfmon__"
                | "__attribute_warn_unused_result__"
                | "__attr_access" | "__attr_access_none" | "__attr_dealloc"
                | "__attr_dealloc_free" | "__fortified_attr_access"
                | "__nonnull_attribute__" | "__returns_nonnull"
                | "__glibc_fortify" | "__glibc_fortify_n"
                | "__REDIRECT" | "__REDIRECT_NTH" | "__REDIRECT_NTHNL"
                | "__COLD" | "__warnattr" | "__errordecl" => {
                    self.skip_optional_parens();
                    return self.next_token();
                }
                // glibc scope markers — skip
                "__BEGIN_DECLS" | "__END_DECLS"
                | "__BEGIN_NAMESPACE_STD" | "__END_NAMESPACE_STD"
                | "__USING_NAMESPACE_STD"
                | "__BEGIN_NAMESPACE_C99" | "__END_NAMESPACE_C99"
                | "__USING_NAMESPACE_C99" => {
                    return self.next_token();
                }
                _ => CTokenKind::Ident(ident),
            };
            return Ok(CToken { kind, line, space_before: false });
        }

        // Punctuation
        self.advance();
        let kind = match ch {
            b'(' => CTokenKind::LParen,
            b')' => CTokenKind::RParen,
            b'{' => CTokenKind::LBrace,
            b'}' => CTokenKind::RBrace,
            b'[' => CTokenKind::LBracket,
            b']' => CTokenKind::RBracket,
            b';' => CTokenKind::Semi,
            b',' => CTokenKind::Comma,
            b'*' => CTokenKind::Star,
            b'&' => CTokenKind::Ampersand,
            b'+' => CTokenKind::Plus,
            b'%' => CTokenKind::Percent,
            b'^' => CTokenKind::Caret,
            b'~' => CTokenKind::Tilde,
            b'?' => CTokenKind::Question,
            b':' => CTokenKind::Colon,
            b'|' => CTokenKind::Pipe,
            b'/' => CTokenKind::Slash,
            b'.' => {
                if self.peek() == b'.' && self.peek_at(1) == b'.' {
                    self.advance();
                    self.advance();
                    CTokenKind::Ellipsis
                } else {
                    CTokenKind::Dot
                }
            }
            b'-' => {
                if self.peek() == b'>' {
                    self.advance();
                    CTokenKind::Arrow
                } else {
                    CTokenKind::Minus
                }
            }
            b'=' => {
                if self.peek() == b'=' {
                    self.advance();
                    CTokenKind::EqEq
                } else {
                    CTokenKind::Eq
                }
            }
            b'!' => {
                if self.peek() == b'=' {
                    self.advance();
                    CTokenKind::BangEq
                } else {
                    CTokenKind::Bang
                }
            }
            b'<' => {
                if self.peek() == b'=' {
                    self.advance();
                    CTokenKind::LtEq
                } else if self.peek() == b'<' {
                    self.advance();
                    CTokenKind::LShift
                } else {
                    CTokenKind::Lt
                }
            }
            b'>' => {
                if self.peek() == b'=' {
                    self.advance();
                    CTokenKind::GtEq
                } else if self.peek() == b'>' {
                    self.advance();
                    CTokenKind::RShift
                } else {
                    CTokenKind::Gt
                }
            }
            _ => {
                return Err(ParseError::new(
                    format!("unexpected character: {:?}", ch as char),
                    line,
                ));
            }
        };
        Ok(CToken { kind, line, space_before: false })
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.source.len()
            && (self.source[self.pos].is_ascii_alphanumeric() || self.source[self.pos] == b'_')
        {
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.source[start..self.pos]).into_owned()
    }

    fn read_number(&mut self, line: usize) -> Result<CToken, ParseError> {
        let start = self.pos;

        // Hex
        if self.peek() == b'0' && (self.peek_at(1) == b'x' || self.peek_at(1) == b'X') {
            self.advance();
            self.advance();
            let hex_start = self.pos;
            while self.pos < self.source.len() && self.source[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            let hex = String::from_utf8_lossy(&self.source[hex_start..self.pos]);
            let val = u64::from_str_radix(&hex, 16)
                .map_err(|e| ParseError::new(format!("bad hex literal: {}", e), line))?;
            self.skip_int_suffix();
            return Ok(CToken { kind: CTokenKind::UIntLit(val), line, space_before: false });
        }

        // Octal check
        let is_octal = self.peek() == b'0' && self.peek_at(1).is_ascii_digit();

        // Read digits
        while self.pos < self.source.len() && self.source[self.pos].is_ascii_digit() {
            self.pos += 1;
        }

        // Float?
        let is_float = self.peek() == b'.' || self.peek() == b'e' || self.peek() == b'E';
        if is_float {
            if self.peek() == b'.' {
                self.pos += 1;
                while self.pos < self.source.len() && self.source[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            }
            if self.peek() == b'e' || self.peek() == b'E' {
                self.pos += 1;
                if self.peek() == b'+' || self.peek() == b'-' {
                    self.pos += 1;
                }
                while self.pos < self.source.len() && self.source[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            }
            // Skip float suffix (f, F, l, L)
            if matches!(self.peek(), b'f' | b'F' | b'l' | b'L') {
                self.pos += 1;
            }
            let text = String::from_utf8_lossy(&self.source[start..self.pos]);
            // Strip suffix for parsing
            let clean: String = text.chars().filter(|c| !matches!(c, 'f' | 'F' | 'l' | 'L')).collect();
            let val: f64 = clean.parse()
                .map_err(|e| ParseError::new(format!("bad float literal: {}", e), line))?;
            return Ok(CToken { kind: CTokenKind::FloatLit(val), line, space_before: false });
        }

        let text = String::from_utf8_lossy(&self.source[start..self.pos]);
        let val = if is_octal && text.len() > 1 {
            i64::from_str_radix(&text[1..], 8)
                .map_err(|e| ParseError::new(format!("bad octal literal: {}", e), line))?
        } else {
            text.parse::<i64>()
                .map_err(|e| ParseError::new(format!("bad integer literal: {}", e), line))?
        };

        let is_unsigned = self.skip_int_suffix();
        if is_unsigned {
            Ok(CToken { kind: CTokenKind::UIntLit(val as u64), line, space_before: false })
        } else {
            Ok(CToken { kind: CTokenKind::IntLit(val), line, space_before: false })
        }
    }

    /// Skip integer suffixes (u, U, l, L, ll, LL, etc). Returns true if unsigned.
    fn skip_int_suffix(&mut self) -> bool {
        let mut unsigned = false;
        loop {
            match self.peek() {
                b'u' | b'U' => { unsigned = true; self.pos += 1; }
                b'l' | b'L' => { self.pos += 1; }
                _ => break,
            }
        }
        unsigned
    }

    fn read_string_lit(&mut self, line: usize) -> Result<CToken, ParseError> {
        self.advance(); // opening quote
        let mut s = String::new();
        loop {
            if self.pos >= self.source.len() {
                return Err(ParseError::new("unterminated string literal".into(), line));
            }
            let ch = self.advance();
            match ch {
                b'"' => break,
                b'\\' => {
                    let esc = self.advance();
                    match esc {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'r' => s.push('\r'),
                        b'0' => s.push('\0'),
                        b'\\' => s.push('\\'),
                        b'"' => s.push('"'),
                        b'\'' => s.push('\''),
                        _ => {
                            s.push('\\');
                            s.push(esc as char);
                        }
                    }
                }
                _ => s.push(ch as char),
            }
        }
        Ok(CToken { kind: CTokenKind::StringLit(s), line, space_before: false })
    }

    fn read_char_lit(&mut self, line: usize) -> Result<CToken, ParseError> {
        self.advance(); // opening quote
        let ch = if self.peek() == b'\\' {
            self.advance();
            match self.advance() {
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'0' => '\0',
                b'\\' => '\\',
                b'\'' => '\'',
                other => other as char,
            }
        } else {
            self.advance() as char
        };
        if self.peek() == b'\'' {
            self.advance();
        }
        Ok(CToken { kind: CTokenKind::CharLit(ch), line, space_before: false })
    }

    /// Skip `__attribute__((...))` or `__declspec(...)` or `asm(...)`.
    fn skip_attribute(&mut self) {
        self.skip_whitespace_no_newline();
        if self.peek() != b'(' {
            return;
        }
        self.skip_balanced_parens();
    }

    /// Skip optional parenthesized arguments (for glibc macros like `__nonnull ((1, 2))`).
    fn skip_optional_parens(&mut self) {
        self.skip_whitespace_no_newline();
        if self.peek() == b'(' {
            self.skip_balanced_parens();
        }
    }

    /// Consume balanced parentheses starting at current `(`.
    fn skip_balanced_parens(&mut self) {
        let mut depth = 0u32;
        loop {
            if self.pos >= self.source.len() {
                break;
            }
            match self.peek() {
                b'(' => { depth += 1; self.advance(); }
                b')' => {
                    self.advance();
                    depth -= 1;
                    if depth == 0 { break; }
                }
                b'\n' => { self.advance(); }
                _ => { self.advance(); }
            }
        }
    }
}

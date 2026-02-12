// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The lexer implementation using logos.

use logos::Logos;
use rask_ast::token::{FloatSuffix, IntSuffix, Token, TokenKind};
use rask_ast::Span;

/// Raw token type for logos - we parse values in a second pass.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t]+")]  // Skip horizontal whitespace (not newlines)
enum RawToken {
    // === Keywords ===
    #[token("func")]
    Func,
    #[token("let")]
    Let,
    #[token("const")]
    Const,
    #[token("struct")]
    Struct,
    #[token("enum")]
    Enum,
    #[token("trait")]
    Trait,
    #[token("extend")]
    Extend,
    #[token("public")]
    Public,
    #[token("import")]
    Import,
    #[token("return")]
    Return,
    #[token("if")]
    If,
    #[token("else")]
    Else,
    #[token("match")]
    Match,
    #[token("for")]
    For,
    #[token("in")]
    In,
    #[token("while")]
    While,
    #[token("loop")]
    Loop,
    #[token("break")]
    Break,
    #[token("continue")]
    Continue,
    #[token("select_priority")]
    SelectPriority,
    #[token("select")]
    Select,
    #[token("with")]
    With,
    #[token("ensure")]
    Ensure,
    #[token("take")]
    Take,
    #[token("read")]
    ReadKw,
    #[token("mutate")]
    MutateKw,
    #[token("own")]
    Own,
    #[token("where")]
    Where,
    #[token("as")]
    As,
    #[token("is")]
    Is,
    #[token("true")]
    True,
    #[token("false")]
    False,
    // Additional keywords per spec
    #[token("unsafe")]
    Unsafe,
    #[token("extern")]
    Extern,
    #[token("comptime")]
    Comptime,
    #[token("type")]
    Type,
    #[token("none")]
    None,
    #[token("null")]
    Null,
    #[token("using")]
    Using,
    #[token("export")]
    Export,
    #[token("lazy")]
    Lazy,
    #[token("asm")]
    Asm,
    #[token("native")]
    Native,
    #[token("test")]
    Test,
    #[token("benchmark")]
    Benchmark,
    #[token("assert")]
    Assert,
    #[token("check")]
    Check,
    #[token("try")]
    Try,
    #[token("or")]
    Or,

    // === Operators (order matters - longer first) ===
    // Three-character operators
    #[token("..=")]
    DotDotEq,
    #[token("<<=")]
    LtLtEq,
    #[token(">>=")]
    GtGtEq,

    // Two-character operators
    #[token("==")]
    EqEq,
    #[token("!=")]
    BangEq,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("&&")]
    AmpAmp,
    #[token("||")]
    PipePipe,
    #[token("??")]
    QuestionQuestion,
    #[token("?.")]
    QuestionDot,
    #[token("..")]
    DotDot,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("::")]
    ColonColon,
    #[token("<<")]
    LtLt,
    #[token(">>")]
    GtGt,
    #[token("+=")]
    PlusEq,
    #[token("-=")]
    MinusEq,
    #[token("*=")]
    StarEq,
    #[token("/=")]
    SlashEq,
    #[token("%=")]
    PercentEq,
    #[token("&=")]
    AmpEq,
    #[token("|=")]
    PipeEq,
    #[token("^=")]
    CaretEq,

    // Single-character operators
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("=")]
    Eq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("!")]
    Bang,
    #[token("?")]
    Question,
    #[token("@")]
    At,
    #[token(".")]
    Dot,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("~")]
    Tilde,

    // === Delimiters ===
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(":")]
    Colon,
    #[token(";")]
    Semi,
    #[token(",")]
    Comma,

    // === Newline (significant in Rask) ===
    #[token("\n")]
    #[token("\r\n")]
    Newline,

    // === Comments (skip them) ===
    #[regex(r"//[^\n]*", logos::skip)]
    LineComment,

    // Block comments - handled specially for nesting
    #[token("/*", block_comment)]
    BlockComment,

    // === Literals ===
    // Hex integers: 0x[0-9a-fA-F_]+ with optional type suffix
    #[regex(r"0x[0-9a-fA-F_]+(i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize)?")]
    HexInt,

    // Binary integers: 0b[01_]+ with optional type suffix
    #[regex(r"0b[01_]+(i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize)?")]
    BinInt,

    // Octal integers: 0o[0-7_]+ with optional type suffix
    #[regex(r"0o[0-7_]+(i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize)?")]
    OctInt,

    // Float literals (must come before decimal int to match properly)
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9]+)?(f32|f64)?")]
    Float,

    // Decimal integers: [0-9][0-9_]* with optional type suffix
    #[regex(r"[0-9][0-9_]*(i8|i16|i32|i64|i128|isize|u8|u16|u32|u64|u128|usize)?")]
    DecInt,

    // Character literal (handles basic escapes and \u{XXXX} unicode escapes)
    #[regex(r"'([^'\\]|\\.|\\u\{[0-9a-fA-F]{1,6}\})'")]
    Char,

    // Multi-line string (triple quotes)
    #[regex(r#""""([^"\\]|\\.|"[^"]|""[^"])*""""#)]
    MultiLineString,

    // Raw string literal (r"..." — no escape processing)
    #[regex(r#"r"[^"]*""#)]
    RawString,

    // Raw string literal with hash delimiters (r#"..."# — handles embedded quotes)
    #[token("r#", raw_hash_string)]
    RawHashString,

    // Regular string (handles basic escapes and \u{XXXX} unicode escapes)
    #[regex(r#""([^"\\]|\\.|\\u\{[0-9a-fA-F]{1,6}\})*""#)]
    String,

    // === Identifier (must come after keywords) ===
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")]
    Ident,
}

/// Lex a raw hash string: r#"..."#, r##"..."##, etc.
/// The `r#` prefix is already consumed by logos. We count additional `#` chars,
/// expect `"`, then scan for the matching `"` + same number of `#`.
fn raw_hash_string(lexer: &mut logos::Lexer<RawToken>) -> bool {
    let bytes = lexer.remainder().as_bytes();
    let mut pos = 0;

    // Count additional # symbols (we already consumed r#, so start at 1)
    let mut hash_count = 1;
    while pos < bytes.len() && bytes[pos] == b'#' {
        hash_count += 1;
        pos += 1;
    }

    // Expect opening "
    if pos >= bytes.len() || bytes[pos] != b'"' {
        return false;
    }
    pos += 1;

    // Scan for closing " followed by hash_count # symbols
    while pos < bytes.len() {
        if bytes[pos] == b'"' {
            let mut count = 0;
            while pos + 1 + count < bytes.len()
                && bytes[pos + 1 + count] == b'#'
                && count < hash_count
            {
                count += 1;
            }
            if count == hash_count {
                pos += 1 + hash_count; // skip " and all #s
                lexer.bump(pos);
                return true;
            }
        }
        pos += 1;
    }

    false
}

/// Skip block comments, handling nesting.
fn block_comment(lexer: &mut logos::Lexer<RawToken>) -> logos::Skip {
    let mut depth = 1;
    let remainder = lexer.remainder();
    let mut chars = remainder.chars().peekable();
    let mut consumed = 0;

    while depth > 0 {
        match chars.next() {
            Some('/') if chars.peek() == Some(&'*') => {
                chars.next();
                consumed += 2;
                depth += 1;
            }
            Some('*') if chars.peek() == Some(&'/') => {
                chars.next();
                consumed += 2;
                depth -= 1;
            }
            Some(c) => {
                consumed += c.len_utf8();
            }
            None => break, // Unterminated - we'll handle error elsewhere
        }
    }

    lexer.bump(consumed);
    logos::Skip
}

/// Maximum number of errors to collect before stopping.
const MAX_ERRORS: usize = 20;

/// The lexer for Rask source code.
pub struct Lexer<'a> {
    source: &'a str,
    errors: Vec<LexError>,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer for the given source code.
    pub fn new(source: &'a str) -> Self {
        Self { source, errors: Vec::new() }
    }

    /// Tokenize the entire source, collecting multiple errors.
    pub fn tokenize(&mut self) -> LexResult {
        let mut tokens = Vec::new();
        let mut logos_lexer = RawToken::lexer(self.source);

        while let Some(result) = logos_lexer.next() {
            // Stop if we have too many errors
            if self.errors.len() >= MAX_ERRORS {
                break;
            }

            let span = logos_lexer.span();
            let slice = logos_lexer.slice();

            let kind = match result {
                Ok(raw) => {
                    match self.convert_token(raw, slice, span.start, span.end) {
                        Ok(kind) => kind,
                        Err(e) => {
                            self.errors.push(e);
                            continue; // Skip this token and continue
                        }
                    }
                }
                Err(()) => {
                    // Get the problematic character
                    let ch = self.source[span.start..].chars().next().unwrap_or('?');
                    self.errors.push(LexError::unexpected_char(ch, span.start));
                    continue; // Skip and continue
                }
            };

            tokens.push(Token {
                kind,
                span: Span::new(span.start, span.end),
            });
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(self.source.len(), self.source.len()),
        });

        LexResult {
            tokens,
            errors: std::mem::take(&mut self.errors),
        }
    }

    /// Convert a raw logos token to our TokenKind, parsing literals.
    fn convert_token(&self, raw: RawToken, slice: &str, start: usize, end: usize) -> Result<TokenKind, LexError> {
        Ok(match raw {
            // Keywords
            RawToken::Func => TokenKind::Func,
            RawToken::Let => TokenKind::Let,
            RawToken::Const => TokenKind::Const,
            RawToken::Struct => TokenKind::Struct,
            RawToken::Enum => TokenKind::Enum,
            RawToken::Trait => TokenKind::Trait,
            RawToken::Extend => TokenKind::Extend,
            RawToken::Public => TokenKind::Public,
            RawToken::Import => TokenKind::Import,
            RawToken::Return => TokenKind::Return,
            RawToken::If => TokenKind::If,
            RawToken::Else => TokenKind::Else,
            RawToken::Match => TokenKind::Match,
            RawToken::For => TokenKind::For,
            RawToken::In => TokenKind::In,
            RawToken::While => TokenKind::While,
            RawToken::Loop => TokenKind::Loop,
            RawToken::Break => TokenKind::Break,
            RawToken::Continue => TokenKind::Continue,
            RawToken::SelectPriority => TokenKind::SelectPriority,
            RawToken::Select => TokenKind::Select,
            RawToken::With => TokenKind::With,
            RawToken::Ensure => TokenKind::Ensure,
            RawToken::Take => TokenKind::Take,
            RawToken::ReadKw => TokenKind::ReadKw,
            RawToken::MutateKw => TokenKind::MutateKw,
            RawToken::Own => TokenKind::Own,
            RawToken::Where => TokenKind::Where,
            RawToken::As => TokenKind::As,
            RawToken::Is => TokenKind::Is,
            RawToken::True => TokenKind::Bool(true),
            RawToken::False => TokenKind::Bool(false),
            RawToken::Unsafe => TokenKind::Unsafe,
            RawToken::Extern => TokenKind::Extern,
            RawToken::Comptime => TokenKind::Comptime,
            RawToken::Type => TokenKind::Type,
            RawToken::None => TokenKind::None,
            RawToken::Null => TokenKind::Null,
            RawToken::Using => TokenKind::Using,
            RawToken::Export => TokenKind::Export,
            RawToken::Lazy => TokenKind::Lazy,
            RawToken::Asm => TokenKind::Asm,
            RawToken::Native => TokenKind::Native,
            RawToken::Test => TokenKind::Test,
            RawToken::Benchmark => TokenKind::Benchmark,
            RawToken::Assert => TokenKind::Assert,
            RawToken::Check => TokenKind::Check,
            RawToken::Try => TokenKind::Try,
            RawToken::Or => TokenKind::Or,

            // Operators
            RawToken::Plus => TokenKind::Plus,
            RawToken::Minus => TokenKind::Minus,
            RawToken::Star => TokenKind::Star,
            RawToken::Slash => TokenKind::Slash,
            RawToken::Percent => TokenKind::Percent,
            RawToken::Eq => TokenKind::Eq,
            RawToken::EqEq => TokenKind::EqEq,
            RawToken::BangEq => TokenKind::BangEq,
            RawToken::Lt => TokenKind::Lt,
            RawToken::Gt => TokenKind::Gt,
            RawToken::LtEq => TokenKind::LtEq,
            RawToken::GtEq => TokenKind::GtEq,
            RawToken::AmpAmp => TokenKind::AmpAmp,
            RawToken::PipePipe => TokenKind::PipePipe,
            RawToken::Bang => TokenKind::Bang,
            RawToken::Question => TokenKind::Question,
            RawToken::QuestionQuestion => TokenKind::QuestionQuestion,
            RawToken::DotDot => TokenKind::DotDot,
            RawToken::Arrow => TokenKind::Arrow,
            RawToken::FatArrow => TokenKind::FatArrow,
            RawToken::At => TokenKind::At,
            RawToken::Dot => TokenKind::Dot,
            RawToken::Amp => TokenKind::Amp,
            RawToken::Pipe => TokenKind::Pipe,
            RawToken::Caret => TokenKind::Caret,
            RawToken::Tilde => TokenKind::Tilde,
            RawToken::LtLt => TokenKind::LtLt,
            RawToken::GtGt => TokenKind::GtGt,
            RawToken::ColonColon => TokenKind::ColonColon,
            RawToken::DotDotEq => TokenKind::DotDotEq,
            RawToken::QuestionDot => TokenKind::QuestionDot,
            RawToken::PlusEq => TokenKind::PlusEq,
            RawToken::MinusEq => TokenKind::MinusEq,
            RawToken::StarEq => TokenKind::StarEq,
            RawToken::SlashEq => TokenKind::SlashEq,
            RawToken::PercentEq => TokenKind::PercentEq,
            RawToken::AmpEq => TokenKind::AmpEq,
            RawToken::PipeEq => TokenKind::PipeEq,
            RawToken::CaretEq => TokenKind::CaretEq,
            RawToken::LtLtEq => TokenKind::LtLtEq,
            RawToken::GtGtEq => TokenKind::GtGtEq,

            // Delimiters
            RawToken::LBrace => TokenKind::LBrace,
            RawToken::RBrace => TokenKind::RBrace,
            RawToken::LParen => TokenKind::LParen,
            RawToken::RParen => TokenKind::RParen,
            RawToken::LBracket => TokenKind::LBracket,
            RawToken::RBracket => TokenKind::RBracket,
            RawToken::Colon => TokenKind::Colon,
            RawToken::Semi => TokenKind::Semi,
            RawToken::Comma => TokenKind::Comma,

            // Special
            RawToken::Newline => TokenKind::Newline,

            // Literals - parse the values
            RawToken::DecInt => {
                let (stripped, suffix) = parse_int_suffix(slice);
                let cleaned: String = stripped
                    .chars()
                    .filter(|c| *c != '_')
                    .collect();
                let value = cleaned.parse::<i64>().map_err(|_| LexError::invalid_number(start, end))?;
                TokenKind::Int(value, suffix)
            }
            RawToken::HexInt => {
                let (stripped, suffix) = parse_int_suffix(slice);
                let cleaned: String = stripped[2..].chars().filter(|c| *c != '_').collect();
                let value = i64::from_str_radix(&cleaned, 16).map_err(|_| LexError::invalid_number(start, end))?;
                TokenKind::Int(value, suffix)
            }
            RawToken::BinInt => {
                let (stripped, suffix) = parse_int_suffix(slice);
                let cleaned: String = stripped[2..].chars().filter(|c| *c != '_').collect();
                let value = i64::from_str_radix(&cleaned, 2).map_err(|_| LexError::invalid_number(start, end))?;
                TokenKind::Int(value, suffix)
            }
            RawToken::OctInt => {
                let (stripped, suffix) = parse_int_suffix(slice);
                let cleaned: String = stripped[2..].chars().filter(|c| *c != '_').collect();
                let value = i64::from_str_radix(&cleaned, 8).map_err(|_| LexError::invalid_number(start, end))?;
                TokenKind::Int(value, suffix)
            }
            RawToken::Float => {
                // Detect suffix
                let suffix = if slice.ends_with("f32") {
                    Some(FloatSuffix::F32)
                } else if slice.ends_with("f64") {
                    Some(FloatSuffix::F64)
                } else {
                    None
                };

                // Remove suffix and underscores
                let cleaned: String = slice
                    .trim_end_matches("f32")
                    .trim_end_matches("f64")
                    .chars()
                    .filter(|c| *c != '_')
                    .collect();
                let value = cleaned.parse::<f64>().map_err(|_| LexError::invalid_number(start, end))?;
                TokenKind::Float(value, suffix)
            }
            RawToken::Char => {
                let inner = &slice[1..slice.len() - 1]; // Remove quotes
                let ch = parse_char(inner, start)?;
                TokenKind::Char(ch)
            }
            RawToken::String => {
                let inner = &slice[1..slice.len() - 1]; // Remove quotes
                let s = parse_string(inner, start)?;
                TokenKind::String(s)
            }
            RawToken::MultiLineString => {
                let inner = &slice[3..slice.len() - 3]; // Remove triple quotes
                // Multi-line strings don't process escapes (raw)
                TokenKind::String(inner.to_string())
            }
            RawToken::RawString => {
                // r"content" — strip r" prefix and " suffix, no escape processing
                let inner = &slice[2..slice.len() - 1];
                TokenKind::String(inner.to_string())
            }
            RawToken::RawHashString => {
                // r#"content"# or r##"content"## etc.
                let after_r = &slice[1..]; // skip 'r'
                let hash_count = after_r.bytes().take_while(|b| *b == b'#').count();
                let content_start = 1 + hash_count + 1; // r + #*n + "
                let content_end = slice.len() - 1 - hash_count; // " + #*n
                let inner = &slice[content_start..content_end];
                TokenKind::String(inner.to_string())
            }
            RawToken::Ident => TokenKind::Ident(slice.to_string()),

            // These are skipped by logos, but we list them for completeness
            RawToken::LineComment | RawToken::BlockComment => {
                unreachable!("comments are skipped")
            }
        })
    }
}

/// Parse integer type suffix from a number literal.
/// Returns the stripped string and the optional suffix.
fn parse_int_suffix(s: &str) -> (&str, Option<IntSuffix>) {
    const SUFFIXES: &[(&str, IntSuffix)] = &[
        ("i128", IntSuffix::I128), ("i64", IntSuffix::I64), ("i32", IntSuffix::I32),
        ("i16", IntSuffix::I16), ("i8", IntSuffix::I8), ("isize", IntSuffix::Isize),
        ("u128", IntSuffix::U128), ("u64", IntSuffix::U64), ("u32", IntSuffix::U32),
        ("u16", IntSuffix::U16), ("u8", IntSuffix::U8), ("usize", IntSuffix::Usize),
    ];
    for (suffix_str, suffix_enum) in SUFFIXES {
        if let Some(stripped) = s.strip_suffix(suffix_str) {
            return (stripped, Some(*suffix_enum));
        }
    }
    (s, None)
}

/// Parse a character literal (handling escape sequences).
fn parse_char(s: &str, pos: usize) -> Result<char, LexError> {
    let mut chars = s.chars();
    match chars.next() {
        Some('\\') => parse_escape(&mut chars, pos),
        Some(c) => Ok(c),
        None => Err(LexError::invalid_escape(pos)),
    }
}

/// Parse a string literal (handling escape sequences).
fn parse_string(s: &str, pos: usize) -> Result<String, LexError> {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            result.push(parse_escape(&mut chars, pos)?);
        } else {
            result.push(c);
        }
    }

    Ok(result)
}

/// Parse an escape sequence.
fn parse_escape(chars: &mut impl Iterator<Item = char>, pos: usize) -> Result<char, LexError> {
    match chars.next() {
        Some('n') => Ok('\n'),
        Some('r') => Ok('\r'),
        Some('t') => Ok('\t'),
        Some('\\') => Ok('\\'),
        Some('0') => Ok('\0'),
        Some('\'') => Ok('\''),
        Some('"') => Ok('"'),
        Some('{') => Ok('{'),  // For string interpolation escaping
        Some('u') => parse_unicode_escape(chars, pos),
        _ => Err(LexError::invalid_escape(pos)),
    }
}

/// Parse a Unicode escape sequence: \u{XXXX} (1-6 hex digits).
fn parse_unicode_escape(chars: &mut impl Iterator<Item = char>, pos: usize) -> Result<char, LexError> {
    // Expect opening brace
    match chars.next() {
        Some('{') => {}
        _ => return Err(LexError::invalid_escape(pos)),
    }

    // Collect hex digits (1-6)
    let mut hex = String::new();
    loop {
        match chars.next() {
            Some('}') => break,
            Some(c) if c.is_ascii_hexdigit() && hex.len() < 6 => hex.push(c),
            _ => return Err(LexError::invalid_escape(pos)),
        }
    }

    if hex.is_empty() {
        return Err(LexError::invalid_escape(pos));
    }

    // Parse the hex value and convert to char
    let code_point = u32::from_str_radix(&hex, 16).map_err(|_| LexError::invalid_escape(pos))?;
    char::from_u32(code_point).ok_or(LexError::invalid_escape(pos))
}

/// Result of lexing: tokens plus any errors found.
#[derive(Debug)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
}

impl LexResult {
    /// Returns true if lexing completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// A lexer error with location and friendly message.
#[derive(Debug, Clone)]
pub struct LexError {
    pub span: Span,
    pub message: String,
    pub hint: Option<String>,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LexError {}

impl LexError {
    fn unexpected_char(ch: char, pos: usize) -> Self {
        Self {
            span: Span::new(pos, pos + ch.len_utf8()),
            message: format!("unexpected character '{}'", ch),
            hint: Some("remove this character or check for typos".to_string()),
        }
    }

    fn invalid_escape(pos: usize) -> Self {
        Self {
            span: Span::new(pos, pos + 1),
            message: "invalid escape sequence".to_string(),
            hint: Some("valid escapes: \\n \\r \\t \\\\ \\0 \\' \\\" \\u{...}".to_string()),
        }
    }

    fn invalid_number(start: usize, end: usize) -> Self {
        Self {
            span: Span::new(start, end),
            message: "invalid number literal".to_string(),
            hint: Some("check for extra digits or invalid prefix (0x, 0b, 0o)".to_string()),
        }
    }
}

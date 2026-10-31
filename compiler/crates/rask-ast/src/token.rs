// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Token definitions for the lexer.

use crate::Span;

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Float literal type suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSuffix {
    F32,
    F64,
}

/// Integer literal type suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntSuffix {
    I8, I16, I32, I64, I128, Isize,
    U8, U16, U32, U64, U128, Usize,
    /// No suffix was written, but the literal's magnitude doesn't fit `i64`,
    /// so it can only be a `u64`. Distinct from a written `u64` because a
    /// leading `-` can still claim it: the parser folds the sign in, and
    /// `-9223372036854775808` is `i64::MIN`, not a negated unsigned.
    U64ByMagnitude,
    /// Doesn't fit `u64` either, so the narrowest type that holds it is
    /// `i128`. A `-` in front still folds, down to `i128::MIN`.
    I128ByMagnitude,
    /// Above `i128::MAX`: only `u128` holds it, and the token carries its bit
    /// pattern because that's the one value range `i128` can't represent.
    U128ByMagnitude,
}

impl IntSuffix {
    /// The width this suffix names, with the `ByMagnitude` markers reading as
    /// the type they landed in.
    pub fn effective(self) -> IntSuffix {
        match self {
            IntSuffix::U64ByMagnitude => IntSuffix::U64,
            IntSuffix::I128ByMagnitude => IntSuffix::I128,
            IntSuffix::U128ByMagnitude => IntSuffix::U128,
            other => other,
        }
    }
}

/// Where a string literal's text sits in its source. The opening delimiter and
/// each escape shift the two apart, so a `{hole}` found in the text needs this
/// to point back at the file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StrPositions {
    /// `(text byte, source byte from the token's start)`, one at the start and
    /// one after each escape, in order.
    anchors: Vec<(u32, u32)>,
}

impl StrPositions {
    pub fn new(delimiter_len: usize) -> Self {
        StrPositions { anchors: vec![(0, delimiter_len as u32)] }
    }

    /// Record that text byte `text` comes from source byte `source`.
    pub fn anchor(&mut self, text: usize, source: usize) {
        self.anchors.push((text as u32, source as u32));
    }

    /// Source byte, from the token's start, of text byte `text`.
    pub fn source_offset(&self, text: usize) -> usize {
        let (t, s) = self.anchors.iter().rev()
            .find(|(t, _)| *t as usize <= text)
            .copied()
            .unwrap_or((0, 0));
        s as usize + (text - t as usize)
    }
}

/// Every keyword and how it's spelled: the one list. The lexer's tokens are
/// checked against it, and the parser, the diagnostics and the LSP read it.
pub const KEYWORDS: &[(TokenKind, &str)] = &[
    (TokenKind::Func, "func"),
    (TokenKind::Let, "let"),
    (TokenKind::Mut, "mut"),
    (TokenKind::Const, "const"),
    (TokenKind::Struct, "struct"),
    (TokenKind::Enum, "enum"),
    (TokenKind::Interface, "interface"),
    (TokenKind::Implements, "implements"),
    (TokenKind::Extend, "extend"),
    (TokenKind::Public, "public"),
    (TokenKind::Private, "private"),
    (TokenKind::Import, "import"),
    (TokenKind::Return, "return"),
    (TokenKind::If, "if"),
    (TokenKind::Else, "else"),
    (TokenKind::Match, "match"),
    (TokenKind::For, "for"),
    (TokenKind::In, "in"),
    (TokenKind::While, "while"),
    (TokenKind::Loop, "loop"),
    (TokenKind::Break, "break"),
    (TokenKind::Continue, "continue"),
    (TokenKind::SelectPriority, "select_priority"),
    (TokenKind::Select, "select"),
    (TokenKind::With, "with"),
    (TokenKind::Ensure, "ensure"),
    (TokenKind::Take, "take"),
    (TokenKind::MutateKw, "mutate"),
    (TokenKind::Where, "where"),
    (TokenKind::As, "as"),
    (TokenKind::Is, "is"),
    (TokenKind::Bool(true), "true"),
    (TokenKind::Bool(false), "false"),
    (TokenKind::Unsafe, "unsafe"),
    (TokenKind::Extern, "extern"),
    (TokenKind::Comptime, "comptime"),
    (TokenKind::Type, "type"),
    (TokenKind::None, "none"),
    (TokenKind::Using, "using"),
    (TokenKind::Export, "export"),
    (TokenKind::Lazy, "lazy"),
    (TokenKind::Asm, "asm"),
    (TokenKind::Native, "native"),
    (TokenKind::Test, "test"),
    (TokenKind::Benchmark, "benchmark"),
    (TokenKind::Assert, "assert"),
    (TokenKind::Check, "check"),
    (TokenKind::Try, "try"),
    (TokenKind::Catch, "catch"),
    (TokenKind::Or, "or"),
    (TokenKind::Union, "union"),
    (TokenKind::Discard, "discard"),
    (TokenKind::Void, "void"),
];

/// The kind of token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Int(i128, Option<IntSuffix>),
    Float(f64, Option<FloatSuffix>),
    String(String, StrPositions),
    /// `x"…"`, already decoded (std.bytes/BY10)
    Bytes(Vec<u8>),
    Char(char),
    Bool(bool),

    // Identifier
    Ident(String),

    // Keywords
    Func,
    Let,
    Mut,
    Const,
    Struct,
    Enum,
    Interface,
    Implements,
    Extend,
    Public,
    Import,
    Return,
    If,
    Else,
    Match,
    For,
    In,
    While,
    Loop,
    Break,
    Continue,
    Select,
    SelectPriority,
    With,
    Ensure,
    Take,
    MutateKw,
    Where,
    As,
    Is,
    Unsafe,
    Extern,
    Comptime,
    Type,
    None,
    Using,
    Export,
    Lazy,
    Asm,
    Native,
    Test,
    Benchmark,
    Assert,
    Check,
    Try,
    Catch,
    Or,
    Private,
    Union,
    Discard,
    Void,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    BangEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    AmpAmp,
    PipePipe,
    Bang,
    Question,
    QuestionQuestion,
    DotDot,
    Arrow,
    FatArrow,
    At,
    Dot,
    Amp,          // &
    Pipe,         // |
    Caret,        // ^
    Tilde,        // ~
    LtLt,         // <<
    GtGt,         // >>
    ColonColon,   // ::
    DotDotEq,     // ..=
    QuestionDot,  // ?.
    PlusEq,       // +=
    MinusEq,      // -=
    StarEq,       // *=
    SlashEq,      // /=
    PercentEq,    // %=
    AmpEq,        // &=
    PipeEq,       // |=
    CaretEq,      // ^=
    LtLtEq,       // <<=
    GtGtEq,       // >>=

    // Delimiters
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Colon,
    Semi,
    Comma,

    // Special
    Newline,
    Eof,

    /// Doc comment (`/// ...`)
    DocComment(String),
}

impl TokenKind {
    /// Returns a human-readable name for this token kind.
    /// How this token is spelled, when it's a keyword.
    pub fn keyword(&self) -> Option<&'static str> {
        KEYWORDS.iter().find(|(k, _)| k == self).map(|(_, spelling)| *spelling)
    }

    pub fn display_name(&self) -> &'static str {
        if let Some(word) = self.keyword() {
            return quoted_keyword(word);
        }
        match self {
            // Literals
            TokenKind::Int(_, _) => "a number",
            TokenKind::Float(_, _) => "a number",
            TokenKind::String(..) => "a string",
            TokenKind::Bytes(_) => "a bytes literal",
            TokenKind::Char(_) => "a character",

            // Identifier
            TokenKind::Ident(_) => "a name",

            // Operators
            TokenKind::Plus => "'+'",
            TokenKind::Minus => "'-'",
            TokenKind::Star => "'*'",
            TokenKind::Slash => "'/'",
            TokenKind::Percent => "'%'",
            TokenKind::Eq => "'='",
            TokenKind::EqEq => "'=='",
            TokenKind::BangEq => "'!='",
            TokenKind::Lt => "'<'",
            TokenKind::Gt => "'>'",
            TokenKind::LtEq => "'<='",
            TokenKind::GtEq => "'>='",
            TokenKind::AmpAmp => "'&&'",
            TokenKind::PipePipe => "'||'",
            TokenKind::Bang => "'!'",
            TokenKind::Question => "'?'",
            TokenKind::QuestionQuestion => "'??'",
            TokenKind::DotDot => "'..'",
            TokenKind::Arrow => "'->'",
            TokenKind::FatArrow => "'=>'",
            TokenKind::At => "'@'",
            TokenKind::Dot => "'.'",
            TokenKind::Amp => "'&'",
            TokenKind::Pipe => "'|'",
            TokenKind::Caret => "'^'",
            TokenKind::Tilde => "'~'",
            TokenKind::LtLt => "'<<'",
            TokenKind::GtGt => "'>>'",
            TokenKind::ColonColon => "'::'",
            TokenKind::DotDotEq => "'..='",
            TokenKind::QuestionDot => "'?.'",
            TokenKind::PlusEq => "'+='",
            TokenKind::MinusEq => "'-='",
            TokenKind::StarEq => "'*='",
            TokenKind::SlashEq => "'/='",
            TokenKind::PercentEq => "'%='",
            TokenKind::AmpEq => "'&='",
            TokenKind::PipeEq => "'|='",
            TokenKind::CaretEq => "'^='",
            TokenKind::LtLtEq => "'<<='",
            TokenKind::GtGtEq => "'>>='",

            // Delimiters
            TokenKind::LBrace => "'{'",
            TokenKind::RBrace => "'}'",
            TokenKind::LParen => "'('",
            TokenKind::RParen => "')'",
            TokenKind::LBracket => "'['",
            TokenKind::RBracket => "']'",
            TokenKind::Colon => "':'",
            TokenKind::Semi => "';'",
            TokenKind::Comma => "','",

            // Special
            TokenKind::Newline => "end of line",
            TokenKind::Eof => "end of file",
            TokenKind::DocComment(_) => "doc comment",
            // `KEYWORDS` has every keyword; the lexer's test holds it to that.
            keyword => unreachable!("{keyword:?} is a keyword missing from KEYWORDS"),
        }
    }
}

/// `'func'` for `func`, built once per keyword.
fn quoted_keyword(word: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static QUOTED: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    QUOTED
        .get_or_init(|| KEYWORDS.iter().map(|(_, w)| (*w, format!("'{w}'"))).collect())
        .get(word)
        .map(String::as_str)
        .unwrap_or(word)
}

/// `bytes` as source: `x"89 50 4E 47"`, uppercase pairs with a space between
/// them. What `{b:debug}` prints, so debug output pastes back as a literal
/// (std.bytes/BY6).
pub fn bytes_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(3 + bytes.len() * 3);
    out.push_str("x\"");
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out.push('"');
    out
}

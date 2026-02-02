//! Token definitions for the lexer.

use crate::Span;

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// The kind of token.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Int(i64),
    Float(f64),
    String(String),
    Char(char),
    Bool(bool),

    // Identifier
    Ident(String),

    // Keywords
    Func,
    Let,
    Const,
    Struct,
    Enum,
    Trait,
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
    Deliver,
    Spawn,
    RawThread,
    Select,
    With,
    Ensure,
    Take,
    Own,
    Where,
    As,
    Is,
    Unsafe,
    Comptime,
    Type,
    None,
    Null,
    Using,
    Export,
    Asm,
    Step,
    Native,
    Timeout,

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
}

impl TokenKind {
    /// Returns a human-readable name for this token kind.
    pub fn display_name(&self) -> &'static str {
        match self {
            // Literals
            TokenKind::Int(_) => "a number",
            TokenKind::Float(_) => "a number",
            TokenKind::String(_) => "a string",
            TokenKind::Char(_) => "a character",
            TokenKind::Bool(_) => "'true' or 'false'",

            // Identifier
            TokenKind::Ident(_) => "a name",

            // Keywords
            TokenKind::Func => "'func'",
            TokenKind::Let => "'let'",
            TokenKind::Const => "'const'",
            TokenKind::Struct => "'struct'",
            TokenKind::Enum => "'enum'",
            TokenKind::Trait => "'trait'",
            TokenKind::Extend => "'extend'",
            TokenKind::Public => "'public'",
            TokenKind::Import => "'import'",
            TokenKind::Return => "'return'",
            TokenKind::If => "'if'",
            TokenKind::Else => "'else'",
            TokenKind::Match => "'match'",
            TokenKind::For => "'for'",
            TokenKind::In => "'in'",
            TokenKind::While => "'while'",
            TokenKind::Loop => "'loop'",
            TokenKind::Break => "'break'",
            TokenKind::Continue => "'continue'",
            TokenKind::Deliver => "'deliver'",
            TokenKind::Spawn => "'spawn'",
            TokenKind::RawThread => "'raw_thread'",
            TokenKind::Select => "'select'",
            TokenKind::With => "'with'",
            TokenKind::Ensure => "'ensure'",
            TokenKind::Take => "'take'",
            TokenKind::Own => "'own'",
            TokenKind::Where => "'where'",
            TokenKind::As => "'as'",
            TokenKind::Is => "'is'",
            TokenKind::Unsafe => "'unsafe'",
            TokenKind::Comptime => "'comptime'",
            TokenKind::Type => "'type'",
            TokenKind::None => "'None'",
            TokenKind::Null => "'null'",
            TokenKind::Using => "'using'",
            TokenKind::Export => "'export'",
            TokenKind::Asm => "'asm'",
            TokenKind::Step => "'step'",
            TokenKind::Native => "'native'",
            TokenKind::Timeout => "'timeout'",

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
        }
    }
}

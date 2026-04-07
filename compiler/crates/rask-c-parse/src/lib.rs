// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Minimal C header parser for `import c "header.h"`.
//!
//! Parses C declarations (functions, structs, unions, enums, typedefs, #define
//! constants) into a C-level AST. No libclang dependency. Handles the subset
//! of C that appears in well-behaved library headers.

mod lexer;
mod parser;
pub mod translate;

pub use lexer::{CLexer, CToken, CTokenKind};
pub use parser::{CParser, ParseError};

/// A parsed C declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum CDecl {
    Function(CFuncDecl),
    Struct(CStructDecl),
    Union(CStructDecl),
    Enum(CEnumDecl),
    Typedef(CTypedef),
    /// `#define NAME value` — integer or string constant.
    Define(CDefine),
    /// Global variable declaration.
    Variable(CVarDecl),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CFuncDecl {
    pub name: String,
    pub params: Vec<CParam>,
    pub ret_ty: CType,
    pub is_variadic: bool,
    pub is_static: bool,
    pub is_inline: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CParam {
    pub name: Option<String>,
    pub ty: CType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CStructDecl {
    pub tag: Option<String>,
    pub fields: Vec<CField>,
    /// True if this is a forward declaration (no body).
    pub is_forward: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CField {
    pub name: String,
    pub ty: CType,
    pub bit_width: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CEnumDecl {
    pub tag: Option<String>,
    pub variants: Vec<CEnumVariant>,
    pub is_forward: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CEnumVariant {
    pub name: String,
    pub value: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CTypedef {
    pub name: String,
    pub target: CType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CDefine {
    pub name: String,
    pub kind: CDefineKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CDefineKind {
    Integer(i64),
    UnsignedInteger(u64),
    Float(f64),
    String(String),
    /// Function-like macro — skipped per spec, but recorded for warnings.
    FunctionMacro { params: Vec<String> },
    /// Unparseable expression — skipped.
    Unparseable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CVarDecl {
    pub name: String,
    pub ty: CType,
    pub is_extern: bool,
    pub is_const: bool,
}

/// C type representation.
#[derive(Debug, Clone, PartialEq)]
pub enum CType {
    Void,
    Char,
    SignedChar,
    UnsignedChar,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
    Float,
    Double,
    Bool,
    /// `size_t`
    SizeT,
    /// `ssize_t` / `ptrdiff_t`
    SSizeT,
    /// `int8_t`, `uint32_t`, etc.
    FixedInt { bits: u8, signed: bool },
    /// `intptr_t` / `uintptr_t`
    IntPtr { signed: bool },
    /// Pointer to T.
    Pointer(Box<CType>),
    /// `const T` (qualifiers on inner type).
    Const(Box<CType>),
    /// Array `T[N]`.
    Array(Box<CType>, Option<u64>),
    /// Named type (struct tag, typedef name, etc.)
    Named(String),
    /// `struct tag` (before resolution to Named).
    StructTag(String),
    /// `union tag`.
    UnionTag(String),
    /// `enum tag`.
    EnumTag(String),
    /// Function pointer: `ret (*)(params...)`.
    FuncPtr {
        ret: Box<CType>,
        params: Vec<CType>,
        is_variadic: bool,
    },
}

/// Warning emitted during parsing (non-fatal).
#[derive(Debug, Clone)]
pub struct CWarning {
    pub message: String,
    pub line: usize,
}

/// Result of parsing a C header.
#[derive(Debug, Clone)]
pub struct CParseResult {
    pub decls: Vec<CDecl>,
    pub warnings: Vec<CWarning>,
}

/// Parse a C header source string into declarations.
pub fn parse_c_header(source: &str) -> Result<CParseResult, ParseError> {
    let preprocessed = preprocess(source);
    let tokens = CLexer::new(&preprocessed).tokenize()?;
    let mut parser = CParser::new(tokens);
    parser.parse()
}

#[cfg(test)]
mod tests;

/// Minimal preprocessor: strips comments, handles simple #define/#include/#ifdef.
fn preprocess(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some(&ch) = chars.peek() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
                out.push('\n');
            }
            chars.next();
            continue;
        }
        if in_block_comment {
            if ch == '*' {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                    out.push(' ');
                }
            } else {
                if ch == '\n' {
                    out.push('\n');
                }
                chars.next();
            }
            continue;
        }
        if ch == '/' {
            chars.next();
            match chars.peek() {
                Some(&'/') => {
                    chars.next();
                    in_line_comment = true;
                }
                Some(&'*') => {
                    chars.next();
                    in_block_comment = true;
                }
                _ => {
                    out.push('/');
                }
            }
            continue;
        }
        out.push(ch);
        chars.next();
    }
    out
}

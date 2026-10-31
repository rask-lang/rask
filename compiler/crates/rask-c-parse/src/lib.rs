// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Minimal C header parser for `import c "header.h"`.
//!
//! Parses C declarations (functions, structs, unions, enums, typedefs, #define
//! constants) into a C-level AST. No libclang dependency. Handles the subset
//! of C that appears in well-behaved library headers.

pub mod consteval;
pub mod shim;
mod lexer;
mod parser;
pub mod toolchain;
pub mod translate;

pub use lexer::{CLexer, CToken, CTokenKind};
pub use parser::{CParser, ParseError};

/// A declaration and where it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct CItem {
    pub decl: CDecl,
    /// The file the preprocessor said it came from; empty for the header
    /// itself when it was parsed without one.
    pub file: String,
    pub line: usize,
}

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
    /// `None` when the value is an expression nothing could evaluate. The
    /// enumerators after it, counting up from it, are unknown too.
    pub value: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CTypedef {
    pub name: String,
    pub target: CType,
}

/// `#define NAME body` or `#define NAME(params) body`.
///
/// The body stays tokens: a macro means nothing until it's used, and it may
/// name macros defined after it, so it's evaluated at translation, against
/// all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct CDefine {
    pub name: String,
    /// `Some` for a function-like macro.
    pub params: Option<Vec<String>>,
    pub body: Vec<CTokenKind>,
    /// The definition as written, for the comment a skipped one leaves.
    pub text: String,
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
    pub decls: Vec<CItem>,
    pub warnings: Vec<CWarning>,
}

/// Parse a C header source string into declarations.
///
/// `source` is either the header as written or the C preprocessor's output
/// for it (`cc -E -dD`), whose line markers say which file each declaration
/// came from.
pub fn parse_c_header(source: &str) -> Result<CParseResult, ParseError> {
    let preprocessed = preprocess(source);
    let mut lexer = CLexer::new(&preprocessed);
    let tokens = lexer.tokenize()?;
    let mut parser = CParser::with_files(tokens, std::mem::take(&mut lexer.files));
    let mut result = parser.parse()?;
    let lines: Vec<&str> = preprocessed.lines().collect();
    for item in &mut result.decls {
        if let CDecl::Define(d) = &mut item.decl {
            d.text = source_text(&lines, item.line);
        }
    }
    Ok(result)
}

/// The logical line starting at `line` (1-based), joined across `\`
/// continuations.
fn source_text(lines: &[&str], line: usize) -> String {
    let mut out = String::new();
    let mut i = line.saturating_sub(1);
    while let Some(l) = lines.get(i) {
        match l.trim_end().strip_suffix('\\') {
            Some(head) => {
                out.push_str(head.trim_end());
                out.push(' ');
                i += 1;
            }
            None => {
                out.push_str(l.trim_end());
                break;
            }
        }
    }
    out.trim().to_string()
}

/// The constants a header defines: object-like macros, enumerators, and the
/// typedefs a cast can name. Shared by the parser, which needs them for array
/// sizes and enumerator values, and the translator, which needs them for
/// macros.
#[derive(Debug, Clone, Default)]
pub struct Constants {
    pub macros: std::collections::HashMap<String, Vec<CTokenKind>>,
    /// Function-like macros: parameters and body.
    pub function_macros: std::collections::HashMap<String, (Vec<String>, Vec<CTokenKind>)>,
    pub enumerators: std::collections::HashMap<String, i64>,
    pub typedefs: std::collections::HashMap<String, CType>,
}

impl Constants {
    pub fn from_items(items: &[CItem]) -> Self {
        let mut c = Constants::default();
        for item in items {
            c.note(&item.decl);
        }
        c
    }

    pub fn note(&mut self, decl: &CDecl) {
        match decl {
            CDecl::Define(d) => match &d.params {
                None => {
                    self.macros.insert(d.name.clone(), d.body.clone());
                }
                Some(params) => {
                    self.function_macros.insert(d.name.clone(), (params.clone(), d.body.clone()));
                }
            },
            CDecl::Enum(e) => {
                for v in &e.variants {
                    if let Some(x) = v.value {
                        self.enumerators.insert(v.name.clone(), x);
                    }
                }
            }
            CDecl::Typedef(t) => {
                self.typedefs.insert(t.name.clone(), t.target.clone());
            }
            _ => {}
        }
    }

    fn value_at(&self, name: &str, depth: u32) -> Option<consteval::CValue> {
        if let Some(v) = self.enumerators.get(name) {
            return Some(consteval::CValue::Int(*v as i128, CType::Int));
        }
        if depth > 32 {
            return None;
        }
        let body = self.macros.get(name)?;
        consteval::eval(body, &Nested { c: self, depth: depth + 1 })
    }
}

impl consteval::Env for Constants {
    fn value(&self, name: &str) -> Option<consteval::CValue> {
        self.value_at(name, 0)
    }

    fn int_type(&self, name: &str) -> Option<CType> {
        int_type_of(self, name, 0)
    }

    fn function_macro(&self, name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
        self.function_macros.get(name).cloned()
    }
}

fn int_type_of(c: &Constants, name: &str, depth: u32) -> Option<CType> {
    if let Some(t) = builtin_typedef(name) {
        return Some(t);
    }
    if depth > 16 {
        return None;
    }
    match strip_qualifiers(c.typedefs.get(name)?) {
        CType::Named(n) => int_type_of(c, n, depth + 1),
        t @ (CType::Char | CType::SignedChar | CType::UnsignedChar | CType::Short
        | CType::UnsignedShort | CType::Int | CType::UnsignedInt | CType::Long
        | CType::UnsignedLong | CType::LongLong | CType::UnsignedLongLong | CType::Bool
        | CType::SizeT | CType::SSizeT | CType::FixedInt { .. } | CType::IntPtr { .. }
        | CType::EnumTag(_)) => Some(match t {
            CType::EnumTag(_) => CType::Int,
            other => other.clone(),
        }),
        _ => None,
    }
}

/// A macro body evaluated from inside another one, so a cycle stops.
struct Nested<'a> {
    c: &'a Constants,
    depth: u32,
}

impl consteval::Env for Nested<'_> {
    fn value(&self, name: &str) -> Option<consteval::CValue> {
        self.c.value_at(name, self.depth)
    }

    fn int_type(&self, name: &str) -> Option<CType> {
        int_type_of(self.c, name, 0)
    }

    fn function_macro(&self, name: &str) -> Option<(Vec<String>, Vec<CTokenKind>)> {
        self.c.function_macros.get(name).cloned()
    }
}

fn strip_qualifiers(t: &CType) -> &CType {
    match t {
        CType::Const(inner) => strip_qualifiers(inner),
        other => other,
    }
}

/// Typedef names every libc spells the same way, mapped without reading
/// their definitions.
pub fn builtin_typedef(name: &str) -> Option<CType> {
    Some(match name {
        "size_t" => CType::SizeT,
        "ssize_t" | "ptrdiff_t" => CType::SSizeT,
        "int8_t" | "__int8_t" => CType::FixedInt { bits: 8, signed: true },
        "int16_t" | "__int16_t" => CType::FixedInt { bits: 16, signed: true },
        "int32_t" | "__int32_t" => CType::FixedInt { bits: 32, signed: true },
        "int64_t" | "__int64_t" => CType::FixedInt { bits: 64, signed: true },
        "uint8_t" | "__uint8_t" => CType::FixedInt { bits: 8, signed: false },
        "uint16_t" | "__uint16_t" => CType::FixedInt { bits: 16, signed: false },
        "uint32_t" | "__uint32_t" => CType::FixedInt { bits: 32, signed: false },
        "uint64_t" | "__uint64_t" => CType::FixedInt { bits: 64, signed: false },
        "intptr_t" => CType::IntPtr { signed: true },
        "uintptr_t" => CType::IntPtr { signed: false },
        _ => return None,
    })
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

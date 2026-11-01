// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! C header parser — turns tokens into C declarations.

use crate::lexer::{CToken, CTokenKind};
use crate::*;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

impl ParseError {
    pub fn new(message: String, line: usize) -> Self {
        Self { message, line }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Point a typedef target at a struct that has just been given a name.
///
/// `typedef struct { … } Rect, *RectPtr;` parses the body before any name
/// exists, so both targets go through a placeholder. Once `Rect` is adopted as
/// the tag, they name the tag instead — including through pointers.
fn retarget_anon(ty: &mut CType, placeholder: &str, adopted: &str, is_struct: bool) {
    match ty {
        CType::Named(n) if n == placeholder => {
            *ty = if is_struct {
                CType::StructTag(adopted.to_string())
            } else {
                CType::UnionTag(adopted.to_string())
            };
        }
        CType::Pointer(inner) => retarget_anon(inner, placeholder, adopted, is_struct),
        _ => {}
    }
}

pub struct CParser {
    tokens: Vec<CToken>,
    files: Vec<String>,
    pos: usize,
    warnings: Vec<CWarning>,
    /// What constants are known so far, for array sizes and enumerators.
    consts: Constants,
    /// The file the preprocessor's last line marker named.
    file: String,
    /// Types declared inside another's body, waiting to be emitted ahead of it.
    nested: Vec<CItem>,
    /// Tags of the structs whose bodies are being parsed, innermost last.
    outer: Vec<Option<String>>,
    anon: usize,
}

impl CParser {
    pub fn new(tokens: Vec<CToken>) -> Self {
        Self::with_files(tokens, vec![String::new()])
    }

    /// `files` is what the lexer's line markers named, indexed by
    /// `CToken::file`.
    pub fn with_files(tokens: Vec<CToken>, files: Vec<String>) -> Self {
        Self {
            tokens,
            files,
            pos: 0,
            warnings: Vec::new(),
            consts: Constants::default(),
            file: String::new(),
            nested: Vec::new(),
            outer: Vec::new(),
            anon: 0,
        }
    }

    fn push(&mut self, out: &mut Vec<CItem>, decl: CDecl, line: usize) {
        self.consts.note(&decl);
        out.extend(self.nested.drain(..));
        out.push(CItem { decl, file: self.file.clone(), line });
    }

    pub fn parse(&mut self) -> Result<CParseResult, ParseError> {
        let mut decls: Vec<CItem> = Vec::new();

        loop {
            self.skip_newlines();
            if self.at_eof() {
                break;
            }
            let line = self.line();
            self.file = self.files.get(self.peek_token().file as usize).cloned().unwrap_or_default();

            match self.peek() {
                // Preprocessor directives
                CTokenKind::PPDefine => match self.parse_define() {
                    Ok(Some(d)) => self.push(&mut decls, CDecl::Define(d), line),
                    Ok(None) => {}
                    // A name the lexer read as an attribute keyword: nothing
                    // a binding can use.
                    Err(_) => self.skip_to_newline(),
                },
                CTokenKind::PPInclude
                | CTokenKind::PPIfdef
                | CTokenKind::PPIfndef
                | CTokenKind::PPIf
                | CTokenKind::PPElse
                | CTokenKind::PPElif
                | CTokenKind::PPEndif
                | CTokenKind::PPUndef
                | CTokenKind::PPPragma
                | CTokenKind::PPError
                | CTokenKind::PPLine => {
                    self.skip_to_newline();
                }
                // Stray closing brace (from extern "C" { ... })
                CTokenKind::RBrace => {
                    self.advance();
                }
                // Declarations
                _ => {
                    match self.try_parse_declaration() {
                        Ok(Some(new_decls)) => {
                            for d in new_decls {
                                self.push(&mut decls, d, line);
                            }
                        }
                        Ok(None) => self.nested.clear(),
                        Err(e) => {
                            self.nested.clear();
                            self.warnings.push(CWarning {
                                message: format!("skipping declaration: {} (at {:?})", e.message, self.peek()),
                                line: e.line,
                            });
                            self.skip_to_semi_or_brace();
                        }
                    }
                }
            }
        }

        Ok(CParseResult {
            decls,
            warnings: self.warnings.clone(),
        })
    }

    // ---- Token helpers ----

    fn at_eof(&self) -> bool {
        self.pos >= self.tokens.len() || self.peek() == CTokenKind::Eof
    }

    fn peek_token(&self) -> &CToken {
        static EOF_TOKEN: CToken = CToken { kind: CTokenKind::Eof, line: 0, space_before: false, file: 0 };
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            &EOF_TOKEN
        }
    }

    fn peek(&self) -> CTokenKind {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].kind.clone()
        } else {
            CTokenKind::Eof
        }
    }

    fn line(&self) -> usize {
        if self.pos < self.tokens.len() {
            self.tokens[self.pos].line
        } else {
            0
        }
    }

    fn advance(&mut self) -> CTokenKind {
        let tok = self.peek();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: &CTokenKind) -> Result<(), ParseError> {
        let got = self.peek();
        if std::mem::discriminant(&got) == std::mem::discriminant(kind) {
            self.advance();
            Ok(())
        } else {
            Err(ParseError::new(
                format!("expected {:?}, got {:?}", kind, got),
                self.line(),
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.advance() {
            CTokenKind::Ident(s) => Ok(s),
            other => Err(ParseError::new(
                format!("expected identifier, got {:?}", other),
                self.line(),
            )),
        }
    }

    fn match_token(&mut self, kind: &CTokenKind) -> bool {
        if std::mem::discriminant(&self.peek()) == std::mem::discriminant(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn skip_newlines(&mut self) {
        while self.peek() == CTokenKind::Newline {
            self.advance();
        }
    }

    fn skip_to_newline(&mut self) {
        while !self.at_eof() && self.peek() != CTokenKind::Newline {
            self.advance();
        }
        if self.peek() == CTokenKind::Newline {
            self.advance();
        }
    }

    fn skip_to_semi_or_brace(&mut self) {
        let mut brace_depth = 0u32;
        loop {
            if self.at_eof() {
                break;
            }
            match self.peek() {
                CTokenKind::LBrace => { brace_depth += 1; self.advance(); }
                CTokenKind::RBrace => {
                    if brace_depth == 0 {
                        break;
                    }
                    brace_depth -= 1;
                    self.advance();
                    if brace_depth == 0 {
                        // Consume trailing semicolon if present
                        self.skip_newlines();
                        self.match_token(&CTokenKind::Semi);
                        break;
                    }
                }
                CTokenKind::Semi if brace_depth == 0 => {
                    self.advance();
                    break;
                }
                _ => { self.advance(); }
            }
        }
    }

    // ---- Preprocessor ----

    fn parse_define(&mut self) -> Result<Option<CDefine>, ParseError> {
        self.advance(); // consume PPDefine
        let name = self.expect_ident()?;

        // Function-like only if `(` touches the name: `#define EOF (-1)` has a space.
        let params = if self.peek() == CTokenKind::LParen && !self.peek_token().space_before {
            let mut params = Vec::new();
            self.advance();
            while !matches!(self.peek(), CTokenKind::RParen | CTokenKind::Newline | CTokenKind::Eof) {
                match self.advance() {
                    CTokenKind::Ident(p) => params.push(p),
                    CTokenKind::Ellipsis => params.push("...".to_string()),
                    _ => {}
                }
            }
            self.match_token(&CTokenKind::RParen);
            Some(params)
        } else {
            None
        };

        let mut body = Vec::new();
        while !matches!(self.peek(), CTokenKind::Newline | CTokenKind::Eof) {
            body.push(self.advance());
        }
        // An empty object-like macro is an include guard or a feature flag.
        if body.is_empty() && params.is_none() {
            return Ok(None);
        }
        Ok(Some(CDefine { name, params, body, text: String::new() }))
    }

    /// The tokens up to the first of `stops` outside brackets, evaluated.
    fn const_expr_until(&mut self, stops: &[CTokenKind]) -> Option<i64> {
        let mut toks = Vec::new();
        let mut depth = 0i32;
        loop {
            let t = self.peek();
            if matches!(t, CTokenKind::Eof) || (depth == 0 && stops.contains(&t)) {
                break;
            }
            match t {
                CTokenKind::LParen | CTokenKind::LBracket => depth += 1,
                CTokenKind::RParen | CTokenKind::RBracket => depth -= 1,
                CTokenKind::Newline => {
                    self.advance();
                    continue;
                }
                _ => {}
            }
            if depth < 0 {
                break;
            }
            toks.push(self.advance());
        }
        crate::consteval::eval_int(&toks, &self.consts).and_then(|v| i64::try_from(v).ok())
    }

    fn skip_whitespace_tokens(&mut self) {
        // In our token stream, whitespace is already consumed by the lexer.
        // Only skip newlines in define context (defines are newline-sensitive).
    }

    // ---- Declarations ----

    /// Try to parse a top-level declaration. Returns None if skipped.
    fn try_parse_declaration(&mut self) -> Result<Option<Vec<CDecl>>, ParseError> {
        let _start_pos = self.pos;

        // Collect storage/qualifier prefixes
        let mut is_typedef = false;
        let mut is_static = false;
        let mut is_extern = false;
        let mut is_inline = false;

        loop {
            match self.peek() {
                CTokenKind::Typedef => { is_typedef = true; self.advance(); }
                CTokenKind::Static => { is_static = true; self.advance(); }
                CTokenKind::Extern => {
                    is_extern = true;
                    self.advance();
                    self.skip_newlines();
                    // `extern "C"` or `extern "C++"` — handle ABI blocks
                    if let CTokenKind::StringLit(ref abi) = self.peek() {
                        let abi = abi.clone();
                        if abi == "C++" {
                            // Skip entire extern "C++" block or declaration
                            self.advance();
                            self.skip_newlines();
                            if self.peek() == CTokenKind::LBrace {
                                self.skip_brace_block();
                            } else {
                                self.skip_to_semi_or_brace();
                            }
                            return Ok(None);
                        } else if abi == "C" {
                            // extern "C" { ... } — just skip the "C" and braces,
                            // parse contents normally
                            self.advance();
                            self.skip_newlines();
                            if self.peek() == CTokenKind::LBrace {
                                self.advance(); // skip {
                                // Contents will be parsed as top-level declarations
                                // by the main loop. Just skip the opening brace.
                                return Ok(None);
                            }
                            // extern "C" func_decl — just continue parsing
                        }
                    }
                }
                CTokenKind::Inline => { is_inline = true; self.advance(); }
                CTokenKind::Newline => { self.advance(); }
                _ => break,
            }
        }

        self.skip_newlines();

        // struct/union/enum with possible tag
        match self.peek() {
            CTokenKind::Struct => {
                self.advance();
                return self.parse_struct_or_union_decl(true, is_typedef);
            }
            CTokenKind::Union => {
                self.advance();
                return self.parse_struct_or_union_decl(false, is_typedef);
            }
            CTokenKind::Enum => {
                self.advance();
                return self.parse_enum_decl(is_typedef);
            }
            _ => {}
        }

        // Parse base type
        let base_ty = self.parse_type_specifier()?;

        // If we just got a semi (e.g. bare `int;`), skip
        if self.peek() == CTokenKind::Semi {
            self.advance();
            return Ok(None);
        }

        // Parse declarator(s)
        let (ptr_ty, name) = self.parse_declarator(base_ty.clone())?;

        // Function declaration?
        if self.peek() == CTokenKind::LParen {
            let func = self.parse_function_decl(name, ptr_ty, is_static, is_inline)?;
            // A static function has internal linkage. A static inline one is
            // still usable: the compiler builds a wrapper for it (IN2).
            if func.is_static && !func.is_inline {
                return Ok(None);
            }
            return Ok(Some(vec![CDecl::Function(func)]));
        }

        // Typedef?
        if is_typedef {
            // Handle comma-separated typedefs: typedef int foo, *bar;
            let mut decls = vec![CDecl::Typedef(CTypedef {
                name: name.clone(),
                target: ptr_ty.clone(),
            })];

            while self.match_token(&CTokenKind::Comma) {
                let (next_ty, next_name) = self.parse_declarator(base_ty.clone())?;
                decls.push(CDecl::Typedef(CTypedef {
                    name: next_name,
                    target: next_ty,
                }));
            }
            self.expect(&CTokenKind::Semi)?;
            return Ok(Some(decls));
        }

        // Variable declaration
        // Skip to semicolon (we don't care about initializers)
        self.skip_to_semi_or_brace();
        if is_extern {
            return Ok(Some(vec![CDecl::Variable(CVarDecl {
                name,
                ty: ptr_ty,
                is_extern: true,
                is_const: false,
            })]));
        }

        // Non-extern global var — skip (implementation detail)
        if is_static {
            return Ok(None);
        }

        Ok(Some(vec![CDecl::Variable(CVarDecl {
            name,
            ty: ptr_ty,
            is_extern: false,
            is_const: false,
        })]))
    }

    /// Parse type specifier: `int`, `unsigned long`, `const char`, `size_t`, etc.
    fn parse_type_specifier(&mut self) -> Result<CType, ParseError> {
        self.skip_newlines();
        let mut is_const = false;
        let mut is_signed: Option<bool> = None;
        let mut base: Option<CType> = None;
        let mut long_count = 0u8;
        let mut is_short = false;

        loop {
            match self.peek() {
                CTokenKind::Const => { is_const = true; self.advance(); }
                CTokenKind::Volatile | CTokenKind::Restrict => { self.advance(); }
                CTokenKind::Signed => { is_signed = Some(true); self.advance(); }
                CTokenKind::Unsigned => { is_signed = Some(false); self.advance(); }
                CTokenKind::Void => { base = Some(CType::Void); self.advance(); break; }
                CTokenKind::Char => { base = Some(CType::Char); self.advance(); break; }
                CTokenKind::Short => { is_short = true; self.advance(); }
                CTokenKind::Int => { base = Some(CType::Int); self.advance(); break; }
                CTokenKind::Long => {
                    long_count += 1;
                    self.advance();
                    // `long int`, `long long`, `long double`
                    continue;
                }
                CTokenKind::Float => { base = Some(CType::Float); self.advance(); break; }
                CTokenKind::Double => { base = Some(CType::Double); self.advance(); break; }
                CTokenKind::Bool => { base = Some(CType::Bool); self.advance(); break; }
                CTokenKind::Struct | CTokenKind::Union => {
                    let is_struct = self.advance() == CTokenKind::Struct;
                    base = Some(self.struct_specifier(is_struct)?);
                    break;
                }
                CTokenKind::Enum => {
                    self.advance();
                    let tag = self.expect_ident()?;
                    base = Some(CType::EnumTag(tag));
                    break;
                }
                CTokenKind::Ident(ref name) => {
                    // If we already have type modifiers (unsigned, long, short),
                    // this ident is a declarator name, not a type name.
                    if long_count > 0 || is_short || is_signed.is_some() {
                        break;
                    }
                    let name = name.clone();
                    base = Some(crate::builtin_typedef(&name).unwrap_or(CType::Named(name)));
                    self.advance();
                    break;
                }
                _ => break,
            }
        }

        // Resolve combined specifiers
        let unsigned = is_signed == Some(false);
        let has_base = base.is_some();
        let ty = if let Some(base) = base.filter(|b| *b != CType::Int) {
            match base {
                CType::Char if is_signed == Some(true) => CType::SignedChar,
                CType::Char if unsigned => CType::UnsignedChar,
                CType::Double if long_count > 0 => CType::Double, // long double → f64 (approximate)
                _ => base,
            }
        } else if is_short {
            if is_signed == Some(false) {
                CType::UnsignedShort
            } else {
                CType::Short
            }
        } else if long_count >= 2 {
            if is_signed == Some(false) {
                CType::UnsignedLongLong
            } else {
                CType::LongLong
            }
        } else if long_count == 1 {
            if is_signed == Some(false) {
                CType::UnsignedLong
            } else {
                CType::Long
            }
        } else if is_signed.is_some() || has_base {
            // `int`, `unsigned int`, or bare `signed` / `unsigned`
            if unsigned { CType::UnsignedInt } else { CType::Int }
        } else {
            return Err(ParseError::new("expected type specifier".into(), self.line()));
        };

        // Skip trailing qualifiers
        loop {
            match self.peek() {
                CTokenKind::Const | CTokenKind::Volatile | CTokenKind::Restrict => {
                    if self.peek() == CTokenKind::Const {
                        is_const = true;
                    }
                    self.advance();
                }
                _ => break,
            }
        }

        if is_const {
            Ok(CType::Const(Box::new(ty)))
        } else {
            Ok(ty)
        }
    }

    /// Parse declarator: pointers, name, array suffixes, function pointer.
    /// Returns (final_type, name).
    fn parse_declarator(&mut self, base_ty: CType) -> Result<(CType, String), ParseError> {
        // Pointer prefixes
        let mut ty = base_ty;
        while self.peek() == CTokenKind::Star {
            self.advance();
            ty = CType::Pointer(Box::new(ty));
            // Skip const/volatile/restrict on pointer
            loop {
                match self.peek() {
                    CTokenKind::Const => {
                        self.advance();
                        ty = CType::Const(Box::new(ty));
                    }
                    CTokenKind::Volatile | CTokenKind::Restrict => { self.advance(); }
                    _ => break,
                }
            }
        }

        // Function pointer: (*name)(params)
        if self.peek() == CTokenKind::LParen {
            let saved = self.pos;
            self.advance(); // (
            if self.peek() == CTokenKind::Star {
                self.advance(); // *
                let name = if let CTokenKind::Ident(_) = self.peek() {
                    self.expect_ident()?
                } else {
                    String::new()
                };
                self.expect(&CTokenKind::RParen)?;
                // Parameter list
                self.expect(&CTokenKind::LParen)?;
                let (params, is_variadic) = self.parse_param_list()?;
                self.expect(&CTokenKind::RParen)?;

                let func_ptr = CType::FuncPtr {
                    ret: Box::new(ty),
                    params: params.into_iter().map(|p| p.ty).collect(),
                    is_variadic,
                };
                return Ok((func_ptr, name));
            }
            // Not a function pointer — restore
            self.pos = saved;
        }

        let name = if let CTokenKind::Ident(_) = self.peek() {
            self.expect_ident()?
        } else {
            String::new()
        };

        // Array suffix: name[N]
        while self.peek() == CTokenKind::LBracket {
            self.advance();
            let size = if self.peek() == CTokenKind::RBracket {
                None
            } else {
                match self.const_expr_until(&[CTokenKind::RBracket]) {
                    Some(n) if n >= 0 => Some(n as u64),
                    _ => return Err(ParseError::new(format!("array size of `{}` isn't a constant", name), self.line())),
                }
            };
            self.expect(&CTokenKind::RBracket)?;
            ty = CType::Array(Box::new(ty), size);
        }

        Ok((ty, name))
    }

    /// Parse function parameter list (already past opening paren).
    fn parse_param_list(&mut self) -> Result<(Vec<CParam>, bool), ParseError> {
        let mut params = Vec::new();
        let mut is_variadic = false;

        if self.peek() == CTokenKind::Void {
            // `void)` means no parameters
            let saved = self.pos;
            self.advance();
            if self.peek() == CTokenKind::RParen {
                return Ok((params, false));
            }
            // It's `void *` or similar — restore
            self.pos = saved;
        }

        if self.peek() == CTokenKind::RParen {
            return Ok((params, false));
        }

        loop {
            if self.peek() == CTokenKind::Ellipsis {
                self.advance();
                is_variadic = true;
                break;
            }

            let ty = self.parse_type_specifier()?;
            let (final_ty, name) = self.parse_declarator(ty)?;

            params.push(CParam {
                name: if name.is_empty() { None } else { Some(name) },
                ty: final_ty,
            });

            if !self.match_token(&CTokenKind::Comma) {
                break;
            }
        }

        Ok((params, is_variadic))
    }

    /// Parse function declaration (name and return type already parsed).
    fn parse_function_decl(
        &mut self,
        name: String,
        ret_ty: CType,
        is_static: bool,
        is_inline: bool,
    ) -> Result<CFuncDecl, ParseError> {
        self.expect(&CTokenKind::LParen)?;
        let (params, is_variadic) = self.parse_param_list()?;
        self.expect(&CTokenKind::RParen)?;

        // Skip function body if present
        if self.peek() == CTokenKind::LBrace {
            self.skip_brace_block();
        } else {
            // Might have trailing attributes, then semicolon
            self.skip_newlines();
            self.match_token(&CTokenKind::Semi);
        }

        Ok(CFuncDecl {
            name,
            params,
            ret_ty,
            is_variadic,
            is_static,
            is_inline,
        })
    }

    fn skip_brace_block(&mut self) {
        let mut depth = 0u32;
        loop {
            if self.at_eof() {
                break;
            }
            match self.peek() {
                CTokenKind::LBrace => { depth += 1; self.advance(); }
                CTokenKind::RBrace => {
                    depth -= 1;
                    self.advance();
                    if depth == 0 { break; }
                }
                _ => { self.advance(); }
            }
        }
    }

    /// Parse struct/union declaration. Handles:
    /// - `struct tag { fields } var;`
    /// - `struct tag;` (forward)
    /// - `typedef struct { ... } name;`
    /// - `typedef struct tag { ... } name;`
    fn parse_struct_or_union_decl(
        &mut self,
        is_struct: bool,
        is_typedef: bool,
    ) -> Result<Option<Vec<CDecl>>, ParseError> {
        let tag = if let CTokenKind::Ident(_) = self.peek() {
            Some(self.expect_ident()?)
        } else {
            None
        };

        // Forward declaration: `struct tag;`
        if self.peek() == CTokenKind::Semi {
            self.advance();
            if !is_typedef {
                let decl = CStructDecl {
                    tag,
                    fields: Vec::new(),
                    is_forward: true,
                    naming: CNaming::Tag,
                };
                return Ok(Some(vec![if is_struct {
                    CDecl::Struct(decl)
                } else {
                    CDecl::Union(decl)
                }]));
            }
            return Ok(None);
        }

        // No body — this is a type specifier in a declaration, not a definition.
        // e.g., `struct foo *ptr;`
        if self.peek() != CTokenKind::LBrace {
            if is_typedef {
                // `typedef struct tag name;`
                let name = self.expect_ident()?;
                self.expect(&CTokenKind::Semi)?;
                let tag_name = tag.unwrap_or_else(|| name.clone());
                return Ok(Some(vec![CDecl::Typedef(CTypedef {
                    name,
                    target: if is_struct {
                        CType::StructTag(tag_name)
                    } else {
                        CType::UnionTag(tag_name)
                    },
                })]));
            }
            // `struct tag <declarator>` — variable of struct type
            // Rewind isn't clean here; skip to semicolon.
            self.skip_to_semi_or_brace();
            return Ok(None);
        }

        // Parse body
        self.expect(&CTokenKind::LBrace)?;
        self.outer.push(tag.clone());
        let fields = self.parse_struct_fields();
        self.outer.pop();
        let fields = fields?;
        self.expect(&CTokenKind::RBrace)?;

        let decl = CStructDecl {
            tag: tag.clone(),
            fields,
            is_forward: false,
            naming: CNaming::Tag,
        };

        let mut decls = Vec::new();

        // Always emit the struct/union itself (if it has a tag)
        if tag.is_some() {
            decls.push(if is_struct {
                CDecl::Struct(decl.clone())
            } else {
                CDecl::Union(decl.clone())
            });
        }

        // `typedef struct { ... } name;` or `typedef struct tag { ... } name;`
        if is_typedef {
            let anon_placeholder = format!("__anon_{}", self.pos);
            // Parse typedef name(s)
            let mut first = true;
            loop {
                self.skip_newlines();
                if self.peek() == CTokenKind::Semi {
                    self.advance();
                    break;
                }
                if !first {
                    self.expect(&CTokenKind::Comma)?;
                }
                first = false;

                // Handle pointer typedefs: typedef struct foo *foo_ptr;
                let mut td_ty = if let Some(ref t) = tag {
                    if is_struct {
                        CType::StructTag(t.clone())
                    } else {
                        CType::UnionTag(t.clone())
                    }
                } else {
                    // Anonymous struct — a placeholder until a typedef name
                    // below gives it one.
                    CType::Named(anon_placeholder.clone())
                };

                while self.peek() == CTokenKind::Star {
                    self.advance();
                    td_ty = CType::Pointer(Box::new(td_ty));
                }

                let name = self.expect_ident()?;
                decls.push(CDecl::Typedef(CTypedef { name, target: td_ty }));
            }

            // If no tag, the first typedef name becomes the struct's tag.
            if tag.is_none() && !decls.is_empty() {
                if let CDecl::Typedef(ref td) = decls[0] {
                    let adopted = td.name.clone();
                    let named_decl = CStructDecl {
                        tag: Some(adopted.clone()),
                        fields: decl.fields,
                        is_forward: false,
                        naming: CNaming::Typedef,
                    };
                    // The struct has a name now, so every typedef that pointed
                    // at the placeholder points at it instead. The one that
                    // donated the name then reads `typedef Rect Rect` — drop
                    // it, or it shadows the struct under the same name and the
                    // fields become unreachable.
                    for d in decls.iter_mut() {
                        if let CDecl::Typedef(td) = d {
                            retarget_anon(&mut td.target, &anon_placeholder, &adopted, is_struct);
                        }
                    }
                    decls.retain(|d| match d {
                        CDecl::Typedef(td) => !matches!(
                            &td.target,
                            CType::StructTag(t) | CType::UnionTag(t) if *t == td.name
                        ),
                        _ => true,
                    });
                    decls.insert(0, if is_struct {
                        CDecl::Struct(named_decl)
                    } else {
                        CDecl::Union(named_decl)
                    });
                }
            }
        } else {
            // Might have variable declarations after the struct body
            self.skip_newlines();
            self.match_token(&CTokenKind::Semi);
        }

        Ok(Some(decls))
    }

    /// `struct tag`, or a struct defined where a type is expected:
    /// `struct tag { … }` or `struct { … }` as a field's type. A body becomes
    /// a declaration of its own, emitted ahead of the one that contains it; an
    /// anonymous one is named after its container.
    fn struct_specifier(&mut self, is_struct: bool) -> Result<CType, ParseError> {
        let tag = if let CTokenKind::Ident(_) = self.peek() {
            Some(self.expect_ident()?)
        } else {
            None
        };
        let as_type = |t: String| if is_struct { CType::StructTag(t) } else { CType::UnionTag(t) };
        if self.peek() != CTokenKind::LBrace {
            return match tag {
                Some(t) => Ok(as_type(t)),
                None => Err(ParseError::new("struct without a tag or a body".into(), self.line())),
            };
        }
        let naming = if tag.is_some() { CNaming::Tag } else { CNaming::Anonymous };
        let tag = tag.unwrap_or_else(|| {
            let outer = self.outer.iter().rev().flatten().next().cloned().unwrap_or_else(|| "anon".into());
            self.anon += 1;
            format!("{}_anon{}", outer, self.anon - 1)
        });
        let line = self.line();
        self.advance();
        self.outer.push(Some(tag.clone()));
        let fields = self.parse_struct_fields();
        self.outer.pop();
        let fields = fields?;
        self.expect(&CTokenKind::RBrace)?;
        let decl = CStructDecl { tag: Some(tag.clone()), fields, is_forward: false, naming };
        self.nested.push(CItem {
            decl: if is_struct { CDecl::Struct(decl) } else { CDecl::Union(decl) },
            file: self.file.clone(),
            line,
        });
        Ok(as_type(tag))
    }

    fn parse_struct_fields(&mut self) -> Result<Vec<CField>, ParseError> {
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == CTokenKind::RBrace || self.at_eof() {
                break;
            }

            let ty = match self.parse_type_specifier() {
                Ok(ty) => ty,
                Err(_) => {
                    self.skip_to_semi_or_brace();
                    continue;
                }
            };

            // Parse one or more field declarators
            loop {
                let (field_ty, mut name) = self.parse_declarator(ty.clone())?;

                let bit_width = if self.match_token(&CTokenKind::Colon) {
                    match self.const_expr_until(&[CTokenKind::Comma, CTokenKind::Semi]) {
                        Some(w) if w >= 0 => Some(w as u32),
                        _ => return Err(ParseError::new("bitfield width isn't a constant".into(), self.line())),
                    }
                } else {
                    None
                };

                // An unnamed member still takes room: a nested struct or union
                // whose fields C lets you reach directly, or bitfield padding.
                // Rask reaches them through a name.
                let unnamed = name.is_empty();
                if unnamed {
                    match (&field_ty, bit_width) {
                        (_, Some(0)) => {}
                        (_, Some(_)) => name = format!("pad{}", fields.len()),
                        (CType::StructTag(_) | CType::UnionTag(_), None) => name = format!("anon{}", fields.len()),
                        _ => {}
                    }
                }
                if !name.is_empty() {
                    fields.push(CField { name, ty: field_ty, bit_width, unnamed });
                }

                if !self.match_token(&CTokenKind::Comma) {
                    break;
                }
            }

            self.expect(&CTokenKind::Semi)?;
        }
        Ok(fields)
    }

    /// Parse enum declaration.
    fn parse_enum_decl(
        &mut self,
        is_typedef: bool,
    ) -> Result<Option<Vec<CDecl>>, ParseError> {
        let tag = if let CTokenKind::Ident(_) = self.peek() {
            Some(self.expect_ident()?)
        } else {
            None
        };

        // Forward declaration
        if self.peek() == CTokenKind::Semi {
            self.advance();
            return Ok(Some(vec![CDecl::Enum(CEnumDecl {
                tag,
                variants: Vec::new(),
                is_forward: true,
            })]));
        }

        if self.peek() != CTokenKind::LBrace {
            if is_typedef {
                let name = self.expect_ident()?;
                self.expect(&CTokenKind::Semi)?;
                return Ok(Some(vec![CDecl::Typedef(CTypedef {
                    name,
                    target: CType::EnumTag(tag.unwrap_or_default()),
                })]));
            }
            self.skip_to_semi_or_brace();
            return Ok(None);
        }

        self.expect(&CTokenKind::LBrace)?;
        let mut variants = Vec::new();
        // `None` once an explicit value couldn't be evaluated: the ones
        // counting up from it are unknown too, until the next one that can.
        let mut next_value: Option<i64> = Some(0);

        loop {
            self.skip_newlines();
            if self.peek() == CTokenKind::RBrace || self.at_eof() {
                break;
            }

            let name = self.expect_ident()?;
            let value = if self.match_token(&CTokenKind::Eq) {
                let v = self.const_expr_until(&[CTokenKind::Comma, CTokenKind::RBrace]);
                self.skip_to_comma_or_brace();
                v
            } else {
                next_value
            };
            next_value = value.and_then(|v| v.checked_add(1));
            if let Some(v) = value {
                self.consts.enumerators.insert(name.clone(), v);
            }

            variants.push(CEnumVariant { name, value });
            self.skip_newlines();
            self.match_token(&CTokenKind::Comma);
        }
        self.expect(&CTokenKind::RBrace)?;

        let mut decls = Vec::new();
        let decl = CEnumDecl { tag: tag.clone(), variants, is_forward: false };

        if tag.is_some() {
            decls.push(CDecl::Enum(decl.clone()));
        }

        if is_typedef {
            loop {
                self.skip_newlines();
                if self.peek() == CTokenKind::Semi {
                    self.advance();
                    break;
                }
                let name = self.expect_ident()?;
                decls.push(CDecl::Typedef(CTypedef {
                    name: name.clone(),
                    target: CType::EnumTag(tag.clone().unwrap_or(name)),
                }));
                if !self.match_token(&CTokenKind::Comma) {
                    self.expect(&CTokenKind::Semi)?;
                    break;
                }
            }

            if tag.is_none() && decls.is_empty() {
                // Unusual: `typedef enum { ... };` with no name
            } else if tag.is_none() {
                // Name the enum after first typedef
                if let Some(CDecl::Typedef(ref td)) = decls.first() {
                    let named = CEnumDecl {
                        tag: Some(td.name.clone()),
                        variants: decl.variants,
                        is_forward: false,
                    };
                    decls.insert(0, CDecl::Enum(named));
                }
            }
        } else {
            self.skip_newlines();
            self.match_token(&CTokenKind::Semi);
            if tag.is_none() {
                decls.push(CDecl::Enum(decl));
            }
        }

        Ok(Some(decls))
    }

    fn skip_to_comma_or_brace(&mut self) {
        loop {
            if self.at_eof() { break; }
            match self.peek() {
                CTokenKind::Comma | CTokenKind::RBrace => break,
                _ => { self.advance(); }
            }
        }
    }
}

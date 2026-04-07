// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Translates parsed C declarations into Rask-level intermediate representations.
//!
//! Follows the type mapping from struct.c-interop (TM1–TM3):
//! - C `int` → `c_int`, `unsigned long` → `c_ulong`, etc.
//! - `T*` → `*T`, `void*` → `*void`, `const char*` → `*u8`
//! - `#define FOO 42` → const FOO: c_int = 42
//!
//! Produces self-contained types that the resolver consumes without depending
//! on rask-ast. Type names are Rask syntax strings (`*u8`, `c_int`, etc.).

use crate::*;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A translated Rask declaration from C.
#[derive(Debug, Clone, PartialEq)]
pub enum RaskCDecl {
    Function(RaskCFunc),
    Struct(RaskCStruct),
    Union(RaskCStruct),
    Enum(RaskCEnum),
    Const(RaskCConst),
    TypeAlias(RaskCTypeAlias),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCFunc {
    pub name: String,
    pub params: Vec<RaskCParam>,
    /// Rask type as string. Empty string means no return type (C `void`).
    pub ret_ty: String,
    pub is_variadic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCParam {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCStruct {
    pub name: String,
    pub fields: Vec<RaskCField>,
    /// Forward declaration = opaque type (pointer-only access).
    pub is_opaque: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCField {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCEnum {
    pub name: String,
    pub variants: Vec<(String, Option<i64>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCConst {
    pub name: String,
    pub ty: String,
    /// Literal representation of the value.
    pub value_repr: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RaskCTypeAlias {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct TranslateResult {
    pub decls: Vec<RaskCDecl>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Translate parsed C declarations into Rask intermediate declarations.
///
/// Symbols whose names appear in `hiding` are excluded. Static functions are
/// skipped (internal linkage). Mutable globals produce a warning.
pub fn translate(result: &CParseResult, hiding: &[String]) -> TranslateResult {
    let mut decls = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Collect typedefs for potential resolution.
    let typedefs: std::collections::HashMap<String, CType> = result
        .decls
        .iter()
        .filter_map(|d| {
            if let CDecl::Typedef(td) = d {
                Some((td.name.clone(), td.target.clone()))
            } else {
                None
            }
        })
        .collect();

    let translator = Translator { typedefs: &typedefs };

    // Forward parse warnings.
    for w in &result.warnings {
        warnings.push(format!("line {}: {}", w.line, w.message));
    }

    for cdecl in &result.decls {
        let name = decl_name(cdecl);
        if let Some(n) = &name {
            if hiding.iter().any(|h| h == n) {
                continue;
            }
        }

        match cdecl {
            CDecl::Function(f) => {
                // Static functions have internal linkage — not importable (struct.c-interop edge case).
                if f.is_static {
                    continue;
                }
                decls.push(RaskCDecl::Function(translator.translate_func(f)));
            }
            CDecl::Struct(s) => {
                if let Some(rask) = translator.translate_struct(s) {
                    decls.push(RaskCDecl::Struct(rask));
                }
            }
            CDecl::Union(u) => {
                if let Some(rask) = translator.translate_struct(u) {
                    decls.push(RaskCDecl::Union(rask));
                }
            }
            CDecl::Enum(e) => {
                if let Some(rask) = translator.translate_enum(e) {
                    decls.push(RaskCDecl::Enum(rask));
                }
            }
            CDecl::Typedef(td) => {
                // Skip self-referencing typedefs (`typedef struct foo foo`).
                match &td.target {
                    CType::StructTag(t) | CType::UnionTag(t) | CType::EnumTag(t)
                        if t == &td.name =>
                    {
                        continue;
                    }
                    _ => {}
                }
                decls.push(RaskCDecl::TypeAlias(translator.translate_typedef(td)));
            }
            CDecl::Define(d) => {
                if let Some(c) = translator.translate_define(d, &mut warnings) {
                    decls.push(RaskCDecl::Const(c));
                }
            }
            CDecl::Variable(v) => {
                if v.is_const {
                    decls.push(RaskCDecl::Const(RaskCConst {
                        name: v.name.clone(),
                        ty: translator.translate_type(&v.ty),
                        value_repr: String::new(), // extern — no initializer from header
                    }));
                } else {
                    warnings.push(format!(
                        "skipping mutable global `{}`: mutable C globals are not safe to import",
                        v.name
                    ));
                }
            }
        }
    }

    TranslateResult { decls, warnings }
}

// ---------------------------------------------------------------------------
// Internal translator
// ---------------------------------------------------------------------------

struct Translator<'a> {
    typedefs: &'a std::collections::HashMap<String, CType>,
}

impl<'a> Translator<'a> {
    // -- Type translation ---------------------------------------------------

    /// Convert a `CType` to its Rask type string representation.
    fn translate_type(&self, ty: &CType) -> String {
        match ty {
            CType::Void => "void".to_string(),
            CType::Char => "c_char".to_string(),
            CType::SignedChar => "i8".to_string(),
            CType::UnsignedChar => "u8".to_string(),
            CType::Short => "c_short".to_string(),
            CType::UnsignedShort => "c_ushort".to_string(),
            CType::Int => "c_int".to_string(),
            CType::UnsignedInt => "c_uint".to_string(),
            CType::Long => "c_long".to_string(),
            CType::UnsignedLong => "c_ulong".to_string(),
            CType::LongLong => "c_longlong".to_string(),
            CType::UnsignedLongLong => "c_ulonglong".to_string(),
            CType::Float => "f32".to_string(),
            CType::Double => "f64".to_string(),
            CType::Bool => "bool".to_string(),
            CType::SizeT => "c_size".to_string(),
            CType::SSizeT => "c_ssize".to_string(),
            CType::FixedInt { bits, signed } => {
                if *signed {
                    format!("i{}", bits)
                } else {
                    format!("u{}", bits)
                }
            }
            CType::IntPtr { signed } => {
                if *signed { "isize" } else { "usize" }.to_string()
            }
            CType::Pointer(inner) => self.translate_pointer(inner),
            CType::Const(inner) => {
                // Top-level const on a non-pointer: strip it. Rask handles
                // mutability through const/let, not type qualifiers.
                self.translate_type(inner)
            }
            CType::Array(elem, Some(size)) => {
                let elem_str = self.translate_type(elem);
                format!("[{}; {}]", elem_str, size)
            }
            CType::Array(elem, None) => {
                // Unsized array decays to pointer.
                let elem_str = self.translate_type(elem);
                format!("*{}", elem_str)
            }
            CType::Named(name) => {
                match name.as_str() {
                    "FILE" | "va_list" => "*void".to_string(),
                    _ => name.clone(),
                }
            }
            CType::StructTag(tag) => tag.clone(),
            CType::UnionTag(tag) => tag.clone(),
            CType::EnumTag(tag) => tag.clone(),
            CType::FuncPtr {
                ret,
                params,
                is_variadic,
            } => self.translate_func_ptr(ret, params, *is_variadic),
        }
    }

    /// Translate `T*` → `*T`, with special cases:
    /// - `const char*` → `*u8` (C string convention)
    /// - `void*` → `*void`
    /// - Strips outer `const` on pointer targets.
    fn translate_pointer(&self, inner: &CType) -> String {
        let stripped = strip_const(inner);
        match stripped {
            CType::Char | CType::SignedChar | CType::UnsignedChar => "*u8".to_string(),
            CType::Void => "*void".to_string(),
            CType::FuncPtr {
                ret,
                params,
                is_variadic,
            } => {
                // Pointer-to-function-pointer collapses to the func ptr.
                self.translate_func_ptr(ret, params, *is_variadic)
            }
            other => {
                let inner_ty = self.translate_type(other);
                format!("*{}", inner_ty)
            }
        }
    }

    /// Translate a C function pointer to Rask `*func(...) -> R` syntax.
    fn translate_func_ptr(&self, ret: &CType, params: &[CType], is_variadic: bool) -> String {
        let param_strs: Vec<String> = params.iter().map(|p| self.translate_type(p)).collect();
        let mut sig = String::from("*func(");
        if is_variadic {
            if !param_strs.is_empty() {
                sig.push_str(&param_strs.join(", "));
                sig.push_str(", ...");
            } else {
                sig.push_str("...");
            }
        } else {
            sig.push_str(&param_strs.join(", "));
        }
        sig.push(')');

        let ret_str = self.translate_type(ret);
        if ret_str != "void" {
            sig.push_str(" -> ");
            sig.push_str(&ret_str);
        }
        sig
    }

    // -- Declaration translators --------------------------------------------

    fn translate_func(&self, f: &CFuncDecl) -> RaskCFunc {
        let params: Vec<RaskCParam> = f
            .params
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                // C `f(void)` means no params — skip single unnamed void param.
                if f.params.len() == 1 && p.name.is_none() && p.ty == CType::Void {
                    return None;
                }
                let name = p.name.clone().unwrap_or_else(|| format!("p{}", i));
                Some(RaskCParam {
                    name,
                    ty: self.translate_type(&p.ty),
                })
            })
            .collect();

        let ret_ty = match &f.ret_ty {
            CType::Void => String::new(),
            other => self.translate_type(other),
        };

        RaskCFunc {
            name: f.name.clone(),
            params,
            ret_ty,
            is_variadic: f.is_variadic,
        }
    }

    fn translate_struct(&self, s: &CStructDecl) -> Option<RaskCStruct> {
        // Anonymous structs without a tag are not importable.
        let name = s.tag.as_ref()?;

        let fields = s
            .fields
            .iter()
            .map(|f| RaskCField {
                name: f.name.clone(),
                ty: self.translate_type(&f.ty),
            })
            .collect();

        Some(RaskCStruct {
            name: name.clone(),
            fields,
            is_opaque: s.is_forward,
        })
    }

    fn translate_enum(&self, e: &CEnumDecl) -> Option<RaskCEnum> {
        let name = e.tag.as_ref()?;
        if e.is_forward {
            return None;
        }

        let variants = e.variants.iter().map(|v| (v.name.clone(), v.value)).collect();

        Some(RaskCEnum {
            name: name.clone(),
            variants,
        })
    }

    fn translate_typedef(&self, td: &CTypedef) -> RaskCTypeAlias {
        RaskCTypeAlias {
            name: td.name.clone(),
            target: self.translate_type(&td.target),
        }
    }

    fn translate_define(
        &self,
        d: &CDefine,
        warnings: &mut Vec<String>,
    ) -> Option<RaskCConst> {
        match &d.kind {
            CDefineKind::Integer(v) => Some(RaskCConst {
                name: d.name.clone(),
                ty: "c_int".to_string(),
                value_repr: v.to_string(),
            }),
            CDefineKind::UnsignedInteger(v) => Some(RaskCConst {
                name: d.name.clone(),
                ty: "c_uint".to_string(),
                value_repr: v.to_string(),
            }),
            CDefineKind::Float(v) => Some(RaskCConst {
                name: d.name.clone(),
                ty: "f64".to_string(),
                value_repr: format!("{}", v),
            }),
            CDefineKind::String(s) => Some(RaskCConst {
                name: d.name.clone(),
                ty: "*u8".to_string(),
                value_repr: format!(
                    "c\"{}\"",
                    s.replace('\\', "\\\\").replace('"', "\\\"")
                ),
            }),
            CDefineKind::FunctionMacro { .. } => {
                warnings.push(format!(
                    "skipping function-like macro `{}`",
                    d.name
                ));
                None
            }
            CDefineKind::Unparseable => {
                warnings.push(format!(
                    "skipping unparseable macro `{}`",
                    d.name
                ));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip one layer of `Const` wrapper.
fn strip_const(ty: &CType) -> &CType {
    match ty {
        CType::Const(inner) => inner,
        other => other,
    }
}

/// Extract the primary name of a C declaration for hiding checks.
fn decl_name(decl: &CDecl) -> Option<String> {
    match decl {
        CDecl::Function(f) => Some(f.name.clone()),
        CDecl::Struct(s) => s.tag.clone(),
        CDecl::Union(u) => u.tag.clone(),
        CDecl::Enum(e) => e.tag.clone(),
        CDecl::Typedef(td) => Some(td.name.clone()),
        CDecl::Define(d) => Some(d.name.clone()),
        CDecl::Variable(v) => Some(v.name.clone()),
    }
}

// ---------------------------------------------------------------------------
// Rendering (for debugging / `rask c-header` command)
// ---------------------------------------------------------------------------

/// Render translated declarations as Rask source text.
pub fn render_rask(result: &TranslateResult) -> String {
    let mut out = String::new();

    for decl in &result.decls {
        match decl {
            RaskCDecl::Function(f) => {
                out.push_str("extern \"C\" func ");
                out.push_str(&f.name);
                out.push('(');
                let mut first = true;
                for p in &f.params {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(&p.name);
                    out.push_str(": ");
                    out.push_str(&p.ty);
                }
                if f.is_variadic {
                    if !first {
                        out.push_str(", ");
                    }
                    out.push_str("...");
                }
                out.push(')');
                if !f.ret_ty.is_empty() {
                    out.push_str(" -> ");
                    out.push_str(&f.ret_ty);
                }
                out.push('\n');
            }
            RaskCDecl::Struct(s) | RaskCDecl::Union(s) => {
                let keyword = if matches!(decl, RaskCDecl::Union(_)) {
                    "union"
                } else {
                    "struct"
                };
                out.push_str("extern \"C\" ");
                out.push_str(keyword);
                out.push(' ');
                out.push_str(&s.name);
                if s.is_opaque {
                    out.push('\n');
                } else {
                    out.push_str(" {\n");
                    for f in &s.fields {
                        out.push_str("    ");
                        out.push_str(&f.name);
                        out.push_str(": ");
                        out.push_str(&f.ty);
                        out.push('\n');
                    }
                    out.push_str("}\n");
                }
            }
            RaskCDecl::Enum(e) => {
                out.push_str("extern \"C\" enum ");
                out.push_str(&e.name);
                out.push_str(" {\n");
                for (vname, value) in &e.variants {
                    out.push_str("    ");
                    out.push_str(vname);
                    if let Some(v) = value {
                        out.push_str(&format!(" = {}", v));
                    }
                    out.push('\n');
                }
                out.push_str("}\n");
            }
            RaskCDecl::Const(c) => {
                out.push_str("const ");
                out.push_str(&c.name);
                out.push_str(": ");
                out.push_str(&c.ty);
                if !c.value_repr.is_empty() {
                    out.push_str(" = ");
                    out.push_str(&c.value_repr);
                }
                out.push('\n');
            }
            RaskCDecl::TypeAlias(a) => {
                out.push_str("type ");
                out.push_str(&a.name);
                out.push_str(" = ");
                out.push_str(&a.target);
                out.push('\n');
            }
        }
    }

    for w in &result.warnings {
        out.push_str("// WARNING: ");
        out.push_str(w);
        out.push('\n');
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_result(decls: Vec<CDecl>) -> CParseResult {
        CParseResult {
            decls,
            warnings: vec![],
        }
    }

    #[test]
    fn translate_basic_function() {
        let result = empty_result(vec![CDecl::Function(CFuncDecl {
            name: "puts".into(),
            params: vec![CParam {
                name: Some("s".into()),
                ty: CType::Pointer(Box::new(CType::Const(Box::new(CType::Char)))),
            }],
            ret_ty: CType::Int,
            is_variadic: false,
            is_static: false,
            is_inline: false,
        })]);

        let out = translate(&result, &[]);
        assert_eq!(out.decls.len(), 1);
        match &out.decls[0] {
            RaskCDecl::Function(f) => {
                assert_eq!(f.name, "puts");
                assert_eq!(f.params.len(), 1);
                assert_eq!(f.params[0].name, "s");
                assert_eq!(f.params[0].ty, "*u8");
                assert_eq!(f.ret_ty, "c_int");
                assert!(!f.is_variadic);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_void_return() {
        let result = empty_result(vec![CDecl::Function(CFuncDecl {
            name: "free".into(),
            params: vec![CParam {
                name: Some("ptr".into()),
                ty: CType::Pointer(Box::new(CType::Void)),
            }],
            ret_ty: CType::Void,
            is_variadic: false,
            is_static: false,
            is_inline: false,
        })]);

        let out = translate(&result, &[]);
        match &out.decls[0] {
            RaskCDecl::Function(f) => {
                assert_eq!(f.ret_ty, "");
                assert_eq!(f.params[0].ty, "*void");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_void_params() {
        // C `int getpid(void)` — single void param means no params.
        let result = empty_result(vec![CDecl::Function(CFuncDecl {
            name: "getpid".into(),
            params: vec![CParam {
                name: None,
                ty: CType::Void,
            }],
            ret_ty: CType::Int,
            is_variadic: false,
            is_static: false,
            is_inline: false,
        })]);

        let out = translate(&result, &[]);
        match &out.decls[0] {
            RaskCDecl::Function(f) => {
                assert!(f.params.is_empty());
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_variadic_function() {
        let result = empty_result(vec![CDecl::Function(CFuncDecl {
            name: "printf".into(),
            params: vec![CParam {
                name: Some("fmt".into()),
                ty: CType::Pointer(Box::new(CType::Const(Box::new(CType::Char)))),
            }],
            ret_ty: CType::Int,
            is_variadic: true,
            is_static: false,
            is_inline: false,
        })]);

        let out = translate(&result, &[]);
        match &out.decls[0] {
            RaskCDecl::Function(f) => {
                assert!(f.is_variadic);
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn skip_static_function() {
        let result = empty_result(vec![CDecl::Function(CFuncDecl {
            name: "internal_helper".into(),
            params: vec![],
            ret_ty: CType::Void,
            is_variadic: false,
            is_static: true,
            is_inline: false,
        })]);

        let out = translate(&result, &[]);
        assert!(out.decls.is_empty());
    }

    #[test]
    fn translate_struct_and_opaque() {
        let result = empty_result(vec![
            CDecl::Struct(CStructDecl {
                tag: Some("point".into()),
                fields: vec![
                    CField { name: "x".into(), ty: CType::Double, bit_width: None },
                    CField { name: "y".into(), ty: CType::Double, bit_width: None },
                ],
                is_forward: false,
            }),
            CDecl::Struct(CStructDecl {
                tag: Some("sqlite3".into()),
                fields: vec![],
                is_forward: true,
            }),
        ]);

        let out = translate(&result, &[]);
        assert_eq!(out.decls.len(), 2);

        match &out.decls[0] {
            RaskCDecl::Struct(s) => {
                assert_eq!(s.name, "point");
                assert!(!s.is_opaque);
                assert_eq!(s.fields.len(), 2);
                assert_eq!(s.fields[0].ty, "f64");
            }
            other => panic!("expected Struct, got {:?}", other),
        }

        match &out.decls[1] {
            RaskCDecl::Struct(s) => {
                assert_eq!(s.name, "sqlite3");
                assert!(s.is_opaque);
            }
            other => panic!("expected Struct, got {:?}", other),
        }
    }

    #[test]
    fn translate_enum() {
        let result = empty_result(vec![CDecl::Enum(CEnumDecl {
            tag: Some("color".into()),
            variants: vec![
                CEnumVariant { name: "RED".into(), value: Some(0) },
                CEnumVariant { name: "GREEN".into(), value: Some(1) },
                CEnumVariant { name: "BLUE".into(), value: Some(2) },
            ],
            is_forward: false,
        })]);

        let out = translate(&result, &[]);
        match &out.decls[0] {
            RaskCDecl::Enum(e) => {
                assert_eq!(e.name, "color");
                assert_eq!(e.variants.len(), 3);
                assert_eq!(e.variants[0], ("RED".into(), Some(0)));
            }
            other => panic!("expected Enum, got {:?}", other),
        }
    }

    #[test]
    fn translate_typedef() {
        let result = empty_result(vec![CDecl::Typedef(CTypedef {
            name: "size_t".into(),
            target: CType::UnsignedLong,
        })]);

        let out = translate(&result, &[]);
        match &out.decls[0] {
            RaskCDecl::TypeAlias(a) => {
                assert_eq!(a.name, "size_t");
                assert_eq!(a.target, "c_ulong");
            }
            other => panic!("expected TypeAlias, got {:?}", other),
        }
    }

    #[test]
    fn translate_defines() {
        let result = empty_result(vec![
            CDecl::Define(CDefine {
                name: "EXIT_SUCCESS".into(),
                kind: CDefineKind::Integer(0),
            }),
            CDecl::Define(CDefine {
                name: "VERSION".into(),
                kind: CDefineKind::String("1.0".into()),
            }),
            CDecl::Define(CDefine {
                name: "MAX".into(),
                kind: CDefineKind::FunctionMacro {
                    params: vec!["a".into(), "b".into()],
                },
            }),
            CDecl::Define(CDefine {
                name: "WEIRD".into(),
                kind: CDefineKind::Unparseable,
            }),
        ]);

        let out = translate(&result, &[]);
        assert_eq!(out.decls.len(), 2); // integer + string only

        match &out.decls[0] {
            RaskCDecl::Const(c) => {
                assert_eq!(c.name, "EXIT_SUCCESS");
                assert_eq!(c.ty, "c_int");
                assert_eq!(c.value_repr, "0");
            }
            other => panic!("expected Const, got {:?}", other),
        }

        // Two warnings for skipped macros.
        assert_eq!(
            out.warnings
                .iter()
                .filter(|w| w.contains("skipping"))
                .count(),
            2
        );
    }

    #[test]
    fn translate_variable_const_vs_mutable() {
        let result = empty_result(vec![
            CDecl::Variable(CVarDecl {
                name: "errno".into(),
                ty: CType::Int,
                is_extern: true,
                is_const: false,
            }),
            CDecl::Variable(CVarDecl {
                name: "STDOUT".into(),
                ty: CType::Int,
                is_extern: true,
                is_const: true,
            }),
        ]);

        let out = translate(&result, &[]);
        // Mutable skipped, const kept.
        assert_eq!(out.decls.len(), 1);
        match &out.decls[0] {
            RaskCDecl::Const(c) => assert_eq!(c.name, "STDOUT"),
            other => panic!("expected Const, got {:?}", other),
        }
        assert!(out.warnings.iter().any(|w| w.contains("errno")));
    }

    #[test]
    fn hiding_filters_symbols() {
        let result = empty_result(vec![
            CDecl::Function(CFuncDecl {
                name: "keep".into(),
                params: vec![],
                ret_ty: CType::Void,
                is_variadic: false,
                is_static: false,
                is_inline: false,
            }),
            CDecl::Function(CFuncDecl {
                name: "hide_me".into(),
                params: vec![],
                ret_ty: CType::Void,
                is_variadic: false,
                is_static: false,
                is_inline: false,
            }),
        ]);

        let out = translate(&result, &["hide_me".into()]);
        assert_eq!(out.decls.len(), 1);
        match &out.decls[0] {
            RaskCDecl::Function(f) => assert_eq!(f.name, "keep"),
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn translate_fixed_int_types() {
        let t = Translator {
            typedefs: &std::collections::HashMap::new(),
        };
        assert_eq!(
            t.translate_type(&CType::FixedInt { bits: 8, signed: true }),
            "i8"
        );
        assert_eq!(
            t.translate_type(&CType::FixedInt {
                bits: 32,
                signed: false
            }),
            "u32"
        );
        assert_eq!(
            t.translate_type(&CType::FixedInt {
                bits: 64,
                signed: true
            }),
            "i64"
        );
    }

    #[test]
    fn translate_func_ptr_type() {
        let t = Translator {
            typedefs: &std::collections::HashMap::new(),
        };
        let ty = CType::FuncPtr {
            ret: Box::new(CType::Int),
            params: vec![CType::Pointer(Box::new(CType::Void)), CType::Int],
            is_variadic: false,
        };
        assert_eq!(t.translate_type(&ty), "*func(*void, c_int) -> c_int");
    }

    #[test]
    fn translate_array_type() {
        let t = Translator {
            typedefs: &std::collections::HashMap::new(),
        };
        assert_eq!(
            t.translate_type(&CType::Array(Box::new(CType::Int), Some(16))),
            "[c_int; 16]"
        );
        assert_eq!(
            t.translate_type(&CType::Array(Box::new(CType::Char), None)),
            "*c_char"
        );
    }

    #[test]
    fn translate_unnamed_params() {
        let result = empty_result(vec![CDecl::Function(CFuncDecl {
            name: "memcpy".into(),
            params: vec![
                CParam {
                    name: None,
                    ty: CType::Pointer(Box::new(CType::Void)),
                },
                CParam {
                    name: None,
                    ty: CType::Pointer(Box::new(CType::Const(Box::new(CType::Void)))),
                },
                CParam {
                    name: None,
                    ty: CType::SizeT,
                },
            ],
            ret_ty: CType::Pointer(Box::new(CType::Void)),
            is_variadic: false,
            is_static: false,
            is_inline: false,
        })]);

        let out = translate(&result, &[]);
        match &out.decls[0] {
            RaskCDecl::Function(f) => {
                assert_eq!(f.params[0].name, "p0");
                assert_eq!(f.params[0].ty, "*void");
                assert_eq!(f.params[1].name, "p1");
                assert_eq!(f.params[1].ty, "*void"); // const stripped on pointer target
                assert_eq!(f.params[2].name, "p2");
                assert_eq!(f.params[2].ty, "c_size");
                assert_eq!(f.ret_ty, "*void");
            }
            other => panic!("expected Function, got {:?}", other),
        }
    }

    #[test]
    fn self_referencing_typedef_skipped() {
        let result = empty_result(vec![
            CDecl::Struct(CStructDecl {
                tag: Some("foo".into()),
                fields: vec![CField {
                    name: "x".into(),
                    ty: CType::Int,
                    bit_width: None,
                }],
                is_forward: false,
            }),
            CDecl::Typedef(CTypedef {
                name: "foo".into(),
                target: CType::StructTag("foo".into()),
            }),
        ]);

        let out = translate(&result, &[]);
        // Struct emitted, typedef suppressed.
        assert_eq!(out.decls.len(), 1);
        assert!(matches!(&out.decls[0], RaskCDecl::Struct(_)));
    }

    #[test]
    fn render_round_trip() {
        let result = empty_result(vec![
            CDecl::Function(CFuncDecl {
                name: "open".into(),
                params: vec![
                    CParam { name: Some("path".into()), ty: CType::Pointer(Box::new(CType::Const(Box::new(CType::Char)))) },
                    CParam { name: Some("flags".into()), ty: CType::Int },
                ],
                ret_ty: CType::Int,
                is_variadic: false,
                is_static: false,
                is_inline: false,
            }),
            CDecl::Struct(CStructDecl {
                tag: Some("sqlite3".into()),
                fields: vec![],
                is_forward: true,
            }),
        ]);

        let translated = translate(&result, &[]);
        let rendered = render_rask(&translated);
        assert!(rendered.contains("extern \"C\" func open(path: *u8, flags: c_int) -> c_int"));
        assert!(rendered.contains("extern \"C\" struct sqlite3\n"));
    }
}

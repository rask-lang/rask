// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Translates parsed C declarations into Rask-level representations.
//!
//! Follows the type mapping from struct.c-interop (TM1–TM3):
//! - C `int` → `c_int`, `unsigned long` → `c_ulong`, etc.
//! - `T*` → `*T`, `void*` → `*void`
//! - `#define FOO 42` → const FOO: c_int = 42

use crate::*;

/// A Rask-level declaration translated from C.
#[derive(Debug, Clone)]
pub enum RaskCDecl {
    /// `extern "C" func name(params...) -> ret`
    ExternFunc {
        name: String,
        params: Vec<RaskCParam>,
        ret_ty: String,
        is_variadic: bool,
    },
    /// `extern "C" struct Name { fields... }`
    ExternStruct {
        name: String,
        fields: Vec<RaskCField>,
    },
    /// `extern "C" union Name { fields... }`
    ExternUnion {
        name: String,
        fields: Vec<RaskCField>,
    },
    /// `extern "C" enum Name { variants... }`
    ExternEnum {
        name: String,
        variants: Vec<(String, Option<i64>)>,
    },
    /// `const NAME: type = value`
    Const {
        name: String,
        ty: String,
        value: String,
    },
    /// Type alias: `type name = target`
    TypeAlias {
        name: String,
        target: String,
    },
    /// Opaque type (forward-declared struct/union).
    OpaqueType {
        name: String,
    },
    /// Warning about a skipped declaration.
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct RaskCParam {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone)]
pub struct RaskCField {
    pub name: String,
    pub ty: String,
}

/// Translate C parse results into Rask declarations.
///
/// `hiding` is the set of symbol names to suppress (from `import c "..." hiding { ... }`).
pub fn translate(result: &CParseResult, hiding: &[String]) -> Vec<RaskCDecl> {
    let mut out = Vec::new();
    let mut translator = Translator {
        hiding,
        typedefs: std::collections::HashMap::new(),
    };

    // First pass: collect typedefs for resolution
    for decl in &result.decls {
        if let CDecl::Typedef(td) = decl {
            translator.typedefs.insert(td.name.clone(), td.target.clone());
        }
    }

    for decl in &result.decls {
        translator.translate_decl(decl, &mut out);
    }

    // Emit warnings
    for w in &result.warnings {
        out.push(RaskCDecl::Warning {
            message: w.message.clone(),
        });
    }

    out
}

struct Translator<'a> {
    hiding: &'a [String],
    typedefs: std::collections::HashMap<String, CType>,
}

impl<'a> Translator<'a> {
    fn is_hidden(&self, name: &str) -> bool {
        self.hiding.iter().any(|h| h == name)
    }

    fn translate_decl(&self, decl: &CDecl, out: &mut Vec<RaskCDecl>) {
        match decl {
            CDecl::Function(f) => {
                if self.is_hidden(&f.name) { return; }
                let params = f.params.iter().enumerate().map(|(i, p)| {
                    RaskCParam {
                        name: p.name.clone().unwrap_or_else(|| format!("arg{}", i)),
                        ty: self.translate_type(&p.ty),
                    }
                }).collect();
                out.push(RaskCDecl::ExternFunc {
                    name: f.name.clone(),
                    params,
                    ret_ty: self.translate_type(&f.ret_ty),
                    is_variadic: f.is_variadic,
                });
            }
            CDecl::Struct(s) => {
                let name = match &s.tag {
                    Some(t) => t.clone(),
                    None => return, // anonymous, handled via typedef
                };
                if self.is_hidden(&name) { return; }
                if s.is_forward {
                    out.push(RaskCDecl::OpaqueType { name });
                    return;
                }
                let fields = s.fields.iter().map(|f| {
                    RaskCField {
                        name: f.name.clone(),
                        ty: self.translate_type(&f.ty),
                    }
                }).collect();
                out.push(RaskCDecl::ExternStruct { name, fields });
            }
            CDecl::Union(u) => {
                let name = match &u.tag {
                    Some(t) => t.clone(),
                    None => return,
                };
                if self.is_hidden(&name) { return; }
                if u.is_forward {
                    out.push(RaskCDecl::OpaqueType { name });
                    return;
                }
                let fields = u.fields.iter().map(|f| {
                    RaskCField {
                        name: f.name.clone(),
                        ty: self.translate_type(&f.ty),
                    }
                }).collect();
                out.push(RaskCDecl::ExternUnion { name, fields });
            }
            CDecl::Enum(e) => {
                let name = match &e.tag {
                    Some(t) => t.clone(),
                    None => return,
                };
                if self.is_hidden(&name) { return; }
                if e.is_forward { return; }
                let variants = e.variants.iter().map(|v| {
                    (v.name.clone(), v.value)
                }).collect();
                out.push(RaskCDecl::ExternEnum { name, variants });
            }
            CDecl::Typedef(td) => {
                if self.is_hidden(&td.name) { return; }
                // Don't emit typedef if it maps to a struct/union/enum tag with same name
                match &td.target {
                    CType::StructTag(t) | CType::UnionTag(t) | CType::EnumTag(t)
                        if t == &td.name => return,
                    _ => {}
                }
                out.push(RaskCDecl::TypeAlias {
                    name: td.name.clone(),
                    target: self.translate_type(&td.target),
                });
            }
            CDecl::Define(d) => {
                if self.is_hidden(&d.name) { return; }
                match &d.kind {
                    CDefineKind::Integer(v) => {
                        out.push(RaskCDecl::Const {
                            name: d.name.clone(),
                            ty: "c_int".to_string(),
                            value: v.to_string(),
                        });
                    }
                    CDefineKind::UnsignedInteger(v) => {
                        out.push(RaskCDecl::Const {
                            name: d.name.clone(),
                            ty: "c_uint".to_string(),
                            value: v.to_string(),
                        });
                    }
                    CDefineKind::Float(v) => {
                        out.push(RaskCDecl::Const {
                            name: d.name.clone(),
                            ty: "f64".to_string(),
                            value: format!("{}", v),
                        });
                    }
                    CDefineKind::String(s) => {
                        out.push(RaskCDecl::Const {
                            name: d.name.clone(),
                            ty: "*u8".to_string(),
                            value: format!("c\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
                        });
                    }
                    CDefineKind::FunctionMacro { .. } => {
                        out.push(RaskCDecl::Warning {
                            message: format!("function-like macro `{}` cannot be auto-imported", d.name),
                        });
                    }
                    CDefineKind::Unparseable => {
                        // Silently skip — too many #defines are noise
                    }
                }
            }
            CDecl::Variable(v) => {
                if self.is_hidden(&v.name) { return; }
                if v.is_extern {
                    // Extern globals become accessible via unsafe
                    out.push(RaskCDecl::Const {
                        name: v.name.clone(),
                        ty: self.translate_type(&v.ty),
                        value: String::new(), // extern — no value
                    });
                }
            }
        }
    }

    /// Translate a C type to its Rask string representation.
    /// Follows TM1–TM3 from struct.c-interop.
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
                if *signed { "isize".to_string() } else { "usize".to_string() }
            }
            CType::Pointer(inner) => {
                // `const char *` and `char *` → `*u8` (C string convention)
                match inner.as_ref() {
                    CType::Char | CType::SignedChar => return "*u8".to_string(),
                    CType::Const(inner2) if matches!(inner2.as_ref(), CType::Char | CType::SignedChar) => {
                        return "*u8".to_string();
                    }
                    CType::UnsignedChar => return "*u8".to_string(),
                    _ => {}
                }
                let inner_str = self.translate_type(inner);
                format!("*{}", inner_str)
            }
            CType::Const(inner) => {
                // Drop const qualifier — Rask handles mutability via borrow checker.
                self.translate_type(inner)
            }
            CType::Array(elem, Some(size)) => {
                let elem_str = self.translate_type(elem);
                format!("[{}; {}]", elem_str, size)
            }
            CType::Array(elem, None) => {
                // Unsized array — treat as pointer
                let elem_str = self.translate_type(elem);
                format!("*{}", elem_str)
            }
            CType::Named(name) => {
                // Check if it's a well-known typedef
                match name.as_str() {
                    "FILE" => "*void".to_string(),
                    "va_list" => "*void".to_string(),
                    _ => name.clone(),
                }
            }
            CType::StructTag(tag) => tag.clone(),
            CType::UnionTag(tag) => tag.clone(),
            CType::EnumTag(tag) => tag.clone(),
            CType::FuncPtr { ret, params, is_variadic } => {
                let ret_str = self.translate_type(ret);
                let param_strs: Vec<String> = params.iter()
                    .map(|p| self.translate_type(p))
                    .collect();
                let mut sig = format!("*func({})", param_strs.join(", "));
                if *is_variadic {
                    if !param_strs.is_empty() {
                        sig = format!("*func({}, ...)", param_strs.join(", "));
                    } else {
                        sig = "*func(...)".to_string();
                    }
                }
                if ret_str != "void" {
                    sig.push_str(&format!(" -> {}", ret_str));
                }
                sig
            }
        }
    }
}

/// Render translated declarations as Rask source (for debugging / `rask c-header` command).
pub fn render_rask(decls: &[RaskCDecl]) -> String {
    let mut out = String::new();

    for decl in decls {
        match decl {
            RaskCDecl::ExternFunc { name, params, ret_ty, is_variadic } => {
                out.push_str("extern \"C\" func ");
                out.push_str(name);
                out.push('(');
                let mut first = true;
                for p in params {
                    if !first { out.push_str(", "); }
                    first = false;
                    out.push_str(&p.name);
                    out.push_str(": ");
                    out.push_str(&p.ty);
                }
                if *is_variadic {
                    if !first { out.push_str(", "); }
                    out.push_str("...");
                }
                out.push(')');
                if ret_ty != "void" {
                    out.push_str(" -> ");
                    out.push_str(ret_ty);
                }
                out.push('\n');
            }
            RaskCDecl::ExternStruct { name, fields } => {
                out.push_str("extern \"C\" struct ");
                out.push_str(name);
                out.push_str(" {\n");
                for f in fields {
                    out.push_str("    ");
                    out.push_str(&f.name);
                    out.push_str(": ");
                    out.push_str(&f.ty);
                    out.push('\n');
                }
                out.push_str("}\n");
            }
            RaskCDecl::ExternUnion { name, fields } => {
                out.push_str("extern \"C\" union ");
                out.push_str(name);
                out.push_str(" {\n");
                for f in fields {
                    out.push_str("    ");
                    out.push_str(&f.name);
                    out.push_str(": ");
                    out.push_str(&f.ty);
                    out.push('\n');
                }
                out.push_str("}\n");
            }
            RaskCDecl::ExternEnum { name, variants } => {
                out.push_str("extern \"C\" enum ");
                out.push_str(name);
                out.push_str(" {\n");
                for (vname, value) in variants {
                    out.push_str("    ");
                    out.push_str(vname);
                    if let Some(v) = value {
                        out.push_str(&format!(" = {}", v));
                    }
                    out.push('\n');
                }
                out.push_str("}\n");
            }
            RaskCDecl::Const { name, ty, value } => {
                out.push_str("const ");
                out.push_str(name);
                out.push_str(": ");
                out.push_str(ty);
                if !value.is_empty() {
                    out.push_str(" = ");
                    out.push_str(value);
                }
                out.push('\n');
            }
            RaskCDecl::TypeAlias { name, target } => {
                out.push_str("type ");
                out.push_str(name);
                out.push_str(" = ");
                out.push_str(target);
                out.push('\n');
            }
            RaskCDecl::OpaqueType { name } => {
                out.push_str("extern \"C\" struct ");
                out.push_str(name);
                out.push('\n');
            }
            RaskCDecl::Warning { message } => {
                out.push_str("// WARNING: ");
                out.push_str(message);
                out.push('\n');
            }
        }
    }

    out
}

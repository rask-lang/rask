// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Calls to C functions that return a struct by value (struct.c-interop/SR1).
//!
//! Where a returned struct travels is the target's rule, and it isn't one
//! rule: x86-64 splits a 16-byte struct across two registers by what its
//! fields are, a larger one comes back through memory the caller passes a
//! hidden pointer to, and arm64 and Windows each do it differently again. The
//! C compiler knows all of that. So each such function is called through a C
//! function that makes the call and stores the result where Rask asks:
//! `g = SDL_GetGUID(j)` calls `rask_sret_SDL_GetGUID(j, &g)`. The wrappers go
//! in the object's `.sret.c` companion, compiled at link like the variadic
//! ones.
//!
//! The wrapper includes no header. It declares the struct from the Rask
//! declaration, field by field, so a hand-written `extern "C"` works the same
//! as an imported one, and the C compiler classifies the same fields the
//! library was built with.

use std::collections::{BTreeMap, HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, ExternDecl, Field};
use rask_ast::ty::TypeExpr;
use rask_mir::{MirFunction, MirOperand, MirStmtKind};

use crate::variadic::Wrapper;

/// The declarations a wrapper's C is written from, by name.
struct Decls<'a> {
    structs: HashMap<&'a str, (&'a [Field], bool, &'a [rask_ast::attr::Attr])>,
    aliases: HashMap<&'a str, &'a TypeExpr>,
}

impl<'a> Decls<'a> {
    fn new(decls: &'a [Decl]) -> Self {
        let mut structs = HashMap::new();
        let mut aliases = HashMap::new();
        for d in decls {
            match &d.kind {
                DeclKind::Struct(s) => {
                    structs.insert(s.name.as_str(), (s.fields.as_slice(), false, s.attrs.as_slice()));
                }
                DeclKind::Union(u) => {
                    structs.insert(u.name.as_str(), (u.fields.as_slice(), true, &[][..]));
                }
                DeclKind::TypeAlias(a) if a.type_params.is_empty() => {
                    aliases.insert(a.name.as_str(), &a.target);
                }
                _ => {}
            }
        }
        Decls { structs, aliases }
    }

    /// Follow aliases to the type they name.
    fn resolve(&self, ty: &'a TypeExpr) -> &'a TypeExpr {
        let mut ty = ty;
        for _ in 0..32 {
            let TypeExpr::Named { path, args } = ty else { return ty };
            if !args.is_empty() {
                return ty;
            }
            match self.aliases.get(path.join(".").as_str()) {
                Some(next) => ty = next,
                None => return ty,
            }
        }
        ty
    }

    /// The struct or union a type names, after aliases.
    fn aggregate(&self, ty: &'a TypeExpr) -> Option<&'a str> {
        let TypeExpr::Named { path, args } = self.resolve(ty) else { return None };
        if !args.is_empty() {
            return None;
        }
        let name = path.join(".");
        self.structs.get_key_value(name.as_str()).map(|(k, _)| *k)
    }
}

/// C source for the struct declarations a set of wrappers needs, each one
/// after the ones it holds.
struct CWriter<'a, 'd> {
    decls: &'d Decls<'a>,
    written: HashSet<String>,
    out: String,
}

impl<'a, 'd> CWriter<'a, 'd> {
    fn tag(name: &str) -> String {
        let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        format!("rask_{}", clean)
    }

    /// The C spelling of a type that sits before a declarator: `int`,
    /// `struct rask_P`, `void *`. Arrays are the declarator's business.
    fn base(&mut self, ty: &'a TypeExpr) -> Result<String, String> {
        let ty = self.decls.resolve(ty);
        Ok(match ty {
            TypeExpr::RawPtr(_) | TypeExpr::Func { .. } => "void *".to_string(),
            // `*T?` is the same word as `*T`, null for `none`.
            TypeExpr::Optional(inner) if matches!(self.decls.resolve(inner), TypeExpr::RawPtr(_)) => "void *".to_string(),
            TypeExpr::Named { path, args } if args.is_empty() => {
                let name = path.join(".");
                if let Some(c) = scalar(&name) {
                    return Ok(c.to_string());
                }
                if let Some(&(fields, is_union, attrs)) = self.decls.structs.get(name.as_str()) {
                    self.define(&name, fields, is_union, attrs)?;
                    return Ok(format!("{} {}", if is_union { "union" } else { "struct" }, Self::tag(&name)));
                }
                return Err(format!("`{}` has no C equivalent", name));
            }
            other => return Err(format!("`{}` has no C equivalent", other)),
        })
    }

    /// `T name`, `T name[4][2]`.
    fn declarator(&mut self, ty: &'a TypeExpr, name: &str) -> Result<String, String> {
        let mut dims = String::new();
        let mut ty = self.decls.resolve(ty);
        while let TypeExpr::Array { elem, len } = ty {
            dims.push_str(&format!("[{}]", len));
            ty = self.decls.resolve(elem);
        }
        let base = self.base(ty)?;
        let sep = if base.ends_with('*') { "" } else { " " };
        Ok(format!("{}{}{}{}", base, sep, name, dims))
    }

    fn define(&mut self, name: &str, fields: &'a [Field], is_union: bool, attrs: &[rask_ast::attr::Attr]) -> Result<(), String> {
        if !self.written.insert(name.to_string()) {
            return Ok(());
        }
        let mut body = String::new();
        for (i, f) in fields.iter().enumerate() {
            let member = format!("f{}", i);
            let decl = self.declarator(&f.ty, &member)?;
            match rask_layout::bit_width(&f.attrs) {
                Some(bits) => body.push_str(&format!("    {} : {};\n", decl, bits)),
                None => body.push_str(&format!("    {};\n", decl)),
            }
        }
        let request = rask_ast::attr::layout_request(attrs).unwrap_or_default();
        let mut after = String::new();
        if request.packed {
            after.push_str(" __attribute__((packed))");
        }
        if let Some(a) = request.align {
            after.push_str(&format!(" __attribute__((aligned({})))", a));
        }
        self.out.push_str(&format!(
            "{} {} {{\n{}}}{};\n",
            if is_union { "union" } else { "struct" },
            Self::tag(name),
            body,
            after
        ));
        Ok(())
    }
}

/// The C spelling of a Rask scalar.
fn scalar(name: &str) -> Option<&'static str> {
    Some(match name {
        "bool" => "_Bool",
        "i8" => "signed char",
        "u8" => "unsigned char",
        "i16" => "short",
        "u16" => "unsigned short",
        "i32" => "int",
        "u32" | "char" => "unsigned int",
        "i64" => "long long",
        "u64" => "unsigned long long",
        "isize" => "__INTPTR_TYPE__",
        "usize" => "__UINTPTR_TYPE__",
        "f32" => "float",
        "f64" => "double",
        "c_char" => "char",
        "c_short" => "short",
        "c_ushort" => "unsigned short",
        "c_int" => "int",
        "c_uint" => "unsigned int",
        "c_long" => "long",
        "c_ulong" => "unsigned long",
        "c_longlong" => "long long",
        "c_ulonglong" => "unsigned long long",
        "c_size" => "__SIZE_TYPE__",
        "c_ssize" => "__PTRDIFF_TYPE__",
        _ => return None,
    })
}

/// The prototype of `e` and its wrapper. The structs they name are written
/// into `w`, once for the whole file.
fn wrapper_source<'a>(e: &'a ExternDecl, target: &str, wrapper: &str, w: &mut CWriter<'a, '_>) -> Result<String, String> {
    let ret = e.ret_ty.as_ref().expect("only struct-returning externs reach here");
    let ret_c = w.base(ret)?;
    let mut proto = Vec::new();
    let mut params = Vec::new();
    let mut args = Vec::new();
    for (i, p) in e.params.iter().enumerate() {
        let ty = p.ty.as_ref().ok_or_else(|| format!("parameter {} has no type", i))?;
        proto.push(w.declarator(ty, "")?.trim_end().to_string());
        params.push(w.declarator(ty, &format!("a{}", i))?);
        args.push(format!("a{}", i));
    }
    params.push(format!("{} *out", ret_c));
    let proto = if proto.is_empty() { "void".to_string() } else { proto.join(", ") };
    Ok(format!(
        "{ret} {target}({proto});\nvoid {wrapper}({params}) {{ *out = {target}({args}); }}\n",
        ret = ret_c,
        target = target,
        proto = proto,
        wrapper = wrapper,
        params = params.join(", "),
        args = args.join(", "),
    ))
}

/// The wrappers a program needs: the C file that defines them, and their
/// declarations for codegen.
#[derive(Default)]
pub struct Wrappers {
    pub source: String,
    pub sigs: Vec<rask_codegen::ExternFuncSig>,
}

/// Point each call to a struct-returning C function in `functions` at its
/// wrapper, and answer the wrappers.
pub fn route_calls(functions: &mut [MirFunction], decls: &[Decl]) -> Result<Wrappers, Vec<String>> {
    let table = Decls::new(decls);
    let returning: HashMap<&str, &ExternDecl> = decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Extern(e) if e.ret_ty.as_ref().is_some_and(|t| table.aggregate(t).is_some()) => {
                Some((e.name.as_str(), e))
            }
            _ => None,
        })
        .collect();
    if returning.is_empty() {
        return Ok(Wrappers::default());
    }
    let mut writer = CWriter { decls: &table, written: HashSet::new(), out: String::new() };
    let mut wrappers: BTreeMap<String, Wrapper> = BTreeMap::new();
    let mut errors = Vec::new();
    for f in functions.iter_mut() {
        for block in &mut f.blocks {
            for stmt in &mut block.statements {
                let MirStmtKind::Call { dst, func, args } = &mut stmt.kind else { continue };
                if !func.is_extern {
                    continue;
                }
                let Some(e) = returning.get(func.name.as_str()) else { continue };
                if e.is_variadic {
                    errors.push(format!("`{}` is variadic and returns a struct; neither wrapper can make that call", e.name));
                    continue;
                }
                let Some(out) = dst.take() else {
                    errors.push(format!("a call to `{}` has nowhere to put the struct it returns", e.name));
                    continue;
                };
                let wrapper = format!("rask_sret_{}", e.name);
                if !wrappers.contains_key(&wrapper) {
                    // A `static inline` function has no symbol of its own; its
                    // inline wrapper is the one to call (IN2).
                    let target = match crate::c_import::inline_header(e) {
                        Some(_) => rask_c_parse::shim::wrapper_symbol(&e.name),
                        None => e.name.clone(),
                    };
                    match wrapper_source(e, &target, &wrapper, &mut writer) {
                        Ok(source) => {
                            let ret = e.ret_ty.clone().expect("filtered above");
                            let mut param_types: Vec<TypeExpr> = e.params.iter().filter_map(|p| p.ty.clone()).collect();
                            param_types.push(TypeExpr::RawPtr(Box::new(ret)));
                            wrappers.insert(
                                wrapper.clone(),
                                Wrapper {
                                    source,
                                    sig: rask_codegen::ExternFuncSig {
                                        name: wrapper.clone(),
                                        link_name: None,
                                        param_types,
                                        ret_ty: None,
                                    },
                                },
                            );
                        }
                        Err(why) => {
                            errors.push(format!("`{}` returns a struct, and its wrapper can't be written: {}", e.name, why));
                            continue;
                        }
                    }
                }
                // The struct's local is its storage's address: the wrapper
                // writes the result there.
                args.push(MirOperand::Local(out));
                func.name = wrapper;
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut source = writer.out;
    let mut sigs = Vec::new();
    for w in wrappers.into_values() {
        source.push_str(&w.source);
        sigs.push(w.sig);
    }
    Ok(Wrappers { source, sigs })
}

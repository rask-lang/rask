// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The C runtime's prototypes, read from `rask_runtime.h`.
//!
//! A runtime function's register signature is written once, in the header the
//! C compiler checks the definition against. Native codegen reads it from
//! there rather than keeping a second copy: a copy is what let
//! `rask_net_is_invalid` return an `int8_t` while its row took an `i64` back,
//! upper bytes and all.

use cranelift::prelude::types;
use cranelift::prelude::Type;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One `rask_*` prototype, as registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CPrototype {
    pub params: Vec<Type>,
    /// `None` for `void`.
    pub ret: Option<Type>,
}

/// Every `rask_*` declaration in the header, by name: return type and
/// parameter types as written. One this can't read (a struct by value) is
/// left out.
fn written() -> &'static HashMap<String, (String, Vec<String>)> {
    static CACHE: OnceLock<HashMap<String, (String, Vec<String>)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let header = include_str!("../../../runtime/rask_runtime.h");
        // Comments and preprocessor lines out, then one declaration per `;`.
        let mut text = String::new();
        let mut in_block = false;
        for line in header.lines() {
            let mut line = line;
            if in_block {
                match line.find("*/") {
                    Some(end) => {
                        line = &line[end + 2..];
                        in_block = false;
                    }
                    None => continue,
                }
            }
            if let Some(start) = line.find("/*") {
                if !line[start..].contains("*/") {
                    in_block = true;
                }
                line = &line[..start];
            }
            let line = line.split("//").next().unwrap_or("");
            if line.trim_start().starts_with('#') {
                continue;
            }
            text.push_str(line);
            text.push(' ');
        }
        let mut out = HashMap::new();
        for decl in text.split(|c| c == ';' || c == '{' || c == '}') {
            let decl = decl.trim();
            let (Some(open), Some(close)) = (decl.find('('), decl.rfind(')')) else { continue };
            let head = decl[..open].trim();
            let Some(name_at) = head.rfind(|c: char| !(c.is_alphanumeric() || c == '_')) else {
                continue;
            };
            let name = &head[name_at + 1..];
            if !name.starts_with("rask_") || head.contains("typedef") {
                continue;
            }
            let ret = head[..name_at + 1].trim().to_string();
            let params: Vec<String> = decl[open + 1..close]
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty() && p != "void")
                .collect();
            out.insert(name.to_string(), (ret, params));
        }
        out
    })
}

/// The register a C type travels in, or `None` for one this can't say (a
/// struct by value). `void` is `Some(None)`.
fn c_register(ty: &str) -> Option<Option<Type>> {
    if ty.contains('*') {
        return Some(Some(types::I64));
    }
    // A parameter's name follows its type.
    let words: Vec<&str> = ty
        .split_whitespace()
        .filter(|w| !matches!(*w, "const" | "_Noreturn" | "extern" | "static" | "inline"))
        .collect();
    let base = match words.as_slice() {
        [] => return None,
        [t] => *t,
        [.., t, _name] if !matches!(*t, "unsigned" | "signed" | "long") => *t,
        [.., t] => *t,
    };
    Some(match base {
        "void" => None,
        "int8_t" | "uint8_t" | "bool" | "char" => Some(types::I8),
        "int16_t" | "uint16_t" => Some(types::I16),
        "int32_t" | "uint32_t" | "int" | "unsigned" => Some(types::I32),
        "int64_t" | "uint64_t" | "size_t" | "ssize_t" | "intptr_t" | "uintptr_t" | "long" => {
            Some(types::I64)
        }
        "RaskI128" | "RaskU128" => Some(types::I128),
        "float" => Some(types::F32),
        "double" => Some(types::F64),
        _ => return None,
    })
}

/// The registers `symbol` takes and returns, when the header declares it in
/// a form this can read.
pub fn prototype(symbol: &str) -> Option<CPrototype> {
    let (ret, params) = written().get(symbol)?;
    let params: Option<Vec<Type>> = params.iter().map(|p| c_register(p).flatten()).collect();
    Some(CPrototype { params: params?, ret: c_register(ret)? })
}

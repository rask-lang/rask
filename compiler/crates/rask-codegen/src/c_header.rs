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

/// One prototype, as registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CPrototype {
    pub params: Vec<Type>,
    /// `None` for `void`.
    pub ret: Option<Type>,
    /// Marked `RASK_PANICS`: the caller records its location first.
    pub panics: bool,
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
        t if rask_stdlib::runtime_header::is_fn_pointer_type(t) => Some(types::I64),
        "double" => Some(types::F64),
        _ => return None,
    })
}

/// The registers `symbol` takes and returns, when the header declares it in
/// a form this can read.
pub fn prototype(symbol: &str) -> Option<CPrototype> {
    let w = rask_stdlib::runtime_header::prototype(symbol)?;
    let params: Option<Vec<Type>> = w.params.iter().map(|p| c_register(p).flatten()).collect();
    Some(CPrototype { params: params?, ret: c_register(&w.ret)?, panics: w.panics })
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Reading a union member that isn't the live one: the live member's bytes,
//! read as the other's type (mem.unsafe/UN2).
//!
//! Native reads the same memory through a different type. Here a member is a
//! value, so its bytes are rebuilt from it, little-endian as on every target
//! Rask builds for. Bytes past the live member's size read as zero.

use indexmap::IndexMap;
use rask_ast::decl::UnionDecl;
use rask_ast::ty::TypeExpr;

use crate::value::{FloatKind, IntKind, Value};

/// Scalar kinds the interpreter can rebuild from bytes.
#[derive(Clone, Copy)]
enum Scalar {
    Int(IntKind, usize),
    F32,
    F64,
    Bool,
}

fn scalar(ty: &TypeExpr) -> Option<Scalar> {
    let name = ty.bare_name()?;
    let name = rask_ast::primitives::c_type_spelling(name).unwrap_or(name);
    let name = match name {
        "usize" => rask_ast::primitives::usize_spelling(),
        "isize" => rask_ast::primitives::isize_spelling(),
        n => n,
    };
    Some(match name {
        "i8" => Scalar::Int(IntKind::I8, 1),
        "i16" => Scalar::Int(IntKind::I16, 2),
        "i32" => Scalar::Int(IntKind::I32, 4),
        "i64" => Scalar::Int(IntKind::I64, 8),
        "u8" => Scalar::Int(IntKind::U8, 1),
        "u16" => Scalar::Int(IntKind::U16, 2),
        "u32" => Scalar::Int(IntKind::U32, 4),
        "u64" => Scalar::Int(IntKind::U64, 8),
        "f32" => Scalar::F32,
        "f64" => Scalar::F64,
        "bool" => Scalar::Bool,
        _ => return None,
    })
}

fn to_bytes(v: &Value, kind: Scalar) -> Option<Vec<u8>> {
    Some(match (v, kind) {
        (Value::Int(i, _), Scalar::Int(_, n)) => i.to_le_bytes()[..n].to_vec(),
        (Value::Float(f, _), Scalar::F32) => (*f as f32).to_bits().to_le_bytes().to_vec(),
        (Value::Float(f, _), Scalar::F64) => f.to_bits().to_le_bytes().to_vec(),
        (Value::Bool(b), Scalar::Bool) => vec![*b as u8],
        _ => return None,
    })
}

fn from_bytes(b: &[u8], kind: Scalar) -> Value {
    let mut buf = [0u8; 8];
    let n = b.len().min(8);
    buf[..n].copy_from_slice(&b[..n]);
    let raw = u64::from_le_bytes(buf);
    match kind {
        Scalar::Int(k, size) => {
            let bits = size * 8;
            let masked = if bits == 64 { raw } else { raw & ((1u64 << bits) - 1) };
            let signed = matches!(k, IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64);
            let v = if signed && bits < 64 && (masked >> (bits - 1)) & 1 == 1 {
                (masked as i64) - (1i64 << bits)
            } else {
                masked as i64
            };
            Value::Int(v, k)
        }
        Scalar::F32 => Value::Float(f32::from_bits(raw as u32) as f64, FloatKind::F32),
        Scalar::F64 => Value::Float(f64::from_bits(raw), FloatKind::F64),
        Scalar::Bool => Value::Bool(raw & 0xff != 0),
    }
}

/// `want` read from whichever member of `u` is live in `fields`.
pub(crate) fn reinterpret(u: &UnionDecl, fields: &IndexMap<String, Value>, want: &str) -> Result<Value, String> {
    let ty_of = |name: &str| u.fields.iter().find(|f| f.name == name).map(|f| &f.ty);
    let want_ty = ty_of(want).ok_or_else(|| format!("union `{}` has no member `{}`", u.name, want))?;
    let (live, value) = fields.iter().next().ok_or_else(|| format!("union `{}` holds no member", u.name))?;
    let unsupported = || {
        format!(
            "the interpreter reads one union member as another only between numbers and `bool`; `{}.{}` read as `{}` needs native (`rask run`)",
            u.name, live, want
        )
    };
    let live_kind = ty_of(live).and_then(scalar).ok_or_else(unsupported)?;
    let want_kind = scalar(want_ty).ok_or_else(unsupported)?;
    let bytes = to_bytes(value, live_kind).ok_or_else(unsupported)?;
    Ok(from_bytes(&bytes, want_kind))
}

impl crate::interp::Interpreter {
    /// A value written to a `@bits(n)` field fits in n bits, or the write
    /// panics as an overflowing integer does (struct.c-interop/TM7). The
    /// interpreter keeps each field apart, so the width is all there is to
    /// check.
    pub(crate) fn check_bits(&self, struct_name: &str, field: &str, value: &Value) -> Result<(), crate::interp::RuntimeError> {
        let Some(decl) = self.struct_decls.get(struct_name) else { return Ok(()) };
        let Some(f) = decl.fields.iter().find(|f| f.name == field) else { return Ok(()) };
        let Some(width) = rask_types::layout::bit_width(&f.attrs) else { return Ok(()) };
        let Value::Int(v, kind) = value else { return Ok(()) };
        let signed = matches!(kind, IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64);
        let fits = if width >= 64 {
            true
        } else if signed {
            let half = 1i64 << (width - 1);
            (-half..half).contains(v)
        } else {
            *v >= 0 && (*v as u64) < (1u64 << width)
        };
        if fits {
            Ok(())
        } else {
            Err(crate::interp::RuntimeError::Panic(format!("value doesn't fit in a {}-bit field", width)))
        }
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What a `@binary` field's declared type says about the wire (type.binary).
//!
//! The declared type of a `@binary` field is a wire specifier, not a type
//! name: `u16be` is sixteen bits, big-endian, held in a `u16`. The checker,
//! layout and desugar all need the same reading, so it lives here once.

use crate::ty::TypeExpr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Big,
    Little,
}

/// The value a field holds in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// An integer of this primitive (`"u16"`, `"i8"`, …).
    Int(&'static str),
    /// A float of this primitive (`"f32"`, `"f64"`).
    Float(&'static str),
    /// `[N]u8`.
    Bytes(usize),
}

/// One field's wire form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub bits: u32,
    /// `None` for a bare bit count and for single bytes.
    pub endian: Option<Endian>,
    pub held: Held,
}

impl FieldSpec {
    /// The primitive the field is held in, by name.
    pub fn held_name(&self) -> &'static str {
        match self.held {
            Held::Int(n) | Held::Float(n) => n,
            Held::Bytes(_) => "u8",
        }
    }
}

/// Read a field's specifier, or say why it isn't one.
pub fn field_spec(ty: &TypeExpr) -> Result<FieldSpec, String> {
    let s = match ty {
        TypeExpr::FixedCount { count, elem } => {
            if !elem.is_name("u8") {
                return Err(format!("binary byte arrays only support u8, found {}", ty.source()));
            }
            let count: usize = count
                .parse()
                .map_err(|_| format!("invalid byte array count: {count}"))?;
            return Ok(FieldSpec { bits: count as u32 * 8, endian: None, held: Held::Bytes(count) });
        }
        TypeExpr::Int(n) => n.as_str(),
        _ => match ty.bare_name() {
            Some(name) => name,
            None => return Err(format!("unknown binary type: {}", ty.source())),
        },
    };

    // A bare number is that many bits, held in the smallest unsigned integer
    // that fits (F1).
    if let Ok(n) = s.parse::<u32>() {
        if n == 0 || n > 64 {
            return Err(format!("bit count must be >= 1 and <= 64, found {n}"));
        }
        let held = match n {
            1..=8 => "u8",
            9..=16 => "u16",
            17..=32 => "u32",
            _ => "u64",
        };
        return Ok(FieldSpec { bits: n, endian: None, held: Held::Int(held) });
    }

    let (base, endian) = if let Some(base) = s.strip_suffix("be") {
        (base, Endian::Big)
    } else if let Some(base) = s.strip_suffix("le") {
        (base, Endian::Little)
    } else {
        return match s {
            "u8" => Ok(FieldSpec { bits: 8, endian: None, held: Held::Int("u8") }),
            "i8" => Ok(FieldSpec { bits: 8, endian: None, held: Held::Int("i8") }),
            _ => Err(format!("multi-byte field '{s}' must specify endianness (be/le)")),
        };
    };
    let (bits, held) = match base {
        "u16" => (16, Held::Int("u16")),
        "i16" => (16, Held::Int("i16")),
        "u32" => (32, Held::Int("u32")),
        "i32" => (32, Held::Int("i32")),
        "u64" => (64, Held::Int("u64")),
        "i64" => (64, Held::Int("i64")),
        "f32" => (32, Held::Float("f32")),
        "f64" => (64, Held::Float("f64")),
        _ => return Err(format!("unknown binary type: {s}")),
    };
    Ok(FieldSpec { bits, endian: Some(endian), held })
}

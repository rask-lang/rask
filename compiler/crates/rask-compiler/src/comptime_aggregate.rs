// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! A folded struct or enum, written out as it sits in memory.
//!
//! The evaluator hands back a struct as names and values. A const needs the
//! bytes a running program would have built: each field at its layout offset,
//! at its declared width. Those go in read-only data and every use copies them
//! out, so `const ORIGIN: Point = comptime { … }` costs nothing at startup.

use rask_comptime::{ComptimeValue, SerializedConst};
use rask_mir::ConstAggregate;
use rask_mono::{EnumLayout, FieldLayout, MonoProgram, StructLayout};
use rask_types::Type;

/// Why a value has no bytes.
pub(crate) enum NoBytes {
    /// Nothing static can hold it: a `Vec` field owns a heap buffer, an
    /// optional has no layout of its own here. It runs at startup instead.
    Gap,
    /// A field's value doesn't fit the field's type. Overflow at compile time
    /// is a compile error (type.overflow/CT1).
    Overflow(String),
}

/// `val` as const bytes, and the layout they're in.
///
/// `checked` is the name of the struct or enum the checker typed the folded
/// expression as. The evaluator doesn't model every conversion: for
/// `Marker(v: 0) as any Opaque` it hands back the `Marker`, and those bytes
/// in a slot that holds an interface value crashed the first read. The value
/// has to be the type the program expects, or it has no bytes here.
pub(crate) fn aggregate_const(
    val: &ComptimeValue,
    checked: Option<&str>,
    mono: &MonoProgram,
) -> Result<(SerializedConst, ConstAggregate), NoBytes> {
    let mut out = Writer { mono, bytes: Vec::new(), relocs: Vec::new() };
    let agg = out.aggregate(val, 0)?;
    let built = match agg {
        ConstAggregate::Struct(i) => &mono.struct_layouts[i].name,
        ConstAggregate::Enum(i) => &mono.enum_layouts[i].name,
    };
    if checked != Some(built.as_str()) {
        return Err(NoBytes::Gap);
    }
    Ok((SerializedConst { bytes: out.bytes, string_relocs: out.relocs }, agg))
}

struct Writer<'a> {
    mono: &'a MonoProgram,
    bytes: Vec<u8>,
    relocs: Vec<(usize, String)>,
}

impl Writer<'_> {
    /// Write a struct or enum value at `at`, growing the buffer to hold it.
    fn aggregate(&mut self, val: &ComptimeValue, at: usize) -> Result<ConstAggregate, NoBytes> {
        match val {
            // A variant with fields is built by the same literal as a struct,
            // under the variant's dotted name: `Mode.On(level: 7)`.
            ComptimeValue::Struct { name, fields } => match name.rsplit_once('.') {
                Some((enum_path, variant)) => {
                    let enum_name = enum_path.rsplit('.').next().unwrap_or(enum_path);
                    self.variant(enum_name, variant, Some(fields), at)
                }
                None => {
                    let (i, layout) = find(&self.mono.struct_layouts, |l: &StructLayout| &l.name, name)?;
                    self.grow(at, layout.size);
                    if fields.len() != layout.fields.len() {
                        return Err(NoBytes::Gap);
                    }
                    for f in &layout.fields {
                        let v = fields.get(&f.name).ok_or(NoBytes::Gap)?;
                        self.field(f, v, at + f.offset as usize)?;
                    }
                    Ok(ConstAggregate::Struct(i))
                }
            },
            ComptimeValue::Enum { name, variant, data: None } => self.variant(name, variant, None, at),
            _ => Err(NoBytes::Gap),
        }
    }

    fn variant(
        &mut self,
        enum_name: &str,
        variant: &str,
        fields: Option<&std::collections::HashMap<String, ComptimeValue>>,
        at: usize,
    ) -> Result<ConstAggregate, NoBytes> {
        let (i, layout) = find(&self.mono.enum_layouts, |l: &EnumLayout| &l.name, enum_name)?;
        let v = layout.variants.iter().find(|v| v.name == variant).ok_or(NoBytes::Gap)?;
        self.grow(at, layout.size);
        let tag_width = layout.tag_ty.scalar_bytes().ok_or(NoBytes::Gap)? as usize;
        let tag_at = at + layout.tag_offset as usize;
        self.bytes[tag_at..tag_at + tag_width].copy_from_slice(&v.tag.to_le_bytes()[..tag_width]);
        let given = fields.map_or(0, |f| f.len());
        if given != v.fields.len() {
            return Err(NoBytes::Gap);
        }
        for f in &v.fields {
            let value = fields.and_then(|fs| fs.get(&f.name)).ok_or(NoBytes::Gap)?;
            self.field(f, value, at + (v.payload_offset + f.offset) as usize)?;
        }
        Ok(ConstAggregate::Enum(i))
    }

    fn field(&mut self, f: &FieldLayout, v: &ComptimeValue, at: usize) -> Result<(), NoBytes> {
        let size = f.size as usize;
        if f.ty == Type::String {
            let ComptimeValue::String(_) = v else { return Err(NoBytes::Gap) };
            let mut slot = SerializedConst::default();
            v.write_element(&mut slot).ok_or(NoBytes::Gap)?;
            if slot.bytes.len() != size {
                return Err(NoBytes::Gap);
            }
            self.bytes[at..at + size].copy_from_slice(&slot.bytes);
            self.relocs.extend(slot.string_relocs.into_iter().map(|(o, t)| (at + o, t)));
            return Ok(());
        }
        if let Some(raw) = scalar_bytes(&f.ty, v, &f.name)? {
            if raw.len() > size {
                return Err(NoBytes::Gap);
            }
            self.bytes[at..at + raw.len()].copy_from_slice(&raw);
            return Ok(());
        }
        // Held in place: a nested struct or enum is its bytes, and they have
        // to be exactly the field's slot. A field that holds a pointer (an
        // `Owned<T>`, a shared generic slot) is narrower, and has no static form.
        let before = self.bytes.len();
        let agg = self.aggregate(v, at)?;
        let written = match agg {
            ConstAggregate::Struct(i) => self.mono.struct_layouts[i].size,
            ConstAggregate::Enum(i) => self.mono.enum_layouts[i].size,
        };
        if written != f.size || self.bytes.len() != before {
            return Err(NoBytes::Gap);
        }
        Ok(())
    }

    fn grow(&mut self, at: usize, size: u32) {
        let end = at + size as usize;
        if self.bytes.len() < end {
            self.bytes.resize(end, 0);
        }
    }
}

/// The layout named `name`, and its index. A name two layouts share — a
/// generic type's instances — says nothing about which one this is.
fn find<'a, L>(layouts: &'a [L], name_of: impl Fn(&L) -> &String, name: &str) -> Result<(usize, &'a L), NoBytes> {
    let mut hits = layouts.iter().enumerate().filter(|(_, l)| name_of(l) == name);
    match (hits.next(), hits.next()) {
        (Some(hit), None) => Ok(hit),
        _ => Err(NoBytes::Gap),
    }
}

/// A scalar field's bytes at the field's own width. `Ok(None)` when the field
/// isn't a scalar.
fn scalar_bytes(ty: &Type, v: &ComptimeValue, field: &str) -> Result<Option<Vec<u8>>, NoBytes> {
    let overflow = |n: &dyn std::fmt::Display| {
        NoBytes::Overflow(format!("field `{field}` is `{ty}`, and {n} doesn't fit in it"))
    };
    let int = || -> Result<i128, NoBytes> {
        Ok(match v {
            ComptimeValue::I8(n) => *n as i128,
            ComptimeValue::I16(n) => *n as i128,
            ComptimeValue::I32(n) => *n as i128,
            ComptimeValue::I64(n) => *n as i128,
            ComptimeValue::I128(n) => *n,
            ComptimeValue::U8(n) => *n as i128,
            ComptimeValue::U16(n) => *n as i128,
            ComptimeValue::U32(n) => *n as i128,
            ComptimeValue::U64(n) => *n as i128,
            ComptimeValue::U128(n) => i128::try_from(*n).map_err(|_| NoBytes::Gap)?,
            _ => return Err(NoBytes::Gap),
        })
    };
    macro_rules! fit {
        ($t:ty) => {{
            let n = int()?;
            <$t>::try_from(n).map_err(|_| overflow(&n))?.to_le_bytes().to_vec()
        }};
    }
    Ok(Some(match ty {
        Type::Bool => vec![v.as_bool().ok_or(NoBytes::Gap)? as u8],
        Type::I8 => fit!(i8),
        Type::I16 => fit!(i16),
        Type::I32 => fit!(i32),
        Type::I64 => fit!(i64),
        Type::I128 => fit!(i128),
        Type::U8 => fit!(u8),
        Type::U16 => fit!(u16),
        Type::U32 => fit!(u32),
        Type::U64 => fit!(u64),
        Type::U128 => match v {
            ComptimeValue::U128(n) => n.to_le_bytes().to_vec(),
            _ => fit!(u128),
        },
        Type::F32 => (v.as_f64().ok_or(NoBytes::Gap)? as f32).to_le_bytes().to_vec(),
        Type::F64 => v.as_f64().ok_or(NoBytes::Gap)?.to_le_bytes().to_vec(),
        Type::Char => match v {
            ComptimeValue::Char(c) => (*c as u32).to_le_bytes().to_vec(),
            _ => return Err(NoBytes::Gap),
        },
        _ => return Ok(None),
    }))
}

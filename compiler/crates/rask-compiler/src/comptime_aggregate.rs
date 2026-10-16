// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! A folded value written out as the bytes of its type.
//!
//! The evaluator hands back a value as names and numbers. What a running
//! program needs is the bytes it would have built: every field at its layout
//! offset and declared width, an optional's tag and payload, a string's
//! sixteen-byte header. Those go in read-only data and every use copies them
//! out (ctrl.comptime/CT58).
//!
//! A value with nothing static that can hold it has no bytes, and that's an
//! error at the `comptime` that made it: a `Vec` inside a struct owns a heap
//! buffer, an `any I` holds a pointer to a box.

use rask_comptime::{ComptimeValue, SerializedConst};
use rask_mir::lower::MirContext;
use rask_mir::MirType;
use rask_mono::abi::{TAG_BYTES, TAG_OFFSET};
use rask_mono::runtime_abi::{RASK_TAG_ABSENT, RASK_TAG_PRESENT};
use rask_mono::{FieldLayout, MonoProgram};

/// Why a value has no bytes.
pub(crate) enum NoBytes {
    /// Nothing static can hold it. Says what, for the error.
    Gap(String),
    /// A value doesn't fit the type it's going into. Overflow at compile
    /// time is a compile error (type.overflow/CT1).
    Overflow(String),
}

/// `val` as the bytes of `ty`.
pub(crate) fn encode(
    val: &ComptimeValue,
    ty: &MirType,
    ctx: &MirContext,
    mono: &MonoProgram,
) -> Result<SerializedConst, NoBytes> {
    let mut out = Writer { ctx, mono, bytes: vec![0; ty.size() as usize], relocs: Vec::new() };
    out.write(val, ty, 0)?;
    Ok(SerializedConst { bytes: out.bytes, string_relocs: out.relocs })
}

/// A list's elements back to back at `elem`'s size, the buffer a `Vec<elem>`
/// holds.
pub(crate) fn encode_list(
    items: &[ComptimeValue],
    elem: &MirType,
    ctx: &MirContext,
    mono: &MonoProgram,
) -> Result<SerializedConst, NoBytes> {
    let stride = elem.size() as usize;
    let mut out = Writer { ctx, mono, bytes: vec![0; stride * items.len()], relocs: Vec::new() };
    for (i, item) in items.iter().enumerate() {
        out.write(item, elem, i * stride)?;
    }
    Ok(SerializedConst { bytes: out.bytes, string_relocs: out.relocs })
}

/// A map's entries as `(key, value)` tuples back to back.
pub(crate) fn encode_map(
    entries: &[(ComptimeValue, ComptimeValue)],
    key: &MirType,
    val: &MirType,
    ctx: &MirContext,
    mono: &MonoProgram,
) -> Result<SerializedConst, NoBytes> {
    let parts = [key.clone(), val.clone()];
    let val_at = rask_mir::tuple_offsets(&parts)[1] as usize;
    let stride = MirType::Tuple(parts.to_vec()).size() as usize;
    let mut out = Writer { ctx, mono, bytes: vec![0; stride * entries.len()], relocs: Vec::new() };
    for (i, (k, v)) in entries.iter().enumerate() {
        out.write(k, key, i * stride)?;
        out.write(v, val, i * stride + val_at)?;
    }
    Ok(SerializedConst { bytes: out.bytes, string_relocs: out.relocs })
}

/// The type a scalar or string value is on its own, for a list or map whose
/// checked type didn't come through.
pub(crate) fn natural_type(v: &ComptimeValue) -> Option<MirType> {
    Some(match v {
        ComptimeValue::String(_) => MirType::String,
        ComptimeValue::Bool(_) => MirType::Bool,
        ComptimeValue::Char(_) => MirType::Char,
        ComptimeValue::I8(_) => MirType::I8,
        ComptimeValue::I16(_) => MirType::I16,
        ComptimeValue::I32(_) => MirType::I32,
        ComptimeValue::I64(_) => MirType::I64,
        ComptimeValue::I128(_) => MirType::I128,
        ComptimeValue::U8(_) => MirType::U8,
        ComptimeValue::U16(_) => MirType::U16,
        ComptimeValue::U32(_) => MirType::U32,
        ComptimeValue::U64(_) => MirType::U64,
        ComptimeValue::U128(_) => MirType::U128,
        ComptimeValue::F32(_) => MirType::F32,
        ComptimeValue::F64(_) => MirType::F64,
        _ => return None,
    })
}

struct Writer<'a, 'c> {
    ctx: &'a MirContext<'c>,
    mono: &'a MonoProgram,
    bytes: Vec<u8>,
    relocs: Vec<(usize, String)>,
}

fn gap(what: impl Into<String>) -> NoBytes {
    NoBytes::Gap(what.into())
}

impl Writer<'_, '_> {
    fn write(&mut self, v: &ComptimeValue, ty: &MirType, at: usize) -> Result<(), NoBytes> {
        match ty {
            MirType::Bool => {
                let b = v.as_bool().ok_or_else(|| gap(format!("a `{}` where a `bool` goes", v.type_name())))?;
                self.put(at, &[b as u8]);
            }
            MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64 | MirType::I128
            | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 | MirType::U128 => {
                let raw = int_bytes(v, ty)?;
                self.put(at, &raw);
            }
            MirType::Char => {
                let ComptimeValue::Char(c) = v else {
                    return Err(gap(format!("a `{}` where a `char` goes", v.type_name())));
                };
                self.put(at, &(*c as u32).to_le_bytes());
            }
            MirType::F32 => {
                let f = float(v).ok_or_else(|| gap(format!("a `{}` where an `f32` goes", v.type_name())))?;
                self.put(at, &(f as f32).to_le_bytes());
            }
            MirType::F64 => {
                let f = float(v).ok_or_else(|| gap(format!("a `{}` where an `f64` goes", v.type_name())))?;
                self.put(at, &f.to_le_bytes());
            }
            MirType::String => {
                let ComptimeValue::String(_) = v else {
                    return Err(gap(format!("a `{}` where a `string` goes", v.type_name())));
                };
                let mut slot = SerializedConst::default();
                v.write_element(&mut slot).ok_or_else(|| gap("a string"))?;
                self.put(at, &slot.bytes);
                self.relocs.extend(slot.string_relocs.into_iter().map(|(o, t)| (at + o, t)));
            }
            MirType::Struct(sid) => {
                let ComptimeValue::Struct { fields, .. } = v else {
                    return Err(gap(format!("a `{}` where a struct goes", v.type_name())));
                };
                let layout = &self.mono.struct_layouts[sid.id as usize];
                if fields.len() != layout.fields.len() {
                    return Err(gap(format!("a `{}` with fields `{}` doesn't have", layout.name, layout.name)));
                }
                for f in &layout.fields {
                    let value = fields.get(&f.name).ok_or_else(|| gap(format!("`{}` with no field `{}`", layout.name, f.name)))?;
                    self.field(f, value, at + f.offset as usize, &layout.name)?;
                }
            }
            MirType::Enum(eid) => {
                let layout = &self.mono.enum_layouts[eid.id as usize];
                // A variant with fields is built by the same literal as a
                // struct, under the variant's dotted name: `Mode.On(level: 7)`.
                let (variant, fields) = match v {
                    ComptimeValue::Struct { name, fields } => match name.rsplit_once('.') {
                        Some((_, variant)) => (variant, Some(fields)),
                        None => return Err(gap(format!("a `{name}` where a `{}` goes", layout.name))),
                    },
                    ComptimeValue::Enum { variant, data: None, .. } => (variant.as_str(), None),
                    other => return Err(gap(format!("a `{}` where a `{}` goes", other.type_name(), layout.name))),
                };
                let var = layout
                    .variants
                    .iter()
                    .find(|x| x.name == variant)
                    .ok_or_else(|| gap(format!("`{}` has no variant `{variant}`", layout.name)))?;
                let tag_width = layout.tag_ty.scalar_bytes().ok_or_else(|| gap("an enum tag"))? as usize;
                self.put(at + layout.tag_offset as usize, &var.tag.to_le_bytes()[..tag_width]);
                if fields.map_or(0, |f| f.len()) != var.fields.len() {
                    return Err(gap(format!("`{}.{variant}` with the wrong fields", layout.name)));
                }
                for f in &var.fields {
                    let value = fields
                        .and_then(|fs| fs.get(&f.name))
                        .ok_or_else(|| gap(format!("`{}.{variant}` with no field `{}`", layout.name, f.name)))?;
                    self.field(f, value, at + (var.payload_offset + f.offset) as usize, &layout.name)?;
                }
            }
            MirType::Option(inner) => {
                if matches!(**inner, MirType::Link(_)) {
                    return Err(gap("a link"));
                }
                let payload_at = at + ty.payload_offset() as usize;
                match v {
                    ComptimeValue::Enum { name, variant, data } if name == "Option" => match (variant.as_str(), data) {
                        ("None", _) => self.put_tag(at, RASK_TAG_ABSENT),
                        (_, Some(payload)) => {
                            self.put_tag(at, RASK_TAG_PRESENT);
                            self.write(payload, inner, payload_at)?;
                        }
                        _ => return Err(gap("an optional with no value")),
                    },
                    // A value where an optional goes is a present one.
                    present => {
                        self.put_tag(at, RASK_TAG_PRESENT);
                        self.write(present, inner, payload_at)?;
                    }
                }
            }
            MirType::Result { ok, err } => {
                let payload_at = at + ty.payload_offset() as usize;
                let (failed, payload) = match v {
                    ComptimeValue::Enum { name, variant, data: Some(d) } if name == "Result" => {
                        (variant == "Err", d.as_ref())
                    }
                    // The evaluator hands back a returned error as the error
                    // value itself; the error type's name says which side.
                    other => (self.names_type(other, err), other),
                };
                self.put_tag(at, if failed { RASK_TAG_ABSENT } else { RASK_TAG_PRESENT });
                self.write(payload, if failed { err } else { ok }, payload_at)?;
            }
            MirType::Tuple(parts) => {
                let ComptimeValue::Tuple(items) = v else {
                    return Err(gap(format!("a `{}` where a tuple goes", v.type_name())));
                };
                if items.len() != parts.len() {
                    return Err(gap("a tuple of the wrong length"));
                }
                for ((item, part), offset) in items.iter().zip(parts).zip(rask_mir::tuple_offsets(parts)) {
                    self.write(item, part, at + offset as usize)?;
                }
            }
            MirType::Array { elem, len } => {
                let ComptimeValue::Array(items) = v else {
                    return Err(gap(format!("a `{}` where an array goes", v.type_name())));
                };
                if items.len() != *len as usize {
                    return Err(gap(format!("{} elements where the array holds {len}", items.len())));
                }
                for (i, item) in items.iter().enumerate() {
                    self.write(item, elem, at + i * elem.size() as usize)?;
                }
            }
            MirType::InterfaceObject { .. } => return Err(gap("an `any` value, which points at a box")),
            MirType::Union(_) => return Err(gap("an error union")),
            _ => {
                let what = match v {
                    ComptimeValue::Array(_) => "a list".to_string(),
                    ComptimeValue::Map(_) => "a map".to_string(),
                    ComptimeValue::Closure { .. } => "a closure".to_string(),
                    other => format!("a `{}`", other.type_name()),
                };
                return Err(gap(format!("{what}, which owns memory a constant can't hold")));
            }
        }
        Ok(())
    }

    /// A field of a struct or an enum variant, at the type its layout gives it.
    fn field(&mut self, f: &FieldLayout, v: &ComptimeValue, at: usize, owner: &str) -> Result<(), NoBytes> {
        let ty = self.ctx.type_to_mir(&f.ty);
        self.write(v, &ty, at).map_err(|e| match e {
            NoBytes::Gap(why) => NoBytes::Gap(format!("{why} (field `{}` of `{owner}`)", f.name)),
            other => other,
        })
    }

    /// A wrapper's tag, at its own offset and width. The origin of a
    /// `T or E` stays zero: a constant has no `try` site.
    fn put_tag(&mut self, wrapper_at: usize, tag: i64) {
        let width = TAG_BYTES as usize;
        self.put(wrapper_at + TAG_OFFSET as usize, &tag.to_le_bytes()[..width]);
    }

    /// Does `v` name the type `ty` lays out? Only a struct or an enum carries
    /// its name.
    fn names_type(&self, v: &ComptimeValue, ty: &MirType) -> bool {
        let value_name = match v {
            ComptimeValue::Struct { name, .. } => name.rsplit_once('.').map_or(name.as_str(), |(owner, _)| owner),
            ComptimeValue::Enum { name, .. } => name.as_str(),
            _ => return false,
        };
        let layout_name = match ty {
            MirType::Struct(sid) => &self.mono.struct_layouts[sid.id as usize].name,
            MirType::Enum(eid) => &self.mono.enum_layouts[eid.id as usize].name,
            _ => return false,
        };
        layout_name == value_name || layout_name.rsplit('.').next() == Some(value_name)
    }

    fn put(&mut self, at: usize, raw: &[u8]) {
        if self.bytes.len() < at + raw.len() {
            self.bytes.resize(at + raw.len(), 0);
        }
        self.bytes[at..at + raw.len()].copy_from_slice(raw);
    }
}

/// A float, or an integer literal the checker took as one: `@default(3)` on an
/// `f64` field is 3.0, and the evaluator hands back the integer it was written as.
fn float(v: &ComptimeValue) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|n| n as f64))
}

/// An integer value at the width of `ty`. A value that doesn't fit is
/// overflow, not truncation.
fn int_bytes(v: &ComptimeValue, ty: &MirType) -> Result<Vec<u8>, NoBytes> {
    let n: i128 = match v {
        ComptimeValue::I8(n) => *n as i128,
        ComptimeValue::I16(n) => *n as i128,
        ComptimeValue::I32(n) => *n as i128,
        ComptimeValue::I64(n) => *n as i128,
        ComptimeValue::I128(n) => *n,
        ComptimeValue::U8(n) => *n as i128,
        ComptimeValue::U16(n) => *n as i128,
        ComptimeValue::U32(n) => *n as i128,
        ComptimeValue::U64(n) => *n as i128,
        ComptimeValue::U128(n) => match ty {
            MirType::U128 => return Ok(n.to_le_bytes().to_vec()),
            _ => i128::try_from(*n).map_err(|_| overflow(&n.to_string(), ty))?,
        },
        ComptimeValue::Char(c) => *c as i128,
        other => return Err(gap(format!("a `{}` where an integer goes", other.type_name()))),
    };
    macro_rules! fit {
        ($t:ty) => {
            <$t>::try_from(n).map_err(|_| overflow(&n.to_string(), ty))?.to_le_bytes().to_vec()
        };
    }
    Ok(match ty {
        MirType::I8 => fit!(i8),
        MirType::I16 => fit!(i16),
        MirType::I32 => fit!(i32),
        MirType::I64 => fit!(i64),
        MirType::I128 => n.to_le_bytes().to_vec(),
        MirType::U8 => fit!(u8),
        MirType::U16 => fit!(u16),
        MirType::U32 => fit!(u32),
        MirType::U64 => fit!(u64),
        MirType::U128 => fit!(u128),
        _ => unreachable!("only called for integer types"),
    })
}

fn overflow(n: &str, ty: &MirType) -> NoBytes {
    NoBytes::Overflow(format!("{n} doesn't fit in `{ty}`"))
}

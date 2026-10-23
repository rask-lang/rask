// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `transmute` (mem.unsafe): the value written out as the bytes native holds
//! it in, read back as the target type.
//!
//! The bytes are laid out by rask-mono's layout, the same one native and the
//! checker's size test read, so a field lands at the offset it has natively
//! and padding is zero.

use indexmap::IndexMap;
use rask_ast::decl::DeclKind;
use rask_types::Type;

use super::{Interpreter, RuntimeError};
use crate::value::{FloatKind, IntKind, Value};

fn unsupported(ty: &Type) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "transmute of `{ty}` isn't supported by the interpreter: it reinterprets \
         scalars, fixed arrays, tuples and non-generic structs of those"
    ))
}

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}

/// Width in bytes of an integer type, and whether it's signed.
fn int_shape(ty: &Type) -> Option<(usize, bool)> {
    Some(match ty {
        Type::I8 => (1, true),
        Type::I16 => (2, true),
        Type::I32 => (4, true),
        Type::I64 => (8, true),
        Type::U8 => (1, false),
        Type::U16 => (2, false),
        Type::U32 => (4, false),
        Type::U64 => (8, false),
        _ => return None,
    })
}

impl Interpreter {
    /// `transmute(value)` from `from` to `to`, both the checker's types.
    pub(crate) fn transmute(&self, value: &Value, from: &Type, to: &Type) -> Result<Value, RuntimeError> {
        let spell = |ty: &Type| rask_mono::layout_spelling(ty, &self.types).ok_or_else(|| unsupported(ty));
        let (from, to) = (spell(from)?, spell(to)?);
        let (from_size, _) = self.byte_layout(&from)?;
        let (to_size, _) = self.byte_layout(&to)?;
        if from_size != to_size {
            return Err(RuntimeError::TypeError(format!(
                "transmute from `{from}` ({from_size} bytes) to `{to}` ({to_size} bytes): the sizes differ"
            )));
        }
        let mut bytes = vec![0u8; from_size as usize];
        self.write_bytes(value, &from, &mut bytes, 0)?;
        self.read_bytes(&to, &bytes, 0)
    }

    /// Size and alignment, refusing anything the byte walk can't do.
    fn byte_layout(&self, ty: &Type) -> Result<(u32, u32), RuntimeError> {
        match ty {
            Type::Bool | Type::Char | Type::F32 | Type::F64 | Type::I128 | Type::U128 => {}
            _ if int_shape(ty).is_some() => {}
            Type::Array { elem, .. } => {
                self.byte_layout(elem)?;
            }
            Type::Tuple(elems) => {
                for e in elems {
                    self.byte_layout(e)?;
                }
            }
            Type::UnresolvedNamed(name) => {
                self.struct_layout_named(name).ok_or_else(|| unsupported(ty))?;
            }
            _ => return Err(unsupported(ty)),
        }
        Ok(rask_mono::type_size_align(ty, &self.layout_cache))
    }

    fn struct_layout_named(&self, name: &str) -> Option<rask_mono::StructLayout> {
        let decl = self.type_decls.iter().find(|d| {
            matches!(&d.kind, DeclKind::Struct(s) if s.name == name && s.type_params.is_empty())
        })?;
        Some(rask_mono::compute_struct_layout(decl, &[], &self.layout_cache))
    }

    fn write_bytes(&self, value: &Value, ty: &Type, out: &mut [u8], at: usize) -> Result<(), RuntimeError> {
        let mismatch = || RuntimeError::TypeError(format!("transmute: `{ty}` value doesn't match its type"));
        let mut put = |bytes: &[u8]| out[at..at + bytes.len()].copy_from_slice(bytes);
        match (ty, value) {
            (Type::Bool, Value::Bool(b)) => put(&[*b as u8]),
            (Type::Char, Value::Char(c)) => put(&(*c as u32).to_le_bytes()),
            (Type::F32, Value::Float(f, _)) => put(&(*f as f32).to_bits().to_le_bytes()),
            (Type::F64, Value::Float(f, _)) => put(&f.to_bits().to_le_bytes()),
            (Type::I128, Value::Int128(v)) => put(&v.to_le_bytes()),
            (Type::U128, Value::Uint128(v)) => put(&v.to_le_bytes()),
            (_, Value::Int(v, _)) if int_shape(ty).is_some() => {
                let (width, _) = int_shape(ty).unwrap();
                put(&v.to_le_bytes()[..width]);
            }
            (Type::Array { elem, .. }, Value::Vec(items)) => {
                let (elem_size, _) = self.byte_layout(elem)?;
                for (i, item) in items.lock().unwrap().items.iter().enumerate() {
                    self.write_bytes(item, elem, out, at + i * elem_size as usize)?;
                }
            }
            (Type::Tuple(elems), Value::Tuple(items)) => {
                let mut offset = 0u32;
                for (elem, item) in elems.iter().zip(items.iter()) {
                    let (size, align) = self.byte_layout(elem)?;
                    offset = align_up(offset, align);
                    self.write_bytes(item, elem, out, at + offset as usize)?;
                    offset += size;
                }
            }
            (Type::UnresolvedNamed(name), Value::Struct(data)) => {
                let layout = self.struct_layout_named(name).ok_or_else(|| unsupported(ty))?;
                let data = data.lock().unwrap();
                for field in &layout.fields {
                    let v = data.fields.get(&field.name).ok_or_else(mismatch)?;
                    self.write_bytes(v, &field.ty, out, at + field.offset as usize)?;
                }
            }
            _ => return Err(mismatch()),
        }
        Ok(())
    }

    fn read_bytes(&self, ty: &Type, bytes: &[u8], at: usize) -> Result<Value, RuntimeError> {
        let take = |n: usize| -> [u8; 16] {
            let mut buf = [0u8; 16];
            buf[..n].copy_from_slice(&bytes[at..at + n]);
            buf
        };
        Ok(match ty {
            Type::Bool => Value::Bool(bytes[at] != 0),
            Type::Char => {
                let code = u32::from_le_bytes(take(4)[..4].try_into().unwrap());
                Value::Char(char::from_u32(code).ok_or_else(|| {
                    RuntimeError::TypeError(format!("transmute to char: {code:#x} is not a Unicode scalar value"))
                })?)
            }
            Type::F32 => Value::Float(
                f32::from_bits(u32::from_le_bytes(take(4)[..4].try_into().unwrap())) as f64,
                FloatKind::F32,
            ),
            Type::F64 => Value::Float(f64::from_bits(u64::from_le_bytes(take(8)[..8].try_into().unwrap())), FloatKind::F64),
            Type::I128 => Value::Int128(i128::from_le_bytes(take(16))),
            Type::U128 => Value::Uint128(u128::from_le_bytes(take(16))),
            _ if int_shape(ty).is_some() => {
                let (width, signed) = int_shape(ty).unwrap();
                let raw = u64::from_le_bytes(take(width)[..8].try_into().unwrap());
                let shift = 64 - 8 * width as u32;
                let v = if signed { ((raw << shift) as i64) >> shift } else { raw as i64 };
                Value::Int(v, IntKind::from_type(ty))
            }
            Type::Array { elem, len } => {
                let (elem_size, _) = self.byte_layout(elem)?;
                let items = (0..*len)
                    .map(|i| self.read_bytes(elem, bytes, at + i * elem_size as usize))
                    .collect::<Result<Vec<_>, _>>()?;
                Value::vec(items)
            }
            Type::Tuple(elems) => {
                let mut offset = 0u32;
                let mut items = Vec::with_capacity(elems.len());
                for elem in elems {
                    let (size, align) = self.byte_layout(elem)?;
                    offset = align_up(offset, align);
                    items.push(self.read_bytes(elem, bytes, at + offset as usize)?);
                    offset += size;
                }
                Value::tuple(items)
            }
            Type::UnresolvedNamed(name) => {
                let mut layout = self.struct_layout_named(name).ok_or_else(|| unsupported(ty))?;
                layout.fields.sort_by_key(|f| f.decl_index);
                let mut fields = IndexMap::new();
                for field in &layout.fields {
                    fields.insert(field.name.clone(), self.read_bytes(&field.ty, bytes, at + field.offset as usize)?);
                }
                Value::new_struct(name.clone(), fields, None)
            }
            _ => return Err(unsupported(ty)),
        })
    }
}

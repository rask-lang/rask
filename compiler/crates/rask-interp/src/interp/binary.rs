// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! @binary struct parse/build implementation for the interpreter.

use std::sync::{Arc, Mutex};
use indexmap::IndexMap;

use crate::value::{StructData, Value};
use super::{Interpreter, RuntimeError, RuntimeDiagnostic};
use rask_ast::Span;

/// Endianness for multi-byte fields.
#[derive(Debug, Clone, Copy)]
pub enum Endian {
    Big,
    Little,
}

/// A field in a @binary struct.
#[derive(Debug, Clone)]
pub struct BinaryFieldMeta {
    pub name: String,
    pub bits: u32,
    pub endian: Option<Endian>,
    pub bit_offset: u32,
    pub is_byte_array: bool,
    pub byte_array_len: usize,
    /// "u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "f32", "f64"
    pub runtime_type: String,
}

/// Metadata for a @binary struct.
#[derive(Debug, Clone)]
pub struct BinaryStructMeta {
    pub name: String,
    pub fields: Vec<BinaryFieldMeta>,
    pub total_bits: u32,
    pub size_bytes: u32,
}

impl BinaryStructMeta {
    /// Parse @binary struct fields from AST declarations.
    pub fn from_decl(name: &str, fields: &[rask_ast::decl::Field]) -> Option<Self> {
        let mut binary_fields = Vec::new();
        let mut bit_offset: u32 = 0;

        for field in fields {
            let spec = parse_field_spec(&field.ty)?;
            binary_fields.push(BinaryFieldMeta {
                name: field.name.clone(),
                bits: spec.0,
                endian: spec.1,
                bit_offset,
                is_byte_array: spec.3,
                byte_array_len: spec.4,
                runtime_type: spec.2,
            });
            bit_offset += spec.0;
        }

        let size_bytes = (bit_offset + 7) / 8;
        Some(BinaryStructMeta {
            name: name.to_string(),
            fields: binary_fields,
            total_bits: bit_offset,
            size_bytes,
        })
    }
}

/// Parse a binary field type specifier: returns (bits, endian, runtime_type_name, is_byte_array, byte_array_len)
fn parse_field_spec(ty: &str) -> Option<(u32, Option<Endian>, String, bool, usize)> {
    let s = ty.trim();

    // [N]u8 — fixed byte array
    if s.starts_with('[') {
        let bracket_end = s.find(']')?;
        let count: usize = s[1..bracket_end].parse().ok()?;
        let elem = &s[bracket_end + 1..];
        if elem != "u8" { return None; }
        return Some((count as u32 * 8, None, "byte_array".into(), true, count));
    }

    // Bare number
    if let Ok(n) = s.parse::<u32>() {
        if n == 0 || n > 64 { return None; }
        let rt = match n {
            1..=8 => "u8",
            9..=16 => "u16",
            17..=32 => "u32",
            33..=64 => "u64",
            _ => unreachable!(),
        };
        return Some((n, None, rt.into(), false, 0));
    }

    // Endian types
    let (base, endian) = if let Some(base) = s.strip_suffix("be") {
        (base, Some(Endian::Big))
    } else if let Some(base) = s.strip_suffix("le") {
        (base, Some(Endian::Little))
    } else {
        match s {
            "u8" => return Some((8, None, "u8".into(), false, 0)),
            "i8" => return Some((8, None, "i8".into(), false, 0)),
            _ => return None,
        }
    };

    let (bits, rt) = match base {
        "u16" => (16, "u16"),
        "i16" => (16, "i16"),
        "u32" => (32, "u32"),
        "i32" => (32, "i32"),
        "u64" => (64, "u64"),
        "i64" => (64, "i64"),
        "f32" => (32, "f32"),
        "f64" => (64, "f64"),
        _ => return None,
    };

    Some((bits, endian, rt.into(), false, 0))
}

impl Interpreter {
    /// Detect and dispatch @binary static method calls (e.g. IpHeader.parse(data)).
    pub(crate) fn try_binary_static_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Option<Result<Value, RuntimeDiagnostic>> {
        let meta = self.binary_structs.get(type_name)?.clone();
        match method {
            "parse" => Some(self.binary_parse(&meta, args, span)),
            _ => None,
        }
    }

    /// Detect and dispatch @binary instance method calls (e.g. header.build()).
    pub(crate) fn try_binary_instance_method(
        &mut self,
        receiver: &Value,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Option<Result<Value, RuntimeDiagnostic>> {
        let meta = self.binary_structs.get(type_name)?.clone();
        match method {
            "build" => Some(self.binary_build(&meta, receiver, span)),
            "build_into" => {
                if args.len() != 1 {
                    return Some(Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError("build_into expects 1 argument".into()),
                        span,
                    )));
                }
                Some(self.binary_build_into(&meta, receiver, args.into_iter().next().unwrap(), span))
            }
            _ => None,
        }
    }

    /// G1: parse(data: []u8) -> (T, []u8) or ParseError
    fn binary_parse(
        &mut self,
        meta: &BinaryStructMeta,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeDiagnostic> {
        if args.len() != 1 {
            return Err(RuntimeDiagnostic::new(
                RuntimeError::TypeError("parse expects 1 argument".into()),
                span,
            ));
        }

        let data = match &args[0] {
            Value::Vec(v) => {
                let guard = v.lock().unwrap();
                guard.iter().map(|v| match v {
                    Value::Int(n) => *n as u8,
                    _ => 0,
                }).collect::<Vec<u8>>()
            }
            _ => {
                return Err(RuntimeDiagnostic::new(
                    RuntimeError::TypeError("parse expects []u8 argument".into()),
                    span,
                ));
            }
        };

        let needed = meta.size_bytes as usize;
        if data.len() < needed {
            // Return Err(ParseError)
            return Ok(Value::Enum {
                name: "Result".into(),
                variant: "Err".into(),
                fields: vec![Value::String(Arc::new(Mutex::new(format!(
                    "not enough data: need {} bytes, got {}",
                    needed,
                    data.len()
                ))))],
                variant_index: 1,
                origin: None,
            });
        }

        let mut fields = IndexMap::new();
        for field in &meta.fields {
            let value = read_binary_field(&data, field);
            fields.insert(field.name.clone(), value);
        }

        let struct_val = Value::Struct(Arc::new(Mutex::new(StructData {
            name: meta.name.clone(),
            fields,
            resource_id: None,
        })));

        // Remaining data as slice
        let remaining: Vec<Value> = data[needed..]
            .iter()
            .map(|&b| Value::Int(b as i64))
            .collect();
        let remaining_val = Value::Vec(Arc::new(Mutex::new(remaining)));

        // Return Ok((struct, remaining))
        Ok(Value::Enum {
            name: "Result".into(),
            variant: "Ok".into(),
            fields: vec![Value::Vec(Arc::new(Mutex::new(vec![struct_val, remaining_val])))],
            variant_index: 0,
            origin: None,
        })
    }

    /// G2: build(self) -> Vec<u8>
    fn binary_build(
        &mut self,
        meta: &BinaryStructMeta,
        receiver: &Value,
        span: Span,
    ) -> Result<Value, RuntimeDiagnostic> {
        let guard = match receiver {
            Value::Struct(s) => s.lock().unwrap(),
            _ => {
                return Err(RuntimeDiagnostic::new(
                    RuntimeError::TypeError("build requires a struct receiver".into()),
                    span,
                ));
            }
        };

        let mut buf = vec![0u8; meta.size_bytes as usize];
        for field_meta in &meta.fields {
            let val = guard.fields.get(&field_meta.name).cloned().unwrap_or(Value::Int(0));
            write_binary_field(&mut buf, field_meta, &val);
        }

        let result: Vec<Value> = buf.iter().map(|&b| Value::Int(b as i64)).collect();
        Ok(Value::Vec(Arc::new(Mutex::new(result))))
    }

    /// G3: build_into(self, buffer: []u8) -> usize or BuildError
    fn binary_build_into(
        &mut self,
        meta: &BinaryStructMeta,
        receiver: &Value,
        buffer: Value,
        span: Span,
    ) -> Result<Value, RuntimeDiagnostic> {
        let guard = match receiver {
            Value::Struct(s) => s.lock().unwrap(),
            _ => {
                return Err(RuntimeDiagnostic::new(
                    RuntimeError::TypeError("build_into requires a struct receiver".into()),
                    span,
                ));
            }
        };

        let needed = meta.size_bytes as usize;

        // Build into a local buffer first
        let mut buf = vec![0u8; needed];
        for field_meta in &meta.fields {
            let val = guard.fields.get(&field_meta.name).cloned().unwrap_or(Value::Int(0));
            write_binary_field(&mut buf, field_meta, &val);
        }
        drop(guard);

        // Write to the target buffer
        match buffer {
            Value::Vec(v) => {
                let mut guard = v.lock().unwrap();
                if guard.len() < needed {
                    return Ok(Value::Enum {
                        name: "Result".into(),
                        variant: "Err".into(),
                        fields: vec![Value::String(Arc::new(Mutex::new(format!(
                            "buffer too small: need {} bytes, got {}",
                            needed,
                            guard.len()
                        ))))],
                        variant_index: 1,
                        origin: None,
                    });
                }
                for (i, &b) in buf.iter().enumerate() {
                    guard[i] = Value::Int(b as i64);
                }
            }
            _ => {
                return Err(RuntimeDiagnostic::new(
                    RuntimeError::TypeError("build_into expects []u8 buffer".into()),
                    span,
                ));
            }
        }

        Ok(Value::Enum {
            name: "Result".into(),
            variant: "Ok".into(),
            fields: vec![Value::Int(needed as i64)],
            variant_index: 0,
            origin: None,
        })
    }
}

/// Read a binary field value from a byte buffer at the given bit offset.
fn read_binary_field(data: &[u8], field: &BinaryFieldMeta) -> Value {
    if field.is_byte_array {
        let byte_start = (field.bit_offset / 8) as usize;
        let values: Vec<Value> = data[byte_start..byte_start + field.byte_array_len]
            .iter()
            .map(|&b| Value::Int(b as i64))
            .collect();
        return Value::Vec(Arc::new(Mutex::new(values)));
    }

    let raw = read_bits(data, field.bit_offset, field.bits);

    match field.runtime_type.as_str() {
        "u8" | "u16" | "u32" | "u64" => Value::Int(raw as i64),
        "i8" => Value::Int(raw as u8 as i8 as i64),
        "i16" => {
            let v = match field.endian {
                Some(Endian::Little) => i16::from_le_bytes((raw as u16).to_le_bytes()),
                _ => raw as u16 as i16,
            };
            Value::Int(v as i64)
        }
        "i32" => {
            let v = match field.endian {
                Some(Endian::Little) => i32::from_le_bytes((raw as u32).to_le_bytes()),
                _ => raw as u32 as i32,
            };
            Value::Int(v as i64)
        }
        "i64" => {
            let v = match field.endian {
                Some(Endian::Little) => i64::from_le_bytes(raw.to_le_bytes()),
                _ => raw as i64,
            };
            Value::Int(v)
        }
        "f32" => {
            let bits = raw as u32;
            Value::Float(f32::from_bits(bits) as f64)
        }
        "f64" => {
            Value::Float(f64::from_bits(raw))
        }
        _ => Value::Int(raw as i64),
    }
}

/// Write a binary field value into a byte buffer at the given bit offset.
fn write_binary_field(buf: &mut [u8], field: &BinaryFieldMeta, value: &Value) {
    if field.is_byte_array {
        let byte_start = (field.bit_offset / 8) as usize;
        if let Value::Vec(v) = value {
            let guard = v.lock().unwrap();
            for (i, val) in guard.iter().enumerate().take(field.byte_array_len) {
                if let Value::Int(n) = val {
                    buf[byte_start + i] = *n as u8;
                }
            }
        }
        return;
    }

    let raw: u64 = match value {
        Value::Int(n) => *n as u64,
        Value::Float(f) => {
            if field.bits == 32 {
                (*f as f32).to_bits() as u64
            } else {
                f.to_bits()
            }
        }
        _ => 0,
    };

    // Apply endianness for multi-byte fields
    let raw = match (field.endian, field.bits) {
        (Some(Endian::Little), 16) => {
            let bytes = (raw as u16).to_le_bytes();
            u16::from_be_bytes(bytes) as u64
        }
        (Some(Endian::Little), 32) => {
            let bytes = (raw as u32).to_le_bytes();
            u32::from_be_bytes(bytes) as u64
        }
        (Some(Endian::Little), 64) => {
            let bytes = raw.to_le_bytes();
            u64::from_be_bytes(bytes)
        }
        _ => raw, // Big endian or sub-byte: use as-is (MSB-first is natural)
    };

    write_bits(buf, field.bit_offset, field.bits, raw);
}

/// Read `count` bits starting at `bit_offset` from a byte buffer (MSB-first, B3).
fn read_bits(data: &[u8], bit_offset: u32, count: u32) -> u64 {
    let mut result: u64 = 0;
    for i in 0..count {
        let abs_bit = bit_offset + i;
        let byte_idx = (abs_bit / 8) as usize;
        let bit_idx = 7 - (abs_bit % 8); // MSB-first
        if byte_idx < data.len() && (data[byte_idx] >> bit_idx) & 1 == 1 {
            result |= 1u64 << (count - 1 - i);
        }
    }

    result
}

/// Write `count` bits starting at `bit_offset` into a byte buffer (MSB-first, B3).
fn write_bits(buf: &mut [u8], bit_offset: u32, count: u32, value: u64) {
    for i in 0..count {
        let abs_bit = bit_offset + i;
        let byte_idx = (abs_bit / 8) as usize;
        let bit_idx = 7 - (abs_bit % 8); // MSB-first
        if byte_idx < buf.len() {
            let bit_val = (value >> (count - 1 - i)) & 1;
            if bit_val == 1 {
                buf[byte_idx] |= 1 << bit_idx;
            } else {
                buf[byte_idx] &= !(1 << bit_idx);
            }
        }
    }
}

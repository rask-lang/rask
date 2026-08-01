// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on primitive types: int, float, bool, char.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

/// Reinterpret the low `width` bits as an unsigned value, dropping whatever
/// the i64 carries above them (sign extension, or a wider previous result).
fn mask_to_width(v: i64, width: u32) -> u64 {
    if width >= 64 { v as u64 } else { (v as u64) & ((1u64 << width) - 1) }
}

/// Put a width-masked result back into an i64, sign-extending when the
/// receiver's type is signed so `-1i8` stays -1 rather than becoming 255.
fn sign_extend(v: i64, width: u32, signed: bool) -> i64 {
    if !signed || width >= 64 {
        return v;
    }
    let shift = 64 - width;
    ((v << shift) >> shift) as i64
}

/// Create an Ordering enum value from a std::cmp::Ordering.
fn ordering_value(ord: std::cmp::Ordering) -> Value {
    Value::Enum {
        name: "Ordering".to_string(),
        variant: match ord {
            std::cmp::Ordering::Less => "Less".to_string(),
            std::cmp::Ordering::Equal => "Equal".to_string(),
            std::cmp::Ordering::Greater => "Greater".to_string(),
        },
        fields: vec![],
        variant_index: 0, origin: None,
    }
}

impl Interpreter {
    /// Handle integer method calls. `kind` is the receiver's width, preserved
    /// on integer results and used for checked arithmetic. Note: the desugared
    /// operator path (add/sub/... ) is normally intercepted before dispatch by
    /// `try_checked_int_arith`; these arms are the fallback and stay checked.
    pub(crate) fn call_int_method(
        &self,
        a: i64,
        kind: crate::value::IntKind,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        use crate::interp::overflow::{checked_binop, checked_neg, ArithOp};
        let arg_kind = |args: &[Value]| match args.first() {
            Some(Value::Int(_, k)) => kind.unify(*k),
            _ => kind,
        };
        if let Some(op) = ArithOp::from_method(method) {
            let b = self.expect_int(args, 0)?;
            let k = arg_kind(args);
            return checked_binop(k, op, a, b).map(|v| Value::Int(v, k));
        }
        match method {
            "neg" => checked_neg(kind, a).map(|v| Value::Int(v, kind)),
            "eq" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a == b)) }
            "lt" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a < b)) }
            "le" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a > b)) }
            "ge" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a >= b)) }
            "compare" => { let b = self.expect_int(args, 0)?; Ok(ordering_value(a.cmp(&b))) }
            "bit_and" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a & b, arg_kind(args))) }
            "bit_or" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a | b, arg_kind(args))) }
            "bit_xor" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a ^ b, arg_kind(args))) }
            "bit_not" => Ok(Value::Int(!a, kind)),
            "abs" => Ok(Value::Int(a.wrapping_abs(), kind)),
            "pow" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a.wrapping_pow(b as u32), kind)) }
            "min" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a.min(b), arg_kind(args))) }
            "max" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a.max(b), arg_kind(args))) }
            // An unsigned receiver holds its bit pattern in the i64 slot, so
            // the top half of u64 prints negative without the width (#517).
            "to_string" | "debug_string" => {
                let text = if kind.is_unsigned() {
                    (a as u64).to_string()
                } else {
                    a.to_string()
                };
                Ok(Value::String(Arc::new(Mutex::new(text))))
            }
            "to_float" => Ok(Value::Float(a as f64)),
            // std.bits B1. Every answer depends on the receiver's declared
            // width, not on the i64 the value happens to live in — `(0 as
            // i32).count_zeros()` is 32, not 64 — so mask to the width first.
            // An untyped literal is i64-wide.
            "count_ones" | "count_zeros"
            | "leading_zeros" | "trailing_zeros"
            | "leading_ones" | "trailing_ones"
            | "reverse_bits" | "swap_bytes"
            | "rotate_left" | "rotate_right"
            | "to_be" | "to_le" => {
                let width = kind.bits().unwrap_or(64);
                let masked = mask_to_width(a, width);
                let out = match method {
                    "count_ones" => masked.count_ones() as i64,
                    "count_zeros" => (width - masked.count_ones()) as i64,
                    // Leading counts are over the declared width, so drop the
                    // high bits the u64 carries beyond it.
                    "leading_zeros" => (masked.leading_zeros() - (64 - width)) as i64,
                    "leading_ones" => ((masked << (64 - width)).leading_ones()) as i64,
                    "trailing_zeros" => masked.trailing_zeros().min(width) as i64,
                    "trailing_ones" => masked.trailing_ones().min(width) as i64,
                    "reverse_bits" => (masked.reverse_bits() >> (64 - width)) as i64,
                    // Hosts Rask targets are little-endian, so to_be is a byte
                    // swap and to_le is the identity.
                    "swap_bytes" | "to_be" => (masked.swap_bytes() >> (64 - width)) as i64,
                    "to_le" => masked as i64,
                    "rotate_left" | "rotate_right" => {
                        let n = (self.expect_int(args, 0)? as u32).rem_euclid(width);
                        let n = if method == "rotate_right" { (width - n) % width } else { n };
                        let rotated = (masked << n) | (masked >> ((width - n) % width));
                        mask_to_width(rotated as i64, width) as i64
                    }
                    _ => unreachable!(),
                };
                Ok(Value::Int(sign_extend(out, width, kind.signed()), kind))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "i64".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle i128 method calls.
    pub(crate) fn call_int128_method(
        &self,
        a: i128,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        let overflow = |op: &str, b: i128| RuntimeError::IntegerOverflow(format!(
            "integer overflow: {} {} {} exceeds i128 range", a, op, b
        ));
        match method {
            "add" => { let b = self.expect_int128(args, 0)?; a.checked_add(b).map(Value::Int128).ok_or_else(|| overflow("+", b)) }
            "sub" => { let b = self.expect_int128(args, 0)?; a.checked_sub(b).map(Value::Int128).ok_or_else(|| overflow("-", b)) }
            "mul" => { let b = self.expect_int128(args, 0)?; a.checked_mul(b).map(Value::Int128).ok_or_else(|| overflow("*", b)) }
            "div" => {
                let b = self.expect_int128(args, 0)?;
                if b == 0 { return Err(RuntimeError::DivisionByZero); }
                a.checked_div(b).map(Value::Int128).ok_or_else(|| overflow("/", b))
            }
            "rem" => {
                let b = self.expect_int128(args, 0)?;
                if b == 0 { return Err(RuntimeError::DivisionByZero); }
                a.checked_rem(b).map(Value::Int128).ok_or_else(|| overflow("%", b))
            }
            "neg" => a.checked_neg().map(Value::Int128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: negating {} exceeds i128 range", a))),
            "eq" => { let b = self.expect_int128(args, 0)?; Ok(Value::Bool(a == b)) }
            "lt" => { let b = self.expect_int128(args, 0)?; Ok(Value::Bool(a < b)) }
            "le" => { let b = self.expect_int128(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_int128(args, 0)?; Ok(Value::Bool(a > b)) }
            "ge" => { let b = self.expect_int128(args, 0)?; Ok(Value::Bool(a >= b)) }
            "compare" => { let b = self.expect_int128(args, 0)?; Ok(ordering_value(a.cmp(&b))) }
            "bit_and" => { let b = self.expect_int128(args, 0)?; Ok(Value::Int128(a & b)) }
            "bit_or" => { let b = self.expect_int128(args, 0)?; Ok(Value::Int128(a | b)) }
            "bit_xor" => { let b = self.expect_int128(args, 0)?; Ok(Value::Int128(a ^ b)) }
            "shl" => {
                let b = self.expect_int(args, 0)?;
                a.checked_shl(b as u32).map(Value::Int128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds i128 bit width (128)", b)))
            }
            "shr" => {
                let b = self.expect_int(args, 0)?;
                a.checked_shr(b as u32).map(Value::Int128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds i128 bit width (128)", b)))
            }
            "bit_not" => Ok(Value::Int128(!a)),
            "abs" => a.checked_abs().map(Value::Int128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: negating {} exceeds i128 range", a))),
            "pow" => { let b = self.expect_int(args, 0)?; a.checked_pow(b as u32).map(Value::Int128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: {} ** {} exceeds i128 range", a, b))) }
            "min" => { let b = self.expect_int128(args, 0)?; Ok(Value::Int128(a.min(b))) }
            "max" => { let b = self.expect_int128(args, 0)?; Ok(Value::Int128(a.max(b))) }
            "to_string" | "debug_string" => Ok(Value::String(Arc::new(Mutex::new(a.to_string())))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "i128".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle u128 method calls.
    pub(crate) fn call_uint128_method(
        &self,
        a: u128,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        let overflow = |op: &str, b: u128| RuntimeError::IntegerOverflow(format!(
            "integer overflow: {} {} {} exceeds u128 range", a, op, b
        ));
        match method {
            "add" => { let b = self.expect_uint128(args, 0)?; a.checked_add(b).map(Value::Uint128).ok_or_else(|| overflow("+", b)) }
            "sub" => { let b = self.expect_uint128(args, 0)?; a.checked_sub(b).map(Value::Uint128).ok_or_else(|| overflow("-", b)) }
            "mul" => { let b = self.expect_uint128(args, 0)?; a.checked_mul(b).map(Value::Uint128).ok_or_else(|| overflow("*", b)) }
            "div" => {
                let b = self.expect_uint128(args, 0)?;
                if b == 0 { return Err(RuntimeError::DivisionByZero); }
                Ok(Value::Uint128(a / b))
            }
            "rem" => {
                let b = self.expect_uint128(args, 0)?;
                if b == 0 { return Err(RuntimeError::DivisionByZero); }
                Ok(Value::Uint128(a % b))
            }
            "eq" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Bool(a == b)) }
            "lt" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Bool(a < b)) }
            "le" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Bool(a > b)) }
            "ge" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Bool(a >= b)) }
            "compare" => { let b = self.expect_uint128(args, 0)?; Ok(ordering_value(a.cmp(&b))) }
            "bit_and" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Uint128(a & b)) }
            "bit_or" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Uint128(a | b)) }
            "bit_xor" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Uint128(a ^ b)) }
            "shl" => {
                let b = self.expect_int(args, 0)?;
                a.checked_shl(b as u32).map(Value::Uint128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds u128 bit width (128)", b)))
            }
            "shr" => {
                let b = self.expect_int(args, 0)?;
                a.checked_shr(b as u32).map(Value::Uint128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds u128 bit width (128)", b)))
            }
            "bit_not" => Ok(Value::Uint128(!a)),
            "pow" => { let b = self.expect_int(args, 0)?; a.checked_pow(b as u32).map(Value::Uint128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: {} ** {} exceeds u128 range", a, b))) }
            "min" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Uint128(a.min(b))) }
            "max" => { let b = self.expect_uint128(args, 0)?; Ok(Value::Uint128(a.max(b))) }
            "to_string" | "debug_string" => Ok(Value::String(Arc::new(Mutex::new(a.to_string())))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "u128".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle float method calls.
    pub(crate) fn call_float_method(
        &self,
        a: f64,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        match method {
            "add" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a + b)) }
            "sub" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a - b)) }
            "mul" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a * b)) }
            "div" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a / b)) }
            "neg" => Ok(Value::Float(-a)),
            "eq" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a == b)) }
            "lt" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a < b)) }
            "le" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a > b)) }
            "ge" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a >= b)) }
            "compare" => {
                let b = self.expect_float(args, 0)?;
                Ok(ordering_value(a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)))
            }
            "abs" => Ok(Value::Float(a.abs())),
            "floor" => Ok(Value::Float(a.floor())),
            "ceil" => Ok(Value::Float(a.ceil())),
            "round" => Ok(Value::Float(a.round())),
            "sqrt" => Ok(Value::Float(a.sqrt())),
            "min" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.min(b))) }
            "max" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.max(b))) }
            "to_string" | "debug_string" => Ok(Value::String(Arc::new(Mutex::new(a.to_string())))),
            "to_int" => Ok(Value::int(a as i64)),
            "pow" | "powf" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.powf(b))) }
            "powi" => { let b = self.expect_int(args, 0)?; Ok(Value::Float(a.powi(b as i32))) }
            "rem" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.rem_euclid(b))) }
            "sin" => Ok(Value::Float(a.sin())),
            "cos" => Ok(Value::Float(a.cos())),
            "tan" => Ok(Value::Float(a.tan())),
            "asin" => Ok(Value::Float(a.asin())),
            "acos" => Ok(Value::Float(a.acos())),
            "atan" => Ok(Value::Float(a.atan())),
            "ln" => Ok(Value::Float(a.ln())),
            "log10" => Ok(Value::Float(a.log10())),
            "log2" => Ok(Value::Float(a.log2())),
            "exp" => Ok(Value::Float(a.exp())),
            "trunc" => Ok(Value::Float(a.trunc())),
            "fract" => Ok(Value::Float(a.fract())),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "f64".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle bool method calls.
    pub(crate) fn call_bool_method(
        &self,
        a: bool,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        match method {
            "eq" => { let b = self.expect_bool(args, 0)?; Ok(Value::Bool(a == b)) }
            "compare" => { let b = self.expect_bool(args, 0)?; Ok(ordering_value(a.cmp(&b))) }
            "to_string" | "debug_string" => Ok(Value::String(Arc::new(Mutex::new(if a { "true" } else { "false" }.to_string())))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "bool".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle char method calls.
    pub(crate) fn call_char_method(
        &self,
        c: char,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        match method {
            "is_whitespace" => Ok(Value::Bool(c.is_whitespace())),
            "is_ascii" => Ok(Value::Bool(c.is_ascii())),
            "is_alphabetic" => Ok(Value::Bool(c.is_alphabetic())),
            "is_numeric" => Ok(Value::Bool(c.is_numeric())),
            "is_alphanumeric" => Ok(Value::Bool(c.is_alphanumeric())),
            "is_digit" => Ok(Value::Bool(c.is_ascii_digit())),
            "is_uppercase" => Ok(Value::Bool(c.is_uppercase())),
            "is_lowercase" => Ok(Value::Bool(c.is_lowercase())),
            "to_uppercase" => Ok(Value::Char(c.to_uppercase().next().unwrap_or(c))),
            "to_lowercase" => Ok(Value::Char(c.to_lowercase().next().unwrap_or(c))),
            "len_utf8" => Ok(Value::int(c.len_utf8() as i64)),
            "to_string" => Ok(Value::String(Arc::new(Mutex::new(c.to_string())))),
            "eq" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c == other)) }
            "compare" => { let other = self.expect_char(args, 0)?; Ok(ordering_value(c.cmp(&other))) }
            "debug_string" => Ok(Value::String(Arc::new(Mutex::new(format!("'{}'", c))))),
            "to_int" => Ok(Value::int(c as i64)),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "char".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

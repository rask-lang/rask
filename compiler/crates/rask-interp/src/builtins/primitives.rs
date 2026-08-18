// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on primitive types: int, float, bool, char.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{FloatKind, Value};

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
        // For a comparison, the other operand's kind stands on its own —
        // `unify` picks one and erases the very difference being compared.
        fn other_kind(args: &[Value], fallback: crate::value::IntKind) -> crate::value::IntKind {
            match args.first() {
                Some(Value::Int(_, k)) if *k != crate::value::IntKind::Untyped => *k,
                _ => fallback,
            }
        }
        if let Some(op) = ArithOp::from_method(method) {
            let b = self.expect_int(args, 0)?;
            let k = arg_kind(args);
            return checked_binop(k, op, a, b).map(|v| Value::Int(v, k));
        }
        match method {
            "neg" => checked_neg(kind, a).map(|v| Value::Int(v, kind)),
            "eq" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a == b)) }
            // Ordered by *value*, not by the i64 slot: an unsigned value is
            // carried in that slot as its bit pattern, so u64::MAX reads as -1
            // and `>` on the slots got it backwards. A mixed-signedness pair is
            // the interesting case and has an obviously-correct answer — a
            // negative signed value is below every unsigned one (#308).
            "lt" => {
                let b = self.expect_int(args, 0)?;
                Ok(Value::Bool(Self::int_value_cmp(a, kind, b, other_kind(args, kind))
                    == std::cmp::Ordering::Less))
            }
            "le" => {
                let b = self.expect_int(args, 0)?;
                Ok(Value::Bool(Self::int_value_cmp(a, kind, b, other_kind(args, kind))
                    != std::cmp::Ordering::Greater))
            }
            "gt" => {
                let b = self.expect_int(args, 0)?;
                Ok(Value::Bool(Self::int_value_cmp(a, kind, b, other_kind(args, kind))
                    == std::cmp::Ordering::Greater))
            }
            "ge" => {
                let b = self.expect_int(args, 0)?;
                Ok(Value::Bool(Self::int_value_cmp(a, kind, b, other_kind(args, kind))
                    != std::cmp::Ordering::Less))
            }
            "compare" => {
                let b = self.expect_int(args, 0)?;
                Ok(ordering_value(Self::int_value_cmp(a, kind, b, other_kind(args, kind))))
            }
            "bit_and" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a & b, arg_kind(args))) }
            "bit_or" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a | b, arg_kind(args))) }
            "bit_xor" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a ^ b, arg_kind(args))) }
            "bit_not" => Ok(Value::Int(!a, kind)),
            "abs" => Ok(Value::Int(a.wrapping_abs(), kind)),
            // AR3: the floored answer, always non-negative — `(-1).mod(10)` is
            // 9, where `-1 % 10` is -1. This is the expression people write by
            // hand as `((a % n) + n) % n`, with a name and one evaluation of
            // each operand.
            "mod" => {
                let b = self.expect_int(args, 0)?;
                if b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                if kind.is_unsigned() {
                    // Already non-negative; `mod` and `%` coincide.
                    return Ok(Value::Int(((a as u64) % (b as u64)) as i64, kind));
                }
                a.checked_rem_euclid(b)
                    .map(|r| Value::Int(r, kind))
                    .ok_or_else(|| RuntimeError::IntegerOverflow(format!(
                        "integer overflow: {}.mod({}) exceeds range", a, b
                    )))
            }
            "pow" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a.wrapping_pow(b as u32), kind)) }
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
            "to_float" => Ok(Value::Float(a as f64, FloatKind::F64)),
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
                let b = self.expect_shift_amount(args, 0)?;
                a.checked_shl(b as u32).map(Value::Int128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds i128 bit width (128)", b)))
            }
            "shr" => {
                let b = self.expect_shift_amount(args, 0)?;
                a.checked_shr(b as u32).map(Value::Int128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds i128 bit width (128)", b)))
            }
            "bit_not" => Ok(Value::Int128(!a)),
            "abs" => a.checked_abs().map(Value::Int128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: negating {} exceeds i128 range", a))),
            "pow" => { let b = self.expect_shift_amount(args, 0)?; a.checked_pow(b as u32).map(Value::Int128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: {} ** {} exceeds i128 range", a, b))) }
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
                let b = self.expect_shift_amount(args, 0)?;
                a.checked_shl(b as u32).map(Value::Uint128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds u128 bit width (128)", b)))
            }
            "shr" => {
                let b = self.expect_shift_amount(args, 0)?;
                a.checked_shr(b as u32).map(Value::Uint128).ok_or_else(|| RuntimeError::IntegerOverflow(
                    format!("shift amount {} exceeds u128 bit width (128)", b)))
            }
            "bit_not" => Ok(Value::Uint128(!a)),
            "pow" => { let b = self.expect_shift_amount(args, 0)?; a.checked_pow(b as u32).map(Value::Uint128).ok_or_else(||
                RuntimeError::IntegerOverflow(format!("integer overflow: {} ** {} exceeds u128 range", a, b))) }
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
        ka: FloatKind,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        // `a + b` desugars to `a.add(b)`, so this is the arithmetic path for
        // most float code. The result carries the operands' width and is
        // rounded onto it — an f32 that keeps computing at f64 precision is
        // what made the interpreter disagree with native.
        let k = match args.first() {
            Some(Value::Float(_, kb)) => ka.unify(*kb),
            _ => ka,
        };
        match method {
            "add" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(k.round(a + b), k)) }
            "sub" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(k.round(a - b), k)) }
            "mul" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(k.round(a * b), k)) }
            "div" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(k.round(a / b), k)) }
            "neg" => Ok(Value::Float(k.round(-a), k)),
            "eq" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a == b)) }
            "ne" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a != b)) }
            "lt" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a < b)) }
            "le" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a > b)) }
            "ge" => { let b = self.expect_float(args, 0)?; Ok(Value::Bool(a >= b)) }
            // ORD3: `compare` is the total order, so a sort keyed on it
            // terminates with every element present. `<`/`>` stay IEEE below.
            "compare" => {
                let b = self.expect_float(args, 0)?;
                Ok(ordering_value(a.total_cmp(&b)))
            }
            "abs" => Ok(Value::Float(k.round(a.abs()), k)),
            "floor" => Ok(Value::Float(k.round(a.floor()), k)),
            "ceil" => Ok(Value::Float(k.round(a.ceil()), k)),
            "round" => Ok(Value::Float(k.round(a.round()), k)),
            "sqrt" => Ok(Value::Float(k.round(a.sqrt()), k)),
            "is_nan" => Ok(Value::Bool(a.is_nan())),
            "is_inf" => Ok(Value::Bool(a.is_infinite())),
            "is_finite" => Ok(Value::Bool(a.is_finite())),
            "to_string" | "debug_string" => Ok(Value::String(Arc::new(Mutex::new(
                k.format(a),
            )))),
            "to_int" => Ok(Value::int(a as i64)),
            // HA4's escape hatch: the raw bit pattern, so a caller who wants a
            // float-keyed Map decides for itself what "the same key" means.
            //
            // Always the f64 pattern, at both widths. MIR mangles f32 and f64
            // receivers to the same `f64_*` calls, so an f32's own 32-bit
            // pattern isn't recoverable there — and one width keeps the two
            // backends from disagreeing about what the key is. Distinct values
            // still get distinct keys, which is all the hatch has to do.
            "to_bits" => Ok(Value::Int(a.to_bits() as i64, crate::value::IntKind::U64)),
            "pow" | "powf" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(k.round(a.powf(b)), k)) }
            "powi" => { let b = self.expect_int(args, 0)?; Ok(Value::Float(k.round(a.powi(b as i32)), k)) }
            "rem" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(k.round(a.rem_euclid(b)), k)) }
            "sin" => Ok(Value::Float(k.round(a.sin()), k)),
            "cos" => Ok(Value::Float(k.round(a.cos()), k)),
            "tan" => Ok(Value::Float(k.round(a.tan()), k)),
            "asin" => Ok(Value::Float(k.round(a.asin()), k)),
            "acos" => Ok(Value::Float(k.round(a.acos()), k)),
            "atan" => Ok(Value::Float(k.round(a.atan()), k)),
            "ln" => Ok(Value::Float(k.round(a.ln()), k)),
            "log10" => Ok(Value::Float(k.round(a.log10()), k)),
            "log2" => Ok(Value::Float(k.round(a.log2()), k)),
            "exp" => Ok(Value::Float(k.round(a.exp()), k)),
            "trunc" => Ok(Value::Float(k.round(a.trunc()), k)),
            "fract" => Ok(Value::Float(k.round(a.fract()), k)),
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
            "ne" => { let b = self.expect_bool(args, 0)?; Ok(Value::Bool(a != b)) }
            // false < true (type.operators support table)
            "lt" => { let b = self.expect_bool(args, 0)?; Ok(Value::Bool(!a & b)) }
            "le" => { let b = self.expect_bool(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_bool(args, 0)?; Ok(Value::Bool(a & !b)) }
            "ge" => { let b = self.expect_bool(args, 0)?; Ok(Value::Bool(a >= b)) }
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
            "ne" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c != other)) }
            // Unicode scalar order (type.operators support table)
            "lt" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c < other)) }
            "le" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c <= other)) }
            "gt" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c > other)) }
            "ge" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c >= other)) }
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

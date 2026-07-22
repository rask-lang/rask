// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Arithmetic, comparison, bitwise, unary, and cast operations.

use rask_mir::{BinOp, MirType, UnaryOp};

use crate::{MiriError, MiriValue};

/// Evaluate a binary operation.
pub fn eval_binop(op: BinOp, left: &MiriValue, right: &MiriValue) -> Result<MiriValue, MiriError> {
    // String concatenation
    if let BinOp::Add = op {
        if let (MiriValue::String(a), MiriValue::String(b)) = (left, right) {
            return Ok(MiriValue::String(format!("{a}{b}")));
        }
    }

    match (left, right) {
        // i64 (most common path)
        (MiriValue::I64(a), MiriValue::I64(b)) => eval_binop_i64(op, *a, *b),
        (MiriValue::I32(a), MiriValue::I32(b)) => eval_binop_i32(op, *a, *b),
        (MiriValue::I16(a), MiriValue::I16(b)) => eval_binop_i16(op, *a, *b),
        (MiriValue::I8(a), MiriValue::I8(b)) => eval_binop_i8(op, *a, *b),

        (MiriValue::U64(a), MiriValue::U64(b)) => eval_binop_u64(op, *a, *b),
        (MiriValue::U32(a), MiriValue::U32(b)) => eval_binop_u32(op, *a, *b),
        (MiriValue::U16(a), MiriValue::U16(b)) => eval_binop_u16(op, *a, *b),
        (MiriValue::U8(a), MiriValue::U8(b)) => eval_binop_u8(op, *a, *b),

        (MiriValue::F64(a), MiriValue::F64(b)) => eval_binop_f64(op, *a, *b),
        (MiriValue::F32(a), MiriValue::F32(b)) => eval_binop_f32(op, *a, *b),

        (MiriValue::Bool(a), MiriValue::Bool(b)) => eval_binop_bool(op, *a, *b),

        (MiriValue::Char(a), MiriValue::Char(b)) => match op {
            BinOp::Eq => Ok(MiriValue::Bool(a == b)),
            BinOp::Ne => Ok(MiriValue::Bool(a != b)),
            BinOp::Lt => Ok(MiriValue::Bool(a < b)),
            BinOp::Gt => Ok(MiriValue::Bool(a > b)),
            BinOp::Le => Ok(MiriValue::Bool(a <= b)),
            BinOp::Ge => Ok(MiriValue::Bool(a >= b)),
            _ => Err(MiriError::UnsupportedOperation(
                format!("binary op {op:?} not supported for char"),
            )),
        },

        _ => Err(MiriError::UnsupportedOperation(
            format!("binary op {op:?} on mismatched types: {left:?} vs {right:?}"),
        )),
    }
}

/// Evaluate a unary operation.
pub fn eval_unaryop(op: UnaryOp, operand: &MiriValue) -> Result<MiriValue, MiriError> {
    match (op, operand) {
        (UnaryOp::Neg, MiriValue::I8(v)) => v.checked_neg().map(MiriValue::I8).ok_or_else(|| neg_ovf(*v, "i8")),
        (UnaryOp::Neg, MiriValue::I16(v)) => v.checked_neg().map(MiriValue::I16).ok_or_else(|| neg_ovf(*v as i64, "i16")),
        (UnaryOp::Neg, MiriValue::I32(v)) => v.checked_neg().map(MiriValue::I32).ok_or_else(|| neg_ovf(*v as i64, "i32")),
        (UnaryOp::Neg, MiriValue::I64(v)) => v.checked_neg().map(MiriValue::I64).ok_or_else(|| neg_ovf(*v, "i64")),
        (UnaryOp::Neg, MiriValue::F32(v)) => Ok(MiriValue::F32(-v)),
        (UnaryOp::Neg, MiriValue::F64(v)) => Ok(MiriValue::F64(-v)),

        (UnaryOp::Not, MiriValue::Bool(v)) => Ok(MiriValue::Bool(!v)),

        (UnaryOp::BitNot, MiriValue::I8(v)) => Ok(MiriValue::I8(!v)),
        (UnaryOp::BitNot, MiriValue::I16(v)) => Ok(MiriValue::I16(!v)),
        (UnaryOp::BitNot, MiriValue::I32(v)) => Ok(MiriValue::I32(!v)),
        (UnaryOp::BitNot, MiriValue::I64(v)) => Ok(MiriValue::I64(!v)),
        (UnaryOp::BitNot, MiriValue::U8(v)) => Ok(MiriValue::U8(!v)),
        (UnaryOp::BitNot, MiriValue::U16(v)) => Ok(MiriValue::U16(!v)),
        (UnaryOp::BitNot, MiriValue::U32(v)) => Ok(MiriValue::U32(!v)),
        (UnaryOp::BitNot, MiriValue::U64(v)) => Ok(MiriValue::U64(!v)),

        _ => Err(MiriError::UnsupportedOperation(
            format!("unary op {op:?} not supported for {operand:?}"),
        )),
    }
}

/// Cast a value to a target MIR type.
pub fn eval_cast(value: &MiriValue, target: &MirType) -> Result<MiriValue, MiriError> {
    let as_i64 = value.to_i64();
    let as_u64 = value.to_u64();
    let as_f64 = value.to_f64();

    match target {
        MirType::I8 => Ok(MiriValue::I8(as_i64.ok_or_else(|| cant_cast(value, target))? as i8)),
        MirType::I16 => Ok(MiriValue::I16(as_i64.ok_or_else(|| cant_cast(value, target))? as i16)),
        MirType::I32 => Ok(MiriValue::I32(as_i64.ok_or_else(|| cant_cast(value, target))? as i32)),
        MirType::I64 => Ok(MiriValue::I64(as_i64.ok_or_else(|| cant_cast(value, target))?)),
        MirType::U8 => Ok(MiriValue::U8(as_u64.ok_or_else(|| cant_cast(value, target))? as u8)),
        MirType::U16 => Ok(MiriValue::U16(as_u64.ok_or_else(|| cant_cast(value, target))? as u16)),
        MirType::U32 => Ok(MiriValue::U32(as_u64.ok_or_else(|| cant_cast(value, target))? as u32)),
        MirType::U64 => Ok(MiriValue::U64(as_u64.ok_or_else(|| cant_cast(value, target))?)),
        MirType::F32 => Ok(MiriValue::F32(as_f64.ok_or_else(|| cant_cast(value, target))? as f32)),
        MirType::F64 => Ok(MiriValue::F64(as_f64.ok_or_else(|| cant_cast(value, target))?)),
        MirType::Bool => match value {
            MiriValue::Bool(b) => Ok(MiriValue::Bool(*b)),
            MiriValue::I64(v) => Ok(MiriValue::Bool(*v != 0)),
            _ => Err(cant_cast(value, target)),
        },
        MirType::Char => match value {
            MiriValue::Char(c) => Ok(MiriValue::Char(*c)),
            MiriValue::U32(v) => {
                char::from_u32(*v)
                    .map(MiriValue::Char)
                    .ok_or_else(|| MiriError::UnsupportedOperation(
                        format!("invalid char codepoint: {v}"),
                    ))
            }
            _ => Err(cant_cast(value, target)),
        },
        _ => Err(cant_cast(value, target)),
    }
}

/// Inclusive range of a MIR integer type, as i128.
fn int_bounds(t: &MirType) -> Option<(i128, i128)> {
    Some(match t {
        MirType::I8 => (i8::MIN as i128, i8::MAX as i128),
        MirType::I16 => (i16::MIN as i128, i16::MAX as i128),
        MirType::I32 => (i32::MIN as i128, i32::MAX as i128),
        MirType::I64 => (i64::MIN as i128, i64::MAX as i128),
        MirType::U8 => (0, u8::MAX as i128),
        MirType::U16 => (0, u16::MAX as i128),
        MirType::U32 => (0, u32::MAX as i128),
        MirType::U64 => (0, u64::MAX as i128),
        _ => return None,
    })
}

fn store_int(t: &MirType, v: i128) -> MiriValue {
    match t {
        MirType::I8 => MiriValue::I8(v as i8),
        MirType::I16 => MiriValue::I16(v as i16),
        MirType::I32 => MiriValue::I32(v as i32),
        MirType::I64 => MiriValue::I64(v as i64),
        MirType::U8 => MiriValue::U8(v as u8),
        MirType::U16 => MiriValue::U16(v as u16),
        MirType::U32 => MiriValue::U32(v as u32),
        MirType::U64 => MiriValue::U64(v as u64),
        _ => MiriValue::I64(v as i64),
    }
}

/// Source integer as its logical value (unsigned reinterprets the bits).
fn logical_i128(value: &MiriValue) -> Option<i128> {
    match value {
        MiriValue::U8(_) | MiriValue::U16(_) | MiriValue::U32(_) | MiriValue::U64(_) => {
            value.to_u64().map(|v| v as i128)
        }
        _ => value.to_i64().map(|v| v as i128),
    }
}

/// Comptime evaluation of an explicit conversion form (CV5–CV10).
pub fn eval_convert(
    value: &MiriValue,
    target: &MirType,
    kind: rask_mir::ConvertKind,
) -> Result<MiriValue, MiriError> {
    use rask_mir::ConvertKind::*;
    let bounds = int_bounds(target);
    match kind {
        // Wrapping truncation — same as the primitive `as` in eval_cast.
        Truncate => eval_cast(value, target),
        Saturate => {
            let (min, max) = bounds.ok_or_else(|| cant_cast(value, target))?;
            let src = logical_i128(value).ok_or_else(|| cant_cast(value, target))?;
            Ok(store_int(target, src.clamp(min, max)))
        }
        FloatToInt | FloatToIntSat => {
            let (min, max) = bounds.ok_or_else(|| cant_cast(value, target))?;
            let f = value.to_f64().ok_or_else(|| cant_cast(value, target))?;
            if f.is_nan() {
                if matches!(kind, FloatToIntSat) {
                    return Ok(store_int(target, 0));
                }
                return Err(MiriError::UnsupportedOperation(format!(
                    "cannot convert NaN to {target:?}"
                )));
            }
            let t = f.trunc();
            if t < min as f64 || t > max as f64 {
                if matches!(kind, FloatToIntSat) {
                    return Ok(store_int(target, if t > 0.0 { max } else { min }));
                }
                return Err(MiriError::UnsupportedOperation(format!(
                    "float {f} out of range for {target:?}"
                )));
            }
            Ok(store_int(target, t as i128))
        }
        // Optional-producing forms aren't evaluated at comptime yet.
        TryConvert | TryFloatToInt => Err(MiriError::UnsupportedOperation(
            "`try convert`/`try float to int` is not supported in comptime evaluation".to_string(),
        )),
    }
}

fn neg_ovf(v: impl std::fmt::Display, ty: &str) -> MiriError {
    MiriError::IntegerOverflow(format!("integer overflow: negating {v} exceeds {ty} range"))
}

fn cant_cast(value: &MiriValue, target: &MirType) -> MiriError {
    MiriError::UnsupportedOperation(
        format!("cannot cast {value:?} to {target:?}"),
    )
}

// --- Typed binop implementations ---
// Macro to reduce repetition across integer types.

// Comptime arithmetic is checked (type.overflow CT1): overflow is a compile
// error, never a silent wrap. The bit width comes from the value's variant.
macro_rules! impl_int_binop {
    ($name:ident, $ty:ty, $variant:ident, $signed:expr) => {
        fn $name(op: BinOp, a: $ty, b: $ty) -> Result<MiriValue, MiriError> {
            let ovf = |sym: &str| MiriError::IntegerOverflow(format!(
                "integer overflow: {} {} {} exceeds {} range", a, sym, b, stringify!($ty)
            ));
            match op {
                BinOp::Add => a.checked_add(b).map(MiriValue::$variant).ok_or_else(|| ovf("+")),
                BinOp::Sub => a.checked_sub(b).map(MiriValue::$variant).ok_or_else(|| ovf("-")),
                BinOp::Mul => a.checked_mul(b).map(MiriValue::$variant).ok_or_else(|| ovf("*")),
                BinOp::Div => {
                    if b == 0 { return Err(MiriError::DivisionByZero); }
                    a.checked_div(b).map(MiriValue::$variant).ok_or_else(|| ovf("/"))
                }
                BinOp::Mod => {
                    if b == 0 { return Err(MiriError::DivisionByZero); }
                    a.checked_rem(b).map(MiriValue::$variant).ok_or_else(|| ovf("%"))
                }
                BinOp::Eq => Ok(MiriValue::Bool(a == b)),
                BinOp::Ne => Ok(MiriValue::Bool(a != b)),
                BinOp::Lt => Ok(MiriValue::Bool(a < b)),
                BinOp::Gt => Ok(MiriValue::Bool(a > b)),
                BinOp::Le => Ok(MiriValue::Bool(a <= b)),
                BinOp::Ge => Ok(MiriValue::Bool(a >= b)),
                BinOp::And => Ok(MiriValue::Bool(a != 0 && b != 0)),
                BinOp::Or => Ok(MiriValue::Bool(a != 0 || b != 0)),
                BinOp::BitAnd => Ok(MiriValue::$variant(a & b)),
                BinOp::BitOr => Ok(MiriValue::$variant(a | b)),
                BinOp::BitXor => Ok(MiriValue::$variant(a ^ b)),
                BinOp::Shl => a.checked_shl(b as u32).map(MiriValue::$variant).ok_or_else(||
                    MiriError::IntegerOverflow(format!(
                        "shift amount {} exceeds {} bit width", b, stringify!($ty)))),
                BinOp::Shr => a.checked_shr(b as u32).map(MiriValue::$variant).ok_or_else(||
                    MiriError::IntegerOverflow(format!(
                        "shift amount {} exceeds {} bit width", b, stringify!($ty)))),
            }
        }
    };
}

impl_int_binop!(eval_binop_i8, i8, I8, true);
impl_int_binop!(eval_binop_i16, i16, I16, true);
impl_int_binop!(eval_binop_i32, i32, I32, true);
impl_int_binop!(eval_binop_i64, i64, I64, true);
impl_int_binop!(eval_binop_u8, u8, U8, false);
impl_int_binop!(eval_binop_u16, u16, U16, false);
impl_int_binop!(eval_binop_u32, u32, U32, false);
impl_int_binop!(eval_binop_u64, u64, U64, false);

fn eval_binop_f64(op: BinOp, a: f64, b: f64) -> Result<MiriValue, MiriError> {
    match op {
        BinOp::Add => Ok(MiriValue::F64(a + b)),
        BinOp::Sub => Ok(MiriValue::F64(a - b)),
        BinOp::Mul => Ok(MiriValue::F64(a * b)),
        BinOp::Div => Ok(MiriValue::F64(a / b)),
        BinOp::Mod => Ok(MiriValue::F64(a % b)),
        BinOp::Eq => Ok(MiriValue::Bool(a == b)),
        BinOp::Ne => Ok(MiriValue::Bool(a != b)),
        BinOp::Lt => Ok(MiriValue::Bool(a < b)),
        BinOp::Gt => Ok(MiriValue::Bool(a > b)),
        BinOp::Le => Ok(MiriValue::Bool(a <= b)),
        BinOp::Ge => Ok(MiriValue::Bool(a >= b)),
        _ => Err(MiriError::UnsupportedOperation(
            format!("binary op {op:?} not supported for f64"),
        )),
    }
}

fn eval_binop_f32(op: BinOp, a: f32, b: f32) -> Result<MiriValue, MiriError> {
    match op {
        BinOp::Add => Ok(MiriValue::F32(a + b)),
        BinOp::Sub => Ok(MiriValue::F32(a - b)),
        BinOp::Mul => Ok(MiriValue::F32(a * b)),
        BinOp::Div => Ok(MiriValue::F32(a / b)),
        BinOp::Mod => Ok(MiriValue::F32(a % b)),
        BinOp::Eq => Ok(MiriValue::Bool(a == b)),
        BinOp::Ne => Ok(MiriValue::Bool(a != b)),
        BinOp::Lt => Ok(MiriValue::Bool(a < b)),
        BinOp::Gt => Ok(MiriValue::Bool(a > b)),
        BinOp::Le => Ok(MiriValue::Bool(a <= b)),
        BinOp::Ge => Ok(MiriValue::Bool(a >= b)),
        _ => Err(MiriError::UnsupportedOperation(
            format!("binary op {op:?} not supported for f32"),
        )),
    }
}

fn eval_binop_bool(op: BinOp, a: bool, b: bool) -> Result<MiriValue, MiriError> {
    match op {
        BinOp::Eq => Ok(MiriValue::Bool(a == b)),
        BinOp::Ne => Ok(MiriValue::Bool(a != b)),
        BinOp::And => Ok(MiriValue::Bool(a && b)),
        BinOp::Or => Ok(MiriValue::Bool(a || b)),
        BinOp::BitAnd => Ok(MiriValue::Bool(a & b)),
        BinOp::BitOr => Ok(MiriValue::Bool(a | b)),
        BinOp::BitXor => Ok(MiriValue::Bool(a ^ b)),
        _ => Err(MiriError::UnsupportedOperation(
            format!("binary op {op:?} not supported for bool"),
        )),
    }
}

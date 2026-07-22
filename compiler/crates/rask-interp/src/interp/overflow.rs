// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Width-aware checked integer arithmetic (type.overflow OV1–OV4, SH1).
//!
//! Each `Value::Int` carries its `IntKind`, so arithmetic is self-describing:
//! the width comes from the operands themselves, not a side table. This checks
//! correctly even in generic code the interpreter never monomorphizes — the
//! concrete value flowing in carries its width. `IntKind::Untyped` (lengths,
//! indices, internally-produced values) has no fixed width and is unchecked,
//! except divide-by-zero which always panics.

use rask_ast::expr::ConvertKind;

use crate::value::{IntKind, Value};

use super::{Interpreter, RuntimeError};

/// Arithmetic operations that can overflow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
}

impl ArithOp {
    fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Rem => "%",
            ArithOp::Shl => "<<",
            ArithOp::Shr => ">>",
        }
    }

    /// The desugared operator method names that can overflow.
    pub fn from_method(name: &str) -> Option<ArithOp> {
        Some(match name {
            "add" => ArithOp::Add,
            "sub" => ArithOp::Sub,
            "mul" => ArithOp::Mul,
            "div" => ArithOp::Div,
            "rem" => ArithOp::Rem,
            "shl" => ArithOp::Shl,
            "shr" => ArithOp::Shr,
            _ => return None,
        })
    }
}

fn min_of(kind: IntKind, bits: u32) -> i128 {
    if kind.signed() { -(1i128 << (bits - 1)) } else { 0 }
}

fn max_of(kind: IntKind, bits: u32) -> i128 {
    if kind.signed() { (1i128 << (bits - 1)) - 1 } else { (1i128 << bits) - 1 }
}

/// Read the stored i64 as this kind's logical value (unsigned kinds
/// reinterpret the bit pattern; u64 above i64::MAX is stored negative).
fn logical(kind: IntKind, raw: i64) -> i128 {
    if kind.signed() { raw as i128 } else { (raw as u64) as i128 }
}

fn store(kind: IntKind, val: i128) -> i64 {
    if kind.signed() { val as i64 } else { (val as u64) as i64 }
}

fn overflow(kind: IntKind, op: ArithOp, a: i128, b: i128, bits: u32) -> RuntimeError {
    RuntimeError::IntegerOverflow(format!(
        "integer overflow: {} {} {} exceeds {} range [{}, {}]",
        a, op.symbol(), b, kind.name(), min_of(kind, bits), max_of(kind, bits)
    ))
}

/// Checked binary arithmetic. `kind` is the operands' shared int kind.
pub(crate) fn checked_binop(
    kind: IntKind,
    op: ArithOp,
    a: i64,
    b: i64,
) -> Result<i64, RuntimeError> {
    // Divide-by-zero panics regardless of width (OV2).
    if matches!(op, ArithOp::Div | ArithOp::Rem) && b == 0 {
        return Err(RuntimeError::DivisionByZero);
    }

    let bits = match kind.bits() {
        Some(bits) => bits,
        // Untyped: unchecked i64 arithmetic (wrapping, never a host panic).
        None => {
            return Ok(match op {
                ArithOp::Add => a.wrapping_add(b),
                ArithOp::Sub => a.wrapping_sub(b),
                ArithOp::Mul => a.wrapping_mul(b),
                ArithOp::Div => a.wrapping_div(b),
                ArithOp::Rem => a.wrapping_rem(b),
                ArithOp::Shl => a.wrapping_shl(b as u32),
                ArithOp::Shr => a.wrapping_shr(b as u32),
            });
        }
    };

    let la = logical(kind, a);
    let lb = logical(kind, b);

    // Shifts: only the amount is checked (SH1); the value wraps to width.
    if matches!(op, ArithOp::Shl | ArithOp::Shr) {
        if lb < 0 || lb >= bits as i128 {
            return Err(RuntimeError::IntegerOverflow(format!(
                "shift amount {} exceeds {} bit width ({})", lb, kind.name(), bits
            )));
        }
        let shifted = match op {
            ArithOp::Shl => la << (lb as u32),
            ArithOp::Shr if kind.signed() => la >> (lb as u32),
            ArithOp::Shr => ((la as u128) >> (lb as u32)) as i128,
            _ => unreachable!(),
        };
        return Ok(store(kind, wrap_to_width(kind, bits, shifted)));
    }

    // Signed MIN / -1 overflows (OV3).
    if matches!(op, ArithOp::Div | ArithOp::Rem) && kind.signed() && la == min_of(kind, bits) && lb == -1 {
        return Err(overflow(kind, op, la, lb, bits));
    }

    let result = match op {
        ArithOp::Add => la.checked_add(lb),
        ArithOp::Sub => la.checked_sub(lb),
        ArithOp::Mul => la.checked_mul(lb),
        ArithOp::Div => Some(la / lb),
        ArithOp::Rem => Some(la % lb),
        _ => unreachable!(),
    };
    match result {
        Some(r) if r >= min_of(kind, bits) && r <= max_of(kind, bits) => Ok(store(kind, r)),
        _ => Err(overflow(kind, op, la, lb, bits)),
    }
}

/// Checked unary negation (OV1).
pub(crate) fn checked_neg(kind: IntKind, a: i64) -> Result<i64, RuntimeError> {
    let bits = match kind.bits() {
        Some(bits) => bits,
        None => return Ok(a.wrapping_neg()),
    };
    let la = logical(kind, a);
    let result = -la;
    if result < min_of(kind, bits) || result > max_of(kind, bits) {
        Err(RuntimeError::IntegerOverflow(format!(
            "integer overflow: negating {} exceeds {} range [{}, {}]",
            la, kind.name(), min_of(kind, bits), max_of(kind, bits)
        )))
    } else {
        Ok(store(kind, result))
    }
}

// ============================================================================
// Explicit lossy conversions (type.primitives CV5–CV10)
// ============================================================================

/// An integer conversion target. `IntKind` covers i8..i64/u8..u64; i128/u128
/// have dedicated `Value` variants and are tracked separately.
#[derive(Clone, Copy)]
enum IntTarget {
    Kind(IntKind),
    I128,
    U128,
}

impl IntTarget {
    fn parse(name: &str) -> Option<IntTarget> {
        match name {
            "i128" => Some(IntTarget::I128),
            "u128" => Some(IntTarget::U128),
            _ => IntKind::from_name(name).map(IntTarget::Kind),
        }
    }

    /// Inclusive range as i128. `U128` is unbounded above in i128 — callers that
    /// need the true upper bound handle it separately.
    fn bounds(self) -> (i128, i128) {
        match self {
            IntTarget::Kind(k) => {
                let bits = k.bits().unwrap_or(64);
                (min_of(k, bits), max_of(k, bits))
            }
            IntTarget::I128 => (i128::MIN, i128::MAX),
            IntTarget::U128 => (0, i128::MAX),
        }
    }

    fn store(self, v: i128) -> Value {
        match self {
            IntTarget::Kind(k) => Value::Int(store(k, v), k),
            IntTarget::I128 => Value::Int128(v),
            IntTarget::U128 => Value::Uint128(v as u128),
        }
    }
}

/// Source integer as its logical value (unsigned kinds reinterpret the bits).
fn int_logical(val: &Value) -> Option<i128> {
    match val {
        Value::Int(n, k) => Some(logical(*k, *n)),
        Value::Int128(n) => Some(*n),
        Value::Uint128(n) => Some(*n as i128),
        _ => None,
    }
}

/// Low 64 bits of the source integer's two's-complement representation.
fn raw_i64(val: &Value) -> Option<i64> {
    match val {
        Value::Int(n, _) => Some(*n),
        Value::Int128(n) => Some(*n as i64),
        Value::Uint128(n) => Some(*n as i64),
        _ => None,
    }
}

fn some(val: Value) -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "Some".to_string(),
        fields: vec![val],
        variant_index: 0,
        origin: None,
    }
}

fn none() -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "None".to_string(),
        fields: vec![],
        variant_index: 1,
        origin: None,
    }
}

fn not_int(target: &str) -> RuntimeError {
    RuntimeError::TypeError(format!("conversion target `{}` is not an integer type", target))
}

/// Evaluate an explicit conversion form (CV5–CV10). `Interpreter`-independent.
pub(crate) fn convert(val: Value, target: &str, kind: ConvertKind) -> Result<Value, RuntimeError> {
    match kind {
        ConvertKind::Truncate => truncate_to(val, target),
        ConvertKind::Saturate => saturate_to(val, target),
        ConvertKind::TryConvert => try_convert_to(val, target),
        ConvertKind::FloatToInt => float_to_int(val, target, false, false),
        ConvertKind::FloatToIntSat => float_to_int(val, target, true, false),
        ConvertKind::TryFloatToInt => float_to_int(val, target, false, true),
    }
}

/// CV5: wrapping/bitwise truncation into the target width.
fn truncate_to(val: Value, target: &str) -> Result<Value, RuntimeError> {
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let raw = raw_i64(&val).ok_or_else(|| RuntimeError::TypeError(
        format!("`truncate to` needs an integer, found {}", val.type_name())))?;
    Ok(match t {
        IntTarget::Kind(k) => Value::Int(k.wrap(raw), k),
        IntTarget::I128 => Value::Int128(int_logical(&val).unwrap_or(raw as i128)),
        IntTarget::U128 => Value::Uint128(match &val {
            Value::Uint128(n) => *n,
            _ => int_logical(&val).unwrap_or(raw as i128) as u128,
        }),
    })
}

/// CV6: clamp to the target range.
fn saturate_to(val: Value, target: &str) -> Result<Value, RuntimeError> {
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let src = int_logical(&val).ok_or_else(|| RuntimeError::TypeError(
        format!("`saturate to` needs an integer, found {}", val.type_name())))?;
    if let IntTarget::U128 = t {
        return Ok(Value::Uint128(if src < 0 { 0 } else { src as u128 }));
    }
    let (min, max) = t.bounds();
    Ok(t.store(src.clamp(min, max)))
}

/// CV7: `T?` — `none` if out of range.
fn try_convert_to(val: Value, target: &str) -> Result<Value, RuntimeError> {
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let src = int_logical(&val).ok_or_else(|| RuntimeError::TypeError(
        format!("`try convert to` needs an integer, found {}", val.type_name())))?;
    if let IntTarget::U128 = t {
        return Ok(if src < 0 { none() } else { some(Value::Uint128(src as u128)) });
    }
    let (min, max) = t.bounds();
    Ok(if src >= min && src <= max { some(t.store(src)) } else { none() })
}

/// CV8/CV9/CV10: float → int, truncating toward zero.
fn float_to_int(val: Value, target: &str, saturating: bool, optional: bool) -> Result<Value, RuntimeError> {
    let f = match val {
        Value::Float(f) => f,
        other => return Err(RuntimeError::TypeError(
            format!("`float to int` needs a float, found {}", other.type_name()))),
    };
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let (min, max) = t.bounds();
    let (min_f, max_f) = (min as f64, max as f64);

    if f.is_nan() {
        if saturating { return Ok(t.store(0)); }
        if optional { return Ok(none()); }
        return Err(RuntimeError::Panic(format!("cannot convert NaN to {}", target)));
    }
    if f.is_infinite() {
        if saturating { return Ok(t.store(if f > 0.0 { max } else { min })); }
        if optional { return Ok(none()); }
        return Err(RuntimeError::Panic(format!("cannot convert {}infinity to {}",
            if f < 0.0 { "-" } else { "" }, target)));
    }
    let truncated = f.trunc();
    if truncated < min_f || truncated > max_f {
        if saturating { return Ok(t.store(if truncated > 0.0 { max } else { min })); }
        if optional { return Ok(none()); }
        return Err(RuntimeError::Panic(format!(
            "float {} out of range for {}", f, target)));
    }
    let v = truncated as i128;
    Ok(if optional { some(t.store(v)) } else { t.store(v) })
}

/// Mask a value into `bits`, sign-extending for signed kinds.
fn wrap_to_width(kind: IntKind, bits: u32, val: i128) -> i128 {
    let mask = (1i128 << bits) - 1;
    let masked = val & mask;
    if kind.signed() && (masked & (1i128 << (bits - 1))) != 0 {
        masked - (1i128 << bits)
    } else {
        masked
    }
}

impl Interpreter {
    /// Intercept the desugared arithmetic operator methods on `Value::Int` and
    /// run them width-aware, reading the width from the operand values. Returns
    /// None to fall through (non-arithmetic method or non-int receiver).
    pub(crate) fn try_checked_int_arith(
        &self,
        receiver: &Value,
        method: &str,
        args: &[Value],
    ) -> Option<Result<Value, RuntimeError>> {
        let (a, ka) = match receiver {
            Value::Int(a, k) => (*a, *k),
            _ => return None,
        };
        if method == "neg" {
            return Some(checked_neg(ka, a).map(|v| Value::Int(v, ka)));
        }
        let op = ArithOp::from_method(method)?;
        let (b, kb) = match args.first() {
            Some(Value::Int(b, k)) => (*b, *k),
            _ => return None,
        };
        let kind = ka.unify(kb);
        Some(checked_binop(kind, op, a, b).map(|v| Value::Int(v, kind)))
    }
}

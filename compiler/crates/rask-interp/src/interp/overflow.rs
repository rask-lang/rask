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

/// Evaluate an explicit conversion form (CV11–CV16). `Interpreter`-independent.
pub(crate) fn convert(val: Value, target: &str, kind: ConvertKind) -> Result<Value, RuntimeError> {
    match kind {
        ConvertKind::Wrap => truncate_to(val, target),
        ConvertKind::Clamp => saturate_to(val, target),
        ConvertKind::CheckedOption => try_convert_to(val, target),
        ConvertKind::To => convert_exact(val, target),
        ConvertKind::Round => convert_rounded(val, target, Rounding::Nearest),
        ConvertKind::Floor => convert_rounded(val, target, Rounding::Down),
        ConvertKind::Ceil => convert_rounded(val, target, Rounding::Up),
    }
}

/// Which way a fraction goes on the way to an integer (CV14–CV16).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rounding {
    Nearest,
    Down,
    Up,
}

/// `ConvertError`'s variants, in declaration order — `stdlib/builtins.rk`.
const CONVERT_ERR_OUT_OF_RANGE: u32 = 0;
const CONVERT_ERR_NOT_EXACT: u32 = 1;
const CONVERT_ERR_NOT_FINITE: u32 = 2;

fn ok(val: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![val],
        variant_index: 0,
        origin: None,
    }
}

fn convert_err(variant: &str, index: u32) -> Value {
    let payload = Value::Enum {
        name: "ConvertError".to_string(),
        variant: variant.to_string(),
        fields: vec![],
        variant_index: index,
        origin: None,
    };
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![payload],
        variant_index: 1,
        origin: None,
    }
}

fn out_of_range() -> Value {
    convert_err("OutOfRange", CONVERT_ERR_OUT_OF_RANGE)
}

fn not_exact() -> Value {
    convert_err("NotExact", CONVERT_ERR_NOT_EXACT)
}

fn not_finite() -> Value {
    convert_err("NotFinite", CONVERT_ERR_NOT_FINITE)
}

/// The float widths a conversion can target.
fn float_target(name: &str) -> Option<crate::value::FloatKind> {
    match name {
        "f32" => Some(crate::value::FloatKind::F32),
        "f64" => Some(crate::value::FloatKind::F64),
        _ => None,
    }
}

fn source_float(val: &Value) -> Option<f64> {
    match val {
        Value::Float(f, _) => Some(*f),
        _ => None,
    }
}

/// CV11: `x.to<T>()` — the value survives exactly, or the conversion fails.
///
/// "Exactly" is the same question CV1 asks about `as`, asked at runtime instead
/// of at compile time: can this value be represented in the target? A float
/// with a fraction can't be an integer, a large `i64` can't be an `f32`, and
/// `NaN` can't be either.
fn convert_exact(val: Value, target: &str) -> Result<Value, RuntimeError> {
    if let Some(fk) = float_target(target) {
        // → float. An integer source is exact when the round trip survives; a
        // float source is exact when narrowing doesn't lose anything.
        let as_f64 = match (&val, int_logical(&val)) {
            (Value::Float(f, _), _) => *f,
            (_, Some(n)) => {
                let f = n as f64;
                if f as i128 != n {
                    return Ok(not_exact());
                }
                f
            }
            _ => return Err(not_numeric("to", &val)),
        };
        let narrowed = narrow_float(fk, as_f64);
        if !as_f64.is_nan() && narrowed != as_f64 {
            return Ok(not_exact());
        }
        return Ok(ok(Value::Float(narrowed, fk)));
    }

    let t = IntTarget::parse(target).ok_or_else(|| not_convertible(target))?;
    if let Some(f) = source_float(&val) {
        return Ok(float_to_int_checked(f, t, Rounding::Nearest, true));
    }
    let src = int_logical(&val).ok_or_else(|| not_numeric("to", &val))?;
    Ok(int_into(t, src))
}

/// CV14–CV16: `round`, `floor`, `ceil`.
fn convert_rounded(val: Value, target: &str, mode: Rounding) -> Result<Value, RuntimeError> {
    if let Some(fk) = float_target(target) {
        // Only `round` reaches a float target, and it can't fail there: an
        // out-of-range `f64` → `f32` gives ±infinity, which is IEEE's answer,
        // not an error (CV14).
        let as_f64 = match (&val, int_logical(&val)) {
            (Value::Float(f, _), _) => *f,
            (_, Some(n)) => n as f64,
            _ => return Err(not_numeric("round", &val)),
        };
        return Ok(Value::Float(narrow_float(fk, as_f64), fk));
    }

    let t = IntTarget::parse(target).ok_or_else(|| not_convertible(target))?;
    let f = source_float(&val).ok_or_else(|| not_numeric("round", &val))?;
    Ok(float_to_int_checked(f, t, mode, false))
}

/// A float into an integer target, with the fraction handled by `mode`.
/// `exact_only` is `to`: a fraction is a failure rather than something to round.
fn float_to_int_checked(f: f64, t: IntTarget, mode: Rounding, exact_only: bool) -> Value {
    if f.is_nan() || f.is_infinite() {
        return not_finite();
    }
    let rounded = if exact_only {
        if f.fract() != 0.0 {
            return not_exact();
        }
        f
    } else {
        match mode {
            // Ties go to even, which is IEEE's own "nearest" and what the
            // hardware instruction does. `round` has to mean one thing at every
            // target: `x.round<f32>()` from an `f64` is IEEE nearest because
            // that's the only nearest a float narrowing has, so the integer
            // target can't quietly be half-away-from-zero instead (CV14).
            Rounding::Nearest => f.round_ties_even(),
            Rounding::Down => f.floor(),
            Rounding::Up => f.ceil(),
        }
    };
    let (min, max) = t.bounds();
    // u128's true ceiling doesn't fit an i128, so it's checked as a float.
    let above = if matches!(t, IntTarget::U128) {
        rounded >= 340282366920938463463374607431768211456.0
    } else {
        rounded > max as f64
    };
    if rounded < min as f64 || above {
        return out_of_range();
    }
    ok(t.store(rounded as i128))
}

/// An integer into an integer target, exactly or not at all.
fn int_into(t: IntTarget, src: i128) -> Value {
    if let IntTarget::U128 = t {
        return if src < 0 { out_of_range() } else { ok(Value::Uint128(src as u128)) };
    }
    let (min, max) = t.bounds();
    if src < min || src > max {
        out_of_range()
    } else {
        ok(t.store(src))
    }
}

/// The value as it survives the target width — an `f32` target rounds to the
/// nearest `f32`, everything else keeps its bits.
fn narrow_float(fk: crate::value::FloatKind, f: f64) -> f64 {
    match fk {
        crate::value::FloatKind::F32 => f as f32 as f64,
        _ => f,
    }
}

fn not_numeric(method: &str, val: &Value) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "`{}` needs a number, found {}",
        method,
        val.type_name()
    ))
}

fn not_convertible(target: &str) -> RuntimeError {
    RuntimeError::TypeError(format!(
        "conversion target `{}` is not a numeric type",
        target
    ))
}

/// CV12: wrapping/bitwise truncation into the target width.
fn truncate_to(val: Value, target: &str) -> Result<Value, RuntimeError> {
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let raw = raw_i64(&val).ok_or_else(|| RuntimeError::TypeError(
        format!("`wrap` needs an integer, found {}", val.type_name())))?;
    Ok(match t {
        IntTarget::Kind(k) => Value::Int(k.wrap(raw), k),
        IntTarget::I128 => Value::Int128(int_logical(&val).unwrap_or(raw as i128)),
        IntTarget::U128 => Value::Uint128(match &val {
            Value::Uint128(n) => *n,
            _ => int_logical(&val).unwrap_or(raw as i128) as u128,
        }),
    })
}

/// CV13: clamp to the target range.
fn saturate_to(val: Value, target: &str) -> Result<Value, RuntimeError> {
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let src = int_logical(&val).ok_or_else(|| RuntimeError::TypeError(
        format!("`clamp` needs an integer, found {}", val.type_name())))?;
    if let IntTarget::U128 = t {
        return Ok(Value::Uint128(if src < 0 { 0 } else { src as u128 }));
    }
    let (min, max) = t.bounds();
    Ok(t.store(src.clamp(min, max)))
}

/// `T?` — `none` if out of range. Compiler-internal (`char.from_u32`).
fn try_convert_to(val: Value, target: &str) -> Result<Value, RuntimeError> {
    let t = IntTarget::parse(target).ok_or_else(|| not_int(target))?;
    let src = int_logical(&val).ok_or_else(|| RuntimeError::TypeError(
        format!("a checked conversion needs an integer, found {}", val.type_name())))?;
    if let IntTarget::U128 = t {
        return Ok(if src < 0 { none() } else { some(Value::Uint128(src as u128)) });
    }
    let (min, max) = t.bounds();
    Ok(if src >= min && src <= max { some(t.store(src)) } else { none() })
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

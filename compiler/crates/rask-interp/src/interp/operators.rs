// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Binary operators and SIMD coercion.

use rask_ast::expr::BinOp;

use crate::value::Value;

use super::overflow::{checked_binop, ArithOp};
use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(super) fn coerce_to_simd_f32x8(value: Value) -> Result<Value, RuntimeError> {
        match value {
            Value::SimdF32x8(_) => Ok(value),
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                let mut arr = [0.0f32; 8];
                for (i, val) in vec.iter().take(8).enumerate() {
                    arr[i] = match val {
                        Value::Float(f, _) => *f as f32,
                        Value::Int(n, _) => *n as f32,
                        _ => 0.0,
                    };
                }
                Ok(Value::SimdF32x8(arr))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "cannot coerce {} to f32x8", value.type_name()
            ))),
        }
    }

    /// Evaluate a binary operation directly (bypasses desugaring). Integer
    /// Order two integers by *value*, whatever widths and signedness they carry.
    ///
    /// An unsigned value lives in the signed i64 slot as its bit pattern, so
    /// u64::MAX is stored as -1. Comparing the slots directly is right only when
    /// both sides read the same way; a mixed pair needs the values (#308). A
    /// negative signed value is below every unsigned one, and two non-negatives
    /// compare as unsigned.
    pub(crate) fn int_value_cmp(
        a: i64,
        ka: crate::value::IntKind,
        b: i64,
        kb: crate::value::IntKind,
    ) -> std::cmp::Ordering {
        match (ka.is_unsigned(), kb.is_unsigned()) {
            (false, false) => a.cmp(&b),
            (true, true) => (a as u64).cmp(&(b as u64)),
            // One side unsigned: a negative signed operand is the smaller one,
            // and otherwise both are non-negative so unsigned order is right.
            (true, false) => {
                if b < 0 { std::cmp::Ordering::Greater } else { (a as u64).cmp(&(b as u64)) }
            }
            (false, true) => {
                if a < 0 { std::cmp::Ordering::Less } else { (a as u64).cmp(&(b as u64)) }
            }
        }
    }

    /// arithmetic reads the width from the operand values' IntKind and is
    /// width-aware checked (type.overflow). Used for the interpolation path
    /// that skips desugaring.
    pub(super) fn eval_binop(
        &self,
        op: BinOp,
        l: Value,
        r: Value,
    ) -> Result<Value, RuntimeError> {
        // Width-aware checked integer arithmetic (width from the values).
        if let (Value::Int(a, ka), Value::Int(b, kb)) = (&l, &r) {
            let arith = match op {
                BinOp::Add => Some(ArithOp::Add),
                BinOp::Sub => Some(ArithOp::Sub),
                BinOp::Mul => Some(ArithOp::Mul),
                BinOp::Div => Some(ArithOp::Div),
                BinOp::Mod => Some(ArithOp::Rem),
                BinOp::Shl => Some(ArithOp::Shl),
                BinOp::Shr => Some(ArithOp::Shr),
                _ => None,
            };
            if let Some(arith_op) = arith {
                let kind = ka.unify(*kb);
                return checked_binop(kind, arith_op, *a, *b).map(|v| Value::Int(v, kind));
            }
        }
        match (op, &l, &r) {
            // The result goes back on the operand width's grid. Skipping that
            // for f32 is what let the interpreter drift away from native: an
            // f32 accumulator kept f64 precision and crossed a `>=` threshold
            // a frame later than the same program does when compiled.
            (BinOp::Add, Value::Float(a, ka), Value::Float(b, kb)) => {
                let k = ka.unify(*kb);
                Ok(Value::Float(k.round(a + b), k))
            }
            (BinOp::Sub, Value::Float(a, ka), Value::Float(b, kb)) => {
                let k = ka.unify(*kb);
                Ok(Value::Float(k.round(a - b), k))
            }
            (BinOp::Mul, Value::Float(a, ka), Value::Float(b, kb)) => {
                let k = ka.unify(*kb);
                Ok(Value::Float(k.round(a * b), k))
            }
            (BinOp::Div, Value::Float(a, ka), Value::Float(b, kb)) => {
                let k = ka.unify(*kb);
                Ok(Value::Float(k.round(a / b), k))
            }
            (BinOp::Add, Value::SimdF32x8(a), Value::SimdF32x8(b)) => {
                let mut r = [0.0f32; 8];
                for i in 0..8 { r[i] = a[i] + b[i]; }
                Ok(Value::SimdF32x8(r))
            }
            (BinOp::Sub, Value::SimdF32x8(a), Value::SimdF32x8(b)) => {
                let mut r = [0.0f32; 8];
                for i in 0..8 { r[i] = a[i] - b[i]; }
                Ok(Value::SimdF32x8(r))
            }
            (BinOp::Mul, Value::SimdF32x8(a), Value::SimdF32x8(b)) => {
                let mut r = [0.0f32; 8];
                for i in 0..8 { r[i] = a[i] * b[i]; }
                Ok(Value::SimdF32x8(r))
            }
            (BinOp::Div, Value::SimdF32x8(a), Value::SimdF32x8(b)) => {
                let mut r = [0.0f32; 8];
                for i in 0..8 { r[i] = a[i] / b[i]; }
                Ok(Value::SimdF32x8(r))
            }
            (BinOp::Eq, _, _) => Ok(Value::Bool(Self::value_eq(&l, &r))),
            (BinOp::Ne, _, _) => Ok(Value::Bool(!Self::value_eq(&l, &r))),
            // An unsigned value is carried in the signed i64 slot as its bit
            // pattern, so `<` on the slots is only right when both sides read the
            // same way. Comparing by value is what makes a mixed pair correct
            // (#308): u64::MAX is not -1, and 5 > -1.
            (BinOp::Lt, Value::Int(a, ka), Value::Int(b, kb)) =>
                Ok(Value::Bool(Self::int_value_cmp(*a, *ka, *b, *kb) == std::cmp::Ordering::Less)),
            (BinOp::Gt, Value::Int(a, ka), Value::Int(b, kb)) =>
                Ok(Value::Bool(Self::int_value_cmp(*a, *ka, *b, *kb) == std::cmp::Ordering::Greater)),
            (BinOp::Le, Value::Int(a, ka), Value::Int(b, kb)) =>
                Ok(Value::Bool(Self::int_value_cmp(*a, *ka, *b, *kb) != std::cmp::Ordering::Greater)),
            (BinOp::Ge, Value::Int(a, ka), Value::Int(b, kb)) =>
                Ok(Value::Bool(Self::int_value_cmp(*a, *ka, *b, *kb) != std::cmp::Ordering::Less)),
            (BinOp::Lt, Value::Float(a, _), Value::Float(b, _)) => Ok(Value::Bool(a < b)),
            (BinOp::Gt, Value::Float(a, _), Value::Float(b, _)) => Ok(Value::Bool(a > b)),
            (BinOp::Le, Value::Float(a, _), Value::Float(b, _)) => Ok(Value::Bool(a <= b)),
            (BinOp::Ge, Value::Float(a, _), Value::Float(b, _)) => Ok(Value::Bool(a >= b)),
            // Bitwise (no overflow); shifts and arithmetic are handled above.
            (BinOp::BitAnd, Value::Int(a, ka), Value::Int(b, kb)) => Ok(Value::Int(a & b, ka.unify(*kb))),
            (BinOp::BitOr, Value::Int(a, ka), Value::Int(b, kb)) => Ok(Value::Int(a | b, ka.unify(*kb))),
            (BinOp::BitXor, Value::Int(a, ka), Value::Int(b, kb)) => Ok(Value::Int(a ^ b, ka.unify(*kb))),
            _ => Err(RuntimeError::TypeError(format!(
                "unsupported binary op {:?} on {} and {}", op, l.type_name(), r.type_name()
            ))),
        }
    }
}


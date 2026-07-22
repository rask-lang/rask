// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Binary operators and SIMD coercion.

use rask_ast::expr::BinOp;

use crate::value::Value;

use super::overflow::{checked_binop, ArithOp, IntWidth};
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
                        Value::Float(f) => *f as f32,
                        Value::Int(n) => *n as f32,
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

    /// Evaluate a binary operation directly (bypasses desugaring).
    /// `width` is the operands' static integer type, when known — arithmetic
    /// on `Value::Int` is then width-aware and panics on overflow. When None
    /// (e.g. generic code), it falls back to unchecked i64 arithmetic.
    pub(super) fn eval_binop(
        &self,
        op: BinOp,
        l: Value,
        r: Value,
        width: Option<IntWidth>,
    ) -> Result<Value, RuntimeError> {
        // Width-aware checked integer arithmetic when the type is known.
        if let (Some(w), Value::Int(a), Value::Int(b)) = (width, &l, &r) {
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
                return checked_binop(w, arith_op, *a, *b).map(Value::Int);
            }
        }
        match (op, &l, &r) {
            (BinOp::Add, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (BinOp::Sub, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
            (BinOp::Mul, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
            (BinOp::Div, Value::Int(a), Value::Int(b)) => {
                if *b == 0 { return Err(RuntimeError::DivisionByZero); }
                Ok(Value::Int(a / b))
            }
            (BinOp::Mod, Value::Int(a), Value::Int(b)) => {
                if *b == 0 { return Err(RuntimeError::DivisionByZero); }
                Ok(Value::Int(a % b))
            }
            (BinOp::Add, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (BinOp::Sub, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (BinOp::Mul, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (BinOp::Div, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
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
            (BinOp::Lt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
            (BinOp::Gt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
            (BinOp::Le, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
            (BinOp::Ge, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
            (BinOp::Lt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
            (BinOp::Gt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
            (BinOp::Le, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
            (BinOp::Ge, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
            (BinOp::BitAnd, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a & b)),
            (BinOp::BitOr, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a | b)),
            (BinOp::BitXor, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a ^ b)),
            (BinOp::Shl, Value::Int(a), Value::Int(b)) => {
                // Default integer width is i32; perform shift in i32 if
                // operands fit, then sign-extend back to i64.
                if *a >= i32::MIN as i64 && *a <= i32::MAX as i64
                    && *b >= 0 && *b < 32
                {
                    Ok(Value::Int(((*a as i32) << (*b as u32)) as i64))
                } else {
                    Ok(Value::Int(a << b))
                }
            }
            (BinOp::Shr, Value::Int(a), Value::Int(b)) => {
                if *a >= i32::MIN as i64 && *a <= i32::MAX as i64
                    && *b >= 0 && *b < 32
                {
                    Ok(Value::Int(((*a as i32) >> (*b as u32)) as i64))
                } else {
                    Ok(Value::Int(a >> b))
                }
            }
            _ => Err(RuntimeError::TypeError(format!(
                "unsupported binary op {:?} on {} and {}", op, l.type_name(), r.type_name()
            ))),
        }
    }
}


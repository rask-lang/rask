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
                        Value::Float(f) => *f as f32,
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
            (BinOp::Lt, Value::Int(a, _), Value::Int(b, _)) => Ok(Value::Bool(a < b)),
            (BinOp::Gt, Value::Int(a, _), Value::Int(b, _)) => Ok(Value::Bool(a > b)),
            (BinOp::Le, Value::Int(a, _), Value::Int(b, _)) => Ok(Value::Bool(a <= b)),
            (BinOp::Ge, Value::Int(a, _), Value::Int(b, _)) => Ok(Value::Bool(a >= b)),
            (BinOp::Lt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
            (BinOp::Gt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
            (BinOp::Le, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
            (BinOp::Ge, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
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


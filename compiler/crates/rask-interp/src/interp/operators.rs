// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Binary operators and SIMD coercion.

use rask_ast::expr::BinOp;

use crate::value::Value;

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
    pub(super) fn eval_binop(&self, op: BinOp, l: Value, r: Value) -> Result<Value, RuntimeError> {
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
            (BinOp::Add, Value::String(a), Value::String(b)) => {
                let s = format!("{}{}", a.lock().unwrap(), b.lock().unwrap());
                Ok(Value::String(std::sync::Arc::new(std::sync::Mutex::new(s))))
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
            (BinOp::Shl, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a << b)),
            (BinOp::Shr, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a >> b)),
            _ => Err(RuntimeError::TypeError(format!(
                "unsupported binary op {:?} on {} and {}", op, l.type_name(), r.type_name()
            ))),
        }
    }
}


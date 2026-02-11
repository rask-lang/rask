// SPDX-License-Identifier: (MIT OR Apache-2.0)
#![allow(dead_code)]
//! Math module methods (math.*).
//!
//! Layer: PURE — mathematical functions, no OS access.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle math module methods.
    pub(crate) fn call_math_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // Trigonometric functions
            "sin" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.sin()))
            }
            "cos" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.cos()))
            }
            "tan" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.tan()))
            }
            "asin" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.asin()))
            }
            "acos" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.acos()))
            }
            "atan" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.atan()))
            }
            "atan2" => {
                let y = self.expect_float_or_int(&args, 0)?;
                let x = self.expect_float_or_int(&args, 1)?;
                Ok(Value::Float(y.atan2(x)))
            }

            // Exponential and logarithmic
            "exp" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.exp()))
            }
            "ln" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.ln()))
            }
            "log2" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.log2()))
            }
            "log10" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.log10()))
            }

            // Multi-argument
            "hypot" => {
                let x = self.expect_float_or_int(&args, 0)?;
                let y = self.expect_float_or_int(&args, 1)?;
                Ok(Value::Float(x.hypot(y)))
            }
            "clamp" => {
                // Works for both int and float
                match &args[0] {
                    Value::Int(x) => {
                        let lo = self.expect_int(&args, 1)?;
                        let hi = self.expect_int(&args, 2)?;
                        Ok(Value::Int((*x).max(lo).min(hi)))
                    }
                    Value::Float(x) => {
                        let lo = self.expect_float_or_int(&args, 1)?;
                        let hi = self.expect_float_or_int(&args, 2)?;
                        Ok(Value::Float(x.max(lo).min(hi)))
                    }
                    _ => Err(RuntimeError::TypeError(
                        "math.clamp: expected numeric type".to_string(),
                    )),
                }
            }

            // Conversion
            "to_radians" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.to_radians()))
            }
            "to_degrees" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Float(x.to_degrees()))
            }

            // Classification
            "is_nan" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Bool(x.is_nan()))
            }
            "is_inf" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Bool(x.is_infinite()))
            }
            "is_finite" => {
                let x = self.expect_float_or_int(&args, 0)?;
                Ok(Value::Bool(x.is_finite()))
            }

            _ => Err(RuntimeError::NoSuchMethod {
                ty: "math".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle math module field access (constants).
    pub(crate) fn get_math_field(&self, field: &str) -> Result<Value, RuntimeError> {
        match field {
            "PI" => Ok(Value::Float(std::f64::consts::PI)),
            "E" => Ok(Value::Float(std::f64::consts::E)),
            "TAU" => Ok(Value::Float(std::f64::consts::TAU)),
            "INF" => Ok(Value::Float(f64::INFINITY)),
            "NEG_INF" => Ok(Value::Float(f64::NEG_INFINITY)),
            "NAN" => Ok(Value::Float(f64::NAN)),
            _ => Err(RuntimeError::TypeError(format!(
                "math module has no member '{}'",
                field
            ))),
        }
    }

    /// Helper: extract f64 from Int or Float.
    fn expect_float_or_int(&self, args: &[Value], idx: usize) -> Result<f64, RuntimeError> {
        match args.get(idx) {
            Some(Value::Float(f)) => Ok(*f),
            Some(Value::Int(n)) => Ok(*n as f64),
            Some(other) => Err(RuntimeError::TypeError(format!(
                "expected number, found {}",
                other.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }
}

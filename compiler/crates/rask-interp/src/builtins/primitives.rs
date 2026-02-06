//! Methods on primitive types: int, float, bool, char.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle integer method calls.
    pub(crate) fn call_int_method(
        &self,
        a: i64,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        match method {
            "add" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a + b)) }
            "sub" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a - b)) }
            "mul" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a * b)) }
            "div" => {
                let b = self.expect_int(args, 0)?;
                if b == 0 { return Err(RuntimeError::DivisionByZero); }
                Ok(Value::Int(a / b))
            }
            "rem" => {
                let b = self.expect_int(args, 0)?;
                if b == 0 { return Err(RuntimeError::DivisionByZero); }
                Ok(Value::Int(a % b))
            }
            "neg" => Ok(Value::Int(-a)),
            "eq" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a == b)) }
            "lt" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a < b)) }
            "le" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a <= b)) }
            "gt" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a > b)) }
            "ge" => { let b = self.expect_int(args, 0)?; Ok(Value::Bool(a >= b)) }
            "bit_and" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a & b)) }
            "bit_or" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a | b)) }
            "bit_xor" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a ^ b)) }
            "shl" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a << b)) }
            "shr" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a >> b)) }
            "bit_not" => Ok(Value::Int(!a)),
            "abs" => Ok(Value::Int(a.abs())),
            "min" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a.min(b))) }
            "max" => { let b = self.expect_int(args, 0)?; Ok(Value::Int(a.max(b))) }
            "to_string" => Ok(Value::String(Arc::new(Mutex::new(a.to_string())))),
            "to_float" => Ok(Value::Float(a as f64)),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "i64".to_string(),
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
            "abs" => Ok(Value::Float(a.abs())),
            "floor" => Ok(Value::Float(a.floor())),
            "ceil" => Ok(Value::Float(a.ceil())),
            "round" => Ok(Value::Float(a.round())),
            "sqrt" => Ok(Value::Float(a.sqrt())),
            "min" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.min(b))) }
            "max" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.max(b))) }
            "to_string" => Ok(Value::String(Arc::new(Mutex::new(a.to_string())))),
            "to_int" => Ok(Value::Int(a as i64)),
            "pow" => { let b = self.expect_float(args, 0)?; Ok(Value::Float(a.powf(b))) }
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
            "is_alphabetic" => Ok(Value::Bool(c.is_alphabetic())),
            "is_alphanumeric" => Ok(Value::Bool(c.is_alphanumeric())),
            "is_digit" => Ok(Value::Bool(c.is_ascii_digit())),
            "is_uppercase" => Ok(Value::Bool(c.is_uppercase())),
            "is_lowercase" => Ok(Value::Bool(c.is_lowercase())),
            "to_uppercase" => Ok(Value::Char(c.to_uppercase().next().unwrap_or(c))),
            "to_lowercase" => Ok(Value::Char(c.to_lowercase().next().unwrap_or(c))),
            "eq" => { let other = self.expect_char(args, 0)?; Ok(Value::Bool(c == other)) }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "char".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

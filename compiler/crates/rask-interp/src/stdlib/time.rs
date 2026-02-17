// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Time module methods, Duration/Instant instance and static methods.
//!
//! Layer: HYBRID — Duration is pure arithmetic, Instant/sleep need OS.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    // === RUNTIME: module-level functions (sleep) ===

    /// Handle time module methods.
    pub(crate) fn call_time_module_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "sleep" => {
                let duration_nanos = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                let duration = std::time::Duration::from_nanos(duration_nanos);
                std::thread::sleep(duration);
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "time".to_string(),
                method: method.to_string(),
            }),
        }
    }

    // === PURE: Duration instance methods (can be Rask) ===

    /// Handle Duration instance methods.
    pub(crate) fn call_duration_method(
        &self,
        nanos: u64,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "as_secs" => Ok(Value::Int((nanos / 1_000_000_000) as i64)),
            "as_millis" => Ok(Value::Int((nanos / 1_000_000) as i64)),
            "as_micros" => Ok(Value::Int((nanos / 1_000) as i64)),
            "as_nanos" => Ok(Value::Int(nanos as i64)),
            "as_secs_f32" => Ok(Value::Float(nanos as f64 / 1_000_000_000.0)),
            "as_secs_f64" => Ok(Value::Float(nanos as f64 / 1_000_000_000.0)),
            // Arithmetic
            "add" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(nanos.wrapping_add(other)))
            }
            "sub" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(nanos.wrapping_sub(other)))
            }
            // Comparisons
            "eq" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(nanos == other))
            }
            "lt" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(nanos < other))
            }
            "le" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(nanos <= other))
            }
            "gt" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(nanos > other))
            }
            "ge" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(nanos >= other))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Duration".to_string(),
                method: method.to_string(),
            }),
        }
    }

    // === RUNTIME: Instant instance methods (needs OS clock) ===

    /// Handle Instant instance methods.
    pub(crate) fn call_instant_method(
        &self,
        instant: &std::time::Instant,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "duration_since" => {
                let other_instant = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                let duration = instant.duration_since(other_instant);
                Ok(Value::Duration(duration.as_nanos() as u64))
            }
            "elapsed" => {
                let duration = instant.elapsed();
                Ok(Value::Duration(duration.as_nanos() as u64))
            }
            // Arithmetic: instant + duration -> Instant
            "add" => {
                let dur_nanos = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Instant(*instant + std::time::Duration::from_nanos(dur_nanos)))
            }
            // Subtraction: instant - instant -> Duration, instant - duration -> Instant
            "sub" => {
                let arg = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                match arg {
                    Value::Instant(other) => {
                        let duration = instant.duration_since(*other);
                        Ok(Value::Duration(duration.as_nanos() as u64))
                    }
                    Value::Duration(dur_nanos) => {
                        Ok(Value::Instant(*instant - std::time::Duration::from_nanos(*dur_nanos)))
                    }
                    _ => Err(RuntimeError::TypeError(
                        format!("Instant.sub expects Instant or Duration, got {}", arg.type_name()),
                    )),
                }
            }
            // Comparisons
            "eq" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(*instant == other))
            }
            "lt" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(*instant < other))
            }
            "le" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(*instant <= other))
            }
            "gt" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(*instant > other))
            }
            "ge" => {
                let other = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Bool(*instant >= other))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Instant".to_string(),
                method: method.to_string(),
            }),
        }
    }

    // === HYBRID: static constructors (Duration pure, Instant.now runtime) ===

    /// Handle Duration/Instant static methods (type methods).
    pub(crate) fn call_time_type_method(
        &self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match (type_name, method) {
            ("Instant", "now") => {
                if !args.is_empty() {
                    return Err(RuntimeError::ArityMismatch {
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Value::Instant(std::time::Instant::now()))
            }
            ("Duration", "seconds") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000_000_000))
            }
            ("Duration", "millis") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000_000))
            }
            ("Duration", "micros") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000))
            }
            ("Duration", "nanos") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n))
            }
            ("Duration", "from_secs_f64") => {
                let secs = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_f64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                let nanos = (secs * 1_000_000_000.0) as u64;
                Ok(Value::Duration(nanos))
            }
            ("Duration", "from_millis") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000_000))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "type {} has no method '{}'",
                type_name, method
            ))),
        }
    }
}

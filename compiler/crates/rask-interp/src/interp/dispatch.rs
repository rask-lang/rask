// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Value calling, builtin dispatch, method routing, and type extractors.

use std::sync::{Arc, Mutex};

use crate::value::{BuiltinKind, Value};

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(crate) fn call_value(&mut self, func: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match func {
            Value::Function { name } => {
                if let Some(decl) = self.functions.get(&name).cloned() {
                    self.call_function(&decl, args)
                } else {
                    Err(RuntimeError::UndefinedFunction(name))
                }
            }
            Value::Builtin(kind) => {
                // Handle AsyncSpawn separately as it needs mutable access
                if kind == BuiltinKind::AsyncSpawn {
                    return self.spawn_async_task(args);
                }
                self.call_builtin(kind, args)
            }
            Value::EnumConstructor {
                enum_name,
                variant_name,
                field_count,
            } => {
                if args.len() != field_count {
                    return Err(RuntimeError::ArityMismatch {
                        expected: field_count,
                        got: args.len(),
                    });
                }
                Ok(Value::Enum {
                    name: enum_name,
                    variant: variant_name,
                    fields: args,
                })
            }
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                self.env.push_scope();
                for (name, val) in captured_env {
                    self.env.define(name, val);
                }
                for (param, arg) in params.iter().zip(args.into_iter()) {
                    self.env.define(param.clone(), arg);
                }
                let result = self.eval_expr(&body);
                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            _ => Err(RuntimeError::TypeError(format!(
                "{} is not callable",
                func.type_name()
            ))),
        }
    }

    /// Call a built-in function.
    fn call_builtin(&self, kind: BuiltinKind, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match kind {
            BuiltinKind::Println => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write_output(" ");
                    }
                    self.write_output(&format!("{}", arg));
                }
                self.write_output_ln();
                Ok(Value::Unit)
            }
            BuiltinKind::Print => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write_output(" ");
                    }
                    self.write_output(&format!("{}", arg));
                }
                Ok(Value::Unit)
            }
            BuiltinKind::Panic => {
                let msg = args
                    .first()
                    .map(|v| format!("{}", v))
                    .unwrap_or_else(|| "panic".to_string());
                Err(RuntimeError::Panic(msg))
            }
            BuiltinKind::AsyncSpawn => {
                // This should have been handled in call_value
                unreachable!("AsyncSpawn should be handled in call_value")
            }
            BuiltinKind::Format => {
                if args.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "format() requires at least one argument (template string)".into(),
                    ));
                }
                match &args[0] {
                    Value::String(s) => {
                        let template = s.lock().unwrap().clone();
                        let result = self.format_string(&template, &args[1..])?;
                        Ok(Value::String(Arc::new(Mutex::new(result))))
                    }
                    _ => Err(RuntimeError::TypeError(
                        "format() first argument must be a string".into(),
                    )),
                }
            }
        }
    }

    pub(super) fn call_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match &receiver {
            Value::Module(module) => self.call_module_method(module, method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::File(f) => self.call_file_method(f, method, args),
            Value::Duration(nanos) => self.call_duration_method(*nanos, method),
            Value::Instant(instant) => self.call_instant_method(instant, method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct { name, fields, .. } if name == "Metadata" => {
                self.call_metadata_method(fields, method)
            }
            Value::Struct { name, fields, .. } if name == "Path" => {
                self.call_path_instance_method(fields, method, args)
            }
            Value::Struct { name, fields, .. } if name == "Args" => {
                self.call_args_method(fields, method, args)
            }
            Value::Enum { name, variant, fields } if name == "JsonValue" => {
                self.call_json_value_method(variant, fields, method)
            }
            Value::SimdF32x8(data) => match method {
                "sum" => {
                    let sum: f32 = data.iter().sum();
                    Ok(Value::Float(sum as f64))
                }
                "add" | "sub" | "mul" | "div" => {
                    if args.is_empty() {
                        return Err(RuntimeError::TypeError(format!("f32x8.{} requires an argument", method)));
                    }
                    let other = match &args[0] {
                        Value::SimdF32x8(d) => d,
                        _ => return Err(RuntimeError::TypeError(format!(
                            "f32x8.{} expects f32x8, found {}", method, args[0].type_name()
                        ))),
                    };
                    let mut r = [0.0f32; 8];
                    for i in 0..8 {
                        r[i] = match method {
                            "add" => data[i] + other[i],
                            "sub" => data[i] - other[i],
                            "mul" => data[i] * other[i],
                            "div" => data[i] / other[i],
                            _ => unreachable!(),
                        };
                    }
                    Ok(Value::SimdF32x8(r))
                }
                _ => Err(RuntimeError::TypeError(format!(
                    "f32x8 has no method '{}'", method
                ))),
            },
            _ => self.call_builtin_method(receiver, method, args),
        }
    }
    /// Helper to extract an integer from args.
    pub(crate) fn expect_int(&self, args: &[Value], idx: usize) -> Result<i64, RuntimeError> {
        match args.get(idx) {
            Some(Value::Int(n)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected int, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a float from args.
    pub(crate) fn expect_float(&self, args: &[Value], idx: usize) -> Result<f64, RuntimeError> {
        match args.get(idx) {
            Some(Value::Float(n)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected float, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a bool from args.
    pub(crate) fn expect_bool(&self, args: &[Value], idx: usize) -> Result<bool, RuntimeError> {
        match args.get(idx) {
            Some(Value::Bool(b)) => Ok(*b),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected bool, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a string from args.
    pub(crate) fn expect_string(&self, args: &[Value], idx: usize) -> Result<String, RuntimeError> {
        match args.get(idx) {
            Some(Value::String(s)) => Ok(s.lock().unwrap().clone()),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected string, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a char from args.
    pub(crate) fn expect_char(&self, args: &[Value], idx: usize) -> Result<char, RuntimeError> {
        match args.get(idx) {
            Some(Value::Char(c)) => Ok(*c),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected char, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract an i128 from args.
    pub(crate) fn expect_int128(&self, args: &[Value], idx: usize) -> Result<i128, RuntimeError> {
        match args.get(idx) {
            Some(Value::Int128(n)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected i128, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a u128 from args.
    pub(crate) fn expect_uint128(&self, args: &[Value], idx: usize) -> Result<u128, RuntimeError> {
        match args.get(idx) {
            Some(Value::Uint128(n)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected u128, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Check if a value is truthy.
    pub(super) fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Bool(b) => *b,
            Value::Unit => false,
            Value::Int(0) => false,
            _ => true,
        }
    }
}

#[cfg(test)]
impl Interpreter {
    /// Check if a method dispatches (doesn't return NoSuchMethod).
    /// A panic from inside a match arm counts as "implemented" — the method
    /// was dispatched, it just has a bug with arg handling.
    pub(crate) fn has_method_dispatch(&mut self, value: Value, method: &str) -> bool {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.call_method(value, method, vec![])
        }));
        !matches!(result, Ok(Err(RuntimeError::NoSuchMethod { .. })))
    }

    /// Check if a module method dispatches.
    pub(crate) fn has_module_dispatch(
        &mut self,
        module: &crate::value::ModuleKind,
        method: &str,
    ) -> bool {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.call_module_method(module, method, vec![])
        }));
        !matches!(result, Ok(Err(RuntimeError::NoSuchMethod { .. })))
    }
}


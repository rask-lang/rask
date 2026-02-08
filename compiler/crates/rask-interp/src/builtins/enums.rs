// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on Result and Option enum types.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle Result method calls.
    pub(crate) fn call_result_method(
        &mut self,
        variant: &str,
        fields: &[Value],
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "map_err" => match variant {
                "Ok" => Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: fields.to_vec(),
                }),
                "Err" => {
                    let closure = args.into_iter().next().ok_or_else(|| {
                        RuntimeError::ArityMismatch { expected: 1, got: 0 }
                    })?;
                    let err_val = fields.first().cloned().unwrap_or(Value::Unit);
                    let mapped = self.call_value(closure, vec![err_val])?;
                    Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![mapped],
                    })
                }
                _ => Err(RuntimeError::TypeError("expected Result.Ok or Result.Err variant".to_string())),
            },
            "map" => match variant {
                "Ok" => {
                    let closure = args.into_iter().next().ok_or_else(|| {
                        RuntimeError::ArityMismatch { expected: 1, got: 0 }
                    })?;
                    let ok_val = fields.first().cloned().unwrap_or(Value::Unit);
                    let mapped = self.call_value(closure, vec![ok_val])?;
                    Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![mapped],
                    })
                }
                "Err" => Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Err".to_string(),
                    fields: fields.to_vec(),
                }),
                _ => Err(RuntimeError::TypeError("expected Result.Ok or Result.Err variant".to_string())),
            },
            "ok" => match variant {
                "Ok" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: fields.to_vec(),
                }),
                "Err" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                }),
                _ => Err(RuntimeError::TypeError("expected Result.Ok or Result.Err variant".to_string())),
            },
            "unwrap_or" => match variant {
                "Ok" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                "Err" => Ok(args.into_iter().next().unwrap_or(Value::Unit)),
                _ => Err(RuntimeError::TypeError("expected Result.Ok or Result.Err variant".to_string())),
            },
            "is_ok" => Ok(Value::Bool(variant == "Ok")),
            "is_err" => Ok(Value::Bool(variant == "Err")),
            "unwrap" => match variant {
                "Ok" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                "Err" => Err(RuntimeError::Panic(format!(
                    "called unwrap on Err: {}",
                    fields.first().map(|v| format!("{}", v)).unwrap_or_default()
                ))),
                _ => Err(RuntimeError::TypeError("expected Result.Ok or Result.Err variant".to_string())),
            },
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Result".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Option method calls.
    pub(crate) fn call_option_method(
        &mut self,
        variant: &str,
        fields: &[Value],
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "unwrap_or" => match variant {
                "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                "None" => Ok(args.into_iter().next().unwrap_or(Value::Unit)),
                _ => Err(RuntimeError::TypeError("expected Option.Some or Option.None variant".to_string())),
            },
            "is_some" => Ok(Value::Bool(variant == "Some")),
            "is_none" => Ok(Value::Bool(variant == "None")),
            "map" => match variant {
                "Some" => {
                    let closure = args.into_iter().next().ok_or_else(|| {
                        RuntimeError::ArityMismatch { expected: 1, got: 0 }
                    })?;
                    let inner = fields.first().cloned().unwrap_or(Value::Unit);
                    let mapped = self.call_value(closure, vec![inner])?;
                    Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![mapped],
                    })
                }
                "None" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                }),
                _ => Err(RuntimeError::TypeError("expected Option.Some or Option.None variant".to_string())),
            },
            "unwrap" => match variant {
                "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                "None" => Err(RuntimeError::Panic("called unwrap on None".to_string())),
                _ => Err(RuntimeError::TypeError("expected Option.Some or Option.None variant".to_string())),
            },
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Option".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

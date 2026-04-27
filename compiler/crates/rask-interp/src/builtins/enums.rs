// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on Result and Option enum types.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

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
                    variant_index: 0, origin: None,
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
                        variant_index: 0, origin: None,
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
                        variant_index: 0, origin: None,
                    })
                }
                "Err" => Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Err".to_string(),
                    fields: fields.to_vec(),
                    variant_index: 0, origin: None,
                }),
                _ => Err(RuntimeError::TypeError("expected Result.Ok or Result.Err variant".to_string())),
            },
            "ok" | "to_option" => match variant {
                "Ok" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: fields.to_vec(),
                    variant_index: 0, origin: None,
                }),
                "Err" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
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
            // `x == none` desugars to `x.eq(none)` — for T or none results
            "eq" if args.len() == 1 => {
                let arg_is_none = matches!(
                    &args[0],
                    Value::Enum { name, variant: v, .. } if name == "Option" && v == "None"
                );
                if arg_is_none {
                    let self_is_absent = variant == "Err" && fields.first().map(|f| {
                        matches!(f, Value::Enum { name, variant: v, .. } if name == "Option" && v == "None")
                    }).unwrap_or(false);
                    Ok(Value::Bool(self_is_absent))
                } else {
                    Err(RuntimeError::NoSuchMethod {
                        ty: "Result".to_string(),
                        method: method.to_string(),
                    })
                }
            }
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
                        variant_index: 0, origin: None,
                    })
                }
                "None" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                }),
                _ => Err(RuntimeError::TypeError("expected Option.Some or Option.None variant".to_string())),
            },
            "unwrap" => match variant {
                "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                "None" => Err(RuntimeError::Panic("called unwrap on None".to_string())),
                _ => Err(RuntimeError::TypeError("expected Option.Some or Option.None variant".to_string())),
            },
            "filter" => match variant {
                "Some" => {
                    let closure = args.into_iter().next().ok_or_else(|| {
                        RuntimeError::ArityMismatch { expected: 1, got: 0 }
                    })?;
                    let inner = fields.first().cloned().unwrap_or(Value::Unit);
                    let result = self.call_value(closure, vec![inner.clone()])?;
                    if self.is_truthy(&result) {
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![inner],
                            variant_index: 0, origin: None,
                        })
                    } else {
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                            variant_index: 0, origin: None,
                        })
                    }
                }
                "None" => Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                }),
                _ => Err(RuntimeError::TypeError("expected Option.Some or Option.None variant".to_string())),
            },
            // `x == none` desugars to `x.eq(none)` — compare by variant
            "eq" if args.len() == 1 => {
                let is_none = variant == "None";
                let arg_is_none = matches!(
                    &args[0],
                    Value::Enum { name, variant: v, .. } if name == "Option" && v == "None"
                );
                Ok(Value::Bool(is_none == arg_is_none))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Option".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

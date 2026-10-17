// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Equality on `T?` and `T or E`.
//!
//! Neither type has methods (`stdlib/option.rk`, `stdlib/result.rk`); `==`
//! desugars to `eq`, and that is the only call that reaches these.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// `==` with a `T or E` on the left.
    pub(crate) fn call_result_method(
        &mut self,
        variant: &str,
        fields: &[Value],
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
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

    /// `==` with a `T?` on the left.
    pub(crate) fn call_option_method(
        &mut self,
        variant: &str,
        fields: &[Value],
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // Equality on `T or none`: both absent, or both present with equal
            // payloads. This arm was written for `x == none` alone — it answered
            // by comparing variants — and every other optional comparison landed
            // in it too, so two present values were "equal" whatever they held:
            // `2.5 == 1.5` on `f32?` came back true (#638).
            "eq" if args.len() == 1 => {
                let is_none = variant == "None";
                match &args[0] {
                    Value::Enum { name, variant: other_variant, fields: other_fields, .. }
                        if name == "Option" =>
                    {
                        let other_is_none = other_variant == "None";
                        if is_none || other_is_none {
                            return Ok(Value::Bool(is_none && other_is_none));
                        }
                        Ok(Value::Bool(match (fields.first(), other_fields.first()) {
                            (Some(a), Some(b)) => Self::value_eq(a, b),
                            (None, None) => true,
                            _ => false,
                        }))
                    }
                    // A right side that never got wrapped — the payload itself.
                    // An absent left is never equal to a value that is there.
                    other => Ok(Value::Bool(
                        !is_none && fields.first().is_some_and(|p| Self::value_eq(p, other)),
                    )),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Option".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

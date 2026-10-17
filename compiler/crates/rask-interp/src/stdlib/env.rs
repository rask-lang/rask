// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Environment module methods (env.*).
//!
//! Layer: RUNTIME — reads OS environment variables.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle env module methods.
    pub(crate) fn call_env_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "var" => {
                let name = self.expect_string(&args, 0)?;
                match std::env::var(&name) {
                    Ok(val) => Ok(Value::some(Value::String(Arc::new(Mutex::new(val))))),
                    Err(_) => Ok(Value::none()),
                }
            }
            "vars" => {
                let vars: Vec<Value> = std::env::vars()
                    .map(|(k, v)| {
                        Value::tuple(vec![
                            Value::String(Arc::new(Mutex::new(k))),
                            Value::String(Arc::new(Mutex::new(v))),
                        ])
                    })
                    .collect();
                Ok(Value::vec(vars))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "env".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

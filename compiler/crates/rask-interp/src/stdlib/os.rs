// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! OS module methods (os.*).
//!
//! Layer: RUNTIME — env vars, process control, and platform detection.
//!
//! Consolidates process/platform operations: env vars, args, exit, pid, platform info.
//! Legacy modules (env, std, cli) forward here.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle os module methods.
    pub(crate) fn call_os_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // --- Environment variables ---
            #[cfg(not(target_arch = "wasm32"))]
            "env" => {
                let name = self.expect_string(&args, 0)?;
                match std::env::var(&name) {
                    Ok(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(val)))],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            #[cfg(target_arch = "wasm32")]
            "env" => {
                // Always return None in browser
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                })
            }

            #[cfg(not(target_arch = "wasm32"))]
            "env_or" => {
                let name = self.expect_string(&args, 0)?;
                let default = self.expect_string(&args, 1)?;
                let val = std::env::var(&name).unwrap_or(default);
                Ok(Value::String(Arc::new(Mutex::new(val))))
            }
            #[cfg(target_arch = "wasm32")]
            "env_or" => {
                // Return default in browser
                let _name = self.expect_string(&args, 0)?;
                let default = self.expect_string(&args, 1)?;
                Ok(Value::String(Arc::new(Mutex::new(default))))
            }

            #[cfg(not(target_arch = "wasm32"))]
            "set_env" | "remove_env" | "vars" => {
                match method {
                    "set_env" => {
                        let key = self.expect_string(&args, 0)?;
                        let value = self.expect_string(&args, 1)?;
                        std::env::set_var(&key, &value);
                        Ok(Value::Unit)
                    }
                    "remove_env" => {
                        let key = self.expect_string(&args, 0)?;
                        std::env::remove_var(&key);
                        Ok(Value::Unit)
                    }
                    "vars" => {
                        let vars: Vec<Value> = std::env::vars()
                            .map(|(k, v)| {
                                Value::Vec(Arc::new(Mutex::new(vec![
                                    Value::String(Arc::new(Mutex::new(k))),
                                    Value::String(Arc::new(Mutex::new(v))),
                                ])))
                            })
                            .collect();
                        Ok(Value::Vec(Arc::new(Mutex::new(vars))))
                    }
                    _ => unreachable!()
                }
            }
            #[cfg(target_arch = "wasm32")]
            "set_env" | "remove_env" | "vars" => {
                Err(RuntimeError::Generic(
                    format!("os.{} not available in browser playground", method)
                ))
            }

            // --- Command-line arguments ---
            "args" => {
                let args_vec: Vec<Value> = self
                    .cli_args
                    .iter()
                    .map(|s| Value::String(Arc::new(Mutex::new(s.clone()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(args_vec))))
            }

            // --- Process control ---
            "exit" => {
                let code = args
                    .first()
                    .map(|v| match v {
                        Value::Int(n) => *n as i32,
                        _ => 1,
                    })
                    .unwrap_or(0);
                Err(RuntimeError::Exit(code))
            }

            #[cfg(not(target_arch = "wasm32"))]
            "getpid" => {
                Ok(Value::Int(std::process::id() as i64))
            }
            #[cfg(target_arch = "wasm32")]
            "getpid" => {
                Err(RuntimeError::Generic(
                    "os.getpid() not available in browser playground".to_string()
                ))
            }

            // --- Platform info ---
            "platform" => {
                let platform = if cfg!(target_os = "linux") {
                    "linux"
                } else if cfg!(target_os = "macos") {
                    "macos"
                } else if cfg!(target_os = "windows") {
                    "windows"
                } else if cfg!(target_arch = "wasm32") {
                    "wasm"
                } else {
                    "unknown"
                };
                Ok(Value::String(Arc::new(Mutex::new(platform.to_string()))))
            }
            "arch" => {
                let arch = if cfg!(target_arch = "x86_64") {
                    "x86_64"
                } else if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else if cfg!(target_arch = "wasm32") {
                    "wasm32"
                } else {
                    "unknown"
                };
                Ok(Value::String(Arc::new(Mutex::new(arch.to_string()))))
            }

            _ => Err(RuntimeError::NoSuchMethod {
                ty: "os".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

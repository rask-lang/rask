// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Standard library module dispatch.
//!
//! Routes `module.method()` calls to the appropriate stdlib module handler.

mod cli;
mod env;
#[cfg(not(target_arch = "wasm32"))]
mod fs;
#[cfg(not(target_arch = "wasm32"))]
mod io;
mod json;
mod math;
#[cfg(not(target_arch = "wasm32"))]
mod net;
mod os;
mod path;
mod random;
mod time;

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ModuleKind, Value};

impl Interpreter {
    /// Dispatch a module method call to the appropriate stdlib handler.
    pub(crate) fn call_module_method(
        &mut self,
        module: &ModuleKind,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match module {
            #[cfg(not(target_arch = "wasm32"))]
            ModuleKind::Fs => self.call_fs_method(method, args),
            #[cfg(target_arch = "wasm32")]
            ModuleKind::Fs => Err(RuntimeError::Generic(
                "fs module not available in browser playground".to_string()
            )),

            #[cfg(not(target_arch = "wasm32"))]
            ModuleKind::Io => self.call_io_method(method, args),
            #[cfg(target_arch = "wasm32")]
            ModuleKind::Io => Err(RuntimeError::Generic(
                "io module not available in browser playground".to_string()
            )),

            #[cfg(not(target_arch = "wasm32"))]
            ModuleKind::Net => self.call_net_method(method, args),
            #[cfg(target_arch = "wasm32")]
            ModuleKind::Net => Err(RuntimeError::Generic(
                "net module not available in browser playground".to_string()
            )),

            ModuleKind::Time => self.call_time_module_method(method, args),
            ModuleKind::Random => self.call_random_method(method, args),
            ModuleKind::Math => self.call_math_method(method, args),
            ModuleKind::Os => self.call_os_method(method, args),
            ModuleKind::Json => self.call_json_method(method, args),
            ModuleKind::Path => self.call_path_module_method(method, args),
            // Legacy aliases — forward to new modules
            ModuleKind::Env => self.call_env_method(method, args),
            ModuleKind::Cli => self.call_cli_module_method(method, args),
            ModuleKind::Std => self.call_os_method(method, args),
        }
    }

    /// Handle path module methods (only type access currently).
    fn call_path_module_method(
        &self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        // The path module itself doesn't have methods — Path.new() goes through
        // type method dispatch. But in case someone tries path.something():
        Err(RuntimeError::NoSuchMethod {
            ty: "path".to_string(),
            method: method.to_string(),
        })
    }

    /// Dispatch a type static method (e.g., Instant.now(), Duration.seconds(5), Path.new()).
    /// SIMD f32x8 type methods (e.g., f32x8.load(slice)).
    fn call_simd_type_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "load" => {
                // f32x8.load(array_slice) — load 8 f32s from a Vec slice
                if args.is_empty() {
                    return Err(RuntimeError::TypeError("f32x8.load requires an argument".to_string()));
                }
                let data = match &args[0] {
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
                        arr
                    }
                    _ => return Err(RuntimeError::TypeError(format!(
                        "f32x8.load expects a Vec, found {}", args[0].type_name()
                    ))),
                };
                Ok(Value::SimdF32x8(data))
            }
            "splat" => {
                // f32x8.splat(value) — fill all 8 lanes with the same value
                if args.is_empty() {
                    return Err(RuntimeError::TypeError("f32x8.splat requires an argument".to_string()));
                }
                let val = match &args[0] {
                    Value::Float(f) => *f as f32,
                    Value::Int(n) => *n as f32,
                    _ => return Err(RuntimeError::TypeError(format!(
                        "f32x8.splat expects a number, found {}", args[0].type_name()
                    ))),
                };
                Ok(Value::SimdF32x8([val; 8]))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "f32x8 has no static method '{}'", method
            ))),
        }
    }

    pub(crate) fn call_type_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match type_name {
            "Instant" | "Duration" => self.call_time_type_method(type_name, method, args),
            "Path" => self.call_path_type_method(method, args),
            "f32x8" => self.call_simd_type_method(method, args),
            "Rng" => Err(RuntimeError::TypeError(format!(
                "Rng.{} is not yet implemented", method
            ))),
            _ => {
                // User-defined static methods from extend blocks
                if let Some(type_methods) = self.methods.get(type_name).cloned() {
                    if let Some(method_fn) = type_methods.get(method) {
                        let is_static = method_fn
                            .params
                            .first()
                            .map(|p| p.name != "self")
                            .unwrap_or(true);
                        if is_static {
                            return self.call_function(&method_fn, args);
                        }
                    }
                }
                Err(RuntimeError::TypeError(format!(
                    "type {} has no method '{}'",
                    type_name, method
                )))
            }
        }
    }
}

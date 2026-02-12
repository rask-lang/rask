// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Async module - green task spawning.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ThreadHandleInner, Value};
use std::sync::{Arc, Mutex};

impl Interpreter {
    /// Handle async module functions (spawn).
    pub(crate) fn call_async_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "spawn" => {
                // spawn(|| {}) - green task spawner
                if args.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "spawn requires a closure argument".to_string(),
                    ));
                }

                // Extract closure
                let closure = &args[0];
                match closure {
                    Value::Closure {
                        params,
                        body,
                        captured_env,
                    } => {
                        // Check for using Multitasking context
                        // TODO: Implement this check when context system is complete
                        // For now, treat as Thread.spawn for compatibility

                        if !params.is_empty() {
                            return Err(RuntimeError::TypeError(
                                "spawn closure must take no parameters".to_string(),
                            ));
                        }

                        let body = body.clone();
                        let captured = captured_env.clone();
                        let child = self.spawn_child(captured);

                        let join_handle = std::thread::spawn(move || {
                            let mut interp = child;
                            match interp.eval_expr(&body) {
                                Ok(val) => Ok(val),
                                Err(RuntimeError::Return(val)) => Ok(val),
                                Err(e) => Err(format!("{}", e)),
                            }
                        });

                        Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                            handle: Mutex::new(Some(join_handle)),
                        })))
                    }
                    _ => Err(RuntimeError::TypeError(format!(
                        "spawn expects a closure, got {}",
                        closure.type_name()
                    ))),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "async".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

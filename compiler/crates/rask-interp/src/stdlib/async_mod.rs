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
                        if self.env.get("__multitasking_ctx").is_none() {
                            return Err(RuntimeError::TypeError(
                                "spawn() requires 'using Multitasking' context".to_string(),
                            ));
                        }

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
                            match interp.eval_expr(&body).map_err(|diag| diag.error) {
                                Ok(val) => Ok(val),
                                Err(RuntimeError::Return(val)) => Ok(val),
                                Err(e) => Err(format!("{}", e)),
                            }
                        });

                        let handle_inner = Arc::new(ThreadHandleInner {
                            handle: Mutex::new(Some(join_handle)),
                            receiver: Mutex::new(None),
                        });

                        // Register for affine tracking (conc.async/H1)
                        let ptr = Arc::as_ptr(&handle_inner) as usize;
                        self.resource_tracker.register_handle(ptr, "TaskHandle", self.env.scope_depth());

                        Ok(Value::TaskHandle(handle_inner))
                    }
                    _ => Err(RuntimeError::TypeError(format!(
                        "spawn expects a closure, got {}",
                        closure.type_name()
                    ))),
                }
            }
            "join_all" => {
                // join_all(handles) — wait for all task handles, return Vec of results
                if args.is_empty() {
                    return Ok(Value::Vec(Arc::new(Mutex::new(Vec::new()))));
                }

                // Accept either a Vec of handles or variadic handles
                let handles: Vec<Value> = match &args[0] {
                    Value::Vec(v) => v.lock().unwrap().clone(),
                    _ => args,
                };

                let mut results = Vec::with_capacity(handles.len());
                for handle in handles {
                    match handle {
                        Value::TaskHandle(h) => {
                            let result = self.call_task_handle_method(&h, "join")?;
                            results.push(result);
                        }
                        Value::ThreadHandle(h) => {
                            let result = self.call_thread_handle_method(&h, "join")?;
                            results.push(result);
                        }
                        _ => {
                            return Err(RuntimeError::TypeError(format!(
                                "join_all expects TaskHandle or ThreadHandle, got {}",
                                handle.type_name()
                            )));
                        }
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(results))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "async".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

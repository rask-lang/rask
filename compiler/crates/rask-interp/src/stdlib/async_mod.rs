// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Async module - green task spawning.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{ThreadHandleInner, Value, ACTIVE_RUNTIME};
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
                        if ACTIVE_RUNTIME.read().unwrap().is_none() {
                            return Err(RuntimeError::Panic(
                                "RUNTIME PANIC: spawn() called with no active `using Multitasking` scope\n\
                                 Install a `using Multitasking { ... }` block that encloses the call.".to_string(),
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
            "select_first" => {
                // select_first(handles) — return first completed, cancel rest
                if args.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "select_first requires at least one handle".to_string(),
                    ));
                }

                let handles: Vec<Value> = match &args[0] {
                    Value::Vec(v) => v.lock().unwrap().clone(),
                    _ => args,
                };

                if handles.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "select_first requires at least one handle".to_string(),
                    ));
                }

                // Phase A: no real cancellation — join sequentially, return first
                // that succeeds. In a real green-task runtime, we'd race them.
                for handle in handles {
                    match handle {
                        Value::TaskHandle(h) => {
                            let result = self.call_task_handle_method(&h, "join")?;
                            return Ok(result);
                        }
                        Value::ThreadHandle(h) => {
                            let result = self.call_thread_handle_method(&h, "join")?;
                            return Ok(result);
                        }
                        _ => {
                            return Err(RuntimeError::TypeError(format!(
                                "select_first expects TaskHandle or ThreadHandle, got {}",
                                handle.type_name()
                            )));
                        }
                    }
                }
                unreachable!()
            }
            "cancelled" => {
                // Phase A: cooperative cancellation not yet implemented with OS threads.
                // Always returns false — tasks must use other mechanisms to check.
                Ok(Value::Bool(false))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "async".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle TaskGroup method calls.
    pub(crate) fn call_task_group_method(
        &mut self,
        tasks: &Arc<Mutex<Vec<Value>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "spawn" => {
                let closure = args.into_iter().next().ok_or_else(|| {
                    RuntimeError::TypeError("TaskGroup.spawn requires a closure".to_string())
                })?;

                match closure {
                    Value::Closure { params, body, captured_env } => {
                        if !params.is_empty() {
                            return Err(RuntimeError::TypeError(
                                "TaskGroup.spawn closure must take no parameters".to_string(),
                            ));
                        }

                        let captured = captured_env.clone();
                        let child = self.spawn_child(captured);
                        let body_clone = body.clone();

                        let join_handle = std::thread::spawn(move || {
                            let mut interp = child;
                            match interp.eval_expr(&body_clone).map_err(|diag| diag.error) {
                                Ok(val) => Ok(val),
                                Err(RuntimeError::Return(val)) => Ok(val),
                                Err(e) => Err(format!("{}", e)),
                            }
                        });

                        let handle_inner = Arc::new(ThreadHandleInner {
                            handle: Mutex::new(Some(join_handle)),
                            receiver: Mutex::new(None),
                        });

                        tasks.lock().unwrap().push(Value::TaskHandle(handle_inner));
                        Ok(Value::Unit)
                    }
                    _ => Err(RuntimeError::TypeError(format!(
                        "TaskGroup.spawn expects a closure, got {}",
                        closure.type_name()
                    ))),
                }
            }
            "join_all" => {
                let handles: Vec<Value> = tasks.lock().unwrap().drain(..).collect();
                let mut results = Vec::with_capacity(handles.len());
                for handle in handles {
                    if let Value::TaskHandle(h) = handle {
                        let result = self.call_task_handle_method(&h, "join")?;
                        results.push(result);
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(results))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "TaskGroup".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

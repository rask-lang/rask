// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared<T> methods — closure-based RwLock wrapper.

use std::sync::{Arc, RwLock};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle Shared<T> instance methods.
    pub(crate) fn call_shared_method(
        &mut self,
        shared: &Arc<RwLock<Value>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read" => {
                let closure = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                let snapshot = {
                    let guard = shared.read().map_err(|e| {
                        RuntimeError::Panic(format!("Shared.read: lock poisoned: {}", e))
                    })?;
                    guard.clone()
                };
                self.call_closure_with_arg(&closure, snapshot)
            }
            "write" => {
                let closure = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                self.call_shared_write_closure(shared, &closure)
            }
            "clone" => Ok(Value::Shared(Arc::clone(shared))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Shared".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Execute a closure with one argument, returning the closure's result.
    pub(crate) fn call_closure_with_arg(
        &mut self,
        closure: &Value,
        arg: Value,
    ) -> Result<Value, RuntimeError> {
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                self.env.push_scope();
                for (k, v) in captured_env {
                    self.env.define(k.clone(), v.clone());
                }
                if let Some(param_name) = params.first() {
                    self.env.define(param_name.clone(), arg);
                }
                let result = self.eval_expr(body);
                self.env.pop_scope();
                result
            }
            _ => Err(RuntimeError::TypeError(format!(
                "expected closure, found {}",
                closure.type_name()
            ))),
        }
    }

    /// Execute a write closure — locks the RwLock, runs the closure with the
    /// inner value, writes back any mutations, then unlocks.
    fn call_shared_write_closure(
        &mut self,
        shared: &Arc<RwLock<Value>>,
        closure: &Value,
    ) -> Result<Value, RuntimeError> {
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                let mut guard = shared.write().map_err(|e| {
                    RuntimeError::Panic(format!("Shared.write: lock poisoned: {}", e))
                })?;

                self.env.push_scope();
                for (k, v) in captured_env {
                    self.env.define(k.clone(), v.clone());
                }
                let param_name = params
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "_".to_string());
                self.env.define(param_name.clone(), guard.clone());

                let result = self.eval_expr(body);

                // Write back mutations to the shared value
                if let Some(updated) = self.env.get(&param_name) {
                    *guard = updated.clone();
                }

                self.env.pop_scope();
                result
            }
            _ => Err(RuntimeError::TypeError(format!(
                "expected closure, found {}",
                closure.type_name()
            ))),
        }
    }
}

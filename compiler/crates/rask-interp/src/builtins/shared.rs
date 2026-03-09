// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared<T> and Mutex<T> methods — sync primitive wrappers.
//!
//! Layer: RUNTIME — RwLock/Mutex require OS synchronization primitives.

use std::sync::{Arc, Mutex, RwLock};

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
            // Shared<T>.read() -> T  (inline access, E5/R5)
            // Returns a snapshot — lock held only during clone.
            // For aggregate types (Struct, Vec, etc.), clone shares the Arc,
            // so subsequent field access operates on shared data.
            "read" if args.is_empty() => {
                let guard = shared.read().map_err(|e| {
                    RuntimeError::Panic(format!("Shared.read: lock poisoned: {}", e))
                })?;
                Ok(guard.clone())
            }
            // Shared<T>.write() -> T  (inline access, E5/R5)
            // Returns a snapshot for inline mutation. Aggregate types share
            // through Arc, so field mutations go to the shared data.
            "write" if args.is_empty() => {
                let guard = shared.write().map_err(|e| {
                    RuntimeError::Panic(format!("Shared.write: lock poisoned: {}", e))
                })?;
                Ok(guard.clone())
            }
            // Shared<T>.read(|T| -> R) -> R  (closure-based)
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
            // Shared<T>.write(|T| -> R) -> R  (closure-based)
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

    /// Inline sync access helper — acquires the appropriate lock and returns
    /// a clone of the inner value. Used by assign_target for field assignment
    /// through .write()/.lock()/.read() chains.
    pub(crate) fn call_inline_sync_access(
        &mut self,
        receiver: &Value,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match (receiver, method) {
            (Value::Shared(s), "read") => {
                let guard = s.read().map_err(|e| {
                    RuntimeError::Panic(format!("Shared.read: lock poisoned: {}", e))
                })?;
                Ok(guard.clone())
            }
            (Value::Shared(s), "write") => {
                let guard = s.write().map_err(|e| {
                    RuntimeError::Panic(format!("Shared.write: lock poisoned: {}", e))
                })?;
                Ok(guard.clone())
            }
            (Value::RaskMutex(m), "lock") => {
                let guard = m.lock().map_err(|e| {
                    RuntimeError::Panic(format!("Mutex.lock: lock poisoned: {}", e))
                })?;
                Ok(guard.clone())
            }
            _ => Err(RuntimeError::TypeError(format!(
                "inline sync access: .{}() not supported on {}",
                method, receiver.type_name()
            ))),
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
                let result = self.eval_expr(body).map_err(|diag| diag.error);
                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            _ => Err(RuntimeError::TypeError(format!(
                "expected closure, found {}",
                closure.type_name()
            ))),
        }
    }

    /// Execute a closure with no arguments, returning the closure's result.
    pub(crate) fn call_closure_no_args(
        &mut self,
        closure: &Value,
    ) -> Result<Value, RuntimeError> {
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(format!(
                        "expected zero-argument closure, got closure with {} parameter(s)",
                        params.len()
                    )));
                }

                self.env.push_scope();
                for (k, v) in captured_env {
                    self.env.define(k.clone(), v.clone());
                }
                let result = self.eval_expr(body).map_err(|diag| diag.error);
                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
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

                let result = self.eval_expr(body).map_err(|diag| diag.error);

                // Write back: use closure return value if available,
                // fall back to checking environment mutations
                match &result {
                    Err(RuntimeError::Return(v)) => {
                        *guard = v.clone();
                    }
                    Ok(v) if !matches!(v, Value::Unit) => {
                        *guard = v.clone();
                    }
                    _ => {
                        if let Some(updated) = self.env.get(&param_name) {
                            *guard = updated.clone();
                        }
                    }
                }

                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            _ => Err(RuntimeError::TypeError(format!(
                "expected closure, found {}",
                closure.type_name()
            ))),
        }
    }

    /// Handle Mutex<T> instance methods.
    pub(crate) fn call_mutex_method(
        &mut self,
        mutex: &Arc<Mutex<Value>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // Mutex<T>.lock() -> T  (inline access, E5/MX3)
            // Returns a snapshot for inline mutation. Aggregate types share
            // through Arc, so field mutations go to the shared data.
            "lock" if args.is_empty() => {
                let guard = mutex.lock().map_err(|e| {
                    RuntimeError::Panic(format!("Mutex.lock: lock poisoned: {}", e))
                })?;
                Ok(guard.clone())
            }
            // Mutex<T>.lock(|T| -> R) -> R  (closure-based)
            "lock" => {
                let closure = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                self.call_mutex_lock_closure(mutex, &closure)
            }
            "try_lock" => {
                let closure = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                match mutex.try_lock() {
                    Ok(guard) => {
                        let snapshot = guard.clone();
                        drop(guard);
                        let result = self.call_closure_with_arg(&closure, snapshot)?;
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![result],
                            variant_index: 0,
                        })
                    }
                    Err(_) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 1,
                    }),
                }
            }
            "clone" => Ok(Value::RaskMutex(Arc::clone(mutex))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Mutex".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Execute a closure under a Mutex lock — locks, runs the closure with the
    /// inner value, writes back mutations, then unlocks.
    fn call_mutex_lock_closure(
        &mut self,
        mutex: &Arc<Mutex<Value>>,
        closure: &Value,
    ) -> Result<Value, RuntimeError> {
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                let mut guard = mutex.lock().map_err(|e| {
                    RuntimeError::Panic(format!("Mutex.lock: lock poisoned: {}", e))
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

                let result = self.eval_expr(body).map_err(|diag| diag.error);

                // Write back
                match &result {
                    Err(RuntimeError::Return(v)) => {
                        *guard = v.clone();
                    }
                    Ok(v) if !matches!(v, Value::Unit) => {
                        *guard = v.clone();
                    }
                    _ => {
                        if let Some(updated) = self.env.get(&param_name) {
                            *guard = updated.clone();
                        }
                    }
                }

                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            _ => Err(RuntimeError::TypeError(format!(
                "expected closure, found {}",
                closure.type_name()
            ))),
        }
    }
}

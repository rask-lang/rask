// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Async module - green task spawning.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;
use std::sync::{Arc, Mutex};

impl Interpreter {
    /// Handle async module functions (spawn).
    pub(crate) fn call_async_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // One implementation, in `spawn_async_task`. There were two —
            // this one and that one — with the same runtime check, the same
            // child interpreter and the same handle registration, differing
            // only in how they word "spawn needs a closure". Only that one is
            // reached by `spawn(|| …)`, so a fix applied here did nothing
            // (#882 was landed into this copy first and changed no behaviour).
            "spawn" => self.spawn_async_task(args),
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

    /// The natives under `TaskGroup<T>`: its handle list. `spawn`, `join_all`
    /// and `detach` are Rask (`stdlib/async.rk`) and reach this through them,
    /// so both backends run the same loops. There used to be a second `spawn`
    /// here, with its own child interpreter and no runtime check.
    pub(crate) fn call_task_group_method(
        &mut self,
        tasks: &Arc<Mutex<Vec<Value>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "adopt" => {
                let handle = args.into_iter().next().ok_or_else(|| {
                    RuntimeError::TypeError("TaskGroup.adopt expects a handle".to_string())
                })?;
                // The group owns the handle now, and the checker holds the group
                // to one join or detach. The frame that spawned it no longer
                // owes it.
                if let Value::TaskHandle(h) = &handle {
                    if let Some(id) = self.resource_tracker.lookup_handle_id(Arc::as_ptr(h) as usize) {
                        self.resource_tracker.take_entry(id);
                    }
                }
                tasks.lock().unwrap().push(handle);
                Ok(Value::Unit)
            }
            "len" => Ok(Value::Int(tasks.lock().unwrap().len() as i64, crate::value::IntKind::I64)),
            "at" => {
                let i = self.expect_int(&args, 0)?;
                let guard = tasks.lock().unwrap();
                guard.get(i as usize).cloned().ok_or_else(|| {
                    RuntimeError::Panic(format!(
                        "task group index {} out of range (len {})",
                        i,
                        guard.len()
                    ))
                })
            }
            "release" => {
                tasks.lock().unwrap().clear();
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "TaskGroup".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

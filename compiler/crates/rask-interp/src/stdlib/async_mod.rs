// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Async module - green task spawning.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

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
            "cancelled" => Ok(Value::Bool(crate::value::cancel_requested())),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "async".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

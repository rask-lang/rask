// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Async module. Tasks start with `spawn { … }`, an expression form of its
//! own; what's left here is asking whether the running one was cancelled.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle async module functions.
    pub(crate) fn call_async_method(
        &mut self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "cancelled" => Ok(Value::Bool(crate::value::cancel_requested())),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "async".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

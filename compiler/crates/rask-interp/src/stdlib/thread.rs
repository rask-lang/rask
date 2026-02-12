// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Thread module - Thread and ThreadPool type access.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle thread module member access.
    /// Returns types (Thread, ThreadPool) that can have static methods called on them.
    pub(crate) fn call_thread_method(
        &mut self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "Thread" => {
                // Return Thread type for static method access (Thread.spawn)
                Ok(Value::Type("Thread".to_string()))
            }
            "ThreadPool" => {
                // Return ThreadPool type for static method access (ThreadPool.spawn)
                Ok(Value::Type("ThreadPool".to_string()))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "thread".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

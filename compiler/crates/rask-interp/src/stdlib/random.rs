// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Random module methods (random.*).
//!
//! Layer: HYBRID — seed from system time, computation is pure PRNG.

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle random module methods.
    pub(crate) fn call_random_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "f32" | "f64" => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let f = (x as f64) / (u64::MAX as f64);
                Ok(Value::Float(f))
            }
            "i64" => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                Ok(Value::Int(x as i64))
            }
            "bool" => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                Ok(Value::Bool(hash & 1 == 1))
            }
            "range" => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                let low = args[0].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                let high = args[1].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                if low >= high {
                    return Err(RuntimeError::TypeError(
                        format!("random.range: low ({}) must be less than high ({})", low, high)
                    ));
                }
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let range = (high - low) as u64;
                let value = low + (x % range) as i64;
                Ok(Value::Int(value))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "random".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

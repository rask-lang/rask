// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Random module methods (random.*).
//!
//! Layer: HYBRID — seed from system time, computation is pure PRNG.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{RngState, Value};

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

    /// Handle Rng static methods (Rng.new(), Rng.from_seed()).
    pub(crate) fn call_rng_type_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "new" => {
                Ok(Value::Rng(Arc::new(Mutex::new(RngState::from_system()))))
            }
            "from_seed" => {
                if args.is_empty() {
                    return Err(RuntimeError::ArityMismatch { expected: 1, got: 0 });
                }
                let seed = args[0].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Rng(Arc::new(Mutex::new(RngState::from_seed(seed as u64)))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Rng".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Rng instance methods (rng.u64(), rng.f64(), etc.).
    pub(crate) fn call_rng_instance_method(
        &self,
        rng: &Arc<Mutex<RngState>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let mut state = rng.lock().unwrap();
        match method {
            "u64" => Ok(Value::Int(state.next_u64() as i64)),
            "i64" => Ok(Value::Int(state.next_u64() as i64)),
            "f64" => Ok(Value::Float(state.next_f64())),
            "f32" => Ok(Value::Float(state.next_f32() as f64)),
            "bool" => Ok(Value::Bool(state.next_bool())),
            "range" => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                let lo = args[0].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                let hi = args[1].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Int(state.range_i64(lo, hi)))
            }
            "shuffle" => {
                if args.is_empty() {
                    return Err(RuntimeError::ArityMismatch { expected: 1, got: 0 });
                }
                if let Value::Vec(v) = &args[0] {
                    let mut vec = v.lock().unwrap();
                    // Fisher-Yates shuffle
                    let len = vec.len();
                    for i in (1..len).rev() {
                        let j = (state.next_u64() as usize) % (i + 1);
                        vec.swap(i, j);
                    }
                    Ok(Value::Unit)
                } else {
                    Err(RuntimeError::TypeError("shuffle requires a Vec".to_string()))
                }
            }
            "choice" => {
                if args.is_empty() {
                    return Err(RuntimeError::ArityMismatch { expected: 1, got: 0 });
                }
                if let Value::Vec(v) = &args[0] {
                    let vec = v.lock().unwrap();
                    if vec.is_empty() {
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                        })
                    } else {
                        let idx = (state.next_u64() as usize) % vec.len();
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![vec[idx].clone()],
                        })
                    }
                } else {
                    Err(RuntimeError::TypeError("choice requires a Vec".to_string()))
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Rng".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

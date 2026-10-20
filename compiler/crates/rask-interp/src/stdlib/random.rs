// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Random module methods (random.*).
//!
//! Layer: HYBRID — seed from system time, computation is pure PRNG.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{FloatKind, IntKind, RngState, Value};

impl Interpreter {
    /// Handle random module methods: the instance methods, run on this
    /// thread's own generator (std.random/C1, C2).
    pub(crate) fn call_random_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        thread_local! {
            static THREAD_RNG: Arc<Mutex<RngState>> =
                Arc::new(Mutex::new(RngState::from_system()));
        }
        let rng = THREAD_RNG.with(Arc::clone);
        self.call_rng_instance_method(&rng, method, args).map_err(|e| match e {
            RuntimeError::NoSuchMethod { method, .. } => {
                RuntimeError::NoSuchMethod { ty: "random".to_string(), method }
            }
            other => other,
        })
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
                let seed = args[0].as_u64().map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Rng(Arc::new(Mutex::new(RngState::from_seed(seed)))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Random".to_string(),
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
            "u64" => Ok(Value::Int(state.next_u64() as i64, IntKind::U64)),
            "i64" => Ok(Value::int(state.next_u64() as i64)),
            "f64" => Ok(Value::Float(state.next_f64(), FloatKind::Untyped)),
            "f32" => Ok(Value::Float(state.next_f32() as f64, FloatKind::F32)),
            "bool" => Ok(Value::Bool(state.next_bool())),
            "range" => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                let lo = args[0].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                let hi = args[1].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                if lo >= hi {
                    return Err(RuntimeError::Panic(format!(
                        "Random.range: lo ({}) >= hi ({})", lo, hi
                    )));
                }
                Ok(Value::int(state.range_i64(lo, hi)))
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
                        Ok(Value::none())
                    } else {
                        let idx = (state.next_u64() as usize) % vec.len();
                        Ok(Value::some(vec[idx].clone()))
                    }
                } else {
                    Err(RuntimeError::TypeError("choice requires a Vec".to_string()))
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Random".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

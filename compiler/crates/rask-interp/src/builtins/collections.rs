//! Methods on collection types: Vec, Pool, Handle, and type constructors.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{PoolData, TypeConstructorKind, Value};

impl Interpreter {
    /// Handle Vec method calls.
    pub(crate) fn call_vec_method(
        &self,
        v: &Arc<Mutex<Vec<Value>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "push" => {
                let item = args.into_iter().next().unwrap_or(Value::Unit);
                v.lock().unwrap().push(item);
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                })
            }
            "pop" => Ok(v.lock().unwrap().pop().unwrap_or(Value::Unit)),
            "len" => Ok(Value::Int(v.lock().unwrap().len() as i64)),
            "get" => {
                let idx = self.expect_int(&args, 0)? as usize;
                Ok(v.lock().unwrap().get(idx).cloned().unwrap_or(Value::Unit))
            }
            "is_empty" => Ok(Value::Bool(v.lock().unwrap().is_empty())),
            "clear" => { v.lock().unwrap().clear(); Ok(Value::Unit) }
            "iter" => Ok(Value::Vec(Arc::clone(v))),
            "skip" => {
                let n = self.expect_int(&args, 0)? as usize;
                let skipped: Vec<Value> = v.lock().unwrap().iter().skip(n).cloned().collect();
                Ok(Value::Vec(Arc::new(Mutex::new(skipped))))
            }
            "first" => {
                match v.lock().unwrap().first().cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            "last" => {
                match v.lock().unwrap().last().cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            "contains" => {
                if let Some(needle) = args.first() {
                    let found = v.lock().unwrap().iter().any(|item| Self::value_eq(item, needle));
                    Ok(Value::Bool(found))
                } else {
                    Err(RuntimeError::ArityMismatch { expected: 1, got: 0 })
                }
            }
            "reverse" => { v.lock().unwrap().reverse(); Ok(Value::Unit) }
            "join" => {
                let sep = self.expect_string(&args, 0)?;
                let joined: String = v
                    .lock().unwrap()
                    .iter()
                    .map(|item| format!("{}", item))
                    .collect::<Vec<_>>()
                    .join(&sep);
                Ok(Value::String(Arc::new(Mutex::new(joined))))
            }
            "eq" => {
                if let Some(Value::Vec(other)) = args.first() {
                    let a = v.lock().unwrap();
                    let b = other.lock().unwrap();
                    let eq = a.len() == b.len()
                        && a.iter().zip(b.iter()).all(|(x, y)| Self::value_eq(x, y));
                    Ok(Value::Bool(eq))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            "ne" => {
                if let Some(Value::Vec(other)) = args.first() {
                    let a = v.lock().unwrap();
                    let b = other.lock().unwrap();
                    let eq = a.len() == b.len()
                        && a.iter().zip(b.iter()).all(|(x, y)| Self::value_eq(x, y));
                    Ok(Value::Bool(!eq))
                } else {
                    Ok(Value::Bool(true))
                }
            }
            "clone" | "to_vec" => {
                let cloned = v.lock().unwrap().clone();
                Ok(Value::Vec(Arc::new(Mutex::new(cloned))))
            }
            "insert" => {
                let idx = self.expect_int(&args, 0)? as usize;
                let item = args.into_iter().nth(1).unwrap_or(Value::Unit);
                let mut vec = v.lock().unwrap();
                if idx > vec.len() {
                    return Err(RuntimeError::Panic(format!("index {} out of bounds for Vec of length {}", idx, vec.len())));
                }
                vec.insert(idx, item);
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                })
            }
            "remove" => {
                let idx = self.expect_int(&args, 0)? as usize;
                let mut vec = v.lock().unwrap();
                if idx >= vec.len() {
                    return Err(RuntimeError::Panic(format!("index {} out of bounds for Vec of length {}", idx, vec.len())));
                }
                let removed = vec.remove(idx);
                Ok(removed)
            }
            "collect" => {
                // No-op: Vec is already collected
                Ok(Value::Vec(Arc::clone(v)))
            }
            "chunks" => {
                let chunk_size = self.expect_int(&args, 0)? as usize;
                if chunk_size == 0 {
                    return Err(RuntimeError::Panic("chunk size must be > 0".to_string()));
                }
                let vec = v.lock().unwrap();
                let chunks: Vec<Value> = vec.chunks(chunk_size)
                    .map(|chunk| Value::Vec(Arc::new(Mutex::new(chunk.to_vec()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(chunks))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Vec".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Pool method calls.
    pub(crate) fn call_pool_method(
        &self,
        p: &Arc<Mutex<PoolData>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "insert" => {
                let item = args.into_iter().next().unwrap_or(Value::Unit);
                let mut pool = p.lock().unwrap();
                let pool_id = pool.pool_id;
                let (index, generation) = pool.insert(item);
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Handle { pool_id, index, generation }],
                })
            }
            "get" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    let pool = p.lock().unwrap();
                    match pool.validate(*pool_id, *index, *generation) {
                        Ok(idx) => {
                            let val = pool.slots[idx].1.as_ref().unwrap().clone();
                            Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![val],
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                        }),
                    }
                } else {
                    Err(RuntimeError::TypeError("pool.get() requires a Handle argument".to_string()))
                }
            }
            "remove" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    let mut pool = p.lock().unwrap();
                    match pool.validate(*pool_id, *index, *generation) {
                        Ok(idx) => {
                            let val = pool.remove_at(idx).unwrap();
                            Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![val],
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                        }),
                    }
                } else {
                    Err(RuntimeError::TypeError("pool.remove() requires a Handle argument".to_string()))
                }
            }
            "len" => Ok(Value::Int(p.lock().unwrap().len as i64)),
            "is_empty" => Ok(Value::Bool(p.lock().unwrap().len == 0)),
            "contains" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    let pool = p.lock().unwrap();
                    Ok(Value::Bool(pool.validate(*pool_id, *index, *generation).is_ok()))
                } else {
                    Err(RuntimeError::TypeError("pool.contains() requires a Handle argument".to_string()))
                }
            }
            "clear" => {
                let mut pool = p.lock().unwrap();
                let slot_count = pool.slots.len();
                for (_i, (gen, slot)) in pool.slots.iter_mut().enumerate() {
                    if slot.is_some() {
                        *slot = None;
                        *gen = gen.saturating_add(1);
                    }
                }
                pool.free_list.clear();
                for i in 0..slot_count {
                    pool.free_list.push(i as u32);
                }
                pool.len = 0;
                Ok(Value::Unit)
            }
            "handles" | "cursor" => {
                let pool = p.lock().unwrap();
                let pool_id = pool.pool_id;
                let handles: Vec<Value> = pool
                    .valid_handles()
                    .iter()
                    .map(|(idx, gen)| Value::Handle {
                        pool_id,
                        index: *idx,
                        generation: *gen,
                    })
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(handles))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Pool".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Handle method calls.
    pub(crate) fn call_handle_method(
        &mut self,
        receiver: &Value,
        pool_id: u32,
        index: u32,
        generation: u32,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "eq" => {
                if let Some(Value::Handle { pool_id: p2, index: i2, generation: g2 }) = args.first() {
                    Ok(Value::Bool(pool_id == *p2 && index == *i2 && generation == *g2))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            "ne" => {
                let eq_result = self.call_handle_method(receiver, pool_id, index, generation, "eq", args)?;
                if let Value::Bool(b) = eq_result {
                    Ok(Value::Bool(!b))
                } else {
                    Ok(Value::Bool(true))
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Handle".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle type constructor method calls (Vec.new(), string.new(), etc.).
    pub(crate) fn call_type_constructor_method(
        &self,
        kind: &TypeConstructorKind,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match (kind, method) {
            (TypeConstructorKind::Vec, "new") => {
                Ok(Value::Vec(Arc::new(Mutex::new(Vec::new()))))
            }
            (TypeConstructorKind::Vec, "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::Vec(Arc::new(Mutex::new(Vec::with_capacity(cap)))))
            }
            (TypeConstructorKind::String, "new") => {
                Ok(Value::String(Arc::new(Mutex::new(String::new()))))
            }
            (TypeConstructorKind::Pool, "new") => {
                Ok(Value::Pool(Arc::new(Mutex::new(PoolData::new()))))
            }
            (TypeConstructorKind::Pool, "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                let mut pool = PoolData::new();
                pool.slots.reserve(cap);
                Ok(Value::Pool(Arc::new(Mutex::new(pool))))
            }
            (TypeConstructorKind::Channel, "buffered") => {
                let cap = self.expect_int(&args, 0)? as usize;
                let (tx, rx) = mpsc::sync_channel::<Value>(cap);
                let mut fields = HashMap::new();
                fields.insert("sender".to_string(), Value::Sender(Arc::new(Mutex::new(tx))));
                fields.insert("receiver".to_string(), Value::Receiver(Arc::new(Mutex::new(rx))));
                Ok(Value::Struct {
                    name: "ChannelPair".to_string(),
                    fields,
                    resource_id: None,
                })
            }
            (TypeConstructorKind::Channel, "unbuffered") => {
                let (tx, rx) = mpsc::sync_channel::<Value>(0);
                let mut fields = HashMap::new();
                fields.insert("sender".to_string(), Value::Sender(Arc::new(Mutex::new(tx))));
                fields.insert("receiver".to_string(), Value::Receiver(Arc::new(Mutex::new(rx))));
                Ok(Value::Struct {
                    name: "ChannelPair".to_string(),
                    fields,
                    resource_id: None,
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: format!("{:?}", kind),
                method: method.to_string(),
            }),
        }
    }
}

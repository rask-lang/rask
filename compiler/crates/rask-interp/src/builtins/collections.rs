// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on collection types: Vec, Pool, Handle, and type constructors.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{PoolData, TypeConstructorKind, Value};

/// Helper function to check if a value is truthy.
fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Unit => false,
        Value::Int(0) => false,
        _ => true,
    }
}

impl Interpreter {
    /// Handle Vec method calls.
    pub(crate) fn call_vec_method(
        &mut self,
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
            "pop" => {
                let result = v.lock().unwrap().pop();
                match result {
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
            "take" => {
                let n = self.expect_int(&args, 0)? as usize;
                let taken: Vec<Value> = v.lock().unwrap().iter().take(n).cloned().collect();
                Ok(Value::Vec(Arc::new(Mutex::new(taken))))
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
                    return Err(RuntimeError::IndexOutOfBounds { index: idx as i64, len: vec.len() });
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
                    return Err(RuntimeError::IndexOutOfBounds { index: idx as i64, len: vec.len() });
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
            "filter" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                let mut filtered = Vec::new();
                for item in vec.iter() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    if is_truthy(&result) {
                        filtered.push(item.clone());
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(filtered))))
            }
            "map" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                let mut mapped = Vec::new();
                for item in vec.iter() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    mapped.push(result);
                }
                Ok(Value::Vec(Arc::new(Mutex::new(mapped))))
            }
            "flat_map" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                let mut result = Vec::new();
                for item in vec.iter() {
                    let mapped = self.call_value(closure.clone(), vec![item.clone()])?;
                    if let Value::Vec(inner) = mapped {
                        result.extend(inner.lock().unwrap().clone());
                    } else {
                        result.push(mapped);
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(result))))
            }
            "fold" => {
                let init = args.get(0).cloned().unwrap_or(Value::Unit);
                let closure = args.get(1).cloned().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                let mut acc = init;
                for item in vec.iter() {
                    acc = self.call_value(closure.clone(), vec![acc, item.clone()])?;
                }
                Ok(acc)
            }
            "reduce" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                if vec.is_empty() {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    });
                }
                let mut acc = vec[0].clone();
                for item in vec.iter().skip(1) {
                    acc = self.call_value(closure.clone(), vec![acc, item.clone()])?;
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![acc],
                })
            }
            "enumerate" => {
                let vec = v.lock().unwrap();
                let enumerated: Vec<Value> = vec
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        Value::Vec(Arc::new(Mutex::new(vec![Value::Int(i as i64), item.clone()])))
                    })
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(enumerated))))
            }
            "zip" => {
                if let Some(Value::Vec(other)) = args.first() {
                    let vec1 = v.lock().unwrap();
                    let vec2 = other.lock().unwrap();
                    let zipped: Vec<Value> = vec1
                        .iter()
                        .zip(vec2.iter())
                        .map(|(a, b)| {
                            Value::Vec(Arc::new(Mutex::new(vec![a.clone(), b.clone()])))
                        })
                        .collect();
                    Ok(Value::Vec(Arc::new(Mutex::new(zipped))))
                } else {
                    Err(RuntimeError::TypeError("zip requires a Vec argument".to_string()))
                }
            }
            "limit" => {
                let n = self.expect_int(&args, 0)? as usize;
                let vec = v.lock().unwrap();
                let taken: Vec<Value> = vec.iter().take(n).cloned().collect();
                Ok(Value::Vec(Arc::new(Mutex::new(taken))))
            }
            "flatten" => {
                let vec = v.lock().unwrap();
                let mut flattened = Vec::new();
                for item in vec.iter() {
                    if let Value::Vec(inner) = item {
                        flattened.extend(inner.lock().unwrap().clone());
                    } else {
                        flattened.push(item.clone());
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(flattened))))
            }
            "sort" => {
                let mut vec = v.lock().unwrap();
                vec.sort_by(|a, b| {
                    Self::value_cmp(a, b).unwrap_or(std::cmp::Ordering::Equal)
                });
                Ok(Value::Unit)
            }
            "sort_by" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let mut vec = v.lock().unwrap();
                // Custom comparison via closure
                vec.sort_by(|a, b| {
                    match self.call_value(closure.clone(), vec![a.clone(), b.clone()]) {
                        Ok(Value::Int(n)) if n < 0 => std::cmp::Ordering::Less,
                        Ok(Value::Int(n)) if n > 0 => std::cmp::Ordering::Greater,
                        _ => std::cmp::Ordering::Equal,
                    }
                });
                Ok(Value::Unit)
            }
            "any" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                for item in vec.iter() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    if is_truthy(&result) {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            "all" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                for item in vec.iter() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    if !is_truthy(&result) {
                        return Ok(Value::Bool(false));
                    }
                }
                Ok(Value::Bool(true))
            }
            "find" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                for item in vec.iter() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    if is_truthy(&result) {
                        return Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![item.clone()],
                        });
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                })
            }
            "position" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                for (i, item) in vec.iter().enumerate() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    if is_truthy(&result) {
                        return Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![Value::Int(i as i64)],
                        });
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                })
            }
            "dedup" => {
                let mut vec = v.lock().unwrap();
                vec.dedup_by(|a, b| Self::value_eq(a, b));
                Ok(Value::Unit)
            }
            "sum" => {
                let vec = v.lock().unwrap();
                let mut sum = 0i64;
                let mut float_sum = 0.0f64;
                let mut is_float = false;
                for item in vec.iter() {
                    match item {
                        Value::Int(n) => {
                            if is_float {
                                float_sum += *n as f64;
                            } else {
                                sum += n;
                            }
                        }
                        Value::Float(f) => {
                            if !is_float {
                                float_sum = sum as f64 + f;
                                is_float = true;
                            } else {
                                float_sum += f;
                            }
                        }
                        _ => {
                            return Err(RuntimeError::TypeError(format!(
                                "sum requires numeric values, got {}",
                                item.type_name()
                            )))
                        }
                    }
                }
                if is_float {
                    Ok(Value::Float(float_sum))
                } else {
                    Ok(Value::Int(sum))
                }
            }
            "min" => {
                let vec = v.lock().unwrap();
                if vec.is_empty() {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    });
                }
                let mut min = vec[0].clone();
                for item in vec.iter().skip(1) {
                    if let Some(std::cmp::Ordering::Less) = Self::value_cmp(item, &min) {
                        min = item.clone();
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![min],
                })
            }
            "max" => {
                let vec = v.lock().unwrap();
                if vec.is_empty() {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    });
                }
                let mut max = vec[0].clone();
                for item in vec.iter().skip(1) {
                    if let Some(std::cmp::Ordering::Greater) = Self::value_cmp(item, &max) {
                        max = item.clone();
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![max],
                })
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
            "insert" | "alloc" => {
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
                    Err(RuntimeError::TypeError("pool.get() expects a Handle; use the handle returned by pool.add()".to_string()))
                }
            }
            "get_mut" => {
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
                    Err(RuntimeError::TypeError("pool.get_mut() expects a Handle; use the handle returned by pool.add()".to_string()))
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
                    Err(RuntimeError::TypeError("pool.remove() expects a Handle; use the handle returned by pool.add()".to_string()))
                }
            }
            "len" => Ok(Value::Int(p.lock().unwrap().len as i64)),
            "is_empty" => Ok(Value::Bool(p.lock().unwrap().len == 0)),
            "contains" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    let pool = p.lock().unwrap();
                    Ok(Value::Bool(pool.validate(*pool_id, *index, *generation).is_ok()))
                } else {
                    Err(RuntimeError::TypeError("pool.contains() expects a Handle; use the handle returned by pool.add()".to_string()))
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
            "clone" => {
                let pool = p.lock().unwrap();
                // Create a new pool with a new ID (old handles won't work with clone)
                let mut new_pool = PoolData::new();
                // Clone all slots with their generations
                for (gen, slot) in pool.slots.iter() {
                    if let Some(val) = slot {
                        new_pool.slots.push((*gen, Some(val.clone())));
                    } else {
                        new_pool.slots.push((*gen, None));
                    }
                }
                // Clone free list and length
                new_pool.free_list = pool.free_list.clone();
                new_pool.len = pool.len;
                new_pool.type_param = pool.type_param.clone();
                Ok(Value::Pool(Arc::new(Mutex::new(new_pool))))
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

    /// Handle Map method calls.
    pub(crate) fn call_map_method(
        &mut self,
        m: &Arc<Mutex<Vec<(Value, Value)>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "insert" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let value = args.get(1).cloned().unwrap_or(Value::Unit);
                let mut map = m.lock().unwrap();

                // Check if key exists, update if so
                for (k, v) in map.iter_mut() {
                    if Self::value_eq(k, &key) {
                        let old_value = v.clone();
                        *v = value;
                        return Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![old_value],
                            }],
                        });
                    }
                }

                // Key doesn't exist, insert new
                map.push((key, value));
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }],
                })
            }
            "get" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let map = m.lock().unwrap();

                for (k, v) in map.iter() {
                    if Self::value_eq(k, &key) {
                        return Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![v.clone()],
                        });
                    }
                }

                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                })
            }
            "remove" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let mut map = m.lock().unwrap();

                let mut index = None;
                for (i, (k, _)) in map.iter().enumerate() {
                    if Self::value_eq(k, &key) {
                        index = Some(i);
                        break;
                    }
                }

                if let Some(idx) = index {
                    let (_, v) = map.remove(idx);
                    Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![v],
                    })
                } else {
                    Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    })
                }
            }
            "contains" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let map = m.lock().unwrap();

                for (k, _) in map.iter() {
                    if Self::value_eq(k, &key) {
                        return Ok(Value::Bool(true));
                    }
                }

                Ok(Value::Bool(false))
            }
            "keys" => {
                let map = m.lock().unwrap();
                let keys: Vec<Value> = map.iter().map(|(k, _)| k.clone()).collect();
                Ok(Value::Vec(Arc::new(Mutex::new(keys))))
            }
            "values" => {
                let map = m.lock().unwrap();
                let values: Vec<Value> = map.iter().map(|(_, v)| v.clone()).collect();
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }
            "len" => Ok(Value::Int(m.lock().unwrap().len() as i64)),
            "is_empty" => Ok(Value::Bool(m.lock().unwrap().is_empty())),
            "clear" => {
                m.lock().unwrap().clear();
                Ok(Value::Unit)
            }
            "iter" => {
                let map = m.lock().unwrap();
                let pairs: Vec<Value> = map
                    .iter()
                    .map(|(k, v)| {
                        Value::Vec(Arc::new(Mutex::new(vec![k.clone(), v.clone()])))
                    })
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(pairs))))
            }
            "clone" => {
                let map = m.lock().unwrap();
                let cloned: Vec<(Value, Value)> = map
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                Ok(Value::Map(Arc::new(Mutex::new(cloned))))
            }
            "ensure" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let factory = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                })?;

                // Check if key already exists
                let key_exists = {
                    let map = m.lock().unwrap();
                    map.iter().any(|(k, _)| Self::value_eq(k, &key))
                };

                if key_exists {
                    return Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    });
                }

                // Key doesn't exist, call factory and insert
                let new_value = self.call_closure_no_args(factory)?;
                m.lock().unwrap().push((key, new_value));

                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Map".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle type constructor method calls (Vec.new(), string.new(), etc.).
    pub(crate) fn call_type_constructor_method(
        &self,
        kind: &TypeConstructorKind,
        type_param: Option<String>,
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
            (TypeConstructorKind::Vec, "from") => {
                // Vec.from(array) — copy array elements into new Vec
                let arr = args
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError::TypeError("Vec.from requires 1 argument".to_string()))?;

                match arr {
                    Value::Vec(v) => {
                        let vec = v.lock().unwrap();
                        let cloned = vec.clone();
                        Ok(Value::Vec(Arc::new(Mutex::new(cloned))))
                    }
                    _ => Err(RuntimeError::TypeError(
                        "Vec.from expects an array/vec".to_string(),
                    )),
                }
            }
            (TypeConstructorKind::String, "new") => {
                Ok(Value::String(Arc::new(Mutex::new(String::new()))))
            }
            (TypeConstructorKind::Pool, "new") => {
                Ok(Value::Pool(Arc::new(Mutex::new(PoolData::with_type_param(type_param.clone())))))
            }
            (TypeConstructorKind::Pool, "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                let mut pool = PoolData::with_type_param(type_param.clone());
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
            (TypeConstructorKind::Map, "new") => {
                Ok(Value::Map(Arc::new(Mutex::new(Vec::new()))))
            }
            (TypeConstructorKind::Map, "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::Map(Arc::new(Mutex::new(Vec::with_capacity(cap)))))
            }
            (TypeConstructorKind::Map, "from") => {
                // Map.from(array_of_tuples) — build map from [(key, value), ...]
                let arr = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                match arr {
                    Value::Vec(v) => {
                        let vec = v.lock().unwrap();
                        let mut pairs = Vec::with_capacity(vec.len());
                        for item in vec.iter() {
                            match item {
                                Value::Vec(tuple) => {
                                    let t = tuple.lock().unwrap();
                                    if t.len() >= 2 {
                                        pairs.push((t[0].clone(), t[1].clone()));
                                    }
                                }
                                _ => {}
                            }
                        }
                        Ok(Value::Map(Arc::new(Mutex::new(pairs))))
                    }
                    _ => Err(RuntimeError::TypeError(
                        "Map.from expects an array of pairs".to_string(),
                    )),
                }
            }
            (TypeConstructorKind::Shared, "new") => {
                let value = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                Ok(Value::Shared(Arc::new(RwLock::new(value))))
            }
            (TypeConstructorKind::Atomic, "new") => {
                let value = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
                match value {
                    Value::Bool(b) => Ok(Value::AtomicBool(Arc::new(AtomicBool::new(b)))),
                    Value::Int(n) => Ok(Value::AtomicUsize(Arc::new(AtomicUsize::new(n as usize)))),
                    _ => Err(RuntimeError::TypeError(format!(
                        "Atomic.new requires bool or int, got {}",
                        value.type_name()
                    ))),
                }
            }
            (TypeConstructorKind::Ordering, "Relaxed") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "Relaxed".to_string(),
                    fields: vec![],
                })
            }
            (TypeConstructorKind::Ordering, "Acquire") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "Acquire".to_string(),
                    fields: vec![],
                })
            }
            (TypeConstructorKind::Ordering, "Release") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "Release".to_string(),
                    fields: vec![],
                })
            }
            (TypeConstructorKind::Ordering, "AcqRel") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "AcqRel".to_string(),
                    fields: vec![],
                })
            }
            (TypeConstructorKind::Ordering, "SeqCst") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "SeqCst".to_string(),
                    fields: vec![],
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: format!("{:?}", kind),
                method: method.to_string(),
            }),
        }
    }

    /// Handle AtomicBool method calls.
    pub(crate) fn call_atomic_bool_method(
        &self,
        atomic: &Arc<std::sync::atomic::AtomicBool>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "load" => {
                let ordering = self.parse_ordering(&args, 0)?;
                let value = atomic.load(ordering);
                Ok(Value::Bool(value))
            }
            "store" => {
                let value = self.expect_bool(&args, 0)?;
                let ordering = self.parse_ordering(&args, 1)?;
                atomic.store(value, ordering);
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Atomic<bool>".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle AtomicUsize method calls.
    pub(crate) fn call_atomic_usize_method(
        &self,
        atomic: &Arc<std::sync::atomic::AtomicUsize>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "load" => {
                let ordering = self.parse_ordering(&args, 0)?;
                let value = atomic.load(ordering);
                Ok(Value::Int(value as i64))
            }
            "store" => {
                let value = self.expect_int(&args, 0)?;
                let ordering = self.parse_ordering(&args, 1)?;
                atomic.store(value as usize, ordering);
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Atomic<usize>".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle AtomicU64 method calls.
    pub(crate) fn call_atomic_u64_method(
        &self,
        atomic: &Arc<std::sync::atomic::AtomicU64>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "load" => {
                let ordering = self.parse_ordering(&args, 0)?;
                let value = atomic.load(ordering);
                Ok(Value::Int(value as i64))
            }
            "store" => {
                let value = self.expect_int(&args, 0)?;
                let ordering = self.parse_ordering(&args, 1)?;
                atomic.store(value as u64, ordering);
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Atomic<u64>".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Parse an Ordering enum value from arguments.
    fn parse_ordering(
        &self,
        args: &[Value],
        idx: usize,
    ) -> Result<std::sync::atomic::Ordering, RuntimeError> {
        use std::sync::atomic::Ordering;
        match args.get(idx) {
            Some(Value::Enum { name, variant, .. }) if name == "Ordering" => {
                match variant.as_str() {
                    "Relaxed" => Ok(Ordering::Relaxed),
                    "Acquire" => Ok(Ordering::Acquire),
                    "Release" => Ok(Ordering::Release),
                    "AcqRel" => Ok(Ordering::AcqRel),
                    "SeqCst" => Ok(Ordering::SeqCst),
                    _ => Err(RuntimeError::TypeError(format!(
                        "unknown Ordering variant: {}",
                        variant
                    ))),
                }
            }
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected Ordering, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }
}

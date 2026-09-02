// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on collection types: Vec, Pool, Handle, and type constructors.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

use std::sync::{Arc, Mutex, RwLock, mpsc};

use crate::interp::{Interpreter, RuntimeError};
use crate::ptr::RawPtr;
use crate::value::{map_entries_seeded, FloatKind, IteratorState, MapData, MapKey, PoolData, RackData, StructData, TypeConstructorKind, Value, VecData};

/// UTF-8 or a clear error. `from_raw` promises no validation natively, but a
/// Rust `String` can't carry the malformed bytes, so saying so beats inventing
/// replacement characters.
fn string_from_bytes(bytes: Vec<u8>) -> Result<String, RuntimeError> {
    String::from_utf8(bytes).map_err(|_| {
        RuntimeError::Panic(
            "those bytes aren't valid UTF-8 — the interpreter can't hold a malformed string"
                .to_string(),
        )
    })
}

/// The node a link names, seeing through the `Link<T>?` optional that every
/// edge field carries.
pub(crate) fn link_node(v: &Value) -> Option<Arc<Mutex<StructData>>> {
    match v {
        Value::Link { node, .. } => Some(Arc::clone(node)),
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            fields.first().and_then(link_node)
        }
        _ => None,
    }
}

/// Helper function to check if a value is truthy.
fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Unit => false,
        Value::Int(0, _) => false,
        _ => true,
    }
}

/// `Some(v)` / `none` as interpreter values.
fn option_value(v: Option<Value>) -> Value {
    match v {
        Some(v) => Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            fields: vec![v],
            variant_index: 0,
            origin: None,
        },
        None => Value::Enum {
            name: "Option".to_string(),
            variant: "None".to_string(),
            fields: vec![],
            variant_index: 0,
            origin: None,
        },
    }
}

impl Interpreter {
    /// Handle Vec method calls.
    pub(crate) fn call_vec_method(
        &mut self,
        v: &Arc<Mutex<VecData>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "push" => {
                let item = args.into_iter().next().unwrap_or(Value::Unit).copy_on_bind();
                let mut guard = v.lock().unwrap();
                // C2: growth past the bound panics. `try_push` is the variant
                // that hands the value back instead.
                if guard.is_full() {
                    let bound = guard.bound.unwrap_or(0);
                    return Err(RuntimeError::Panic(format!(
                        "push failed - collection at capacity (bound {})",
                        bound
                    )));
                }
                let pushed = item.clone();
                guard.push(item);
                drop(guard);
                // Pushing onto an edge list creates an incoming edge; record it
                // so the target's delete can drop the entry.
                crate::rack::register_element(v, &pushed);
                Ok(Value::Unit)
            }
            // C2: hands the value back rather than panicking. Native lowers the
            // same shape inline at the call site (stdlib generic bodies aren't
            // monomorphized, so the element can't cross a call boundary there).
            // `NoMemory` has no path on either backend — the allocator panics on
            // OOM rather than reporting it.
            "try_push" => {
                let item = args.into_iter().next().unwrap_or(Value::Unit).copy_on_bind();
                let mut guard = v.lock().unwrap();
                if guard.is_full() {
                    return Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "GrowError".to_string(),
                            variant: "Full".to_string(),
                            fields: vec![item],
                            variant_index: 0,
                            origin: None,
                        }],
                        variant_index: 0,
                        origin: None,
                    });
                }
                guard.push(item);
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                    variant_index: 0, origin: None,
                })
            }
            // `as_mut_ptr` differs only in taking `mutate self`, which the
            // checker enforces — both hand back the same buffer.
            "as_ptr" | "as_mut_ptr" => Ok(Value::RawPtr(RawPtr::elements(v))),
            // CP1/CP2: `none` when the vector may grow freely.
            "capacity" => {
                let bound = v.lock().unwrap().bound;
                Ok(option_value(bound.map(|b| Value::int(b as i64))))
            }
            "remaining" => {
                let left = v.lock().unwrap().remaining();
                Ok(option_value(left.map(|n| Value::int(n as i64))))
            }
            "is_bounded" => Ok(Value::Bool(v.lock().unwrap().bound.is_some())),
            "is_full" => Ok(Value::Bool(v.lock().unwrap().is_full())),
            "pop" => {
                let result = v.lock().unwrap().pop();
                match result {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "len" | "count" => Ok(Value::int(v.lock().unwrap().len() as i64)),
            "get" => {
                let idx = self.expect_int(&args, 0)? as usize;
                match v.lock().unwrap().get(idx).cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "is_empty" => Ok(Value::Bool(v.lock().unwrap().is_empty())),
            "clear" => { v.lock().unwrap().clear(); Ok(Value::Unit) }
            "iter" => {
                let state = IteratorState::Vec {
                    items: Arc::clone(v),
                    index: 0,
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "skip" => {
                let n = self.expect_int(&args, 0)? as usize;
                let skipped: Vec<Value> = v.lock().unwrap().iter().skip(n).cloned().collect();
                Ok(Value::vec(skipped))
            }
            "take" => {
                let n = self.expect_int(&args, 0)? as usize;
                let taken: Vec<Value> = v.lock().unwrap().iter().take(n).cloned().collect();
                Ok(Value::vec(taken))
            }
            "first" => {
                match v.lock().unwrap().first().cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "last" => {
                match v.lock().unwrap().last().cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
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
            "swap" => {
                let i = self.expect_int(&args, 0)?;
                let j = self.expect_int(&args, 1)?;
                let mut guard = v.lock().unwrap();
                let len = guard.len() as i64;
                if i < 0 || i >= len || j < 0 || j >= len {
                    return Err(RuntimeError::Panic(format!(
                        "Vec.swap: index out of bounds: {}/{} but length is {}",
                        i, j, len
                    )));
                }
                guard.swap(i as usize, j as usize);
                Ok(Value::Unit)
            }
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
            "set" => {
                let idx = self.expect_int(&args, 0)? as usize;
                let val = args.into_iter().nth(1).unwrap_or(Value::Unit).copy_on_bind();
                let mut vec = v.lock().unwrap();
                if idx >= vec.len() {
                    return Err(RuntimeError::IndexOutOfBounds { index: idx as i64, len: vec.len() });
                }
                vec[idx] = val;
                Ok(Value::Unit)
            }
            "insert" => {
                let idx = self.expect_int(&args, 0)? as usize;
                let item = args.into_iter().nth(1).unwrap_or(Value::Unit).copy_on_bind();
                let mut vec = v.lock().unwrap();
                if idx > vec.len() {
                    return Err(RuntimeError::IndexOutOfBounds { index: idx as i64, len: vec.len() });
                }
                vec.insert(idx, item);
                Ok(Value::Unit)
            }
            "remove" | "remove_at" => {
                let idx = self.expect_int(&args, 0)? as usize;
                let mut vec = v.lock().unwrap();
                if idx >= vec.len() {
                    return Err(RuntimeError::IndexOutOfBounds { index: idx as i64, len: vec.len() });
                }
                let removed = vec.remove(idx);
                Ok(removed)
            }
            "chunks" => {
                let chunk_size = self.expect_int(&args, 0)? as usize;
                if chunk_size == 0 {
                    return Err(RuntimeError::Panic("chunk size must be > 0".to_string()));
                }
                let vec = v.lock().unwrap();
                let chunks: Vec<Value> = vec.chunks(chunk_size)
                    .map(|chunk| Value::vec(chunk.to_vec()))
                    .collect();
                Ok(Value::vec(chunks))
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
                Ok(Value::vec(filtered))
            }
            "map" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                let mut mapped = Vec::new();
                for item in vec.iter() {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    mapped.push(result);
                }
                Ok(Value::vec(mapped))
            }
            "wide" => {
                // Stage this Vec as a data-parallel plan (conc.data-parallel).
                Ok(Value::Wide(Arc::new(crate::value::WidePlan::Source(Arc::clone(v)))))
            }
            "flat_map" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let vec = v.lock().unwrap();
                let mut result = Vec::new();
                for item in vec.iter() {
                    let mapped = self.call_value(closure.clone(), vec![item.clone()])?;
                    if let Value::Vec(inner) = mapped {
                        result.extend(inner.lock().unwrap().items.clone());
                    } else {
                        result.push(mapped);
                    }
                }
                Ok(Value::vec(result))
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
                        variant_index: 0, origin: None,
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
                    variant_index: 0, origin: None,
                })
            }
            "enumerate" => {
                let vec = v.lock().unwrap();
                let enumerated: Vec<Value> = vec
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        Value::vec(vec![Value::int(i as i64), item.clone()])
                    })
                    .collect();
                Ok(Value::vec(enumerated))
            }
            "zip" => {
                if let Some(Value::Vec(other)) = args.first() {
                    let vec1 = v.lock().unwrap();
                    let vec2 = other.lock().unwrap();
                    let zipped: Vec<Value> = vec1
                        .iter()
                        .zip(vec2.iter())
                        .map(|(a, b)| {
                            Value::vec(vec![a.clone(), b.clone()])
                        })
                        .collect();
                    Ok(Value::vec(zipped))
                } else {
                    Err(RuntimeError::TypeError("zip requires a Vec argument".to_string()))
                }
            }
            "limit" => {
                let n = self.expect_int(&args, 0)? as usize;
                let vec = v.lock().unwrap();
                let taken: Vec<Value> = vec.iter().take(n).cloned().collect();
                Ok(Value::vec(taken))
            }
            "flatten" => {
                let vec = v.lock().unwrap();
                let mut flattened = Vec::new();
                for item in vec.iter() {
                    if let Value::Vec(inner) = item {
                        flattened.extend(inner.lock().unwrap().items.clone());
                    } else {
                        flattened.push(item.clone());
                    }
                }
                Ok(Value::vec(flattened))
            }
            "sort" => {
                // std.collections/SO3: `sort()` is `T: Comparable`, so a type
                // that writes its own `compare` is sorted by it. `value_cmp` is
                // the *derived* order (CO3, lexicographic by field), which is
                // right only when nothing overrode it — `derive.rs` skips
                // generating one when the user wrote theirs, and this has to
                // agree or the two orders disagree by backend.
                let user_compare = {
                    let vec = v.lock().unwrap();
                    vec.items.first().and_then(Self::nominal_type_name)
                        .filter(|ty| {
                            self.methods.get(ty)
                                .and_then(|ms| ms.get("compare"))
                                .is_some_and(|f| !f.body.is_empty())
                        })
                };
                let items: Vec<Value> = v.lock().unwrap().items.clone();
                let sorted = match user_compare {
                    Some(ty) => self.sort_values_by_compare(items, &ty)?,
                    None => {
                        let mut items = items;
                        items.sort_by(|a, b| {
                            Self::value_cmp(a, b).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        items
                    }
                };
                v.lock().unwrap().items = sorted;
                Ok(Value::Unit)
            }
            "sort_by" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let mut vec = v.lock().unwrap();
                // Custom comparison via closure — accepts Ordering enum or Int
                vec.sort_by(|a, b| {
                    match self.call_value(closure.clone(), vec![a.clone(), b.clone()]) {
                        Ok(Value::Enum { name, variant, .. }) if name == "Ordering" => {
                            match variant.as_str() {
                                "Less" => std::cmp::Ordering::Less,
                                "Greater" => std::cmp::Ordering::Greater,
                                _ => std::cmp::Ordering::Equal,
                            }
                        }
                        Ok(Value::Int(n, _)) if n < 0 => std::cmp::Ordering::Less,
                        Ok(Value::Int(n, _)) if n > 0 => std::cmp::Ordering::Greater,
                        _ => std::cmp::Ordering::Equal,
                    }
                });
                Ok(Value::Unit)
            }
            "sort_by_key" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                let mut vec = v.lock().unwrap();
                vec.sort_by(|a, b| {
                    let ka = self.call_value(closure.clone(), vec![a.clone()]).ok();
                    let kb = self.call_value(closure.clone(), vec![b.clone()]).ok();
                    match (ka, kb) {
                        (Some(ref va), Some(ref vb)) => {
                            Self::value_cmp(va, vb).unwrap_or(std::cmp::Ordering::Equal)
                        }
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
                            variant_index: 0, origin: None,
                        });
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
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
                            fields: vec![Value::int(i as i64)],
                            variant_index: 0, origin: None,
                        });
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            "remove_adjacent_duplicates" => {
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
                        Value::Int(n, _) => {
                            if is_float {
                                float_sum += *n as f64;
                            } else {
                                sum += n;
                            }
                        }
                        Value::Float(f, _) => {
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
                    Ok(Value::Float(float_sum, FloatKind::Untyped))
                } else {
                    Ok(Value::int(sum))
                }
            }
            "min" => {
                let vec = v.lock().unwrap();
                if vec.is_empty() {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
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
                    variant_index: 0, origin: None,
                })
            }
            "max" => {
                let vec = v.lock().unwrap();
                if vec.is_empty() {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
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
                    variant_index: 0, origin: None,
                })
            }
            "take_all" => {
                // Draining leaves the vector empty but keeps its bound — a
                // fixed vector is still fixed after you empty it.
                let items = std::mem::take(&mut v.lock().unwrap().items);
                Ok(Value::vec(items))
            }
            "read" => {
                let index = self.expect_int(&args, 0)? as usize;
                let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                })?;

                let vec = v.lock().unwrap();
                if let Some(item) = vec.get(index) {
                    let result = self.call_value(closure.clone(), vec![item.clone()])?;
                    Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![result],
                        variant_index: 0, origin: None,
                    })
                } else {
                    Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    })
                }
            }
            "modify" => {
                let index = self.expect_int(&args, 0)? as usize;
                let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                })?;

                // The lock is released before the closure runs: the body can
                // reach the same Vec, and holding it across the call deadlocks.
                let item = v.lock().unwrap().get(index).cloned();
                let Some(item) = item else {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 1, origin: None,
                    });
                };
                let (result, written) =
                    self.call_closure_keeping_arg(closure.clone(), vec![item])?;
                // What the closure left in its parameter is the new element.
                // This used to be dropped, so `modify` never modified (#843).
                if let Some(new_item) = written {
                    if let Some(slot) = v.lock().unwrap().get_mut(index) {
                        *slot = new_item;
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![result],
                    variant_index: 0, origin: None,
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Vec".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `Rack<T>` methods — the arena side of the delete-time-fixup model
    /// (analysis.fourth-option). Structural ops only; following a link never
    /// comes here, because a link is a pointer.
    pub(crate) fn call_rack_method(
        &mut self,
        s: &Arc<Mutex<RackData>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "insert" => {
                let item = args.into_iter().next().unwrap_or(Value::Unit);
                // Nodes live in the rack, so they keep their identity rather
                // than being copied into it — links must observe writes made
                // through any other link to the same node.
                let node = match item {
                    Value::Struct(st) => st,
                    other => {
                        return Err(RuntimeError::TypeError(format!(
                            "rack.insert() expects a struct node, found {}",
                            other.type_name()
                        )))
                    }
                };
                let (rack_id, _idx) = {
                    let mut rack = s.lock().unwrap();
                    let id = rack.rack_id;
                    let idx = rack.insert(Arc::clone(&node));
                    (id, idx)
                };
                // A node can arrive with edges already in its fields
                // (`Node { prev: list.tail, .. }`). Record them now — the
                // struct literal that built it registered what it could see,
                // but the node's own identity only exists from here.
                crate::rack::register_nested(&Value::Struct(Arc::clone(&node)), 0);
                Ok(Value::Link { rack_id, node })
            }
            "delete" => {
                let target = args.first().ok_or_else(|| {
                    RuntimeError::TypeError("rack.delete() expects a Link".to_string())
                })?;
                let node = match link_node(target) {
                    Some(n) => n,
                    None => {
                        return Err(RuntimeError::TypeError(format!(
                            "rack.delete() expects a Link, found {}",
                            target.type_name()
                        )))
                    }
                };
                match crate::rack::delete_node(s, &node) {
                    Some(_owned) => Ok(Value::Unit),
                    // The node is already gone. Under the model a link to a
                    // dead node cannot exist, so reaching this means the link
                    // came from somewhere the fixup walk could not see.
                    None => Err(RuntimeError::Panic(
                        "rack.delete(): link target is not in this rack".to_string(),
                    )),
                }
            }
            "len" => Ok(Value::int(s.lock().unwrap().len as i64)),
            "is_empty" => Ok(Value::Bool(s.lock().unwrap().len == 0)),
            "contains" => {
                let found = args
                    .first()
                    .and_then(link_node)
                    .map(|n| s.lock().unwrap().index_of(&n).is_some())
                    .unwrap_or(false);
                Ok(Value::Bool(found))
            }
            // Every live node, as links. This is what `for n in rack` walks.
            "nodes" | "links" => {
                let rack = s.lock().unwrap();
                let rack_id = rack.rack_id;
                let links: Vec<Value> = rack
                    .live_nodes()
                    .into_iter()
                    .map(|node| Value::Link { rack_id, node })
                    .collect();
                Ok(Value::vec(links))
            }
            // A reader gets its own graph rather than a pointer into someone
            // else's. No link crosses the boundary, so T2 is not in question.
            "snapshot" => Ok(crate::rack::snapshot_rack(s)),
            // Translate a link the caller still holds into this snapshot's copy
            // of the same node. One lookup at the boundary, not per access.
            "corresponding" => {
                let Some(node) = args.first().and_then(link_node) else {
                    return Err(RuntimeError::TypeError(
                        "rack.corresponding() expects a Link".to_string(),
                    ));
                };
                let rack = s.lock().unwrap();
                let found = rack
                    .origin
                    .get(&crate::value::node_key(&node))
                    .map(|copy| Value::Link {
                        rack_id: rack.rack_id,
                        node: Arc::clone(copy),
                    });
                Ok(option_value(found))
            }
            // Every node dies, so every edge pointing into this rack must be
            // nulled. Truncating the slots would leave root edges and
            // cross-rack edges pointing at freed nodes — the one thing the
            // model promises can't happen.
            "clear" => {
                let live = s.lock().unwrap().live_nodes();
                for node in &live {
                    crate::rack::delete_node(s, node);
                }
                let mut rack = s.lock().unwrap();
                rack.slots.clear();
                rack.free_list.clear();
                rack.len = 0;
                Ok(Value::Unit)
            }
            _ => Err(RuntimeError::TypeError(format!(
                "no method `{}` on Rack; structural ops are insert, delete, len, is_empty, contains, nodes, clear, snapshot, corresponding",
                method
            ))),
        }
    }

    /// `Link<T>` methods. Deliberately almost empty: a link's whole interface
    /// is field access, which never routes through here.
    pub(crate) fn call_link_method(
        &mut self,
        rack_id: u32,
        node: &Arc<Mutex<StructData>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // Identity, not structural equality: two links are equal when they
            // name the same node.
            "eq" | "ne" => {
                let same = args
                    .first()
                    .and_then(link_node)
                    .map(|other| Arc::ptr_eq(node, &other))
                    .unwrap_or(false);
                Ok(Value::Bool(if method == "eq" { same } else { !same }))
            }
            _ => {
                // Fall through to the node's own methods, so `l.take_damage(3)`
                // works the way `l.health` does.
                let recv = Value::Struct(Arc::clone(node));
                let _ = rack_id;
                self.call_builtin_method(recv, method, args)
            }
        }
    }

    /// Handle Pool method calls.
    pub(crate) fn call_pool_method(
        &mut self,
        p: &Arc<Mutex<PoolData>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "insert" | "alloc" => {
                let item = args.into_iter().next().unwrap_or(Value::Unit).copy_on_bind();
                let mut pool = p.lock().unwrap();
                // mem.pools/PL8: a bounded pool at capacity panics on `insert`.
                if pool.is_full() {
                    let cap = pool.capacity.unwrap_or(0);
                    return Err(RuntimeError::Panic(format!(
                        "pool at capacity: cannot insert into a bounded pool of capacity {} (use try_insert)",
                        cap
                    )));
                }
                let pool_id = pool.pool_id;
                let (index, generation) = pool.insert(item);
                Ok(Value::Handle { pool_id, index, generation })
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
                                variant_index: 0, origin: None,
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                            variant_index: 0, origin: None,
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
                                variant_index: 0, origin: None,
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                            variant_index: 0, origin: None,
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
                                variant_index: 0, origin: None,
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                            variant_index: 0, origin: None,
                        }),
                    }
                } else {
                    Err(RuntimeError::TypeError("pool.remove() expects a Handle; use the handle returned by pool.add()".to_string()))
                }
            }
            "len" => Ok(Value::int(p.lock().unwrap().len as i64)),
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
                Ok(Value::vec(handles))
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
                new_pool.capacity = pool.capacity;
                Ok(Value::Pool(Arc::new(Mutex::new(new_pool))))
            }
            "try_insert" => {
                // mem.pools/PL8: `try_insert` on a full bounded pool returns `none`
                // instead of panicking; otherwise `Some(handle)`.
                let item = args.into_iter().next().unwrap_or(Value::Unit).copy_on_bind();
                let mut pool = p.lock().unwrap();
                if pool.is_full() {
                    return Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 1, origin: None,
                    });
                }
                let pool_id = pool.pool_id;
                let (index, generation) = pool.insert(item);
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![Value::Handle { pool_id, index, generation }],
                    variant_index: 0, origin: None,
                })
            }
            "drain" => {
                let mut pool = p.lock().unwrap();
                let mut items = Vec::new();
                for (gen, slot) in pool.slots.iter_mut() {
                    if let Some(value) = slot.take() {
                        items.push(value);
                        *gen = gen.saturating_add(1);
                    }
                }
                pool.free_list = (0..pool.slots.len() as u32).collect();
                pool.len = 0;
                Ok(Value::vec(items))
            }
            "entries" => {
                let pool = p.lock().unwrap();
                let pool_id = pool.pool_id;
                let pairs: Vec<Value> = pool.slots.iter().enumerate()
                    .filter_map(|(i, (gen, slot))| {
                        slot.as_ref().map(|val| {
                            // Pair as a 2-element Vec (tuple representation)
                            Value::vec(vec![
                                Value::Handle { pool_id, index: i as u32, generation: *gen },
                                val.clone(),
                            ])
                        })
                    })
                    .collect();
                Ok(Value::vec(pairs))
            }
            "get_unchecked" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    let pool = p.lock().unwrap();
                    match pool.validate(*pool_id, *index, *generation) {
                        Ok(idx) => {
                            let val = pool.slots[idx].1.as_ref().unwrap().clone();
                            Ok(val)
                        }
                        Err(msg) => Err(RuntimeError::Panic(format!(
                            "get_unchecked: invalid handle — {}", msg
                        ))),
                    }
                } else {
                    Err(RuntimeError::TypeError(
                        "pool.get_unchecked() expects a Handle".to_string(),
                    ))
                }
            }
            "get_mut_unchecked" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    let pool = p.lock().unwrap();
                    match pool.validate(*pool_id, *index, *generation) {
                        Ok(idx) => {
                            let val = pool.slots[idx].1.as_ref().unwrap().clone();
                            Ok(val)
                        }
                        Err(msg) => Err(RuntimeError::Panic(format!(
                            "get_mut_unchecked: invalid handle — {}", msg
                        ))),
                    }
                } else {
                    Err(RuntimeError::TypeError(
                        "pool.get_mut_unchecked() expects a Handle".to_string(),
                    ))
                }
            }
            "with_valid" | "with_valid_mut" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.get(0) {
                    let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                        expected: 2,
                        got: args.len(),
                    })?;
                    let pool = p.lock().unwrap();
                    if let Ok(idx) = pool.validate(*pool_id, *index, *generation) {
                        let val = pool.slots[idx].1.as_ref().unwrap().clone();
                        drop(pool);
                        let result = self.call_value(closure.clone(), vec![val])?;
                        return Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![result],
                            variant_index: 0, origin: None,
                        });
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            "capacity" => {
                let pool = p.lock().unwrap();
                Ok(Value::int(pool.slots.len() as i64))
            }
            "remaining" => {
                let pool = p.lock().unwrap();
                Ok(Value::int(pool.free_list.len() as i64))
            }
            "weak" => {
                if let Some(Value::Handle { pool_id, index, generation }) = args.first() {
                    Ok(Value::WeakHandle {
                        pool_id: *pool_id,
                        index: *index,
                        generation: *generation,
                    })
                } else {
                    Err(RuntimeError::TypeError(
                        "pool.weak() expects a Handle".to_string(),
                    ))
                }
            }
            "snapshot" => {
                let pool = p.lock().unwrap();
                let clone_pool = |pool: &PoolData| -> Value {
                    let mut new_pool = PoolData::new();
                    for (gen, slot) in pool.slots.iter() {
                        new_pool.slots.push((*gen, slot.clone()));
                    }
                    new_pool.free_list = pool.free_list.clone();
                    new_pool.len = pool.len;
                    new_pool.type_param = pool.type_param.clone();
                    Value::Pool(Arc::new(Mutex::new(new_pool)))
                };
                let p1 = clone_pool(&pool);
                let p2 = clone_pool(&pool);
                Ok(Value::vec(vec![p1, p2]))
            }
            "take_all" => {
                let mut pool = p.lock().unwrap();
                let mut items = Vec::new();
                // Iterate through all slots and collect active items
                for (_gen, slot) in &mut pool.slots {
                    if let Some(value) = slot.take() {
                        items.push(value);
                    }
                }
                // Reset pool state
                pool.free_list = (0..pool.slots.len() as u32).collect();
                pool.len = 0;
                Ok(Value::vec(items))
            }
            "read" => {
                if let Some(Value::Handle { pool_id: _, index, generation }) = args.get(0) {
                    let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                        expected: 2,
                        got: args.len(),
                    })?;

                    let pool = p.lock().unwrap();
                    if let Some((gen, Some(item))) = pool.slots.get(*index as usize) {
                        if gen == generation {
                            let result = self.call_value(closure.clone(), vec![item.clone()])?;
                            return Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![result],
                                variant_index: 0, origin: None,
                            });
                        }
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            "modify" => {
                if let Some(Value::Handle { pool_id: _, index, generation }) = args.get(0) {
                    let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                        expected: 2,
                        got: args.len(),
                    })?;

                    let mut pool = p.lock().unwrap();
                    if let Some((gen, Some(item))) = pool.slots.get_mut(*index as usize) {
                        if gen == generation {
                            let result = self.call_value(closure.clone(), vec![item.clone()])?;
                            return Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![result],
                                variant_index: 0, origin: None,
                            });
                        }
                    }
                }
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
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

    /// WeakHandle method calls.
    pub(crate) fn call_weak_handle_method(
        &mut self,
        pool_id: u32,
        index: u32,
        generation: u32,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "valid" => {
                // Check if the weak handle still points to a valid slot.
                // Walk all pools in the environment to find the right one.
                // For simplicity, return true if the handle data is non-zero.
                // Real validation requires pool access — upgrade() does that.
                Ok(Value::Bool(true))
            }
            "upgrade" => {
                // Convert WeakHandle back to Handle — the caller needs the pool
                // to validate. Return Some(Handle) optimistically; pool.get()
                // will do real validation.
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![Value::Handle { pool_id, index, generation }],
                    variant_index: 0, origin: None,
                })
            }
            "eq" => {
                if let Some(Value::WeakHandle { pool_id: p2, index: i2, generation: g2 }) = _args.first() {
                    Ok(Value::Bool(pool_id == *p2 && index == *i2 && generation == *g2))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            "ne" => {
                if let Some(Value::WeakHandle { pool_id: p2, index: i2, generation: g2 }) = _args.first() {
                    Ok(Value::Bool(pool_id != *p2 || index != *i2 || generation != *g2))
                } else {
                    Ok(Value::Bool(true))
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "WeakHandle".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Map method calls.
    pub(crate) fn call_map_method(
        &mut self,
        m: &Arc<Mutex<MapData>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let option_of = |v: Option<Value>| match v {
            Some(v) => Value::Enum {
                name: "Option".to_string(),
                variant: "Some".to_string(),
                fields: vec![v],
                variant_index: 0, origin: None,
            },
            None => Value::Enum {
                name: "Option".to_string(),
                variant: "None".to_string(),
                fields: vec![],
                variant_index: 0, origin: None,
            },
        };
        match method {
            "insert" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit).copy_on_bind();
                let value = args.get(1).cloned().unwrap_or(Value::Unit).copy_on_bind();
                let inserted = value.clone();
                let old = m.lock().unwrap().insert(MapKey(key), value);
                // A secondary index (`by_name: Map<string, Link<Task>>`) holds
                // edges: deleting the node drops its entry, which is the
                // database's index-maintenance move. Overwriting a key can
                // remove the map's last edge to the displaced target, so the old
                // value goes in too.
                crate::rack::register_entry(m, old.as_ref(), &inserted);
                Ok(option_of(old))
            }
            "get" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let found = m.lock().unwrap().get(&MapKey(key)).cloned();
                Ok(option_of(found))
            }
            "remove" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let removed = m.lock().unwrap().remove(&MapKey(key));
                Ok(option_of(removed))
            }
            "contains" | "contains_key" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                Ok(Value::Bool(m.lock().unwrap().contains_key(&MapKey(key))))
            }
            "keys" => {
                let keys: Vec<Value> = map_entries_seeded(&m.lock().unwrap())
                    .into_iter().map(|(k, _)| k).collect();
                Ok(Value::vec(keys))
            }
            "values" => {
                let values: Vec<Value> = map_entries_seeded(&m.lock().unwrap())
                    .into_iter().map(|(_, v)| v).collect();
                Ok(Value::vec(values))
            }
            "len" => Ok(Value::int(m.lock().unwrap().len() as i64)),
            "is_empty" => Ok(Value::Bool(m.lock().unwrap().is_empty())),
            "clear" => {
                m.lock().unwrap().clear();
                Ok(Value::Unit)
            }
            "iter" => {
                let pairs: Vec<Value> = map_entries_seeded(&m.lock().unwrap())
                    .into_iter()
                    .map(|(k, v)| Value::vec(vec![k, v]))
                    .collect();
                Ok(Value::vec(pairs))
            }
            "clone" => {
                let cloned: MapData = m.lock().unwrap().clone();
                Ok(Value::Map(Arc::new(Mutex::new(cloned))))
            }
            "insert_if_missing" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let factory = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                })?;

                let key_exists = m.lock().unwrap().contains_key(&MapKey(key.clone()));
                if !key_exists {
                    // Key doesn't exist, call factory and insert
                    let new_value = self.call_closure_no_args(factory)?;
                    m.lock().unwrap().insert(MapKey(key), new_value);
                }

                Ok(Value::Unit)
            }
            "modify_with_default" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let factory = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 3,
                    got: args.len(),
                })?;
                let modifier = args.get(2).ok_or(RuntimeError::ArityMismatch {
                    expected: 3,
                    got: args.len(),
                })?;

                let existing = m.lock().unwrap().get(&MapKey(key.clone())).cloned();
                let value_to_modify = match existing {
                    Some(v) => v,
                    None => {
                        // Key doesn't exist, call factory and insert
                        let new_value = self.call_closure_no_args(factory)?;
                        m.lock().unwrap().insert(MapKey(key.clone()), new_value.clone());
                        new_value
                    }
                };

                let (result, written) =
                    self.call_closure_keeping_arg(modifier.clone(), vec![value_to_modify])?;
                // The spec's own example for this is
                // `|u| { u.last_seen = now(); u.visit_count += 1 }` — the whole
                // reason the entry API exists is that the write lands (#843).
                if let Some(new_value) = written {
                    m.lock().unwrap().insert(MapKey(key), new_value);
                }
                Ok(result)
            }
            "take_all" => {
                let items: MapData = std::mem::take(&mut *m.lock().unwrap());
                let pairs = map_entries_seeded(&items);
                Ok(Value::vec(
                    pairs.into_iter().map(|(k, v)| {
                        Value::vec(vec![k, v])
                    }).collect()
                ))
            }
            "read" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                })?;

                let found = m.lock().unwrap().get(&MapKey(key)).cloned();
                match found {
                    Some(v) => {
                        let result = self.call_value(closure.clone(), vec![v])?;
                        Ok(option_of(Some(result)))
                    }
                    None => Ok(option_of(None)),
                }
            }
            "modify" => {
                let key = args.get(0).cloned().unwrap_or(Value::Unit);
                let closure = args.get(1).ok_or(RuntimeError::ArityMismatch {
                    expected: 2,
                    got: args.len(),
                })?;

                let found = m.lock().unwrap().get(&MapKey(key.clone())).cloned();
                match found {
                    Some(v) => {
                        let (result, written) =
                            self.call_closure_keeping_arg(closure.clone(), vec![v])?;
                        // Same as `Vec.modify`: keep what the closure left
                        // behind, which is the whole point of the name (#843).
                        if let Some(new_value) = written {
                            m.lock().unwrap().insert(MapKey(key), new_value);
                        }
                        Ok(option_of(Some(result)))
                    }
                    None => Ok(option_of(None)),
                }
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
                Ok(Value::vec(Vec::new()))
            }
            (TypeConstructorKind::Vec, "with_capacity") => {
                // CP1: a hint, not a ceiling — the vector stays unbounded.
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::vec(Vec::with_capacity(cap)))
            }
            // CP2/CP3: bounded and pre-allocated. Past the bound, `push` panics
            // and `try_push` hands the value back.
            (TypeConstructorKind::Vec, "fixed") => {
                let n = self.expect_int(&args, 0)?;
                if n < 0 {
                    return Err(RuntimeError::Panic(
                        "Vec.fixed needs a non-negative bound".to_string(),
                    ));
                }
                Ok(Value::vec_fixed(n as usize))
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
            (TypeConstructorKind::String, "from_char") => {
                let c = match args.first() {
                    Some(Value::Char(c)) => *c,
                    _ => return Err(RuntimeError::TypeError(
                        "string.from_char expects a char".to_string(),
                    )),
                };
                Ok(Value::String(Arc::new(Mutex::new(c.to_string()))))
            }
            // Copy in from a NUL-terminated C string. The interpreter's
            // buffers have no embedded NULs, so "to the terminator" is "to the
            // end of the buffer the pointer came from".
            (TypeConstructorKind::String, "from_c") => {
                match args.first() {
                    Some(Value::RawPtr(p)) => {
                        let bytes = p.bytes_from_here()?;
                        Ok(Value::String(Arc::new(Mutex::new(string_from_bytes(bytes)?))))
                    }
                    Some(Value::String(s)) => Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone())))),
                    _ => Err(RuntimeError::TypeError(
                        "string.from_c expects a `*u8`".to_string(),
                    )),
                }
            }
            // Pointer plus length, no NUL involved.
            (TypeConstructorKind::String, "from_raw") => {
                match args.first() {
                    Some(Value::RawPtr(p)) => {
                        let len = self.expect_int(&args, 1)?;
                        let mut bytes = p.bytes_from_here()?;
                        if len < 0 || len as usize > bytes.len() {
                            return Err(RuntimeError::Panic(format!(
                                "string.from_raw asked for {} bytes, {} are readable from here",
                                len,
                                bytes.len()
                            )));
                        }
                        bytes.truncate(len as usize);
                        Ok(Value::String(Arc::new(Mutex::new(string_from_bytes(bytes)?))))
                    }
                    Some(Value::String(s)) => Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone())))),
                    _ => Err(RuntimeError::TypeError(
                        "string.from_raw expects a `*u8` and a length".to_string(),
                    )),
                }
            }
            (TypeConstructorKind::Char, "from_u32") => {
                // CH3: returns char? — none for surrogates / out-of-range code points.
                let n = self.expect_int(&args, 0)? as u32;
                let opt = match char::from_u32(n) {
                    Some(c) => Value::Enum {
                        name: "Option".to_string(), variant: "Some".to_string(),
                        fields: vec![Value::Char(c)], variant_index: 0, origin: None,
                    },
                    None => Value::Enum {
                        name: "Option".to_string(), variant: "None".to_string(),
                        fields: vec![], variant_index: 1, origin: None,
                    },
                };
                Ok(opt)
            }
            (TypeConstructorKind::Pool, "new") => {
                Ok(Value::Pool(Arc::new(Mutex::new(PoolData::with_type_param(type_param.clone())))))
            }
            (TypeConstructorKind::Pool, "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                let mut pool = PoolData::with_type_param(type_param.clone());
                pool.slots.reserve(cap);
                // mem.pools/PL2: a with_capacity pool is bounded — enforce the limit.
                pool.capacity = Some(cap);
                Ok(Value::Pool(Arc::new(Mutex::new(pool))))
            }
            (TypeConstructorKind::Rack, "new") => {
                let rack = Arc::new(Mutex::new(RackData::with_type_param(type_param.clone())));
                crate::value::register_rack(&rack);
                Ok(Value::Rack(rack))
            }
            (TypeConstructorKind::Channel, "buffered") => {
                let cap = self.expect_int(&args, 0)? as usize;
                let (tx, rx) = mpsc::sync_channel::<Value>(cap);
                let tuple = vec![
                    Value::Sender(Arc::new(Mutex::new(tx))),
                    Value::Receiver(Arc::new(Mutex::new(rx))),
                ];
                Ok(Value::vec(tuple))
            }
            (TypeConstructorKind::Channel, "unbuffered") => {
                let (tx, rx) = mpsc::sync_channel::<Value>(0);
                let tuple = vec![
                    Value::Sender(Arc::new(Mutex::new(tx))),
                    Value::Receiver(Arc::new(Mutex::new(rx))),
                ];
                Ok(Value::vec(tuple))
            }
            (TypeConstructorKind::Map, "new") => {
                Ok(Value::Map(Arc::new(Mutex::new(MapData::new()))))
            }
            (TypeConstructorKind::Map, "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::Map(Arc::new(Mutex::new(MapData::with_capacity(cap)))))
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
                        let mut pairs = MapData::with_capacity(vec.len());
                        for item in vec.iter() {
                            match item {
                                Value::Vec(tuple) => {
                                    let t = tuple.lock().unwrap();
                                    if t.len() >= 2 {
                                        pairs.insert(MapKey(t[0].clone()), t[1].clone());
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
            // conc.sync/SH2: the constructor names the strategy, and the
            // strategy is the whole difference between the three values. `new`
            // is `Readers`, which is what bare `Shared<T>` means.
            (TypeConstructorKind::Shared, "new" | "mutex" | "local") => {
                let value = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                match method {
                    "mutex" => Ok(Value::RaskMutex(Arc::new(Mutex::new(value)))),
                    "local" => Ok(Value::Cell(Arc::new(Mutex::new(value)))),
                    _ => Ok(Value::Shared(Arc::new(RwLock::new(value)))),
                }
            }
            // CE1: Cell.new(value) — heap-allocate a single mutable value
            (TypeConstructorKind::Cell, "new") => {
                let value = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                Ok(Value::Cell(Arc::new(Mutex::new(value))))
            }
            (TypeConstructorKind::Mutex, "new") => {
                let value = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                Ok(Value::RaskMutex(Arc::new(Mutex::new(value))))
            }
            (TypeConstructorKind::Atomic, "new") => {
                let value = args.into_iter().next().ok_or(RuntimeError::ArityMismatch {
                    expected: 1,
                    got: 0,
                })?;
                use std::sync::atomic::{AtomicBool, AtomicUsize};
                match value {
                    Value::Bool(b) => Ok(Value::AtomicBool(Arc::new(AtomicBool::new(b)))),
                    Value::Int(n, _) => Ok(Value::AtomicUsize(Arc::new(AtomicUsize::new(n as usize)))),
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
                    variant_index: 0, origin: None,
                })
            }
            (TypeConstructorKind::Ordering, "Acquire") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "Acquire".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            (TypeConstructorKind::Ordering, "Release") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "Release".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            (TypeConstructorKind::Ordering, "AcqRel") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "AcqRel".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            (TypeConstructorKind::Ordering, "SeqCst") => {
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: "SeqCst".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            (TypeConstructorKind::TaskGroup, "new") => {
                Ok(Value::TaskGroup(Arc::new(Mutex::new(Vec::new()))))
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
                Ok(Value::int(value as i64))
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
                Ok(Value::int(value as i64))
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

impl Interpreter {
    /// Sort by a type's own `compare`, the way `sort()` is specified when the
    /// element implements Comparable (std.collections/SO3).
    ///
    /// `Vec::sort_by` can't be used here: the comparator calls back into the
    /// interpreter, which can fail, and a panicking or error-returning
    /// comparator inside `sort_by` has nowhere to go. Insertion sort over the
    /// same `compare` keeps errors propagable and is stable, matching the
    /// merge sort the native side uses for exactly this case.
    fn sort_values_by_compare(
        &mut self,
        items: Vec<Value>,
        type_name: &str,
    ) -> Result<Vec<Value>, RuntimeError> {
        let mut out: Vec<Value> = Vec::with_capacity(items.len());
        for item in items {
            let mut lo = 0usize;
            let mut hi = out.len();
            // Upper bound: the first position whose element compares Greater,
            // so equal elements keep the order they arrived in.
            while lo < hi {
                let mid = (lo + hi) / 2;
                let ord = self.call_rask_method(
                    type_name,
                    "compare",
                    out[mid].clone(),
                    vec![item.clone()],
                )?;
                let greater = matches!(&ord,
                    Value::Enum { name, variant, .. } if name == "Ordering" && variant == "Greater");
                if greater { hi = mid } else { lo = mid + 1 }
            }
            out.insert(lo, item);
        }
        Ok(out)
    }
}

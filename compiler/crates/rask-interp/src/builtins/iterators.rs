// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Lazy iterator method dispatch.
//!
//! Implements next(), map(), filter(), collect(), and other adapter
//! methods on the Iterator value type.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{IteratorState, Value};

fn is_truthy(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Unit => false,
        Value::Int(0) => false,
        _ => true,
    }
}

impl Interpreter {
    /// Advance an iterator and return the next value, or None.
    pub(crate) fn iter_next(
        &mut self,
        state: &Arc<Mutex<IteratorState>>,
    ) -> Result<Option<Value>, RuntimeError> {
        let mut st = state.lock().unwrap();
        match &mut *st {
            IteratorState::Vec { items, index } => {
                let vec = items.lock().unwrap();
                if *index < vec.len() {
                    let item = vec[*index].clone();
                    *index += 1;
                    Ok(Some(item))
                } else {
                    Ok(None)
                }
            }
            IteratorState::Map { source, mapper } => {
                let src = Arc::clone(source);
                let mapper = mapper.clone();
                drop(st);
                match self.iter_next(&src)? {
                    Some(item) => {
                        let result = self.call_value(mapper, vec![item])?;
                        Ok(Some(result))
                    }
                    None => Ok(None),
                }
            }
            IteratorState::Filter { source, predicate } => {
                let src = Arc::clone(source);
                let pred = predicate.clone();
                drop(st);
                loop {
                    match self.iter_next(&src)? {
                        Some(item) => {
                            let result = self.call_value(pred.clone(), vec![item.clone()])?;
                            if is_truthy(&result) {
                                return Ok(Some(item));
                            }
                        }
                        None => return Ok(None),
                    }
                }
            }
            IteratorState::Enumerate { source, counter } => {
                let src = Arc::clone(source);
                let idx = *counter;
                *counter += 1;
                drop(st);
                match self.iter_next(&src)? {
                    Some(item) => {
                        let pair = Value::Vec(Arc::new(Mutex::new(
                            vec![Value::Int(idx as i64), item],
                        )));
                        Ok(Some(pair))
                    }
                    None => Ok(None),
                }
            }
            IteratorState::Take { source, remaining } => {
                if *remaining == 0 {
                    return Ok(None);
                }
                let src = Arc::clone(source);
                *remaining -= 1;
                drop(st);
                self.iter_next(&src)
            }
            IteratorState::Skip { source, to_skip, skipped } => {
                let src = Arc::clone(source);
                let skip_count = *to_skip;
                let already_skipped = *skipped;
                if !already_skipped {
                    *skipped = true;
                }
                drop(st);
                if !already_skipped {
                    for _ in 0..skip_count {
                        if self.iter_next(&src)?.is_none() {
                            return Ok(None);
                        }
                    }
                }
                self.iter_next(&src)
            }
            IteratorState::Range { current, end, inclusive } => {
                let limit = if *inclusive { *end + 1 } else { *end };
                if *current < limit {
                    let val = *current;
                    *current += 1;
                    Ok(Some(Value::Int(val)))
                } else {
                    Ok(None)
                }
            }
            IteratorState::FlatMap { source, mapper, buffer } => {
                // Drain buffer first
                if !buffer.is_empty() {
                    return Ok(Some(buffer.remove(0)));
                }
                let src = Arc::clone(source);
                let map_fn = mapper.clone();
                drop(st);
                loop {
                    match self.iter_next(&src)? {
                        Some(item) => {
                            let result = self.call_value(map_fn.clone(), vec![item])?;
                            if let Value::Vec(v) = result {
                                let items = v.lock().unwrap();
                                if items.is_empty() {
                                    continue;
                                }
                                let first = items[0].clone();
                                let rest: std::vec::Vec<Value> = items.iter().skip(1).cloned().collect();
                                drop(items);
                                let mut state_lock = state.lock().unwrap();
                                if let IteratorState::FlatMap { buffer, .. } = &mut *state_lock {
                                    buffer.extend(rest);
                                }
                                return Ok(Some(first));
                            } else {
                                return Ok(Some(result));
                            }
                        }
                        None => return Ok(None),
                    }
                }
            }
            IteratorState::Zip { a, b } => {
                let a = Arc::clone(a);
                let b = Arc::clone(b);
                drop(st);
                match (self.iter_next(&a)?, self.iter_next(&b)?) {
                    (Some(va), Some(vb)) => {
                        let pair = Value::Vec(Arc::new(Mutex::new(vec![va, vb])));
                        Ok(Some(pair))
                    }
                    _ => Ok(None),
                }
            }
        }
    }

    /// Dispatch method calls on Iterator values.
    pub(crate) fn call_iterator_method(
        &mut self,
        iter: &Arc<Mutex<IteratorState>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "next" => {
                match self.iter_next(iter)? {
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
            "collect" => {
                let mut result = Vec::new();
                loop {
                    match self.iter_next(iter)? {
                        Some(item) => result.push(item),
                        None => break,
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(result))))
            }
            "map" => {
                let mapper = args.into_iter().next().unwrap_or(Value::Unit);
                let new_state = IteratorState::Map {
                    source: Arc::clone(iter),
                    mapper,
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
            }
            "filter" => {
                let predicate = args.into_iter().next().unwrap_or(Value::Unit);
                let new_state = IteratorState::Filter {
                    source: Arc::clone(iter),
                    predicate,
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
            }
            "enumerate" => {
                let new_state = IteratorState::Enumerate {
                    source: Arc::clone(iter),
                    counter: 0,
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
            }
            "take" => {
                let n = args.first()
                    .and_then(|v| if let Value::Int(i) = v { Some(*i as usize) } else { None })
                    .unwrap_or(0);
                let new_state = IteratorState::Take {
                    source: Arc::clone(iter),
                    remaining: n,
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
            }
            "skip" => {
                let n = args.first()
                    .and_then(|v| if let Value::Int(i) = v { Some(*i as usize) } else { None })
                    .unwrap_or(0);
                let new_state = IteratorState::Skip {
                    source: Arc::clone(iter),
                    to_skip: n,
                    skipped: false,
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
            }
            "flat_map" => {
                let mapper = args.into_iter().next().unwrap_or(Value::Unit);
                let new_state = IteratorState::FlatMap {
                    source: Arc::clone(iter),
                    mapper,
                    buffer: Vec::new(),
                };
                Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
            }
            "zip" => {
                if let Some(Value::Iterator(other)) = args.first() {
                    let new_state = IteratorState::Zip {
                        a: Arc::clone(iter),
                        b: Arc::clone(other),
                    };
                    Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
                } else if let Some(Value::Vec(v)) = args.first() {
                    // Allow zipping with a Vec by wrapping it
                    let vec_iter = Arc::new(Mutex::new(IteratorState::Vec {
                        items: Arc::clone(v),
                        index: 0,
                    }));
                    let new_state = IteratorState::Zip {
                        a: Arc::clone(iter),
                        b: vec_iter,
                    };
                    Ok(Value::Iterator(Arc::new(Mutex::new(new_state))))
                } else {
                    Err(RuntimeError::TypeError("zip requires an Iterator or Vec argument".to_string()))
                }
            }
            "fold" => {
                let init = args.get(0).cloned().unwrap_or(Value::Unit);
                let closure = args.get(1).cloned().unwrap_or(Value::Unit);
                let mut acc = init;
                loop {
                    match self.iter_next(iter)? {
                        Some(item) => {
                            acc = self.call_value(closure.clone(), vec![acc, item])?;
                        }
                        None => break,
                    }
                }
                Ok(acc)
            }
            "reduce" => {
                let closure = args.into_iter().next().unwrap_or(Value::Unit);
                match self.iter_next(iter)? {
                    Some(first) => {
                        let mut acc = first;
                        loop {
                            match self.iter_next(iter)? {
                                Some(item) => {
                                    acc = self.call_value(closure.clone(), vec![acc, item])?;
                                }
                                None => break,
                            }
                        }
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![acc],
                        })
                    }
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            "any" => {
                let pred = args.into_iter().next().unwrap_or(Value::Unit);
                loop {
                    match self.iter_next(iter)? {
                        Some(item) => {
                            let result = self.call_value(pred.clone(), vec![item])?;
                            if is_truthy(&result) {
                                return Ok(Value::Bool(true));
                            }
                        }
                        None => return Ok(Value::Bool(false)),
                    }
                }
            }
            "all" => {
                let pred = args.into_iter().next().unwrap_or(Value::Unit);
                loop {
                    match self.iter_next(iter)? {
                        Some(item) => {
                            let result = self.call_value(pred.clone(), vec![item])?;
                            if !is_truthy(&result) {
                                return Ok(Value::Bool(false));
                            }
                        }
                        None => return Ok(Value::Bool(true)),
                    }
                }
            }
            "find" => {
                let pred = args.into_iter().next().unwrap_or(Value::Unit);
                loop {
                    match self.iter_next(iter)? {
                        Some(item) => {
                            let result = self.call_value(pred.clone(), vec![item.clone()])?;
                            if is_truthy(&result) {
                                return Ok(Value::Enum {
                                    name: "Option".to_string(),
                                    variant: "Some".to_string(),
                                    fields: vec![item],
                                });
                            }
                        }
                        None => return Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                        }),
                    }
                }
            }
            "count" => {
                let mut n = 0usize;
                loop {
                    match self.iter_next(iter)? {
                        Some(_) => n += 1,
                        None => break,
                    }
                }
                Ok(Value::Int(n as i64))
            }
            "sum" => {
                let mut total = 0i64;
                let mut is_float = false;
                let mut ftotal = 0.0f64;
                loop {
                    match self.iter_next(iter)? {
                        Some(Value::Int(n)) => {
                            if is_float { ftotal += n as f64; } else { total += n; }
                        }
                        Some(Value::Float(n)) => {
                            if !is_float { ftotal = total as f64; is_float = true; }
                            ftotal += n;
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                if is_float { Ok(Value::Float(ftotal)) } else { Ok(Value::Int(total)) }
            }
            "to_vec" => {
                // Alias for collect
                let mut result = Vec::new();
                loop {
                    match self.iter_next(iter)? {
                        Some(item) => result.push(item),
                        None => break,
                    }
                }
                Ok(Value::Vec(Arc::new(Mutex::new(result))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Iterator".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

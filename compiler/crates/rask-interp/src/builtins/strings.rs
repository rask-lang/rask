// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on the string type.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{IteratorState, Value};

impl Interpreter {
    /// Handle string method calls.
    pub(crate) fn call_string_method(
        &self,
        s: &Arc<Mutex<String>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "len" => Ok(Value::Int(s.lock().unwrap().len() as i64)),
            "is_empty" => Ok(Value::Bool(s.lock().unwrap().is_empty())),
            "clone" => Ok(Value::String(Arc::clone(s))),
            "starts_with" => {
                let prefix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().starts_with(&prefix)))
            }
            "ends_with" => {
                let suffix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().ends_with(&suffix)))
            }
            "contains" => {
                let pattern = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().contains(&pattern)))
            }
            "push" | "push_char" => {
                let c = self.expect_char(&args, 0)?;
                s.lock().unwrap().push(c);
                Ok(Value::Unit)
            }
            "push_str" => {
                let other = self.expect_string(&args, 0)?;
                s.lock().unwrap().push_str(&other);
                Ok(Value::Unit)
            }
            "trim" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim().to_string()))))
            }
            "trim_start" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim_start().to_string()))))
            }
            "trim_end" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim_end().to_string()))))
            }
            "trim_bounds" => {
                let guard = s.lock().unwrap();
                let trimmed = guard.trim();
                let start = trimmed.as_ptr() as usize - guard.as_ptr() as usize;
                let end = start + trimmed.len();
                Ok(Value::Vec(Arc::new(Mutex::new(vec![Value::Int(start as i64), Value::Int(end as i64)]))))
            }
            "to_string" => Ok(Value::String(Arc::clone(s))),
            "debug_string" => {
                let val = s.lock().unwrap();
                Ok(Value::String(Arc::new(Mutex::new(format!("\"{}\"", val)))))
            }
            "concat" => {
                let other = self.expect_string(&args, 0)?;
                let mut result = s.lock().unwrap().clone();
                result.push_str(&other);
                Ok(Value::String(Arc::new(Mutex::new(result))))
            }
            "to_owned" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone()))))
            }
            "to_uppercase" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().to_uppercase()))))
            }
            "to_lowercase" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().to_lowercase()))))
            }
            "split" => {
                let delimiter = self.expect_string(&args, 0)?;
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split(&delimiter)
                    .map(|p| Value::String(Arc::new(Mutex::new(p.to_string()))))
                    .collect();
                let state = IteratorState::PreComputed { items: parts, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "split_whitespace" => {
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split_whitespace()
                    .map(|part| Value::String(Arc::new(Mutex::new(part.to_string()))))
                    .collect();
                let state = IteratorState::PreComputed { items: parts, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "chars" => {
                let chars: Vec<Value> = s.lock().unwrap().chars().map(Value::Char).collect();
                let state = IteratorState::PreComputed { items: chars, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "char_indices" => {
                let pairs: Vec<Value> = s.lock().unwrap().char_indices()
                    .map(|(i, c)| Value::Vec(Arc::new(Mutex::new(vec![Value::Int(i as i64), Value::Char(c)]))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(pairs))))
            }
            "bytes" => {
                let bytes: Vec<Value> = s.lock().unwrap().bytes()
                    .map(|b| Value::Int(b as i64))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(bytes))))
            }
            "lines" => {
                let lines: Vec<Value> = s
                    .lock().unwrap()
                    .lines()
                    .map(|l| Value::String(Arc::new(Mutex::new(l.to_string()))))
                    .collect();
                let state = IteratorState::PreComputed { items: lines, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "replace" => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().replace(&from, &to)))))
            }
            "substring" => {
                let sb = s.lock().unwrap();
                let start = self.expect_int(&args, 0)? as usize;
                let end = args
                    .get(1)
                    .map(|v| match v {
                        Value::Int(i) => *i as usize,
                        _ => sb.len(),
                    })
                    .unwrap_or(sb.len());
                let substring: String = sb.chars().skip(start).take(end - start).collect();
                Ok(Value::String(Arc::new(Mutex::new(substring))))
            }
            "parse_int" | "parse" => {
                match s.lock().unwrap().trim().parse::<i64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Int(n)],
                        variant_index: 0,
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "invalid integer".to_string(),
                        )))],
                        variant_index: 0,
                    }),
                }
            }
            "char_at" => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.lock().unwrap().chars().nth(idx) {
                    Some(c) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Char(c)],
                        variant_index: 0,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0,
                    }),
                }
            }
            "byte_at" => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.lock().unwrap().as_bytes().get(idx) {
                    Some(&b) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Int(b as i64)],
                        variant_index: 0,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0,
                    }),
                }
            }
            "parse_float" => {
                match s.lock().unwrap().trim().parse::<f64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Float(n)],
                        variant_index: 0,
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "invalid float".to_string(),
                        )))],
                        variant_index: 0,
                    }),
                }
            }
            "index_of" | "find" => {
                let pattern = self.expect_string(&args, 0)?;
                match s.lock().unwrap().find(&pattern) {
                    Some(idx) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Int(idx as i64)],
                        variant_index: 0,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0,
                    }),
                }
            }
            "rfind" => {
                let pattern = self.expect_string(&args, 0)?;
                match s.lock().unwrap().rfind(&pattern) {
                    Some(idx) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Int(idx as i64)],
                        variant_index: 0,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0,
                    }),
                }
            }
            "repeat" => {
                let n = self.expect_int(&args, 0)? as usize;
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().repeat(n)))))
            }
            "reverse" => {
                Ok(Value::String(Arc::new(Mutex::new(
                    s.lock().unwrap().chars().rev().collect(),
                ))))
            }
            "eq" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() == b))
            }
            "ne" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() != b))
            }
            "lt" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() < b))
            }
            "le" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() <= b))
            }
            "gt" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() > b))
            }
            "ge" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() >= b))
            }
            "compare" => {
                let b = self.expect_string(&args, 0)?;
                let ord = s.lock().unwrap().cmp(&b);
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: match ord {
                        std::cmp::Ordering::Less => "Less".to_string(),
                        std::cmp::Ordering::Equal => "Equal".to_string(),
                        std::cmp::Ordering::Greater => "Greater".to_string(),
                    },
                    fields: vec![],
                    variant_index: 0,
                })
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "string".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

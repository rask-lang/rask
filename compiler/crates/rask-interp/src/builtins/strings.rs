//! Methods on the string type.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

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
            "push" => {
                let c = self.expect_char(&args, 0)?;
                s.lock().unwrap().push(c);
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
            "to_string" => Ok(Value::String(Arc::clone(s))),
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
                Ok(Value::Vec(Arc::new(Mutex::new(parts))))
            }
            "split_whitespace" => {
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split_whitespace()
                    .map(|part| Value::String(Arc::new(Mutex::new(part.to_string()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(parts))))
            }
            "chars" => {
                let chars: Vec<Value> = s.lock().unwrap().chars().map(Value::Char).collect();
                Ok(Value::Vec(Arc::new(Mutex::new(chars))))
            }
            "lines" => {
                let lines: Vec<Value> = s
                    .lock().unwrap()
                    .lines()
                    .map(|l| Value::String(Arc::new(Mutex::new(l.to_string()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(lines))))
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
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "invalid integer".to_string(),
                        )))],
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
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
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
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            "parse_float" => {
                match s.lock().unwrap().trim().parse::<f64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Float(n)],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "invalid float".to_string(),
                        )))],
                    }),
                }
            }
            "index_of" => {
                let pattern = self.expect_string(&args, 0)?;
                match s.lock().unwrap().find(&pattern) {
                    Some(idx) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Int(idx as i64)],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
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
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "string".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

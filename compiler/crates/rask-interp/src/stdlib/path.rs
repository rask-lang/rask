// SPDX-License-Identifier: (MIT OR Apache-2.0)
#![allow(dead_code)]
//! Path module — Path type constructor and methods.

use std::collections::HashMap;
use std::path::Path as StdPath;
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle Path type constructor methods (Path.new, Path.from).
    pub(crate) fn call_path_type_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "new" | "from" => {
                let s = self.expect_string(&args, 0)?;
                Ok(make_path_value(&s))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "Path has no static method '{}'",
                method
            ))),
        }
    }

    /// Handle instance methods on Path structs.
    pub(crate) fn call_path_instance_method(
        &self,
        fields: &HashMap<String, Value>,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let path_str = extract_path_string(fields)?;
        let std_path = StdPath::new(&path_str);

        match method {
            "parent" => {
                match std_path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => {
                        let parent = p.to_string_lossy().to_string();
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![make_path_value(&parent)],
                        })
                    }
                    _ => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            "file_name" => {
                option_string(std_path.file_name().map(|s| s.to_string_lossy().to_string()))
            }
            "extension" => {
                option_string(std_path.extension().map(|s| s.to_string_lossy().to_string()))
            }
            "stem" => {
                option_string(std_path.file_stem().map(|s| s.to_string_lossy().to_string()))
            }
            "components" => {
                let components: Vec<Value> = std_path
                    .components()
                    .filter_map(|c| {
                        let s = c.as_os_str().to_string_lossy().to_string();
                        if s.is_empty() { None } else { Some(Value::String(Arc::new(Mutex::new(s)))) }
                    })
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(components))))
            }
            "is_absolute" => {
                Ok(Value::Bool(std_path.is_absolute()))
            }
            "is_relative" => {
                Ok(Value::Bool(std_path.is_relative()))
            }
            "has_extension" => {
                let ext = self.expect_string(&_args, 0).unwrap_or_default();
                let has = std_path.extension()
                    .map(|e| e.to_string_lossy().eq_ignore_ascii_case(&ext))
                    .unwrap_or(false);
                Ok(Value::Bool(has))
            }
            "join" => {
                let other = self.expect_string(&_args, 0)?;
                let joined = std_path.join(&other).to_string_lossy().to_string();
                Ok(make_path_value(&joined))
            }
            "with_extension" => {
                let ext = self.expect_string(&_args, 0)?;
                let new_path = std_path.with_extension(&ext).to_string_lossy().to_string();
                Ok(make_path_value(&new_path))
            }
            "with_file_name" => {
                let name = self.expect_string(&_args, 0)?;
                let new_path = std_path.with_file_name(&name).to_string_lossy().to_string();
                Ok(make_path_value(&new_path))
            }
            "to_string" => {
                Ok(Value::String(Arc::new(Mutex::new(path_str))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Path".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

/// Create a Path struct value from a string.
fn make_path_value(s: &str) -> Value {
    // Normalize separators to forward slash
    let normalized = s.replace('\\', "/");
    // Remove trailing slashes (except root)
    let trimmed = if normalized.len() > 1 {
        normalized.trim_end_matches('/')
    } else {
        &normalized
    };
    // Collapse double separators
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_slash = false;
    for c in trimmed.chars() {
        if c == '/' {
            if !prev_slash || result.is_empty() {
                result.push(c);
            }
            prev_slash = true;
        } else {
            result.push(c);
            prev_slash = false;
        }
    }

    let mut fields = HashMap::new();
    fields.insert(
        "value".to_string(),
        Value::String(Arc::new(Mutex::new(result))),
    );
    Value::Struct {
        name: "Path".to_string(),
        fields,
        resource_id: None,
    }
}

/// Extract the inner string from a Path struct.
fn extract_path_string(fields: &HashMap<String, Value>) -> Result<String, RuntimeError> {
    match fields.get("value") {
        Some(Value::String(s)) => Ok(s.lock().unwrap().clone()),
        _ => Err(RuntimeError::TypeError(
            "Path struct missing 'value' field".to_string(),
        )),
    }
}

/// Wrap an Option<String> into an Option enum value.
fn option_string(opt: Option<String>) -> Result<Value, RuntimeError> {
    match opt {
        Some(s) => Ok(Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            fields: vec![Value::String(Arc::new(Mutex::new(s)))],
        }),
        None => Ok(Value::Enum {
            name: "Option".to_string(),
            variant: "None".to_string(),
            fields: vec![],
        }),
    }
}

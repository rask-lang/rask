// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Filesystem module methods (fs.*) and File/Metadata instance methods.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle fs module methods.
    pub(crate) fn call_fs_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read_file" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(content)))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "read_lines" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let lines: Vec<Value> = content
                            .lines()
                            .map(|l| Value::String(Arc::new(Mutex::new(l.to_string()))))
                            .collect();
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Vec(Arc::new(Mutex::new(lines)))],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "write_file" => {
                let path = self.expect_string(&args, 0)?;
                let content = self.expect_string(&args, 1)?;
                match std::fs::write(&path, &content) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "append_file" => {
                use std::io::Write;
                let path = self.expect_string(&args, 0)?;
                let content = self.expect_string(&args, 1)?;
                let result = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| f.write_all(content.as_bytes()));
                match result {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "exists" => {
                let path = self.expect_string(&args, 0)?;
                Ok(Value::Bool(std::path::Path::new(&path).exists()))
            }
            "open" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::File::open(&path) {
                    Ok(file) => {
                        let arc = Arc::new(Mutex::new(Some(file)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker.register_file(ptr, self.env.scope_depth());
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::File(arc)],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "create" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::File::create(&path) {
                    Ok(file) => {
                        let arc = Arc::new(Mutex::new(Some(file)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker.register_file(ptr, self.env.scope_depth());
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::File(arc)],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "canonicalize" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::canonicalize(&path) {
                    Ok(p) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            p.to_string_lossy().to_string(),
                        )))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "metadata" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let mut fields = HashMap::new();
                        fields.insert("size".to_string(), Value::Int(meta.len() as i64));
                        if let Ok(accessed) = meta.accessed() {
                            if let Ok(dur) = accessed.duration_since(std::time::UNIX_EPOCH) {
                                fields.insert("accessed".to_string(), Value::Int(dur.as_secs() as i64));
                            }
                        }
                        if let Ok(modified) = meta.modified() {
                            if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                                fields.insert("modified".to_string(), Value::Int(dur.as_secs() as i64));
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Struct {
                                name: "Metadata".to_string(),
                                fields,
                                resource_id: None,
                            }],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "remove" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "remove_dir" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::remove_dir(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "create_dir" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::create_dir(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "create_dir_all" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::create_dir_all(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "rename" => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                match std::fs::rename(&from, &to) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "copy" => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                match std::fs::copy(&from, &to) {
                    Ok(bytes) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Int(bytes as i64)],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "fs".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle File instance methods (close, read_all, write, lines).
    pub(crate) fn call_file_method(
        &mut self,
        file: &Arc<Mutex<Option<std::fs::File>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "close" => {
                if file.lock().unwrap().is_none() {
                    return Ok(Value::Unit);
                }
                let ptr = Arc::as_ptr(file) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker.mark_consumed(id)
                        .map_err(|msg| RuntimeError::Panic(msg))?;
                }
                let _ = file.lock().unwrap().take();
                Ok(Value::Unit)
            }
            "read_all" | "read_text" => {
                use std::io::Read;
                let mut file_opt = file.lock().unwrap();
                let f = file_opt.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "File".to_string(), operation: "read from".to_string() }
                })?;
                let mut content = String::new();
                match f.read_to_string(&mut content) {
                    Ok(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(content)))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "write" => {
                use std::io::Write;
                let mut file_opt = file.lock().unwrap();
                let f = file_opt.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "File".to_string(), operation: "write to".to_string() }
                })?;
                let content = self.expect_string(&args, 0)?;
                match f.write_all(content.as_bytes()) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "write_line" => {
                use std::io::Write;
                let mut file_opt = file.lock().unwrap();
                let f = file_opt.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "File".to_string(), operation: "write to".to_string() }
                })?;
                let content = self.expect_string(&args, 0)?;
                match writeln!(f, "{}", content) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            "lines" => {
                use std::io::{BufRead, BufReader};
                let file_opt = file.lock().unwrap();
                let f = file_opt.as_ref().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "File".to_string(), operation: "read lines from".to_string() }
                })?;
                let reader = BufReader::new(f);
                let lines: Vec<Value> = reader
                    .lines()
                    .filter_map(|r| r.ok())
                    .map(|l| Value::String(Arc::new(Mutex::new(l))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(lines))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "File".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Metadata struct methods.
    pub(crate) fn call_metadata_method(
        &self,
        fields: &HashMap<String, Value>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "size" => Ok(fields.get("size").cloned().unwrap_or(Value::Int(0))),
            "accessed" => Ok(fields.get("accessed").cloned().unwrap_or(Value::Int(0))),
            "modified" => Ok(fields.get("modified").cloned().unwrap_or(Value::Int(0))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Metadata".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

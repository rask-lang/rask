// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Filesystem module methods (fs.*) and File/Metadata instance methods.
//!
//! Layer: RUNTIME — all operations require filesystem access.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

/// `Err(IoError.X)` for a failed filesystem call.
///
/// Every `fs.*` and `File` method is declared `… or IoError`, but the error
/// side used to be a bare string, so `if result is IoError as e` never matched
/// and the program silently took the success path. The variant comes from the
/// OS error kind; anything without a spelled-out variant lands in `Other(msg)`.
fn io_error_result(e: &std::io::Error) -> Value {
    use std::io::ErrorKind;
    let msg = e.to_string();
    let (variant, index, carries_msg) = match e.kind() {
        ErrorKind::NotFound => ("NotFound", 0, true),
        ErrorKind::PermissionDenied => ("PermissionDenied", 1, true),
        ErrorKind::AlreadyExists => ("AlreadyExists", 2, true),
        ErrorKind::BrokenPipe => ("BrokenPipe", 3, false),
        ErrorKind::ConnectionReset => ("ConnectionReset", 4, false),
        ErrorKind::TimedOut => ("TimedOut", 5, false),
        ErrorKind::UnexpectedEof => ("UnexpectedEof", 6, false),
        _ => ("Other", 7, true),
    };
    let fields = if carries_msg {
        vec![Value::String(Arc::new(Mutex::new(msg)))]
    } else {
        vec![]
    };
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![Value::Enum {
            name: "IoError".to_string(),
            variant: variant.to_string(),
            fields,
            variant_index: index,
            origin: None,
        }],
        variant_index: 0,
        origin: None,
    }
}

impl Interpreter {
    /// Handle fs module methods.
    pub(crate) fn call_fs_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read_text" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(content)))],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "read_bytes" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let values: Vec<Value> = bytes
                            .into_iter()
                            .map(|b| Value::int(b as i64))
                            .collect();
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Vec(Arc::new(Mutex::new(values)))],
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(io_error_result(&e)),
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
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "write_text" => {
                let path = self.expect_string(&args, 0)?;
                let content = self.expect_string(&args, 1)?;
                match std::fs::write(&path, &content) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "write_bytes" => {
                let path = self.expect_string(&args, 0)?;
                let bytes: Vec<u8> = match args.get(1) {
                    Some(Value::Vec(v)) => v
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|val| match val {
                            Value::Int(n, _) => *n as u8,
                            _ => 0,
                        })
                        .collect(),
                    _ => {
                        return Err(RuntimeError::TypeError(format!(
                            "fs.write_bytes: expected Vec<u8>, got {}",
                            args.get(1).map(|v| v.type_name()).unwrap_or("missing")
                        )));
                    }
                };
                match std::fs::write(&path, &bytes) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "append_text" => {
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
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "exists" => {
                let path = self.expect_string(&args, 0)?;
                Ok(Value::Bool(std::path::Path::new(&path).exists()))
            }
            "open" => {
                let path = self.expect_string(&args, 0)?;
                let mode = args.get(1)
                    .and_then(|v| if let Value::String(s) = v { Some(s.lock().unwrap().clone()) } else { None })
                    .unwrap_or_default();
                let result = match mode.as_str() {
                    "w" => std::fs::File::create(&path),
                    "w+" => std::fs::OpenOptions::new().create(true).read(true).write(true).open(&path),
                    "a" => std::fs::OpenOptions::new().create(true).append(true).open(&path),
                    "a+" => std::fs::OpenOptions::new().create(true).read(true).append(true).open(&path),
                    _ => std::fs::File::open(&path),
                };
                match result {
                    Ok(file) => {
                        let arc = Arc::new(Mutex::new(Some(file)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker.register_file(ptr, self.env.scope_depth());
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::File(arc)],
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(io_error_result(&e)),
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
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(io_error_result(&e)),
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
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "metadata" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let mut fields = IndexMap::new();
                        fields.insert("size".to_string(), Value::int(meta.len() as i64));
                        if let Ok(accessed) = meta.accessed() {
                            if let Ok(dur) = accessed.duration_since(std::time::UNIX_EPOCH) {
                                fields.insert("accessed".to_string(), Value::int(dur.as_secs() as i64));
                            }
                        }
                        if let Ok(modified) = meta.modified() {
                            if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                                fields.insert("modified".to_string(), Value::int(dur.as_secs() as i64));
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::new_struct(
                                "Metadata".to_string(),
                                fields,
                                None,
                            )],
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "delete" | "remove" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "remove_dir" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::remove_dir(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "create_dir" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::create_dir(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "create_dir_all" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::create_dir_all(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
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
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "copy" => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                match std::fs::copy(&from, &to) {
                    Ok(bytes) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::int(bytes as i64)],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            "list_dir" => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut names = Vec::new();
                        for entry in entries {
                            if let Ok(e) = entry {
                                names.push(Value::String(Arc::new(Mutex::new(
                                    e.file_name().to_string_lossy().to_string(),
                                ))));
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Vec(Arc::new(Mutex::new(names)))],
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(io_error_result(&e)),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "fs".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle File instance methods (close, read_all, read_text, write, write_line).
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
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
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
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
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
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(io_error_result(&e)),
                }
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
        fields: &IndexMap<String, Value>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "size" => Ok(fields.get("size").cloned().unwrap_or(Value::int(0))),
            "accessed" => Ok(fields.get("accessed").cloned().unwrap_or(Value::int(0))),
            "modified" => Ok(fields.get("modified").cloned().unwrap_or(Value::int(0))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Metadata".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

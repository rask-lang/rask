// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! I/O module methods (io.*) and stream types (Stdin, Stdout, Stderr, Buffer).
//!
//! Layer: RUNTIME — standard I/O, in-memory buffers.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle io module methods.
    pub(crate) fn call_io_method(
        &self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read_line" => {
                use std::io::{self, BufRead};
                let mut line = String::new();
                match io::stdin().lock().read_line(&mut line) {
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(line)))],
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "stdin" => {
                Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Stdin".to_string(),
                    fields: indexmap::IndexMap::new(),
                    resource_id: None,
                }))))
            }
            "stdout" => {
                Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Stdout".to_string(),
                    fields: indexmap::IndexMap::new(),
                    resource_id: None,
                }))))
            }
            "stderr" => {
                Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Stderr".to_string(),
                    fields: indexmap::IndexMap::new(),
                    resource_id: None,
                }))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "io".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Stdout method calls.
    pub(crate) fn call_stdout_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "write_str" => {
                let s = self.expect_string(&args, 0)?;
                use std::io::Write;
                match std::io::stdout().write_all(s.as_bytes()) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "flush" => {
                use std::io::Write;
                match std::io::stdout().flush() {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "close" => Ok(Value::Unit),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Stdout".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Stderr method calls.
    pub(crate) fn call_stderr_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "write_str" => {
                let s = self.expect_string(&args, 0)?;
                use std::io::Write;
                match std::io::stderr().write_all(s.as_bytes()) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "flush" => {
                use std::io::Write;
                match std::io::stderr().flush() {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "close" => Ok(Value::Unit),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Stderr".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Stdin method calls.
    pub(crate) fn call_stdin_method(
        &self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read_line" => {
                use std::io::{self, BufRead};
                let mut line = String::new();
                match io::stdin().lock().read_line(&mut line) {
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(line)))],
                            variant_index: 0, origin: None,
                        })
                    }
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "read_text" => {
                use std::io::Read;
                let mut buf = String::new();
                match std::io::stdin().read_to_string(&mut buf) {
                    Ok(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(buf)))],
                        variant_index: 0, origin: None,
                    }),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "close" => Ok(Value::Unit),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Stdin".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Helper: construct an IoError.Other(msg) result.
    fn io_error(&self, msg: &str) -> Value {
        Value::Enum {
            name: "Result".to_string(),
            variant: "Err".to_string(),
            fields: vec![Value::Enum {
                name: "IoError".to_string(),
                variant: "Other".to_string(),
                fields: vec![Value::String(Arc::new(Mutex::new(msg.to_string())))],
                variant_index: 0,
                origin: None,
            }],
            variant_index: 0,
            origin: None,
        }
    }
}

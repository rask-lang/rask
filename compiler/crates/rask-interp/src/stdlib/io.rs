// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! I/O module methods (io.*) and stream types (Stdin, Stdout, Stderr, Buffer).
//!
//! Layer: RUNTIME — standard I/O, in-memory buffers.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

/// The bytes out of a `Vec<u8>` argument.
fn expect_bytes(args: &[Value]) -> Vec<u8> {
    match args.first() {
        Some(Value::Vec(v)) => v
            .lock()
            .unwrap()
            .iter()
            .map(|val| match val {
                Value::Int(n, _) => *n as u8,
                _ => 0,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn ok_unit() -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![Value::Unit],
        variant_index: 0,
        origin: None,
    }
}

fn ok_int(n: i64) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![Value::int(n)],
        variant_index: 0,
        origin: None,
    }
}

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
                    // 0 bytes is end of input, not a blank line. Reporting it
                    // as Ok("") makes the two indistinguishable, and every
                    // `loop { read_line() }` spins forever once stdin runs out.
                    Ok(0) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "IoError".to_string(),
                            variant: "UnexpectedEof".to_string(),
                            fields: vec![],
                            variant_index: 6, origin: None,
                        }],
                        variant_index: 1, origin: None,
                    }),
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
            "write_text" => {
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
            // W1/W2: the byte forms. These had no arm at all here while native
            // ran them, so `out.write(bytes)` was "no method `write` on type
            // `Stdout`" on the interpreter only (#859).
            "write" | "write_bytes" => {
                let bytes = expect_bytes(&args);
                use std::io::Write;
                match std::io::stdout().write_all(&bytes) {
                    Ok(()) => {
                        if method == "write" {
                            Ok(ok_int(bytes.len() as i64))
                        } else {
                            Ok(ok_unit())
                        }
                    }
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "flush" => {
                use std::io::Write;
                match std::io::stdout().flush() {
                    Ok(()) => Ok(ok_unit()),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            // Closing flushes: the stream itself stays open — ending the
            // process's own stdout isn't what `close` on a borrowed standard
            // stream should mean — but nothing written through the handle should
            // still be sitting in a buffer once it's gone.
            "close" => {
                use std::io::Write;
                let _ = std::io::stdout().flush();
                Ok(Value::Unit)
            }
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
            "write_text" => {
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
            "write" | "write_bytes" => {
                let bytes = expect_bytes(&args);
                use std::io::Write;
                match std::io::stderr().write_all(&bytes) {
                    Ok(()) => {
                        if method == "write" {
                            Ok(ok_int(bytes.len() as i64))
                        } else {
                            Ok(ok_unit())
                        }
                    }
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "flush" => {
                use std::io::Write;
                match std::io::stderr().flush() {
                    Ok(()) => Ok(ok_unit()),
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            "close" => {
                use std::io::Write;
                let _ = std::io::stderr().flush();
                Ok(Value::Unit)
            }
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
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read_line" => {
                use std::io::{self, BufRead};
                let mut line = String::new();
                match io::stdin().lock().read_line(&mut line) {
                    // 0 bytes is end of input, not a blank line. Reporting it
                    // as Ok("") makes the two indistinguishable, and every
                    // `loop { read_line() }` spins forever once stdin runs out.
                    Ok(0) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "IoError".to_string(),
                            variant: "UnexpectedEof".to_string(),
                            fields: vec![],
                            variant_index: 6, origin: None,
                        }],
                        variant_index: 1, origin: None,
                    }),
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
            // R1/R2: the byte forms. Neither had an arm here while native ran
            // them (#859).
            "read_bytes" => {
                use std::io::Read;
                let mut buf = Vec::new();
                match std::io::stdin().read_to_end(&mut buf) {
                    Ok(_) => {
                        let bytes: Vec<Value> =
                            buf.into_iter().map(|b| Value::int(b as i64)).collect();
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::vec(bytes)],
                            variant_index: 0,
                            origin: None,
                        })
                    }
                    Err(e) => Ok(self.io_error(&e.to_string())),
                }
            }
            // Fills the caller's buffer and answers how many bytes went in,
            // 0 at end of input.
            "read" => {
                let want = match args.first() {
                    Some(Value::Vec(v)) => v.lock().unwrap().len(),
                    _ => 0,
                };
                if want == 0 {
                    return Ok(ok_int(0));
                }
                use std::io::Read;
                let mut buf = vec![0u8; want];
                match std::io::stdin().read(&mut buf) {
                    Ok(n) => {
                        if let Some(Value::Vec(v)) = args.first() {
                            let mut guard = v.lock().unwrap();
                            for i in 0..n {
                                guard[i] = Value::int(buf[i] as i64);
                            }
                        }
                        Ok(ok_int(n as i64))
                    }
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

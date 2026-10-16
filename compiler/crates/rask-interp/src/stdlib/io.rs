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

impl Interpreter {
    /// The `io` module's `@native` half (stdlib/io.rk). The handles, their
    /// methods and `io.copy` are Rask bodies over these, run here as natively.
    ///
    /// The handles and their methods used to be answered in Rust too, ahead of
    /// the bodies: `io.stdout()` built a `Stdout` with no `stream` field, and a
    /// failed read came back as `Err(<a string>)` where the body builds an
    /// `IoError` (#1562).
    pub(crate) fn call_io_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        use std::io::Write;
        let stream = |args: &[Value]| match args.first() {
            Some(Value::Int(n, _)) => *n,
            _ => 1,
        };
        // 0 is stdin, 1 stdout, 2 stderr, as the handles' `stream` says.
        let write_to = |which: i64, bytes: &[u8]| -> i64 {
            let r = if which == 2 {
                std::io::stderr().write_all(bytes)
            } else {
                std::io::stdout().write_all(bytes)
            };
            match r {
                Ok(()) => bytes.len() as i64,
                Err(e) => {
                    super::net::set_last_os_error(&e);
                    -1
                }
            }
        };
        match method {
            "read_line" => self.io_read_line(),
            "write_std_text" => {
                let text = self.expect_string(&args, 1)?;
                Ok(Value::int(write_to(stream(&args), text.as_bytes())))
            }
            "write_std_bytes" => {
                let bytes = expect_bytes(&args[1..]);
                Ok(Value::int(write_to(stream(&args), &bytes)))
            }
            "flush_std" => {
                let r = if stream(&args) == 2 { std::io::stderr().flush() } else { std::io::stdout().flush() };
                Ok(Value::int(match r {
                    Ok(()) => 0,
                    Err(e) => {
                        super::net::set_last_os_error(&e);
                        -1
                    }
                }))
            }
            "read_std_bytes" => {
                use std::io::Read;
                let max = match args.first() {
                    Some(Value::Int(n, _)) => (*n).max(0) as u64,
                    _ => 0,
                };
                let mut buf = Vec::new();
                let _ = std::io::stdin().take(max).read_to_end(&mut buf);
                Ok(Value::vec(buf.into_iter().map(|b| Value::int(b as i64)).collect()))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "io".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `rask_io_read_line`: a line without its terminator, `UnexpectedEof` at
    /// the end of input — not `Ok("")`, which would make every `loop {
    /// read_line() }` spin forever once stdin runs out — or the OS error.
    pub(crate) fn io_read_line(&mut self) -> Result<Value, RuntimeError> {
        use std::io::BufRead;
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) => {
                let eof = self.enum_variant("IoError", "UnexpectedEof", Vec::new())?;
                Ok(Value::err(eof))
            }
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                Ok(Value::ok(Value::String(Arc::new(Mutex::new(line)))))
            }
            Err(e) => {
                super::net::set_last_os_error(&e);
                let err = self.call_rask_static("IoError", "last_os_error", Vec::new())?;
                Ok(Value::err(err))
            }
        }
    }
}

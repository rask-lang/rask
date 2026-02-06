//! I/O module methods (io.*).

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
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "io".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

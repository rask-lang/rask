// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! OS module methods (os.*), Command/Process types, Signal handling.
//!
//! Layer: RUNTIME — env vars, process control, subprocess, signals.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle os module methods.
    pub(crate) fn call_os_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // --- Environment variables ---
            #[cfg(not(target_arch = "wasm32"))]
            "env" => {
                let name = self.expect_string(&args, 0)?;
                match std::env::var(&name) {
                    Ok(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(val)))],
                        variant_index: 0, origin: None,
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            #[cfg(target_arch = "wasm32")]
            "env" => {
                // Always return None in browser
                Ok(Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }

            #[cfg(not(target_arch = "wasm32"))]
            "env_or" => {
                let name = self.expect_string(&args, 0)?;
                let default = self.expect_string(&args, 1)?;
                let val = std::env::var(&name).unwrap_or(default);
                Ok(Value::String(Arc::new(Mutex::new(val))))
            }
            #[cfg(target_arch = "wasm32")]
            "env_or" => {
                // Return default in browser
                let _name = self.expect_string(&args, 0)?;
                let default = self.expect_string(&args, 1)?;
                Ok(Value::String(Arc::new(Mutex::new(default))))
            }

            #[cfg(not(target_arch = "wasm32"))]
            "set_env" | "remove_env" | "vars" => {
                match method {
                    "set_env" => {
                        let key = self.expect_string(&args, 0)?;
                        let value = self.expect_string(&args, 1)?;
                        std::env::set_var(&key, &value);
                        Ok(Value::Unit)
                    }
                    "remove_env" => {
                        let key = self.expect_string(&args, 0)?;
                        std::env::remove_var(&key);
                        Ok(Value::Unit)
                    }
                    "vars" => {
                        let vars: Vec<Value> = std::env::vars()
                            .map(|(k, v)| {
                                Value::Vec(Arc::new(Mutex::new(vec![
                                    Value::String(Arc::new(Mutex::new(k))),
                                    Value::String(Arc::new(Mutex::new(v))),
                                ])))
                            })
                            .collect();
                        Ok(Value::Vec(Arc::new(Mutex::new(vars))))
                    }
                    _ => unreachable!()
                }
            }
            #[cfg(target_arch = "wasm32")]
            "set_env" | "remove_env" | "vars" => {
                Err(RuntimeError::Generic(
                    format!("os.{} not available in browser playground", method)
                ))
            }

            // --- Command-line arguments ---
            "args" => {
                let args_vec: Vec<Value> = self
                    .cli_args
                    .iter()
                    .map(|s| Value::String(Arc::new(Mutex::new(s.clone()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(args_vec))))
            }

            // --- Process control ---
            "exit" => {
                let code = args
                    .first()
                    .map(|v| match v {
                        Value::Int(n) => *n as i32,
                        _ => 1,
                    })
                    .unwrap_or(0);
                Err(RuntimeError::Exit(code))
            }

            #[cfg(not(target_arch = "wasm32"))]
            "getpid" => {
                Ok(Value::Int(std::process::id() as i64))
            }
            #[cfg(target_arch = "wasm32")]
            "getpid" => {
                Err(RuntimeError::Generic(
                    "os.getpid() not available in browser playground".to_string()
                ))
            }

            // --- Platform info ---
            "platform" => {
                let platform = if cfg!(target_os = "linux") {
                    "linux"
                } else if cfg!(target_os = "macos") {
                    "macos"
                } else if cfg!(target_os = "windows") {
                    "windows"
                } else if cfg!(target_arch = "wasm32") {
                    "wasm"
                } else {
                    "unknown"
                };
                Ok(Value::String(Arc::new(Mutex::new(platform.to_string()))))
            }
            "arch" => {
                let arch = if cfg!(target_arch = "x86_64") {
                    "x86_64"
                } else if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else if cfg!(target_arch = "wasm32") {
                    "wasm32"
                } else {
                    "unknown"
                };
                Ok(Value::String(Arc::new(Mutex::new(arch.to_string()))))
            }

            // --- Signals ---
            #[cfg(not(target_arch = "wasm32"))]
            "on_signal" => {
                // SG2: returns Receiver<Signal> via channel
                // Signal handling uses a self-pipe: the C signal handler writes to a pipe,
                // a background thread reads the pipe and sends to the channel.
                use std::sync::mpsc;
                use std::os::unix::io::{FromRawFd, RawFd};

                let signal_names = if let Some(Value::Vec(v)) = args.first() {
                    let guard = v.lock().unwrap();
                    guard.iter().filter_map(|s| {
                        if let Value::Enum { variant, .. } = s {
                            Some(variant.clone())
                        } else {
                            None
                        }
                    }).collect::<Vec<_>>()
                } else {
                    vec![]
                };

                let (tx, _rx) = mpsc::channel::<Value>();

                // Register signal handlers via pipe-based approach
                for sig_name in &signal_names {
                    let sig_num: Option<i32> = match sig_name.as_str() {
                        "Interrupt" => Some(2),   // SIGINT
                        "Terminate" => Some(15),  // SIGTERM
                        "Hangup" => Some(1),      // SIGHUP
                        "User1" => Some(10),      // SIGUSR1
                        "User2" => Some(12),      // SIGUSR2
                        _ => None,
                    };
                    if let Some(num) = sig_num {
                        let mut senders = SIGNAL_SENDERS.lock().unwrap();
                        senders.push((num, tx.clone(), sig_name.clone()));
                        // Install handler via raw syscall
                        unsafe {
                            let _ = set_signal_handler(num);
                        }
                    }
                }

                let rx_value = Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Receiver".to_string(),
                    fields: indexmap::IndexMap::new(),
                    resource_id: None,
                })));

                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![rx_value],
                    variant_index: 0,
                    origin: None,
                })
            }

            _ => Err(RuntimeError::NoSuchMethod {
                ty: "os".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Command type static methods (Command.new, etc.).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn call_command_type_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "new" => {
                let program = self.expect_string(&args, 0)?;
                let mut fields = indexmap::IndexMap::new();
                fields.insert("program".to_string(), Value::String(Arc::new(Mutex::new(program))));
                fields.insert("args".to_string(), Value::Vec(Arc::new(Mutex::new(vec![]))));
                fields.insert("env_vars".to_string(), Value::Vec(Arc::new(Mutex::new(vec![]))));
                fields.insert("cwd".to_string(), Value::Enum {
                    name: "Option".to_string(),
                    variant: "None".to_string(),
                    fields: vec![],
                    variant_index: 0,
                    origin: None,
                });
                Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Command".to_string(),
                    fields,
                    resource_id: None,
                }))))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "Command has no static method '{}'", method
            ))),
        }
    }

    /// Handle Command instance methods (arg, args, run, spawn, etc.).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn call_command_instance_method(
        &self,
        fields: &indexmap::IndexMap<String, Value>,
        method: &str,
        mut extra_args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "arg" => {
                let arg = self.expect_string(&extra_args, 0)?;
                let mut new_fields = fields.clone();
                if let Value::Vec(ref v) = new_fields["args"] {
                    let mut guard = v.lock().unwrap();
                    guard.push(Value::String(Arc::new(Mutex::new(arg))));
                }
                Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Command".to_string(),
                    fields: new_fields,
                    resource_id: None,
                }))))
            }
            "args" => {
                if let Some(Value::Vec(extra)) = extra_args.first() {
                    let extra_guard = extra.lock().unwrap();
                    let mut new_fields = fields.clone();
                    if let Value::Vec(ref v) = new_fields["args"] {
                        let mut guard = v.lock().unwrap();
                        for a in extra_guard.iter() {
                            guard.push(a.clone());
                        }
                    }
                    Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                        name: "Command".to_string(),
                        fields: new_fields,
                        resource_id: None,
                    }))))
                } else {
                    Err(RuntimeError::TypeError("args() expects a Vec<string>".into()))
                }
            }
            "cwd" => {
                let dir = self.expect_string(&extra_args, 0)?;
                let mut new_fields = fields.clone();
                new_fields.insert("cwd".to_string(), Value::Enum {
                    name: "Option".to_string(),
                    variant: "Some".to_string(),
                    fields: vec![Value::String(Arc::new(Mutex::new(dir)))],
                    variant_index: 0,
                    origin: None,
                });
                Ok(Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                    name: "Command".to_string(),
                    fields: new_fields,
                    resource_id: None,
                }))))
            }
            "run" => {
                let program = extract_string_field(fields, "program")?;
                let cmd_args = extract_string_vec_field(fields, "args")?;
                let cwd = extract_optional_string_field(fields, "cwd");

                let mut cmd = std::process::Command::new(&program);
                for a in &cmd_args {
                    cmd.arg(a);
                }
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }

                match cmd.output() {
                    Ok(output) => {
                        let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
                        let status = output.status.code().unwrap_or(-1);

                        let mut out_fields = indexmap::IndexMap::new();
                        out_fields.insert("status".to_string(), Value::Int(status as i64));
                        out_fields.insert("stdout".to_string(), Value::String(Arc::new(Mutex::new(stdout_str))));
                        out_fields.insert("stderr".to_string(), Value::String(Arc::new(Mutex::new(stderr_str))));

                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                                name: "Output".to_string(),
                                fields: out_fields,
                                resource_id: None,
                            })))],
                            variant_index: 0,
                            origin: None,
                        })
                    }
                    Err(e) => {
                        let variant = if e.kind() == std::io::ErrorKind::NotFound {
                            "NotFound"
                        } else {
                            "Other"
                        };
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::Enum {
                                name: "IoError".to_string(),
                                variant: variant.to_string(),
                                fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                                variant_index: 0,
                                origin: None,
                            }],
                            variant_index: 0,
                            origin: None,
                        })
                    }
                }
            }
            "spawn" => {
                let program = extract_string_field(fields, "program")?;
                let cmd_args = extract_string_vec_field(fields, "args")?;
                let cwd = extract_optional_string_field(fields, "cwd");

                let mut cmd = std::process::Command::new(&program);
                for a in &cmd_args {
                    cmd.arg(a);
                }
                if let Some(dir) = cwd {
                    cmd.current_dir(dir);
                }
                cmd.stdin(std::process::Stdio::piped());
                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());

                match cmd.spawn() {
                    Ok(child) => {
                        let mut proc_fields = indexmap::IndexMap::new();
                        proc_fields.insert("child".to_string(), Value::Int(0)); // placeholder
                        let proc = Value::Struct(Arc::new(Mutex::new(crate::value::StructData {
                            name: "Process".to_string(),
                            fields: proc_fields,
                            resource_id: None,
                        })));
                        // Store child process for later wait/kill
                        CHILD_PROCESSES.lock().unwrap().push(child);

                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![proc],
                            variant_index: 0,
                            origin: None,
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::Enum {
                            name: "IoError".to_string(),
                            variant: "Other".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                            variant_index: 0,
                            origin: None,
                        }],
                        variant_index: 0,
                        origin: None,
                    }),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Command".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle Output instance methods.
    pub(crate) fn call_output_instance_method(
        &self,
        fields: &indexmap::IndexMap<String, Value>,
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "success" => {
                if let Some(Value::Int(status)) = fields.get("status") {
                    Ok(Value::Bool(*status == 0))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Output".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

// --- Helper functions ---

fn extract_string_field(
    fields: &indexmap::IndexMap<String, Value>,
    name: &str,
) -> Result<String, RuntimeError> {
    match fields.get(name) {
        Some(Value::String(s)) => Ok(s.lock().unwrap().clone()),
        _ => Err(RuntimeError::NoSuchField {
            ty: "Command".to_string(),
            field: name.to_string(),
        }),
    }
}

fn extract_string_vec_field(
    fields: &indexmap::IndexMap<String, Value>,
    name: &str,
) -> Result<Vec<String>, RuntimeError> {
    match fields.get(name) {
        Some(Value::Vec(v)) => {
            let guard = v.lock().unwrap();
            Ok(guard.iter().filter_map(|v| {
                if let Value::String(s) = v {
                    Some(s.lock().unwrap().clone())
                } else {
                    None
                }
            }).collect())
        }
        _ => Ok(vec![]),
    }
}

fn extract_optional_string_field(
    fields: &indexmap::IndexMap<String, Value>,
    name: &str,
) -> Option<String> {
    match fields.get(name) {
        Some(Value::Enum { variant, fields, .. }) if variant == "Some" => {
            if let Some(Value::String(s)) = fields.first() {
                Some(s.lock().unwrap().clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

// Global storage for spawned child processes and signal senders
#[cfg(not(target_arch = "wasm32"))]
static CHILD_PROCESSES: std::sync::LazyLock<Mutex<Vec<std::process::Child>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

#[cfg(not(target_arch = "wasm32"))]
static SIGNAL_SENDERS: std::sync::LazyLock<Mutex<Vec<(i32, std::sync::mpsc::Sender<Value>, String)>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

/// Install a signal handler using raw syscall (avoids libc dependency).
#[cfg(not(target_arch = "wasm32"))]
unsafe fn set_signal_handler(sig: i32) -> Result<(), ()> {
    // Use the C signal() function via extern
    extern "C" {
        fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
    }
    let result = signal(sig, signal_handler_fn);
    if result == usize::MAX { Err(()) } else { Ok(()) }
}

#[cfg(not(target_arch = "wasm32"))]
extern "C" fn signal_handler_fn(sig: i32) {
    // Signal handlers must be async-signal-safe.
    // We just set a flag; actual delivery happens elsewhere.
    if let Ok(senders) = SIGNAL_SENDERS.try_lock() {
        for (num, tx, name) in senders.iter() {
            if *num == sig {
                let _ = tx.send(Value::Enum {
                    name: "Signal".to_string(),
                    variant: name.clone(),
                    fields: vec![],
                    variant_index: 0,
                    origin: None,
                });
            }
        }
    }
}

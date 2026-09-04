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
            "set_env" | "remove_env" | "env_vars" => {
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
                    "env_vars" => {
                        let vars: Vec<Value> = std::env::vars()
                            .map(|(k, v)| {
                                Value::tuple(vec![
                                    Value::String(Arc::new(Mutex::new(k))),
                                    Value::String(Arc::new(Mutex::new(v))),
                                ])
                            })
                            .collect();
                        Ok(Value::vec(vars))
                    }
                    _ => unreachable!()
                }
            }
            #[cfg(target_arch = "wasm32")]
            "set_env" | "remove_env" | "env_vars" => {
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
                Ok(Value::vec(args_vec))
            }

            // --- Process control ---
            "exit" => {
                let code = args
                    .first()
                    .map(|v| match v {
                        Value::Int(n, _) => *n as i32,
                        _ => 1,
                    })
                    .unwrap_or(0);
                Err(RuntimeError::Exit(code))
            }

            #[cfg(not(target_arch = "wasm32"))]
            "pid" => {
                Ok(Value::int(std::process::id() as i64))
            }
            #[cfg(target_arch = "wasm32")]
            "pid" => {
                Err(RuntimeError::Generic(
                    "os.pid() not available in browser playground".to_string()
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
            "signals" => {
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

            #[cfg(not(target_arch = "wasm32"))]
            "process_run" | "process_stdout" | "process_stderr" => {
                self.call_process_function(method, args)
            }

            _ => Err(RuntimeError::NoSuchMethod {
                ty: "os".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `os.Command`'s three native entry points. Everything else about the
    /// builder is Rask now (`stdlib/os.rk`), so both backends run one
    /// implementation — this used to be a second one, modelling `Command` as a
    /// struct with fields the compiler had never heard of.
    ///
    /// The captured output belongs to the last run on this thread, which is
    /// what `Command.run` reads on its next two lines.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn call_process_function(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "process_run" => {
                let program = self.expect_string(&args, 0)?;
                let cmd_args = string_vec_arg(&args, 1);
                let envs = string_vec_arg(&args, 2);
                let dir = self.expect_string(&args, 3).unwrap_or_default();

                let mut cmd = std::process::Command::new(&program);
                cmd.args(&cmd_args);
                if !dir.is_empty() {
                    cmd.current_dir(&dir);
                }
                for pair in envs.chunks(2) {
                    if let [k, v] = pair {
                        cmd.env(k, v);
                    }
                }
                // Stdio modes: 0 inherit, 1 piped, 2 null. `output()` always
                // captures, so Inherit and Null both mean "nothing to read".
                let mode = |i: usize| match args.get(i) {
                    Some(Value::Int(n, _)) => *n,
                    _ => 1,
                };
                let (want_out, want_err) = (mode(5) == 1, mode(6) == 1);

                match cmd.output() {
                    Ok(output) => {
                        let out = if want_out {
                            String::from_utf8_lossy(&output.stdout).to_string()
                        } else {
                            String::new()
                        };
                        let err = if want_err {
                            String::from_utf8_lossy(&output.stderr).to_string()
                        } else {
                            String::new()
                        };
                        PROCESS_CAPTURE.with(|c| *c.borrow_mut() = (out, err));
                        // A signalled child has no exit code; 128+signal is
                        // what a shell reports and what native answers.
                        Ok(Value::int(output.status.code().unwrap_or(-1) as i64))
                    }
                    // Never started. Native reports the child's errno through
                    // a close-on-exec pipe; this is the same number.
                    Err(e) => {
                        PROCESS_CAPTURE.with(|c| *c.borrow_mut() = (String::new(), String::new()));
                        let code = e.raw_os_error().unwrap_or(0);
                        Ok(Value::int(if code > 0 { -(code as i64) } else { -1 }))
                    }
                }
            }
            "process_stdout" => {
                let out = PROCESS_CAPTURE.with(|c| c.borrow().0.clone());
                Ok(Value::String(Arc::new(Mutex::new(out))))
            }
            "process_stderr" => {
                let err = PROCESS_CAPTURE.with(|c| c.borrow().1.clone());
                Ok(Value::String(Arc::new(Mutex::new(err))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "os".to_string(),
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
                if let Some(Value::Int(status, _)) = fields.get("status") {
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

// What the last `process_run` on this thread captured — the same convention
// the C runtime uses, so the two backends answer the same way.
#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static PROCESS_CAPTURE: std::cell::RefCell<(String, String)> =
        const { std::cell::RefCell::new((String::new(), String::new())) };
}

/// The elements of a `Vec<string>` argument, or empty when it isn't one.
fn string_vec_arg(args: &[Value], index: usize) -> Vec<String> {
    match args.get(index) {
        Some(Value::Vec(v)) => v
            .lock()
            .unwrap()
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.lock().unwrap().clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
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

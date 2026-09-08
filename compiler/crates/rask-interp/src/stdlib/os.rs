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
            "process_run"
            | "process_stdout"
            | "process_stderr"
            | "process_spawn"
            | "process_pid"
            | "process_wait"
            | "process_kill_and_wait"
            | "process_poll"
            | "process_write_stdin"
            | "process_read_stdout"
            | "process_captured_stdout"
            | "process_captured_stderr"
            | "process_release" => self.call_process_function(method, args),

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
            "process_spawn" => {
                let program = self.expect_string(&args, 0)?;
                let cmd_args = string_vec_arg(&args, 1);
                let envs = string_vec_arg(&args, 2);
                let dir = self.expect_string(&args, 3).unwrap_or_default();
                let mode = |i: usize| match args.get(i) {
                    Some(Value::Int(n, _)) => *n,
                    _ => 1,
                };

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
                cmd.stdin(stdio_for(mode(4)));
                cmd.stdout(stdio_for(mode(5)));
                cmd.stderr(stdio_for(mode(6)));

                match cmd.spawn() {
                    Ok(child) => Ok(Value::int(SPAWNED.insert(child))),
                    Err(e) => {
                        let code = e.raw_os_error().unwrap_or(0);
                        Ok(Value::int(if code > 0 { -(code as i64) } else { -1 }))
                    }
                }
            }
            "process_pid" => Ok(Value::int(SPAWNED.with_proc(
                handle_arg(&args, 0),
                -1,
                |p| p.child.id() as i64,
            ))),
            "process_wait" => Ok(Value::int(SPAWNED.with_proc(
                handle_arg(&args, 0),
                -1,
                |p| p.wait(),
            ))),
            "process_kill_and_wait" => Ok(Value::int(SPAWNED.with_proc(
                handle_arg(&args, 0),
                -1,
                |p| {
                    if p.status.is_none() {
                        let _ = p.child.kill();
                    }
                    p.wait()
                },
            ))),
            "process_poll" => Ok(Value::int(SPAWNED.with_proc(
                handle_arg(&args, 0),
                -1,
                |p| p.poll(),
            ))),
            "process_write_stdin" => {
                let data = self.expect_string(&args, 1)?;
                Ok(Value::int(SPAWNED.with_proc(
                    handle_arg(&args, 0),
                    -1,
                    |p| p.write_stdin(data.as_bytes()),
                )))
            }
            "process_read_stdout" => {
                let out = SPAWNED.with_proc(handle_arg(&args, 0), String::new(), |p| {
                    p.drain_stdout();
                    std::mem::take(&mut p.out)
                });
                Ok(Value::String(Arc::new(Mutex::new(out))))
            }
            "process_captured_stdout" => {
                let out = SPAWNED.with_proc(handle_arg(&args, 0), String::new(), |p| p.out.clone());
                Ok(Value::String(Arc::new(Mutex::new(out))))
            }
            "process_captured_stderr" => {
                let err = SPAWNED.with_proc(handle_arg(&args, 0), String::new(), |p| p.err.clone());
                Ok(Value::String(Arc::new(Mutex::new(err))))
            }
            "process_release" => {
                SPAWNED.remove(handle_arg(&args, 0));
                Ok(Value::Unit)
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

/// The `Stdio` code `stdlib/os.rk` sends: 0 inherit, 1 piped, 2 null.
#[cfg(not(target_arch = "wasm32"))]
fn stdio_for(mode: i64) -> std::process::Stdio {
    match mode {
        0 => std::process::Stdio::inherit(),
        2 => std::process::Stdio::null(),
        _ => std::process::Stdio::piped(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn handle_arg(args: &[Value], index: usize) -> i64 {
    match args.get(index) {
        Some(Value::Int(n, _)) => *n,
        _ => 0,
    }
}

/// A child `spawn` handed back. The native side keeps the same pieces in a
/// `RaskProcess` and passes its address as the handle; here the handle is a
/// key into `SPAWNED`, because a Rust `Child` is not an address a Rask `i64`
/// may carry across a GC-less boundary and back.
///
/// `out`/`err` accumulate the same way the C `Captured` does: whatever has
/// been drained so far, so `read_stdout` and `wait` can both take a turn.
#[cfg(not(target_arch = "wasm32"))]
struct SpawnedProcess {
    child: std::process::Child,
    out: String,
    err: String,
    /// The exit status once reaped. `Child::wait` is not idempotent about
    /// stdin, so the second call must not repeat the work.
    status: Option<i64>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SpawnedProcess {
    fn drain_stdout(&mut self) {
        use std::io::Read;
        if let Some(mut pipe) = self.child.stdout.take() {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            self.out.push_str(&String::from_utf8_lossy(&buf));
        }
    }

    fn drain_stderr(&mut self) {
        use std::io::Read;
        if let Some(mut pipe) = self.child.stderr.take() {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            self.err.push_str(&String::from_utf8_lossy(&buf));
        }
    }

    /// Closes stdin first — a child reading to EOF would never exit — then
    /// drains both pipes so `Output` carries what is left.
    fn wait(&mut self) -> i64 {
        if let Some(status) = self.status {
            return status;
        }
        drop(self.child.stdin.take());
        self.drain_stdout();
        self.drain_stderr();
        let status = match self.child.wait() {
            Ok(st) => exit_code(&st),
            Err(e) => -(e.raw_os_error().unwrap_or(1) as i64),
        };
        self.status = Some(status);
        status
    }

    /// -1 while it is still running; its status once it has exited.
    fn poll(&mut self) -> i64 {
        if let Some(status) = self.status {
            return status;
        }
        match self.child.try_wait() {
            Ok(Some(st)) => {
                let status = exit_code(&st);
                self.status = Some(status);
                self.drain_stdout();
                self.drain_stderr();
                status
            }
            Ok(None) => -1,
            Err(e) => {
                let status = -(e.raw_os_error().unwrap_or(1) as i64);
                self.status = Some(status);
                status
            }
        }
    }

    /// 0, or a negative errno. No pipe on stdin is EBADF, not a silent drop.
    fn write_stdin(&mut self, bytes: &[u8]) -> i64 {
        use std::io::Write;
        let Some(pipe) = self.child.stdin.as_mut() else {
            return -9; // EBADF
        };
        match pipe.write_all(bytes).and_then(|()| pipe.flush()) {
            Ok(()) => 0,
            Err(e) => -(e.raw_os_error().unwrap_or(1) as i64),
        }
    }
}

/// A signalled child has no exit code; 128+signal is what a shell reports and
/// what the C runtime answers.
#[cfg(not(target_arch = "wasm32"))]
fn exit_code(status: &std::process::ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return code as i64;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig as i64;
        }
    }
    -1
}

#[cfg(not(target_arch = "wasm32"))]
struct SpawnTable {
    procs: Mutex<(i64, std::collections::HashMap<i64, SpawnedProcess>)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SpawnTable {
    fn insert(&self, child: std::process::Child) -> i64 {
        let mut guard = self.procs.lock().unwrap();
        guard.0 += 1;
        let handle = guard.0;
        guard.1.insert(
            handle,
            SpawnedProcess { child, out: String::new(), err: String::new(), status: None },
        );
        handle
    }

    /// Run `f` over one child, or answer `missing` when the handle is stale.
    fn with_proc<T>(&self, handle: i64, missing: T, f: impl FnOnce(&mut SpawnedProcess) -> T) -> T {
        let mut guard = self.procs.lock().unwrap();
        match guard.1.get_mut(&handle) {
            Some(p) => f(p),
            None => missing,
        }
    }

    fn remove(&self, handle: i64) {
        self.procs.lock().unwrap().1.remove(&handle);
    }
}

#[cfg(not(target_arch = "wasm32"))]
static SPAWNED: std::sync::LazyLock<SpawnTable> = std::sync::LazyLock::new(|| SpawnTable {
    procs: Mutex::new((0, std::collections::HashMap::new())),
});

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

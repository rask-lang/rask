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
            // The same contract as `rask_os_signal_forward` in
            // `runtime/signal.c`: `os.signals` is Rask, and this only hands
            // the sender to the reader.
            #[cfg(not(target_arch = "wasm32"))]
            "signal_forward" => {
                let Some(Value::Sender(tx)) = args.first() else {
                    return Err(RuntimeError::Generic("signal_forward expects a Sender".to_string()));
                };
                let wanted: Vec<usize> = match args.get(1) {
                    Some(Value::Vec(v)) => v
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(|s| match s {
                            Value::Enum { variant_index, .. } => Some(*variant_index as usize),
                            _ => None,
                        })
                        .filter(|i| *i < SIGNAL_NUMBERS.len())
                        .collect(),
                    _ => vec![],
                };
                if let Err(e) = start_signal_reader() {
                    tx.close();
                    return Ok(Value::Int(-(e.raw_os_error().unwrap_or(1) as i64), crate::value::IntKind::I64));
                }
                let mut listeners = SIGNAL_LISTENERS.lock().unwrap();
                for i in wanted {
                    // SG2: the last registration wins. Dropping the old end
                    // closes the old receiver once nothing else feeds it.
                    if let Some(old) = listeners[i].replace(tx.clone_end()) {
                        old.close();
                    }
                    unsafe {
                        let _ = set_signal_handler(SIGNAL_NUMBERS[i]);
                    }
                }
                tx.close();
                Ok(Value::Int(0, crate::value::IntKind::I64))
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

/// `Signal`'s variants by index, as OS signal numbers.
#[cfg(not(target_arch = "wasm32"))]
const SIGNAL_NUMBERS: [i32; 5] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGUSR1, libc::SIGUSR2];

/// The one channel each signal goes to (SG2: the last registration wins).
#[cfg(not(target_arch = "wasm32"))]
static SIGNAL_LISTENERS: Mutex<[Option<Arc<crate::chan::SenderEnd>>; 5]> =
    Mutex::new([None, None, None, None, None]);

/// Variants raised and not yet delivered, one bit per index.
#[cfg(not(target_arch = "wasm32"))]
static SIGNAL_PENDING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write end of the self-pipe; -1 until the reader starts.
#[cfg(not(target_arch = "wasm32"))]
static SIGNAL_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// Start the thread that turns pipe wakeups into channel sends, once.
#[cfg(not(target_arch = "wasm32"))]
fn start_signal_reader() -> Result<(), std::io::Error> {
    // The errno the pipe failed with, if it did.
    static STARTED: std::sync::OnceLock<Option<i32>> = std::sync::OnceLock::new();
    let failed = *STARTED.get_or_init(|| {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return std::io::Error::last_os_error().raw_os_error();
        }
        let read_fd = fds[0];
        SIGNAL_PIPE.store(fds[1], std::sync::atomic::Ordering::SeqCst);
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            loop {
                let n = unsafe { libc::read(read_fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if n <= 0 {
                    return;
                }
                let raised = SIGNAL_PENDING.swap(0, std::sync::atomic::Ordering::SeqCst);
                for (i, &num) in SIGNAL_NUMBERS.iter().enumerate() {
                    if raised & (1 << i) == 0 {
                        continue;
                    }
                    let mut listeners = SIGNAL_LISTENERS.lock().unwrap();
                    let Some(tx) = listeners[i].as_ref() else { continue };
                    // A full channel drops the signal rather than block every
                    // other listener behind a slow one.
                    let sent = tx.try_send(Value::Enum {
                        name: "Signal".to_string(),
                        variant: SIGNAL_NAMES[i].to_string(),
                        fields: vec![],
                        variant_index: i as u32,
                        origin: None,
                    });
                    // SG3: the receiver is gone, so the signal does what it
                    // would have done had nobody registered.
                    if let Err(crate::chan::SendError::Closed(_)) = sent {
                        if let Some(old) = listeners[i].take() {
                            old.close();
                        }
                        drop(listeners);
                        unsafe {
                            libc::signal(num, libc::SIG_DFL);
                            libc::kill(libc::getpid(), num);
                        }
                    }
                }
            }
        });
        None
    });
    match failed {
        Some(errno) => Err(std::io::Error::from_raw_os_error(errno)),
        None => Ok(()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
const SIGNAL_NAMES: [&str; 5] = ["Interrupt", "Terminate", "Hangup", "User1", "User2"];

#[cfg(not(target_arch = "wasm32"))]
unsafe fn set_signal_handler(sig: i32) -> Result<(), ()> {
    let mut action: libc::sigaction = std::mem::zeroed();
    action.sa_sigaction = signal_handler_fn as extern "C" fn(i32) as usize;
    action.sa_flags = libc::SA_RESTART;
    libc::sigemptyset(&mut action.sa_mask);
    if libc::sigaction(sig, &action, std::ptr::null_mut()) != 0 { Err(()) } else { Ok(()) }
}

/// Only async-signal-safe work here: an atomic and a `write`. The reader
/// thread does the allocating and the locking.
#[cfg(not(target_arch = "wasm32"))]
extern "C" fn signal_handler_fn(sig: i32) {
    if let Some(i) = SIGNAL_NUMBERS.iter().position(|&n| n == sig) {
        SIGNAL_PENDING.fetch_or(1 << i, std::sync::atomic::Ordering::SeqCst);
    }
    let fd = SIGNAL_PIPE.load(std::sync::atomic::Ordering::SeqCst);
    if fd >= 0 {
        unsafe {
            let _ = libc::write(fd, (&1u8 as *const u8).cast(), 1);
        }
    }
}

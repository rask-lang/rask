// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Networking module methods (net.*) and TCP connection instance methods.
//!
//! Layer: RUNTIME — socket operations require OS access.

use indexmap::IndexMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{MapData, MapKey, Value};

/// Build a Result.Ok(value).
fn make_result_ok(value: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![value],
        variant_index: 0, origin: None,
    }
}

/// Build a `Result.Err(IoError.Other(message))`.
///
/// The payload used to be a bare string. Every `net` function declares
/// `T or IoError`, so `e.message()` on the error side failed with "no method
/// `message` on type `string`" — while native, which builds a real `IoError`,
/// was fine (#863). The outer tag was wrong too: `Err` is variant 1, not 0.
fn make_result_err(msg: &str) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![Value::Enum {
            name: "IoError".to_string(),
            variant: "Other".to_string(),
            fields: vec![Value::String(Arc::new(Mutex::new(msg.to_string())))],
            // NotFound(0) PermissionDenied(1) AlreadyExists(2) BrokenPipe(3)
            // ConnectionReset(4) TimedOut(5) UnexpectedEof(6) Other(7) Cancelled(8)
            variant_index: 7,
            origin: None,
        }],
        variant_index: 1, origin: None,
    }
}

fn is_cancelled(e: &std::io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<Cancelled>())
}

/// `IoError` for a failed socket call, `Cancelled` when a cancel ended it.
fn io_err(e: &std::io::Error) -> Value {
    if !is_cancelled(e) {
        return make_result_err(&e.to_string());
    }
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![Value::Enum {
            name: "IoError".to_string(),
            variant: "Cancelled".to_string(),
            fields: vec![],
            variant_index: 8,
            origin: None,
        }],
        variant_index: 1, origin: None,
    }
}

#[derive(Debug)]
struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for Cancelled {}

// ─── The last error, for `IoError.last_os_error()` ────────────
//
// stdlib/io.rk asks three natives about the last failure: its errno, which
// `IoError` variant that is, and its message. Native answers from errno; the
// interpreter answers from the last error one of its I/O natives recorded on
// this thread, so the Rask that builds an `IoError` runs on both.

thread_local! {
    static LAST_OS_ERROR: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Record why an I/O native failed. A cancelled wait is `ECANCELED`, which is
/// what native's waits leave in errno.
fn set_last_os_error(e: &std::io::Error) {
    let code = if is_cancelled(e) {
        libc::ECANCELED
    } else {
        e.raw_os_error().unwrap_or(libc::EIO)
    };
    LAST_OS_ERROR.with(|c| c.set(code));
}

/// The `IoError` variant index for an errno, in stdlib/io.rk declaration
/// order. Rust's own reading of the errno, so there's no table to keep beside
/// runtime.c's; `ECANCELED` is the one it has no kind for.
fn error_kind(code: i32) -> i32 {
    use std::io::ErrorKind as K;
    if code == libc::ECANCELED {
        return 8;
    }
    match std::io::Error::from_raw_os_error(code).kind() {
        K::NotFound => 0,
        K::PermissionDenied => 1,
        K::AlreadyExists => 2,
        K::BrokenPipe => 3,
        K::ConnectionReset => 4,
        K::TimedOut => 5,
        _ => 7,
    }
}

impl Interpreter {
    /// The `@native` symbols net.rk and io.rk declare that need a socket or
    /// the last I/O error. Receiver first, as in the C signature.
    pub(crate) fn call_net_native(
        &mut self,
        symbol: &str,
        args: &[Value],
    ) -> Option<Result<Value, RuntimeError>> {
        let int_arg = |i: usize| match args.get(i) {
            Some(Value::Int(n, _)) => Ok(*n),
            _ => Err(RuntimeError::TypeError(format!("{symbol}: expected an integer argument"))),
        };
        Some(match symbol {
            "rask_io_errno" => Ok(Value::Int(LAST_OS_ERROR.with(|c| c.get()) as i64, crate::value::IntKind::I32)),
            "rask_io_error_kind" => int_arg(0)
                .map(|code| Value::Int(error_kind(code as i32) as i64, crate::value::IntKind::I32)),
            "rask_io_error_message" => int_arg(0).map(|code| {
                Value::String(Arc::new(Mutex::new(
                    std::io::Error::from_raw_os_error(code as i32).to_string(),
                )))
            }),
            "rask_net_read_some" => {
                let Some(Value::TcpConnection(stream)) = args.first() else {
                    return Some(Err(RuntimeError::TypeError(
                        "rask_net_read_some: expected a TcpConnection".to_string(),
                    )));
                };
                let max = match int_arg(1) {
                    Ok(n) => n.max(0) as usize,
                    Err(e) => return Some(Err(e)),
                };
                let guard = stream.lock().unwrap();
                let Some(s) = guard.as_ref() else {
                    return Some(Err(RuntimeError::ResourceClosed {
                        resource_type: "TcpConnection".to_string(),
                        operation: "read from".to_string(),
                    }));
                };
                let mut buf = vec![0u8; max];
                match Cancellable::new(s).and_then(|mut c| c.read(&mut buf)) {
                    Ok(n) => {
                        buf.truncate(n);
                        let bytes: Vec<Value> =
                            buf.into_iter().map(|b| Value::Int(b as i64, crate::value::IntKind::U8)).collect();
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![Value::vec(bytes)],
                            variant_index: 0,
                            origin: None,
                        })
                    }
                    Err(e) => {
                        set_last_os_error(&e);
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                            variant_index: 1,
                            origin: None,
                        })
                    }
                }
            }
            _ => return None,
        })
    }
}

// ─── Waiting on a socket, and being cancelled while waiting ──
//
// A task parked on a socket ends its wait when it is cancelled
// (conc.async/CN3), the way native's `rask_thread_io_wait` does it: `poll` on
// the socket and on a pipe of the wait's own, and the cancel writes a byte to
// the pipe. It used to retry every 2ms and look at the token in between.

/// The wake pipe. The cancel's waker holds a reference, so the descriptors
/// outlive a canceller that fetched the waker just before the wait ended.
struct WakePipe {
    read: std::os::fd::RawFd,
    write: std::os::fd::RawFd,
}

impl WakePipe {
    fn new() -> std::io::Result<Self> {
        let mut fds = [0 as libc::c_int; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        for fd in fds {
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        Ok(WakePipe { read: fds[0], write: fds[1] })
    }
}

impl Drop for WakePipe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.read);
            libc::close(self.write);
        }
    }
}

/// Wait until `fd` is ready to read (or to write), or the task is cancelled.
/// True when cancelled. A cancel that is already there wins before the wait.
fn wait_ready(
    fd: std::os::fd::RawFd,
    want_write: bool,
    token: &Arc<crate::value::CancelToken>,
) -> std::io::Result<bool> {
    let pipe = Arc::new(WakePipe::new()?);
    let waker_pipe = pipe.clone();
    let _wake = token.wake_on_cancel(Arc::new(move || unsafe {
        let byte = 1u8;
        libc::write(waker_pipe.write, &byte as *const u8 as *const libc::c_void, 1);
    }));
    loop {
        if token.is_cancelled() {
            return Ok(true);
        }
        let mut fds = [
            libc::pollfd {
                fd,
                events: if want_write { libc::POLLOUT } else { libc::POLLIN },
                revents: 0,
            },
            libc::pollfd { fd: pipe.read, events: libc::POLLIN, revents: 0 },
        ];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if token.is_cancelled() {
            return Ok(true);
        }
        if fds[0].revents != 0 {
            return Ok(false);
        }
    }
}

/// A socket a cancel can interrupt (conc.async/CN3). In a task it's
/// non-blocking while this is alive, and a call that would block waits in
/// `wait_ready`; outside one it's the plain blocking socket.
struct Cancellable {
    s: std::net::TcpStream,
    token: Option<Arc<crate::value::CancelToken>>,
}

impl Cancellable {
    fn new(s: &std::net::TcpStream) -> std::io::Result<Self> {
        let s = s.try_clone()?;
        let token = crate::value::current_cancel();
        if token.is_some() {
            s.set_nonblocking(true)?;
        }
        Ok(Cancellable { s, token })
    }

    fn retry<T>(
        &mut self,
        want_write: bool,
        mut op: impl FnMut(&mut std::net::TcpStream) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        use std::os::fd::AsRawFd;
        loop {
            match op(&mut self.s) {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    let Some(token) = &self.token else { return Err(e) };
                    // Waiting on the socket isn't running, so the task's slot
                    // goes back for the wait (conc.runtime/P2.5).
                    let fd = self.s.as_raw_fd();
                    if crate::without_task_slot(|| wait_ready(fd, want_write, token))? {
                        return Err(std::io::Error::other(Cancelled));
                    }
                }
                r => return r,
            }
        }
    }
}

impl Drop for Cancellable {
    fn drop(&mut self) {
        if self.token.is_some() {
            let _ = self.s.set_nonblocking(false);
        }
    }
}

impl Read for Cancellable {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.retry(false, |s| s.read(buf))
    }
}

impl Write for Cancellable {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.retry(true, |s| s.write(buf))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.retry(true, |s| s.flush())
    }
}

/// `accept`, ended by a cancel the same way.
fn accept_cancellable(l: &std::net::TcpListener) -> std::io::Result<std::net::TcpStream> {
    let Some(token) = crate::value::current_cancel() else {
        return l.accept().map(|(s, _)| s);
    };
    use std::os::fd::AsRawFd;
    l.set_nonblocking(true)?;
    let got = loop {
        match l.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                match wait_ready(l.as_raw_fd(), false, &token) {
                    Ok(true) => break Err(std::io::Error::other(Cancelled)),
                    Ok(false) => {}
                    Err(e) => break Err(e),
                }
            }
            r => break r.map(|(s, _)| s),
        }
    };
    let _ = l.set_nonblocking(false);
    let s = got?;
    s.set_nonblocking(false)?;
    Ok(s)
}

/// `TcpStream::connect`, ended by a cancel the same way. std has no
/// non-blocking connect, so a task's connect starts one by hand for each
/// address the name resolves to and waits for it to finish in `wait_ready`.
fn connect_cancellable(addr: &str) -> std::io::Result<std::net::TcpStream> {
    let Some(token) = crate::value::current_cancel() else {
        return std::net::TcpStream::connect(addr);
    };
    use std::net::ToSocketAddrs;
    let mut last = std::io::Error::new(std::io::ErrorKind::InvalidInput, "no address to connect to");
    for sa in addr.to_socket_addrs()? {
        match connect_one(&sa, &token) {
            Ok(s) => return Ok(s),
            Err(e) if is_cancelled(&e) => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn connect_one(
    sa: &std::net::SocketAddr,
    token: &Arc<crate::value::CancelToken>,
) -> std::io::Result<std::net::TcpStream> {
    use std::os::fd::FromRawFd;
    let (family, storage, len) = sockaddr_of(sa);
    let fd = unsafe { libc::socket(family, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Owned from here, so every early return closes it.
    let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    stream.set_nonblocking(true)?;
    let rc = unsafe {
        libc::connect(fd, &storage as *const libc::sockaddr_storage as *const libc::sockaddr, len)
    };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
        if wait_ready(fd, true, token)? {
            return Err(std::io::Error::other(Cancelled));
        }
        if let Some(e) = stream.take_error()? {
            return Err(e);
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// A `SocketAddr` as the `sockaddr` bytes `connect` takes.
fn sockaddr_of(sa: &std::net::SocketAddr) -> (libc::c_int, libc::sockaddr_storage, libc::socklen_t) {
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    match sa {
        std::net::SocketAddr::V4(a) => {
            let sin = unsafe { &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in) };
            sin.sin_family = libc::AF_INET as libc::sa_family_t;
            sin.sin_port = a.port().to_be();
            sin.sin_addr = libc::in_addr { s_addr: u32::from_ne_bytes(a.ip().octets()) };
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                sin.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            }
            (libc::AF_INET, storage, std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t)
        }
        std::net::SocketAddr::V6(a) => {
            let sin6 = unsafe { &mut *(&mut storage as *mut _ as *mut libc::sockaddr_in6) };
            sin6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            sin6.sin6_port = a.port().to_be();
            sin6.sin6_addr = libc::in6_addr { s6_addr: a.ip().octets() };
            sin6.sin6_flowinfo = a.flowinfo();
            sin6.sin6_scope_id = a.scope_id();
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
            {
                sin6.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
            }
            (libc::AF_INET6, storage, std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t)
        }
    }
}

/// The same address rules `net.check_addr` applies in stdlib/net.rk, so both
/// backends reject the same strings with the same message.
///
/// The interpreter can't run the Rask body: it ends in
/// `IoError.last_os_error()`, which calls into the C runtime. So the rules live
/// in two places and this is the copy — keep it in step with the Rask one
/// (#863).
fn check_addr(addr: &str) -> Option<&'static str> {
    let Some(at) = addr.rfind(':') else {
        return Some("invalid socket address");
    };
    if at == 0 {
        return Some("invalid socket address");
    }
    if addr[at + 1..].parse::<u16>().is_err() {
        return Some("invalid port value");
    }
    None
}

/// An `io::Error` with no OS error code never became a socket address at all —
/// that's a resolution failure, which native reports as -2 rather than through
/// errno. Anything with an errno is a real syscall failure and keeps Rust's
/// wording, which matches `IoError.last_os_error()`.
fn net_error(addr: &str, e: &std::io::Error) -> String {
    if e.raw_os_error().is_none() {
        return format!("could not resolve {}", addr);
    }
    e.to_string()
}

impl Interpreter {
    /// Handle net module methods.
    pub(crate) fn call_net_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "tcp_listen" => {
                let addr = self.expect_string(&args, 0)?;
                if let Some(why) = check_addr(&addr) {
                    return Ok(make_result_err(why));
                }
                match std::net::TcpListener::bind(&addr) {
                    Ok(listener) => {
                        let arc = Arc::new(Mutex::new(Some(listener)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpListener(arc)))
                    }
                    Err(e) => Ok(make_result_err(&net_error(&addr, &e))),
                }
            }
            "tcp_connect" => {
                let addr = self.expect_string(&args, 0)?;
                if let Some(why) = check_addr(&addr) {
                    return Ok(make_result_err(why));
                }
                match connect_cancellable(&addr) {
                    Ok(stream) => {
                        let arc = Arc::new(Mutex::new(Some(stream)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpConnection(arc)))
                    }
                    Err(e) if is_cancelled(&e) => Ok(io_err(&e)),
                    Err(e) => Ok(make_result_err(&net_error(&addr, &e))),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "net".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle TcpListener instance methods.
    pub(crate) fn call_tcp_listener_method(
        &mut self,
        listener: &Arc<Mutex<Option<std::net::TcpListener>>>,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "accept" => {
                let guard = listener.lock().unwrap();
                let l = guard.as_ref().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpListener".to_string(), operation: "accept on".to_string() }
                })?;
                match accept_cancellable(l) {
                    Ok(stream) => {
                        let arc = Arc::new(Mutex::new(Some(stream)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpConnection(arc)))
                    }
                    Err(e) => Ok(io_err(&e)),
                }
            }
            "close" => {
                if listener.lock().unwrap().is_none() {
                    return Ok(make_result_ok(Value::Unit));
                }
                let ptr = Arc::as_ptr(listener) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker
                        .mark_consumed(id)
                        .map_err(|msg| RuntimeError::Panic(msg))?;
                }
                let _ = listener.lock().unwrap().take();
                Ok(make_result_ok(Value::Unit))
            }
            "local_addr" => {
                let guard = listener.lock().unwrap();
                let l = guard.as_ref().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpListener".to_string(), operation: "get address of".to_string() }
                })?;
                match l.local_addr() {
                    Ok(addr) => Ok(Value::String(Arc::new(Mutex::new(addr.to_string())))),
                    Err(e) => Ok(make_result_err(&e.to_string())),
                }
            }
            "clone" => Ok(Value::TcpListener(Arc::clone(listener))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "TcpListener".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle TcpConnection instance methods.
    pub(crate) fn call_tcp_stream_method(
        &mut self,
        stream: &Arc<Mutex<Option<std::net::TcpStream>>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "read_text" => {
                let mut guard = stream.lock().unwrap();
                let s = guard.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "read from".to_string() }
                })?;
                let mut bytes = Vec::new();
                match Cancellable::new(s).and_then(|mut c| c.read_to_end(&mut bytes)) {
                    Ok(_) => match String::from_utf8(bytes) {
                        Ok(buf) => Ok(make_result_ok(Value::String(Arc::new(Mutex::new(buf))))),
                        Err(_) => Ok(make_result_err("stream did not contain valid UTF-8")),
                    },
                    Err(e) => Ok(io_err(&e)),
                }
            }
            "read_bytes" => {
                let mut guard = stream.lock().unwrap();
                let s = guard.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "read from".to_string() }
                })?;
                let mut buf = Vec::new();
                match Cancellable::new(s).and_then(|mut c| c.read_to_end(&mut buf)) {
                    Ok(_) => {
                        let bytes: Vec<Value> = buf.into_iter().map(|b| Value::int(b as i64)).collect();
                        Ok(make_result_ok(Value::vec(bytes)))
                    }
                    Err(e) => Ok(io_err(&e)),
                }
            }
            "write_text" => {
                let data = self.expect_string(&args, 0)?;
                let mut guard = stream.lock().unwrap();
                let s = guard.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "write to".to_string() }
                })?;
                match Cancellable::new(s).and_then(|mut c| c.write_all(data.as_bytes()).and_then(|_| c.flush())) {
                    Ok(()) => Ok(make_result_ok(Value::Unit)),
                    Err(e) => Ok(io_err(&e)),
                }
            }
            "write_bytes" => {
                let bytes: Vec<u8> = match args.first() {
                    Some(Value::Vec(v)) => v.lock().unwrap().iter().map(|val| match val {
                        Value::Int(n, _) => *n as u8,
                        _ => 0,
                    }).collect(),
                    _ => return Err(RuntimeError::TypeError(format!(
                        "TcpConnection.write_bytes: expected Vec<u8>, got {}",
                        args.first().map(|v| v.type_name()).unwrap_or("missing")
                    ))),
                };
                let mut guard = stream.lock().unwrap();
                let s = guard.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "write to".to_string() }
                })?;
                match Cancellable::new(s).and_then(|mut c| c.write_all(&bytes).and_then(|_| c.flush())) {
                    Ok(()) => Ok(make_result_ok(Value::Unit)),
                    Err(e) => Ok(io_err(&e)),
                }
            }
            "remote_addr" => {
                let guard = stream.lock().unwrap();
                let s = guard.as_ref().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "get address of".to_string() }
                })?;
                match s.peer_addr() {
                    Ok(addr) => Ok(Value::String(Arc::new(Mutex::new(addr.to_string())))),
                    Err(e) => Ok(make_result_err(&e.to_string())),
                }
            }
            "close" => {
                if stream.lock().unwrap().is_none() {
                    return Ok(make_result_ok(Value::Unit));
                }
                let ptr = Arc::as_ptr(stream) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker
                        .mark_consumed(id)
                        .map_err(|msg| RuntimeError::Panic(msg))?;
                }
                let _ = stream.lock().unwrap().take();
                Ok(make_result_ok(Value::Unit))
            }
            "clone" => Ok(Value::TcpConnection(Arc::clone(stream))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "TcpConnection".to_string(),
                method: method.to_string(),
            }),
        }
    }

}

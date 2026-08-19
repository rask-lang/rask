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
            // ConnectionReset(4) TimedOut(5) UnexpectedEof(6) Other(7)
            variant_index: 7,
            origin: None,
        }],
        variant_index: 1, origin: None,
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
                match std::net::TcpStream::connect(&addr) {
                    Ok(stream) => {
                        let arc = Arc::new(Mutex::new(Some(stream)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpConnection(arc)))
                    }
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
                match l.accept() {
                    Ok((stream, _addr)) => {
                        let arc = Arc::new(Mutex::new(Some(stream)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpConnection(arc)))
                    }
                    Err(e) => Ok(make_result_err(&e.to_string())),
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
                let mut buf = String::new();
                match s.read_to_string(&mut buf) {
                    Ok(_) => Ok(make_result_ok(Value::String(Arc::new(Mutex::new(buf))))),
                    Err(e) => Ok(make_result_err(&e.to_string())),
                }
            }
            "read_bytes" => {
                let mut guard = stream.lock().unwrap();
                let s = guard.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "read from".to_string() }
                })?;
                let mut buf = Vec::new();
                match s.read_to_end(&mut buf) {
                    Ok(_) => {
                        let bytes: Vec<Value> = buf.into_iter().map(|b| Value::int(b as i64)).collect();
                        Ok(make_result_ok(Value::vec(bytes)))
                    }
                    Err(e) => Ok(make_result_err(&e.to_string())),
                }
            }
            "write_text" => {
                let data = self.expect_string(&args, 0)?;
                let mut guard = stream.lock().unwrap();
                let s = guard.as_mut().ok_or_else(|| {
                    RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "write to".to_string() }
                })?;
                match s.write_all(data.as_bytes()).and_then(|_| s.flush()) {
                    Ok(()) => Ok(make_result_ok(Value::Unit)),
                    Err(e) => Ok(make_result_err(&e.to_string())),
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
                match s.write_all(&bytes).and_then(|_| s.flush()) {
                    Ok(()) => Ok(make_result_ok(Value::Unit)),
                    Err(e) => Ok(make_result_err(&e.to_string())),
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
            "read_http_request" => {
                self.read_http_request(stream)
            }
            "write_http_response" => {
                let response = args.into_iter().next().ok_or(
                    RuntimeError::ArityMismatch { expected: 1, got: 0 },
                )?;
                self.write_http_response(stream, &response)
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

    /// Parse an HTTP/1.1 request from a TCP stream.
    pub(crate) fn read_http_request(
        &self,
        stream: &Arc<Mutex<Option<std::net::TcpStream>>>,
    ) -> Result<Value, RuntimeError> {
        let mut guard = stream.lock().unwrap();
        let tcp = guard.as_mut().ok_or_else(|| {
            RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "read HTTP request from".to_string() }
        })?;

        // Clone the stream for BufReader (TcpStream supports try_clone)
        let read_stream = tcp.try_clone().map_err(|e| {
            RuntimeError::Panic(format!("failed to clone stream: {}", e))
        })?;
        let mut reader = BufReader::new(read_stream);

        // Request line: METHOD /path HTTP/1.1
        let mut request_line = String::new();
        reader.read_line(&mut request_line).map_err(|e| {
            RuntimeError::Panic(format!("failed to read request line: {}", e))
        })?;
        let parts: Vec<&str> = request_line.trim().splitn(3, ' ').collect();
        let method = parts.first().unwrap_or(&"GET").to_string();
        let path = parts.get(1).unwrap_or(&"/").to_string();

        // Headers until empty line
        let mut headers = Vec::new();
        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).map_err(|e| {
                RuntimeError::Panic(format!("failed to read header: {}", e))
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((key, val)) = trimmed.split_once(':') {
                let key = key.trim().to_string();
                let val = val.trim().to_string();
                if key.eq_ignore_ascii_case("content-length") {
                    content_length = val.parse().unwrap_or(0);
                }
                headers.push((key, val));
            }
        }

        // Body (per Content-Length)
        let body = if content_length > 0 {
            let mut buf = vec![0u8; content_length];
            reader.read_exact(&mut buf).map_err(|e| {
                RuntimeError::Panic(format!("failed to read body: {}", e))
            })?;
            String::from_utf8_lossy(&buf).to_string()
        } else {
            String::new()
        };

        // Build headers as Map
        let header_map: MapData = headers
            .into_iter()
            .map(|(k, v)| {
                (
                    MapKey(Value::String(Arc::new(Mutex::new(k)))),
                    Value::String(Arc::new(Mutex::new(v))),
                )
            })
            .collect();

        // Map HTTP method string to Method enum variant
        let method_value = Value::Enum {
            name: "Method".to_string(),
            variant: match method.as_str() {
                "GET" => "Get",
                "HEAD" => "Head",
                "POST" => "Post",
                "PUT" => "Put",
                "DELETE" => "Delete",
                "PATCH" => "Patch",
                "OPTIONS" => "Options",
                _ => "Get",
            }.to_string(),
            fields: vec![],
            variant_index: match method.as_str() {
                "GET" => 0,
                "HEAD" => 1,
                "POST" => 2,
                "PUT" => 3,
                "DELETE" => 4,
                "PATCH" => 5,
                "OPTIONS" => 6,
                _ => 0,
            },
            origin: None,
        };

        let mut fields = IndexMap::new();
        fields.insert(
            "method".to_string(),
            method_value,
        );
        fields.insert(
            "url".to_string(),
            Value::String(Arc::new(Mutex::new(path))),
        );
        fields.insert(
            "headers".to_string(),
            Value::Map(Arc::new(Mutex::new(header_map))),
        );
        fields.insert(
            "body".to_string(),
            Value::String(Arc::new(Mutex::new(body))),
        );

        Ok(make_result_ok(Value::new_struct(
            "Request".to_string(),
            fields,
            None,
        )))
    }

    /// Write an HTTP/1.1 response to a TCP stream.
    pub(crate) fn write_http_response(
        &self,
        stream: &Arc<Mutex<Option<std::net::TcpStream>>>,
        response: &Value,
    ) -> Result<Value, RuntimeError> {
        let (status, headers, body) = match response {
            Value::Struct(ref s) => {
                let guard = s.lock().unwrap();
                let status = match guard.fields.get("status") {
                    Some(Value::Int(n, _)) => *n as i32,
                    _ => 200,
                };
                let body = match guard.fields.get("body") {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                let headers = match guard.fields.get("headers") {
                    Some(Value::Map(m)) => {
                        let map = m.lock().unwrap();
                        map.iter()
                            .filter_map(|(k, v)| {
                                let k_str = match &k.0 {
                                    Value::String(s) => s.lock().unwrap().clone(),
                                    _ => return None,
                                };
                                let v_str = match v {
                                    Value::String(s) => s.lock().unwrap().clone(),
                                    _ => return None,
                                };
                                Some((k_str, v_str))
                            })
                            .collect::<Vec<_>>()
                    }
                    _ => vec![],
                };
                (status, headers, body)
            }
            _ => {
                return Err(RuntimeError::TypeError(
                    "expected Response struct with `status`, `headers`, and `body` fields".to_string(),
                ));
            }
        };

        let status_text = match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            500 => "Internal Server Error",
            _ => "Unknown",
        };

        let mut guard = stream.lock().unwrap();
        let tcp = guard.as_mut().ok_or_else(|| {
            RuntimeError::ResourceClosed { resource_type: "TcpConnection".to_string(), operation: "write HTTP response to".to_string() }
        })?;

        let mut output = format!("HTTP/1.1 {} {}\r\n", status, status_text);
        output.push_str(&format!("Content-Length: {}\r\n", body.len()));
        for (key, val) in &headers {
            output.push_str(&format!("{}: {}\r\n", key, val));
        }
        output.push_str("\r\n");
        output.push_str(&body);

        match tcp.write_all(output.as_bytes()).and_then(|_| tcp.flush()) {
            Ok(()) => Ok(make_result_ok(Value::Unit)),
            Err(e) => Ok(make_result_err(&e.to_string())),
        }
    }
}

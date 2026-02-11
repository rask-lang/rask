// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Networking module methods (net.*) and TCP connection instance methods.
//!
//! Layer: RUNTIME — socket operations require OS access.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

/// Build a Result.Ok(value).
fn make_result_ok(value: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![value],
    }
}

/// Build a Result.Err(message).
fn make_result_err(msg: &str) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![Value::String(Arc::new(Mutex::new(msg.to_string())))],
    }
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
                match std::net::TcpListener::bind(&addr) {
                    Ok(listener) => {
                        let arc = Arc::new(Mutex::new(Some(listener)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpListener(arc)))
                    }
                    Err(e) => Ok(make_result_err(&e.to_string())),
                }
            }
            "tcp_connect" => {
                let addr = self.expect_string(&args, 0)?;
                match std::net::TcpStream::connect(&addr) {
                    Ok(stream) => {
                        let arc = Arc::new(Mutex::new(Some(stream)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker
                            .register_file(ptr, self.env.scope_depth());
                        Ok(make_result_ok(Value::TcpConnection(arc)))
                    }
                    Err(e) => Ok(make_result_err(&e.to_string())),
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
                    return Ok(Value::Unit);
                }
                let ptr = Arc::as_ptr(listener) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker
                        .mark_consumed(id)
                        .map_err(|msg| RuntimeError::Panic(msg))?;
                }
                let _ = listener.lock().unwrap().take();
                Ok(Value::Unit)
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
            "read_all" => {
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
            "write_all" => {
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
                    return Ok(Value::Unit);
                }
                let ptr = Arc::as_ptr(stream) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker
                        .mark_consumed(id)
                        .map_err(|msg| RuntimeError::Panic(msg))?;
                }
                let _ = stream.lock().unwrap().take();
                Ok(Value::Unit)
            }
            "clone" => Ok(Value::TcpConnection(Arc::clone(stream))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "TcpConnection".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Parse an HTTP/1.1 request from a TCP stream.
    fn read_http_request(
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
        let header_map: Vec<(Value, Value)> = headers
            .into_iter()
            .map(|(k, v)| {
                (
                    Value::String(Arc::new(Mutex::new(k))),
                    Value::String(Arc::new(Mutex::new(v))),
                )
            })
            .collect();

        let mut fields = HashMap::new();
        fields.insert(
            "method".to_string(),
            Value::String(Arc::new(Mutex::new(method))),
        );
        fields.insert(
            "path".to_string(),
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

        Ok(make_result_ok(Value::Struct {
            name: "HttpRequest".to_string(),
            fields,
            resource_id: None,
        }))
    }

    /// Write an HTTP/1.1 response to a TCP stream.
    fn write_http_response(
        &self,
        stream: &Arc<Mutex<Option<std::net::TcpStream>>>,
        response: &Value,
    ) -> Result<Value, RuntimeError> {
        let (status, headers, body) = match response {
            Value::Struct { fields, .. } => {
                let status = match fields.get("status") {
                    Some(Value::Int(n)) => *n as i32,
                    _ => 200,
                };
                let body = match fields.get("body") {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                let headers = match fields.get("headers") {
                    Some(Value::Map(m)) => {
                        let map = m.lock().unwrap();
                        map.iter()
                            .filter_map(|(k, v)| {
                                let k_str = match k {
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
                    "expected HttpResponse struct with `status`, `headers`, and `body` fields".to_string(),
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

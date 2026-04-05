// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! HTTP module methods (http.*) and Request/Response instance methods.
//!
//! Provides `http.listen_and_serve(addr, handler)` for the HTTP server litmus program.
//! Builds on the existing TCP and HTTP request/response infrastructure in net.rs.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

/// Build a Result.Ok(value).
fn make_result_ok(value: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![value],
        variant_index: 0, origin: None,
    }
}

fn make_string(s: &str) -> Value {
    Value::String(Arc::new(Mutex::new(s.to_string())))
}

fn make_response(status: i64, body: &str) -> Value {
    let mut fields = IndexMap::new();
    fields.insert("status".to_string(), Value::Int(status));
    fields.insert(
        "headers".to_string(),
        Value::Map(Arc::new(Mutex::new(vec![]))),
    );
    fields.insert("body".to_string(), make_string(body));
    Value::new_struct("Response".to_string(), fields, None)
}

impl Interpreter {
    pub(crate) fn call_http_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "listen_and_serve" => self.http_listen_and_serve(args),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "http".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// http.listen_and_serve(addr, handler)
    fn http_listen_and_serve(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let addr = self.expect_string(&args, 0)?;
        let handler = args.get(1).cloned().ok_or(RuntimeError::ArityMismatch {
            expected: 2,
            got: args.len(),
        })?;

        let listener = std::net::TcpListener::bind(&addr).map_err(|e| {
            RuntimeError::Generic(format!("http.listen_and_serve: bind failed: {}", e))
        })?;

        for stream_result in listener.incoming() {
            let stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("http: accept error: {}", e);
                    continue;
                }
            };

            let conn = Arc::new(Mutex::new(Some(stream)));

            // Read HTTP request
            let request = match self.read_http_request(&conn) {
                Ok(val) => {
                    // Unwrap Result.Ok
                    match &val {
                        Value::Enum { variant, fields, .. } if variant == "Ok" => {
                            fields.first().cloned().unwrap_or(Value::Unit)
                        }
                        Value::Enum { variant, fields, .. } if variant == "Err" => {
                            eprintln!("http: request parse error: {:?}", fields);
                            continue;
                        }
                        other => other.clone(),
                    }
                }
                Err(e) => {
                    eprintln!("http: request error: {}", e);
                    continue;
                }
            };

            // Call the handler function with the request
            let response = match self.call_value(handler.clone(), vec![request]) {
                Ok(val) => val,
                Err(e) => {
                    eprintln!("http: handler error: {}", e);
                    let _ = self.write_500(&conn);
                    continue;
                }
            };

            // Write HTTP response back
            match self.write_http_response(&conn, &response) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("http: response write error: {}", e);
                }
            }
        }

        Ok(make_result_ok(Value::Unit))
    }

    fn write_500(
        &self,
        stream: &Arc<Mutex<Option<std::net::TcpStream>>>,
    ) -> Result<(), RuntimeError> {
        use std::io::Write;
        let mut guard = stream.lock().unwrap();
        if let Some(tcp) = guard.as_mut() {
            let body = "Internal Server Error";
            let response = format!(
                "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = tcp.write_all(response.as_bytes());
            let _ = tcp.flush();
        }
        Ok(())
    }

    /// Request instance methods (path, query_params, etc.)
    pub(crate) fn call_request_instance_method(
        &self,
        fields: &IndexMap<String, Value>,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "path" => {
                // Extract path from url (before '?')
                let url = match fields.get("url") {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => "/".to_string(),
                };
                let path = match url.find('?') {
                    Some(pos) => url[..pos].to_string(),
                    None => url,
                };
                Ok(make_string(&path))
            }
            "query_params" => {
                let url = match fields.get("url") {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                let mut params: Vec<(Value, Value)> = vec![];
                if let Some(pos) = url.find('?') {
                    let query = &url[pos + 1..];
                    for pair in query.split('&') {
                        if let Some(eq) = pair.find('=') {
                            params.push((
                                make_string(&pair[..eq]),
                                make_string(&pair[eq + 1..]),
                            ));
                        } else {
                            params.push((make_string(pair), make_string("")));
                        }
                    }
                }
                Ok(Value::Map(Arc::new(Mutex::new(params))))
            }
            "clone" => {
                let mut new_fields = IndexMap::new();
                for (k, v) in fields {
                    new_fields.insert(k.clone(), v.clone());
                }
                Ok(Value::new_struct("Request".to_string(), new_fields, None))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Request".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Response instance methods (with_status, with_header, is_ok, etc.)
    pub(crate) fn call_response_instance_method(
        &self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "with_status" => {
                let status = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "with_status requires an integer".to_string(),
                        ))
                    }
                };
                // Clone the response, change status
                if let Value::Struct(ref s) = receiver {
                    let guard = s.lock().unwrap();
                    let mut new_fields = guard.fields.clone();
                    new_fields.insert("status".to_string(), Value::Int(status));
                    Ok(Value::new_struct("Response".to_string(), new_fields, None))
                } else {
                    Err(RuntimeError::TypeError(
                        "with_status called on non-Response".to_string(),
                    ))
                }
            }
            "with_header" => {
                let name = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "with_header requires a string name".to_string(),
                        ))
                    }
                };
                let value = match args.get(1) {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "with_header requires a string value".to_string(),
                        ))
                    }
                };
                if let Value::Struct(ref s) = receiver {
                    let guard = s.lock().unwrap();
                    let mut new_fields = guard.fields.clone();
                    // Add header to the headers map
                    if let Some(Value::Map(m)) = new_fields.get("headers") {
                        let mut map = m.lock().unwrap();
                        map.push((make_string(&name), make_string(&value)));
                    }
                    Ok(Value::new_struct("Response".to_string(), new_fields, None))
                } else {
                    Err(RuntimeError::TypeError(
                        "with_header called on non-Response".to_string(),
                    ))
                }
            }
            "clone" => Ok(receiver.clone()),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Response".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Response static constructors (Response.ok, Response.json, etc.)
    pub(crate) fn call_response_type_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "new" => {
                let status = match args.first() {
                    Some(Value::Int(n)) => *n,
                    _ => 200,
                };
                let body = match args.get(1) {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                Ok(make_response(status, &body))
            }
            "ok" => {
                let body = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                Ok(make_response(200, &body))
            }
            "json" => {
                let body = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                let mut fields = IndexMap::new();
                fields.insert("status".to_string(), Value::Int(200));
                let headers = vec![(
                    make_string("Content-Type"),
                    make_string("application/json"),
                )];
                fields.insert(
                    "headers".to_string(),
                    Value::Map(Arc::new(Mutex::new(headers))),
                );
                fields.insert("body".to_string(), make_string(&body));
                Ok(Value::new_struct("Response".to_string(), fields, None))
            }
            "created" => {
                let body = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                Ok(make_response(201, &body))
            }
            "no_content" => Ok(make_response(204, "")),
            "not_found" => Ok(make_response(404, "")),
            "bad_request" => {
                let body = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                Ok(make_response(400, &body))
            }
            "internal_error" => {
                let body = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                Ok(make_response(500, &body))
            }
            "redirect" => {
                let url = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    _ => String::new(),
                };
                let mut fields = IndexMap::new();
                fields.insert("status".to_string(), Value::Int(302));
                let headers = vec![(make_string("Location"), make_string(&url))];
                fields.insert(
                    "headers".to_string(),
                    Value::Map(Arc::new(Mutex::new(headers))),
                );
                fields.insert("body".to_string(), make_string(""));
                Ok(Value::new_struct("Response".to_string(), fields, None))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Response".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Method enum constructor (Method.Get, Method.Post, etc.)
    pub(crate) fn call_method_enum_constructor(
        &self,
        variant: &str,
    ) -> Result<Value, RuntimeError> {
        let (variant_name, variant_index) = match variant {
            "Get" => ("Get", 0),
            "Head" => ("Head", 1),
            "Post" => ("Post", 2),
            "Put" => ("Put", 3),
            "Delete" => ("Delete", 4),
            "Patch" => ("Patch", 5),
            "Options" => ("Options", 6),
            _ => {
                return Err(RuntimeError::TypeError(format!(
                    "Method has no variant '{}'",
                    variant
                )))
            }
        };
        Ok(Value::Enum {
            name: "Method".to_string(),
            variant: variant_name.to_string(),
            fields: vec![],
            variant_index,
            origin: None,
        })
    }
}

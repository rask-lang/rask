// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Value calling, builtin dispatch, method routing, and type extractors.

use std::sync::{Arc, Mutex};

use crate::value::{BuiltinKind, FloatKind, Value};

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(crate) fn call_value(&mut self, func: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match func {
            Value::Function { name } => {
                if let Some(decl) = self.functions.get(&name).cloned() {
                    self.call_function(&decl, args).map_err(|diag| diag.error)
                } else {
                    Err(RuntimeError::UndefinedFunction(name))
                }
            }
            Value::Builtin(kind) => {
                // Handle async builtins separately as they need mutable access
                if kind == BuiltinKind::AsyncSpawn {
                    return self.spawn_async_task(args);
                }
                if kind == BuiltinKind::JoinAll {
                    return self.call_async_method("join_all", args);
                }
                if kind == BuiltinKind::SelectFirst {
                    return self.call_async_method("select_first", args);
                }
                if kind == BuiltinKind::Cancelled {
                    return self.call_async_method("cancelled", args);
                }
                self.call_builtin(kind, args)
            }
            Value::EnumConstructor {
                enum_name,
                variant_name,
                field_count,
                variant_index,
            } => {
                if args.len() != field_count {
                    return Err(RuntimeError::ArityMismatch {
                        expected: field_count,
                        got: args.len(),
                    });
                }
                Ok(Value::Enum {
                    name: enum_name,
                    variant: variant_name,
                    fields: args,
                    variant_index,
                    origin: None,
                })
            }
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                self.env.push_scope();
                for (name, val) in captured_env {
                    self.env.define(name, val);
                }
                for (param, arg) in params.iter().zip(args.into_iter()) {
                    // Closure params are by-value bindings (VS1) — copy so the
                    // body can't alias the caller's value.
                    self.env.define(param.clone(), arg.copy_on_bind());
                }
                let result = self.eval_expr(&body).map_err(|diag| diag.error);
                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
            }
            Value::NominalConstructor { type_name } => {
                if args.len() != 1 {
                    return Err(RuntimeError::ArityMismatch {
                        expected: 1,
                        got: args.len(),
                    });
                }
                Ok(Value::Nominal {
                    type_name,
                    inner: Box::new(args.into_iter().next().unwrap()),
                })
            }
            _ => Err(RuntimeError::TypeError(format!(
                "{} is not callable",
                func.type_name()
            ))),
        }
    }

    /// Call a built-in function.
    fn call_builtin(&self, kind: BuiltinKind, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match kind {
            // Assemble the whole line first, then write it once. Writing the
            // pieces separately let another thread's output land between them,
            // splicing two lines into one (#704).
            BuiltinKind::Println => {
                let mut line: String = args.iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                line.push('\n');
                self.write_output(&line);
                Ok(Value::Unit)
            }
            BuiltinKind::Print => {
                let line: String = args.iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                self.write_output(&line);
                Ok(Value::Unit)
            }
            // stderr isn't captured the way `write_output` captures stdout —
            // test harnesses diff stdout, and the point of eprint is to stay
            // out of that.
            BuiltinKind::EPrintln => {
                let line: Vec<String> = args.iter().map(|a| format!("{}", a)).collect();
                eprintln!("{}", line.join(" "));
                Ok(Value::Unit)
            }
            BuiltinKind::EPrint => {
                let line: Vec<String> = args.iter().map(|a| format!("{}", a)).collect();
                eprint!("{}", line.join(" "));
                Ok(Value::Unit)
            }
            BuiltinKind::Panic => {
                let msg = args
                    .first()
                    .map(|v| format!("{}", v))
                    .unwrap_or_else(|| "panic".to_string());
                Err(RuntimeError::Panic(msg))
            }
            BuiltinKind::AsyncSpawn | BuiltinKind::JoinAll
            | BuiltinKind::SelectFirst | BuiltinKind::Cancelled => {
                // These should have been handled in call_value
                unreachable!("Async builtins should be handled in call_value")
            }
            BuiltinKind::Todo => {
                let msg = if let Some(Value::String(s)) = args.first() {
                    let s = s.lock().unwrap();
                    format!("not yet implemented: {}", s)
                } else {
                    "not yet implemented".to_string()
                };
                Err(RuntimeError::Panic(msg))
            }
            BuiltinKind::Unreachable => {
                let msg = if let Some(Value::String(s)) = args.first() {
                    let s = s.lock().unwrap();
                    format!("entered unreachable code: {}", s)
                } else {
                    "entered unreachable code".to_string()
                };
                Err(RuntimeError::Panic(msg))
            }
            BuiltinKind::Format => {
                if args.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "format() requires at least one argument (template string)".into(),
                    ));
                }
                match &args[0] {
                    Value::String(s) => {
                        let template = s.lock().unwrap().clone();
                        let result = self.format_string(&template, &args[1..])?;
                        Ok(Value::String(Arc::new(Mutex::new(result))))
                    }
                    _ => Err(RuntimeError::TypeError(
                        "format() first argument must be a string".into(),
                    )),
                }
            }
            BuiltinKind::Min => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                match Self::value_cmp(&args[0], &args[1]) {
                    Some(std::cmp::Ordering::Greater) => Ok(args.into_iter().nth(1).unwrap()),
                    _ => Ok(args.into_iter().next().unwrap()),
                }
            }
            BuiltinKind::Max => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                match Self::value_cmp(&args[0], &args[1]) {
                    Some(std::cmp::Ordering::Less) => Ok(args.into_iter().nth(1).unwrap()),
                    _ => Ok(args.into_iter().next().unwrap()),
                }
            }
            BuiltinKind::Clamp => {
                if args.len() != 3 {
                    return Err(RuntimeError::ArityMismatch { expected: 3, got: args.len() });
                }
                let value = &args[0];
                let lo = &args[1];
                let hi = &args[2];
                if Self::value_cmp(value, lo) == Some(std::cmp::Ordering::Less) {
                    Ok(args.into_iter().nth(1).unwrap())
                } else if Self::value_cmp(value, hi) == Some(std::cmp::Ordering::Greater) {
                    Ok(args.into_iter().nth(2).unwrap())
                } else {
                    Ok(args.into_iter().next().unwrap())
                }
            }
            BuiltinKind::AssertEq => {
                if args.len() < 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                let got = &args[0];
                let expected = &args[1];
                if Self::value_eq(got, expected) {
                    Ok(Value::Unit)
                } else {
                    let got_str = format!("{}", got);
                    let expected_str = format!("{}", expected);
                    let msg = if args.len() > 2 {
                        format!("{}", args[2])
                    } else {
                        "assert_eq failed".to_string()
                    };
                    Err(RuntimeError::AssertionFailed(
                        format!("{}\n  got:      {}\n  expected: {}", msg, got_str, expected_str)
                    ))
                }
            }
            BuiltinKind::Skip => {
                let reason = args
                    .first()
                    .map(|v| format!("{}", v))
                    .unwrap_or_else(|| "no reason".to_string());
                Err(RuntimeError::TestSkipped(reason))
            }
            BuiltinKind::ExpectFail => {
                Err(RuntimeError::TestExpectFail)
            }
        }
    }

    /// A method call on a value.
    ///
    /// The Rust implementations below cover the primitive layer. When none of
    /// them recognises the method, the answer is a Rask implementation — from
    /// `stdlib/*.rk` or from user code — rather than an error. Those Rust arms
    /// used to be walls: `JsonValue.to_string`, written in Rask and working
    /// natively, was unreachable on the interpreter however the source read
    /// (#689). One fallback here covers every type, so migrating a module to
    /// Rask needs no interpreter change at all.
    pub(super) fn call_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match self.call_primitive_method(receiver.clone(), method, args.clone()) {
            Err(RuntimeError::NoSuchMethod { ty, method: m }) => {
                match Self::nominal_type_name(&receiver) {
                    // Report the primitive layer's error, not the lookup's — it
                    // names the receiver type the user wrote.
                    Some(name) => self
                        .call_rask_method(&name, method, receiver, args)
                        .map_err(|e| match e {
                            RuntimeError::NoSuchMethod { .. } => {
                                RuntimeError::NoSuchMethod { ty, method: m }
                            }
                            other => other,
                        }),
                    None => Err(RuntimeError::NoSuchMethod { ty, method: m }),
                }
            }
            other => other,
        }
    }

    /// The nominal type a value belongs to, for looking up its Rask methods.
    /// `Value::type_name` answers "struct"/"enum", which no method table is
    /// keyed by.
    pub(crate) fn nominal_type_name(v: &Value) -> Option<String> {
        Some(match v {
            Value::Struct(s) => s.lock().unwrap().name.clone(),
            Value::Enum { name, .. } => name.clone(),
            Value::Duration(_) => "Duration".to_string(),
            Value::Instant(_) => "Instant".to_string(),
            Value::File(_) => "File".to_string(),
            Value::TcpListener(_) => "TcpListener".to_string(),
            Value::TcpConnection(_) => "TcpConnection".to_string(),
            _ => return None,
        })
    }

    fn call_primitive_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match &receiver {
            Value::Module(module) => self.call_module_method(module, method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::File(f) => self.call_file_method(f, method, args),
            Value::Duration(nanos) => self.call_duration_method(*nanos, method, args),
            Value::Instant(instant) => self.call_instant_method(instant, method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Metadata" => {
                let guard = s.lock().unwrap();
                self.call_metadata_method(&guard.fields, method)
            }
            Value::Struct(ref s) if s.lock().unwrap().name == "Args" => {
                let guard = s.lock().unwrap();
                self.call_args_method(&guard.fields, method, args)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Request" => {
                let guard = s.lock().unwrap();
                self.call_request_instance_method(&guard.fields, method, args)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Response" => {
                drop(s.lock().unwrap());
                self.call_response_instance_method(receiver, method, args)
            }
            Value::Struct(ref s) if s.lock().unwrap().name == "BuildContext" => {
                if method == "step" {
                    return self.call_build_step(args);
                }
                let state = self.build_state.as_mut().ok_or_else(|| {
                    RuntimeError::Generic("BuildContext used outside build script".into())
                })?;
                crate::build_context::call_method(state, method, args)
                    .map_err(RuntimeError::Generic)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Command" => {
                let guard = s.lock().unwrap();
                self.call_command_instance_method(&guard.fields, method, args)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Output" => {
                let guard = s.lock().unwrap();
                self.call_output_instance_method(&guard.fields, method)
            }
            // The stdlib `io` module is compiled out on wasm, and it owns both
            // these handlers and the `io.stdout()`/`stdin()`/`stderr()` calls
            // that produce these structs — so on wasm these arms are dead as
            // well as unbuildable.
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Stdin" => {
                self.call_stdin_method(method, args)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Stdout" => {
                self.call_stdout_method(method, args)
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct(ref s) if s.lock().unwrap().name == "Stderr" => {
                self.call_stderr_method(method, args)
            }
            Value::Enum { name, variant, fields, .. } if name == "JsonValue" => {
                self.call_json_value_method(variant, fields, method)
            }
            Value::Enum { name, variant, fields, .. } if name == "JsonError" => {
                self.call_json_error_method(variant, fields, method)
            }
            Value::SimdF32x8(data) => match method {
                "sum" => {
                    let sum: f32 = data.iter().sum();
                    Ok(Value::Float(sum as f64, FloatKind::Untyped))
                }
                "add" | "sub" | "mul" | "div" => {
                    if args.is_empty() {
                        return Err(RuntimeError::TypeError(format!("f32x8.{} requires an argument", method)));
                    }
                    let other = match &args[0] {
                        Value::SimdF32x8(d) => d,
                        _ => return Err(RuntimeError::TypeError(format!(
                            "f32x8.{} expects f32x8, found {}", method, args[0].type_name()
                        ))),
                    };
                    let mut r = [0.0f32; 8];
                    for i in 0..8 {
                        r[i] = match method {
                            "add" => data[i] + other[i],
                            "sub" => data[i] - other[i],
                            "mul" => data[i] * other[i],
                            "div" => data[i] / other[i],
                            _ => unreachable!(),
                        };
                    }
                    Ok(Value::SimdF32x8(r))
                }
                _ => Err(RuntimeError::TypeError(format!(
                    "f32x8 has no method '{}'", method
                ))),
            },
            // @binary struct instance methods (build, build_into)
            Value::Struct(ref s) => {
                let name = s.lock().unwrap().name.clone();
                if self.binary_structs.contains_key(&name)
                    && matches!(method, "build" | "build_into")
                {
                    let span = rask_ast::Span::new(0, 0);
                    if let Some(result) = self.try_binary_instance_method(&receiver, &name, method, args, span) {
                        return result.map_err(|d| d.error);
                    }
                    unreachable!();
                }
                self.call_builtin_method(receiver, method, args)
            }
            // CE6: Cell<T> instance methods
            Value::Cell(ref c) => match method {
                "get" => {
                    let guard = c.lock().unwrap();
                    Ok(guard.clone())
                }
                "set" => {
                    if args.len() != 1 {
                        return Err(RuntimeError::TypeError("Cell.set expects 1 argument".into()));
                    }
                    let mut guard = c.lock().unwrap();
                    *guard = args.into_iter().next().unwrap();
                    Ok(Value::Unit)
                }
                "replace" => {
                    if args.len() != 1 {
                        return Err(RuntimeError::TypeError("Cell.replace expects 1 argument".into()));
                    }
                    let mut guard = c.lock().unwrap();
                    let old = std::mem::replace(&mut *guard, args.into_iter().next().unwrap());
                    Ok(old)
                }
                "into_inner" => {
                    // Consume the cell — return inner value
                    let guard = c.lock().unwrap();
                    Ok(guard.clone())
                }
                _ => Err(RuntimeError::NoSuchMethod {
                    ty: "Cell".to_string(),
                    method: method.to_string(),
                }),
            },
            _ => self.call_builtin_method(receiver, method, args),
        }
    }
    /// Helper to extract an integer from args.
    pub(crate) fn expect_int(&self, args: &[Value], idx: usize) -> Result<i64, RuntimeError> {
        match args.get(idx) {
            Some(Value::Int(n, _)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected int, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a float from args.
    pub(crate) fn expect_float(&self, args: &[Value], idx: usize) -> Result<f64, RuntimeError> {
        match args.get(idx) {
            Some(Value::Float(n, _)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected float, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a bool from args.
    pub(crate) fn expect_bool(&self, args: &[Value], idx: usize) -> Result<bool, RuntimeError> {
        match args.get(idx) {
            Some(Value::Bool(b)) => Ok(*b),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected bool, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a string from args.
    pub(crate) fn expect_string(&self, args: &[Value], idx: usize) -> Result<String, RuntimeError> {
        match args.get(idx) {
            Some(Value::String(s)) => Ok(s.lock().unwrap().clone()),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected string, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a char from args.
    pub(crate) fn expect_char(&self, args: &[Value], idx: usize) -> Result<char, RuntimeError> {
        match args.get(idx) {
            Some(Value::Char(c)) => Ok(*c),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected char, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }


    /// Call a method whose body is Rask, from `stdlib/*.rk` or user code.
    ///
    /// The Rust implementations in `stdlib/` cover the primitive layer. When one
    /// doesn't recognise a method, the answer is the Rask implementation rather
    /// than an error — that's what makes a module written in Rask reachable from
    /// the interpreter instead of shadowed by its Rust twin.
    pub(crate) fn call_rask_method(
        &mut self,
        type_name: &str,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let Some(func) = self
            .methods
            .get(type_name)
            .and_then(|ms| ms.get(method))
            .cloned()
            .filter(|f| !f.body.is_empty())
        else {
            return Err(RuntimeError::NoSuchMethod {
                ty: type_name.to_string(),
                method: method.to_string(),
            });
        };
        let mut all = vec![receiver];
        all.extend(args);
        self.call_function(&func, all).map_err(|d| d.error)
    }

    /// Call a Rask `extend`-block function that takes no `self` —
    /// `json.parse(text)`, and anything else where the Rust layer wants the
    /// Rask body rather than a second copy of it.
    pub(crate) fn call_rask_static(
        &mut self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let Some(func) = self
            .methods
            .get(type_name)
            .and_then(|ms| ms.get(method))
            .cloned()
            .filter(|f| !f.body.is_empty())
        else {
            return Err(RuntimeError::NoSuchMethod {
                ty: type_name.to_string(),
                method: method.to_string(),
            });
        };
        self.call_function(&func, args).map_err(|d| d.error)
    }
    /// Helper to extract an i128 from args.
    pub(crate) fn expect_int128(&self, args: &[Value], idx: usize) -> Result<i128, RuntimeError> {
        match args.get(idx) {
            Some(Value::Int128(n)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected i128, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Helper to extract a u128 from args.
    pub(crate) fn expect_uint128(&self, args: &[Value], idx: usize) -> Result<u128, RuntimeError> {
        match args.get(idx) {
            Some(Value::Uint128(n)) => Ok(*n),
            Some(v) => Err(RuntimeError::TypeError(format!(
                "expected u128, got {}",
                v.type_name()
            ))),
            None => Err(RuntimeError::ArityMismatch {
                expected: idx + 1,
                got: args.len(),
            }),
        }
    }

    /// Check if a value is truthy.
    pub(crate) fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Bool(b) => *b,
            Value::Unit => false,
            Value::Int(0, _) => false,
            _ => true,
        }
    }


    /// Handle ctx.step(name, inputs, body) — incremental build step.
    /// Hashes input files, skips if unchanged since last run.
    fn call_build_step(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::build_context;

        if args.len() != 3 {
            return Err(RuntimeError::Generic(format!(
                "step expects 3 args (name, inputs, body), got {}", args.len()
            )));
        }

        let name = match &args[0] {
            Value::String(s) => s.lock().unwrap().clone(),
            _ => return Err(RuntimeError::TypeError("step: name must be string".into())),
        };

        let inputs: Vec<String> = match &args[1] {
            Value::Vec(v) => {
                let items = v.lock().unwrap();
                items.iter().map(|item| match item {
                    Value::String(s) => Ok(s.lock().unwrap().clone()),
                    _ => Err(RuntimeError::TypeError("step: inputs must be Vec<string>".into())),
                }).collect::<Result<Vec<_>, _>>()?
            }
            _ => return Err(RuntimeError::TypeError("step: inputs must be Vec<string>".into())),
        };

        let closure = args[2].clone();

        let state = self.build_state.as_ref().ok_or_else(|| {
            RuntimeError::Generic("step() used outside build script".into())
        })?;

        // Check if step cache exists and inputs haven't changed
        if let Some(ref cache_dir) = state.step_cache_dir {
            let current_hash = build_context::hash_inputs(
                &state.package_dir,
                &inputs,
                &state.tool_versions,
            ).map_err(RuntimeError::Generic)?;

            if let Some(cached_hash) = build_context::load_step_hash(cache_dir, &name) {
                if cached_hash == current_hash {
                    // Inputs unchanged — skip this step
                    return Ok(Value::Unit);
                }
            }

            // Run the closure body
            let result = match closure {
                Value::Closure { params, body, captured_env } => {
                    if !params.is_empty() {
                        return Err(RuntimeError::TypeError(
                            "step body closure must take no parameters".into(),
                        ));
                    }
                    self.env.push_scope();
                    for (k, v) in &captured_env {
                        self.env.define(k.clone(), v.clone());
                    }
                    let result = self.eval_expr(&body);
                    self.env.pop_scope();
                    result
                }
                _ => return Err(RuntimeError::TypeError("step: body must be a closure".into())),
            };

            // Save hash on success
            match &result {
                Ok(_) => {
                    if let Some(ref cache_dir) = self.build_state.as_ref().and_then(|s| s.step_cache_dir.as_ref()) {
                        let _ = build_context::save_step_hash(cache_dir, &name, current_hash);
                    }
                }
                Err(_) => {} // Don't cache failed steps
            }

            result.map_err(|d| d.error)
        } else {
            // No cache dir configured — always run
            match closure {
                Value::Closure { params, body, captured_env } => {
                    if !params.is_empty() {
                        return Err(RuntimeError::TypeError(
                            "step body closure must take no parameters".into(),
                        ));
                    }
                    self.env.push_scope();
                    for (k, v) in &captured_env {
                        self.env.define(k.clone(), v.clone());
                    }
                    let result = self.eval_expr(&body);
                    self.env.pop_scope();
                    result.map_err(|d| d.error)
                }
                _ => Err(RuntimeError::TypeError("step: body must be a closure".into())),
            }
        }
    }
}

#[cfg(test)]
impl Interpreter {
    /// Check if a method dispatches (doesn't return NoSuchMethod).
    /// A panic from inside a match arm counts as "implemented" — the method
    /// was dispatched, it just has a bug with arg handling.
    pub(crate) fn has_method_dispatch(&mut self, value: Value, method: &str) -> bool {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.call_method(value, method, vec![])
        }));
        !matches!(result, Ok(Err(RuntimeError::NoSuchMethod { .. })))
    }

    /// Check if a module method is recognized by the interpreter dispatch.
    /// Uses name-only matching — never executes the method body — so it
    /// won't block on I/O (stdin, network, etc.).
    pub(crate) fn has_module_dispatch(
        &self,
        module: &crate::value::ModuleKind,
        method: &str,
    ) -> bool {
        use crate::value::ModuleKind::*;
        match module {
            Fs => matches!(method,
                "read_text" | "read_bytes" | "read_lines" | "write_text" | "write_bytes"
                | "append_text" | "exists" | "open" | "create" | "absolute_path" | "metadata"
                | "remove_file" | "remove_dir" | "create_dir" | "create_dir_all"
                | "rename" | "copy" | "list_dir" | "current_dir" | "home_dir"
            ),
            Io => matches!(method, "read_line"),
            Net => matches!(method, "tcp_listen" | "tcp_connect"),
            Time => matches!(method, "sleep"),
            Random => matches!(method, "f32" | "f64" | "i64" | "bool" | "range"),
            Math => matches!(method,
                "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "atan2"
                | "exp" | "ln" | "log2" | "log10"
                | "hypot" | "to_radians" | "to_degrees"
            ),
            Os | Std => matches!(method,
                "env" | "env_or" | "set_env" | "remove_env" | "env_vars"
                | "args" | "exit" | "pid" | "platform" | "arch"
            ),
            // No "parse": the grammar is `json.parse` in stdlib/json.rk now, and
            // the Rust parser here is only still reachable through the typed
            // `decode`, where the shape comes from a struct declaration.
            Json => matches!(method,
                "encode" | "encode_pretty" | "to_value" | "decode"
            ),
            Path => false, // Path module has no module-level methods
            Async => matches!(method, "spawn"),
            Thread => matches!(method, "Thread" | "ThreadPool"),
            Http => matches!(method, "serve"),
            Env => matches!(method, "var" | "vars"),
            Cli => matches!(method, "args" | "parse"),
            Reflect => false,
        }
    }
}


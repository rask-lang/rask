// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

mod register;
mod monomorphize;
mod call;
mod exec_stmt;
mod assign;
mod eval_expr;
mod pattern;
mod collections;
mod format;
mod operators;
mod dispatch;

use rask_ast::decl::{BenchmarkDecl, Decl, EnumDecl, FnDecl, StructDecl, TestDecl};
use rask_ast::Span;

use crate::env::Environment;
use crate::resource::ResourceTracker;
use crate::value::Value;

/// Declarations collected during registration.
struct RegisteredProgram {
    entry_fn: Option<FnDecl>,
    tests: Vec<TestDecl>,
    benchmarks: Vec<BenchmarkDecl>,
    test_fns: Vec<FnDecl>,
}

/// Result of running a single test.
#[derive(Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub duration: std::time::Duration,
    pub errors: Vec<String>,
}

/// Result of running a single benchmark.
#[derive(Debug)]
pub struct BenchmarkResult {
    pub name: String,
    pub iterations: u64,
    pub total: std::time::Duration,
    pub min: std::time::Duration,
    pub max: std::time::Duration,
    pub mean: std::time::Duration,
    pub median: std::time::Duration,
}

/// The tree-walk interpreter.
pub struct Interpreter {
    /// Variable bindings (scoped).
    pub(crate) env: Environment,
    /// Function declarations by name.
    functions: HashMap<String, FnDecl>,
    /// Enum declarations by name.
    enums: HashMap<String, EnumDecl>,
    /// Struct declarations by name (for @resource checking).
    pub(crate) struct_decls: HashMap<String, StructDecl>,
    /// Monomorphized struct declarations (e.g., "Buffer<i32, 256>" -> concrete struct).
    monomorphized_structs: HashMap<String, StructDecl>,
    /// Methods from extend blocks (type_name -> method_name -> FnDecl).
    pub(crate) methods: HashMap<String, HashMap<String, FnDecl>>,
    /// Linear resource tracker.
    pub(crate) resource_tracker: ResourceTracker,
    /// Optional output buffer for capturing stdout (used in tests).
    output_buffer: Option<Arc<Mutex<String>>>,
    /// Command-line arguments passed to the program.
    pub(crate) cli_args: Vec<String>,
    /// Build script state (set when running via `run_build`).
    pub(crate) build_state: Option<crate::build_context::BuildState>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            monomorphized_structs: HashMap::new(),
            methods: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: None,
            cli_args: vec![],
            build_state: None,
        }
    }

    pub fn with_args(args: Vec<String>) -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            monomorphized_structs: HashMap::new(),
            methods: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: None,
            cli_args: args,
            build_state: None,
        }
    }

    /// Returns interpreter and output buffer reference.
    pub fn with_captured_output() -> (Self, Arc<Mutex<String>>) {
        let buffer = Arc::new(Mutex::new(String::new()));
        let interp = Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            monomorphized_structs: HashMap::new(),
            methods: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: Some(buffer.clone()),
            cli_args: vec![],
            build_state: None,
        };
        (interp, buffer)
    }

    /// Inject `cfg` build configuration into the interpreter environment (CT11-CT16).
    pub fn inject_cfg(&mut self, cfg: &rask_comptime::CfgConfig) {
        let mut fields = HashMap::new();
        fields.insert("os".to_string(), Value::String(Arc::new(Mutex::new(cfg.os.clone()))));
        fields.insert("arch".to_string(), Value::String(Arc::new(Mutex::new(cfg.arch.clone()))));
        fields.insert("env".to_string(), Value::String(Arc::new(Mutex::new(cfg.env.clone()))));
        fields.insert("profile".to_string(), Value::String(Arc::new(Mutex::new(cfg.profile.clone()))));
        fields.insert("debug".to_string(), Value::Bool(cfg.profile == "debug"));
        fields.insert("features".to_string(), Value::Vec(Arc::new(Mutex::new(
            cfg.features.iter().map(|f| Value::String(Arc::new(Mutex::new(f.clone())))).collect(),
        ))));
        self.env.define("cfg".to_string(), Value::Struct {
            name: "Cfg".to_string(),
            fields,
            resource_id: None,
        });
    }

    /// Clones function/enum/method tables and captured environment for spawned thread.
    pub(crate) fn spawn_child(&self, captured_vars: HashMap<String, Value>) -> Self {
        let mut child = Interpreter::new();
        child.functions = self.functions.clone();
        child.enums = self.enums.clone();
        child.struct_decls = self.struct_decls.clone();
        child.methods = self.methods.clone();
        for (name, value) in captured_vars {
            child.env.define(name, value);
        }
        child
    }

    /// Spawn an OS thread from a closure (Thread.spawn).
    pub(crate) fn spawn_os_thread(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::value::ThreadHandleInner;

        if args.is_empty() {
            return Err(RuntimeError::TypeError(
                "Thread.spawn requires a closure argument".to_string(),
            ));
        }

        let closure = &args[0];
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "Thread.spawn closure must take no parameters".to_string(),
                    ));
                }

                let body = body.clone();
                let captured = captured_env.clone();
                let child = self.spawn_child(captured);

                let join_handle = std::thread::spawn(move || {
                    let mut interp = child;
                    match interp.eval_expr(&body).map_err(|diag| diag.error) {
                        Ok(val) => Ok(val),
                        Err(RuntimeError::Return(val)) => Ok(val),
                        Err(e) => Err(format!("{}", e)),
                    }
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                    receiver: Mutex::new(None),
                })))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "Thread.spawn expects a closure, got {}",
                closure.type_name()
            ))),
        }
    }

    /// Spawn an async task from a closure (spawn() in using Multitasking).
    /// In interpreter: uses OS thread but returns TaskHandle for type distinction.
    pub(crate) fn spawn_async_task(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::value::ThreadHandleInner;

        if args.is_empty() {
            return Err(RuntimeError::TypeError(
                "spawn() requires a closure argument".to_string(),
            ));
        }

        // Check for Multitasking context
        if self.env.get("__multitasking_ctx").is_none() {
            return Err(RuntimeError::TypeError(
                "spawn() requires 'using Multitasking' context".to_string(),
            ));
        }

        let closure = &args[0];
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "spawn() closure must take no parameters".to_string(),
                    ));
                }

                let body = body.clone();
                let captured = captured_env.clone();
                let child = self.spawn_child(captured);

                let join_handle = std::thread::spawn(move || {
                    let mut interp = child;
                    match interp.eval_expr(&body).map_err(|diag| diag.error) {
                        Ok(val) => Ok(val),
                        Err(RuntimeError::Return(val)) => Ok(val),
                        Err(e) => Err(format!("{}", e)),
                    }
                });

                // Return TaskHandle (not ThreadHandle) for type distinction
                let handle_inner = Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                    receiver: Mutex::new(None),
                });

                // Register handle for affine tracking (conc.async/H1)
                let ptr = Arc::as_ptr(&handle_inner) as usize;
                self.resource_tracker.register_handle(ptr, "TaskHandle", self.env.scope_depth());

                Ok(Value::TaskHandle(handle_inner))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "spawn() expects a closure, got {}",
                closure.type_name()
            ))),
        }
    }

    /// Spawn a thread pool task from a closure (ThreadPool.spawn).
    pub(crate) fn spawn_pool_task(&mut self, args: Vec<Value>) -> Result<Value, RuntimeError> {
        use crate::value::{PoolTask, ThreadHandleInner};

        if args.is_empty() {
            return Err(RuntimeError::TypeError(
                "ThreadPool.spawn requires a closure argument".to_string(),
            ));
        }

        let closure = &args[0];
        match closure {
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                if !params.is_empty() {
                    return Err(RuntimeError::TypeError(
                        "ThreadPool.spawn closure must take no parameters".to_string(),
                    ));
                }

                // Check for thread pool context
                let pool = self.env.get("__thread_pool").cloned();
                let pool = match pool {
                    Some(Value::ThreadPool(p)) => p,
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "ThreadPool.spawn requires `using ThreadPool` context".to_string(),
                        ))
                    }
                };

                let body = body.clone();
                let captured = captured_env.clone();
                let child = self.spawn_child(captured);

                let (result_tx, result_rx) = mpsc::sync_channel::<Result<Value, String>>(1);

                let task = PoolTask {
                    work: Box::new(move || {
                        let mut interp = child;
                        match interp.eval_expr(&body).map_err(|diag| diag.error) {
                            Ok(val) => {
                                let _ = result_tx.send(Ok(val));
                            }
                            Err(RuntimeError::Return(val)) => {
                                let _ = result_tx.send(Ok(val));
                            }
                            Err(e) => {
                                let _ = result_tx.send(Err(format!("{}", e)));
                            }
                        }
                    }),
                };

                let sender = pool.sender.lock().unwrap();
                if let Some(ref tx) = *sender {
                    tx.send(task).map_err(|_| {
                        RuntimeError::ResourceClosed {
                            resource_type: "ThreadPool".to_string(),
                            operation: "spawn on".to_string(),
                        }
                    })?;
                } else {
                    return Err(RuntimeError::TypeError(
                        "thread pool is shut down".to_string(),
                    ));
                }

                let join_handle = std::thread::spawn(move || {
                    result_rx
                        .recv()
                        .unwrap_or(Err("thread pool task dropped".to_string()))
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                    receiver: Mutex::new(None),
                })))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "ThreadPool.spawn expects a closure, got {}",
                closure.type_name()
            ))),
        }
    }

    fn write_output(&self, s: &str) {
        if let Some(buf) = &self.output_buffer {
            buf.lock().unwrap().push_str(s);
        } else {
            print!("{}", s);
        }
    }

    fn write_output_ln(&self) {
        if let Some(buf) = &self.output_buffer {
            buf.lock().unwrap().push('\n');
        } else {
            println!();
        }
    }

    fn is_resource_type(&self, name: &str) -> bool {
        if name == "File" {
            return true;
        }
        self.struct_decls
            .get(name)
            .map(|s| s.attrs.iter().any(|a| a == "resource"))
            .unwrap_or(false)
    }

    pub(crate) fn get_resource_id(&self, value: &Value) -> Option<u64> {
        match value {
            Value::Struct { resource_id, .. } => *resource_id,
            Value::File(rc) => {
                let ptr = Arc::as_ptr(rc) as usize;
                self.resource_tracker.lookup_file_id(ptr)
            }
            Value::TaskHandle(h) | Value::ThreadHandle(h) => {
                let ptr = Arc::as_ptr(h) as usize;
                self.resource_tracker.lookup_handle_id(ptr)
            }
            _ => None,
        }
    }

    /// Handles nested values like Result.Ok(file) or Result.Err(FileError{file}).
    fn transfer_resource_to_scope(&mut self, value: &Value, new_depth: usize) {
        match value {
            Value::File(rc) => {
                let ptr = Arc::as_ptr(rc) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker.transfer_to_scope(id, new_depth);
                }
            }
            Value::TaskHandle(h) | Value::ThreadHandle(h) => {
                let ptr = Arc::as_ptr(h) as usize;
                if let Some(id) = self.resource_tracker.lookup_handle_id(ptr) {
                    self.resource_tracker.transfer_to_scope(id, new_depth);
                }
            }
            Value::Struct { resource_id: Some(id), .. } => {
                self.resource_tracker.transfer_to_scope(*id, new_depth);
            }
            Value::Enum { fields, .. } => {
                for field in fields {
                    self.transfer_resource_to_scope(field, new_depth);
                }
            }
            _ => {}
        }
    }

    /// Register external package names so `pkg.func()` works at runtime.
    pub fn register_packages(&mut self, names: &[String]) {
        for name in names {
            self.env.define(name.clone(), Value::Package(name.clone()));
        }
    }

    pub fn run(&mut self, decls: &[Decl]) -> Result<Value, RuntimeDiagnostic> {
        let registered = self.register_declarations(decls)
            .map_err(|e| RuntimeDiagnostic::new(e, Span::new(0, 0)))?;

        if let Some(entry) = registered.entry_fn {
            self.call_function(&entry, vec![])
        } else {
            Err(RuntimeDiagnostic::new(RuntimeError::NoEntryPoint, Span::new(0, 0)))
        }
    }

    /// Run a build script: register declarations, find `func build(ctx)`,
    /// call it with the BuildContext value. Sets `build_state` so method
    /// dispatch can accumulate link flags and other state.
    pub fn run_build(
        &mut self,
        decls: &[Decl],
        state: crate::build_context::BuildState,
    ) -> Result<Value, RuntimeDiagnostic> {
        let ctx_value = state.to_value();
        self.build_state = Some(state);

        let registered = self.register_declarations(decls)
            .map_err(|e| RuntimeDiagnostic::new(e, Span::new(0, 0)))?;

        // Find func build — it's the entry point for build scripts
        let build_fn = registered.entry_fn
            .or_else(|| self.functions.get("build").cloned())
            .ok_or_else(|| RuntimeDiagnostic::new(
                RuntimeError::Generic("build.rk has no func build()".into()),
                Span::new(0, 0),
            ))?;

        // If func build takes a parameter, pass ctx; otherwise call with no args
        if build_fn.params.is_empty() {
            self.call_function(&build_fn, vec![])
        } else {
            self.call_function(&build_fn, vec![ctx_value])
        }
    }

    /// Take the build state after a build script finishes.
    pub fn take_build_state(&mut self) -> Option<crate::build_context::BuildState> {
        self.build_state.take()
    }

    /// Run all tests in the program (test blocks + @test functions).
    /// Does NOT require an entry point.
    pub fn run_tests(&mut self, decls: &[Decl], filter: Option<&str>) -> Vec<TestResult> {
        let registered = match self.register_declarations(decls) {
            Ok(r) => r,
            Err(e) => {
                return vec![TestResult {
                    name: "<registration>".to_string(),
                    passed: false,
                    duration: std::time::Duration::ZERO,
                    errors: vec![format!("{}", e)],
                }];
            }
        };

        let mut results = Vec::new();

        for test_decl in &registered.tests {
            if let Some(pat) = filter {
                if !test_decl.name.contains(pat) {
                    continue;
                }
            }
            results.push(self.run_single_test(&test_decl.name, &test_decl.body));
        }

        for test_fn in &registered.test_fns {
            if let Some(pat) = filter {
                if !test_fn.name.contains(pat) {
                    continue;
                }
            }
            results.push(self.run_test_function(&test_fn));
        }

        results
    }

    /// Run all benchmarks in the program.
    pub fn run_benchmarks(&mut self, decls: &[Decl], filter: Option<&str>) -> Vec<BenchmarkResult> {
        let registered = match self.register_declarations(decls) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Registration error: {}", e);
                return vec![];
            }
        };

        let mut results = Vec::new();

        for bench in &registered.benchmarks {
            if let Some(pat) = filter {
                if !bench.name.contains(pat) {
                    continue;
                }
            }
            results.push(self.run_single_benchmark(&bench.name, &bench.body));
        }

        results
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// A runtime error.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("undefined variable `{0}`")]
    UndefinedVariable(String),

    #[error("undefined function `{0}`")]
    UndefinedFunction(String),

    #[error("{0}")]
    TypeError(String),

    #[error("division by zero; check divisor before dividing")]
    DivisionByZero,

    #[error("expected {expected} argument{}, got {got}", if *.expected == 1 { "" } else { "s" })]
    ArityMismatch { expected: usize, got: usize },

    #[error("no method `{method}` on type `{ty}`")]
    NoSuchMethod { ty: String, method: String },

    #[error("no field `{field}` on type `{ty}`")]
    NoSuchField { ty: String, field: String },

    #[error("index {index} out of bounds (length is {len})")]
    IndexOutOfBounds { index: i64, len: usize },

    #[error("resource is closed; cannot {operation} a closed {resource_type}")]
    ResourceClosed { resource_type: String, operation: String },

    #[error("panic: {0}")]
    Panic(String),

    #[error("no matching arm in match; add a wildcard `_` arm to handle all cases")]
    NoMatchingArm,

    #[error("multiple @entry functions found; only one `func main()` or `@entry` per program")]
    MultipleEntryPoints,

    #[error("no entry point found; add `func main()` or use `@entry`")]
    NoEntryPoint,

    #[error("{0}")]
    Generic(String),

    #[error("exit with code {0}")]
    Exit(i32),

    // Control flow (not actual errors)
    #[error("return")]
    Return(Value),

    #[error("break")]
    Break,

    #[error("continue")]
    Continue,

    /// Error propagation via try operator
    #[error("try error")]
    TryError(Value),

    /// Unwrap on None panics
    #[error("unwrap failed: value was None")]
    UnwrapError,

    /// Assertion failed (assert expr) — stops test immediately
    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    /// Check failed (check expr) — test continues, marked failed
    #[error("check failed: {0}")]
    CheckFailed(String),
}

/// Runtime error with source location for diagnostic display.
#[derive(Debug)]
pub struct RuntimeDiagnostic {
    pub error: RuntimeError,
    pub span: Span,
}

impl RuntimeDiagnostic {
    pub fn new(error: RuntimeError, span: Span) -> Self {
        Self { error, span }
    }
}

impl std::fmt::Display for RuntimeDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for RuntimeDiagnostic {}

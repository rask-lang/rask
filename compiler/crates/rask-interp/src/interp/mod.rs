// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
        };
        (interp, buffer)
    }

    /// Clones function/enum/method tables and captured environment for spawned thread.
    fn spawn_child(&self, captured_vars: HashMap<String, Value>) -> Self {
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

    pub fn run(&mut self, decls: &[Decl]) -> Result<Value, RuntimeError> {
        let registered = self.register_declarations(decls)?;

        if let Some(entry) = registered.entry_fn {
            self.call_function(&entry, vec![])
        } else {
            Err(RuntimeError::NoEntryPoint)
        }
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

    /// Assertion failed (assert expr) — stops test immediately
    #[error("assertion failed: {0}")]
    AssertionFailed(String),

    /// Check failed (check expr) — test continues, marked failed
    #[error("check failed: {0}")]
    CheckFailed(String),
}

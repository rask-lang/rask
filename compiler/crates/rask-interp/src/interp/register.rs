// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Declaration registration, test runners, and benchmark runners.

use rask_ast::decl::{BenchmarkDecl, ConstDecl, DeclKind, Decl, EnumDecl, FnDecl, TestDecl, Variant, Field};
use rask_ast::stmt::Stmt;
use rask_ast::stmt::StmtKind;
use rask_ast::Span;

use crate::value::{BuiltinKind, ModuleKind, Value};

use super::{Interpreter, RegisteredProgram, RuntimeError, TestResult, BenchmarkResult};

/// Strip generic type parameters from a type name.
/// "Box<T>" → "Box", "SpscRingBuffer<T, N>" → "SpscRingBuffer", "Point" → "Point"
fn strip_generics(name: &str) -> &str {
    match name.find('<') {
        Some(pos) => &name[..pos],
        None => name,
    }
}

impl Interpreter {
    /// Register a selective import (e.g., `import thread.Thread`).
    fn register_selective_import(&mut self, module: ModuleKind, member: &str, alias: &str) {
        match (module, member) {
            // Thread module members
            (ModuleKind::Thread, "Thread") => {
                self.env.define(alias.to_string(), Value::Type("Thread".to_string()));
            }
            (ModuleKind::Thread, "ThreadPool") => {
                self.env.define(alias.to_string(), Value::Type("ThreadPool".to_string()));
            }
            // Async module members
            (ModuleKind::Async, "spawn") => {
                // Define spawn as a builtin function that forwards to async.spawn
                // For now, we'll use a special builtin
                self.env.define(alias.to_string(), Value::Builtin(BuiltinKind::AsyncSpawn));
            }
            // Future: Add more module members as needed
            _ => {
                // Unknown member - ignore for now (could warn)
            }
        }
    }

    pub(super) fn register_declarations(&mut self, decls: &[Decl]) -> Result<RegisteredProgram, RuntimeError> {
        let mut entry_fn: Option<FnDecl> = None;
        let mut imports: Vec<(String, ModuleKind)> = Vec::new();
        let mut tests: Vec<TestDecl> = Vec::new();
        let mut benchmarks: Vec<BenchmarkDecl> = Vec::new();
        let mut test_fns: Vec<FnDecl> = Vec::new();
        let mut top_level_consts: Vec<ConstDecl> = Vec::new();

        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    if f.attrs.iter().any(|a| a == "entry") {
                        if entry_fn.is_some() {
                            return Err(RuntimeError::MultipleEntryPoints);
                        }
                        entry_fn = Some(f.clone());
                    }
                    if f.attrs.iter().any(|a| a == "test") {
                        test_fns.push(f.clone());
                    }
                    let fn_name = strip_generics(&f.name).to_string();
                    self.functions.insert(fn_name, f.clone());
                }
                DeclKind::Enum(e) => {
                    self.enums.insert(e.name.clone(), e.clone());
                }
                DeclKind::Impl(impl_decl) => {
                    let base_name = strip_generics(&impl_decl.target_ty).to_string();
                    let type_methods = self.methods.entry(base_name).or_default();
                    for method in &impl_decl.methods {
                        type_methods.insert(method.name.clone(), method.clone());
                    }
                }
                DeclKind::Import(import) => {
                    if let Some(module_name) = import.path.first() {
                        let module_kind = match module_name.as_str() {
                            "fs" => Some(ModuleKind::Fs),
                            "io" => Some(ModuleKind::Io),
                            "cli" => Some(ModuleKind::Cli),
                            "std" => Some(ModuleKind::Std),
                            "env" => Some(ModuleKind::Env),
                            "time" => Some(ModuleKind::Time),
                            "random" => Some(ModuleKind::Random),
                            "math" => Some(ModuleKind::Math),
                            "os" => Some(ModuleKind::Os),
                            "json" => Some(ModuleKind::Json),
                            "path" => Some(ModuleKind::Path),
                            "net" => Some(ModuleKind::Net),
                            "async" => Some(ModuleKind::Async),
                            "thread" => Some(ModuleKind::Thread),
                            _ => None,
                        };

                        if let Some(kind) = module_kind {
                            // Handle two cases:
                            // 1. `import module` -> bind module itself
                            // 2. `import module.Member` -> bind specific member
                            if import.path.len() == 1 {
                                // Whole module import
                                let alias = import.alias.clone().unwrap_or_else(|| module_name.clone());
                                imports.push((alias, kind));
                            } else if import.path.len() == 2 {
                                // Selective import: import module.Member
                                let member_name = &import.path[1];
                                let alias = import.alias.clone().unwrap_or_else(|| member_name.clone());

                                // Get the member from the module and bind it
                                // For now, we'll push a placeholder and handle it after registration
                                self.register_selective_import(kind, member_name, &alias);
                            }
                        }
                    }
                }
                DeclKind::Struct(s) => {
                    let base_name = strip_generics(&s.name).to_string();
                    self.struct_decls.insert(base_name.clone(), s.clone());
                    if !s.methods.is_empty() {
                        let type_methods = self.methods.entry(base_name).or_default();
                        for method in &s.methods {
                            type_methods.insert(method.name.clone(), method.clone());
                        }
                    }
                }
                DeclKind::Test(t) => {
                    tests.push(t.clone());
                }
                DeclKind::Benchmark(b) => {
                    benchmarks.push(b.clone());
                }
                DeclKind::Const(c) => {
                    top_level_consts.push(c.clone());
                }
                _ => {}
            }
        }

        self.env
            .define("print".to_string(), Value::Builtin(BuiltinKind::Print));
        self.env
            .define("println".to_string(), Value::Builtin(BuiltinKind::Println));
        self.env
            .define("panic".to_string(), Value::Builtin(BuiltinKind::Panic));
        self.env
            .define("format".to_string(), Value::Builtin(BuiltinKind::Format));

        self.env.define(
            "Some".to_string(),
            Value::EnumConstructor {
                enum_name: "Option".to_string(),
                variant_name: "Some".to_string(),
                field_count: 1,
            },
        );
        self.env.define(
            "None".to_string(),
            Value::Enum {
                name: "Option".to_string(),
                variant: "None".to_string(),
                fields: vec![],
            },
        );
        self.env.define(
            "Ok".to_string(),
            Value::EnumConstructor {
                enum_name: "Result".to_string(),
                variant_name: "Ok".to_string(),
                field_count: 1,
            },
        );
        self.env.define(
            "Err".to_string(),
            Value::EnumConstructor {
                enum_name: "Result".to_string(),
                variant_name: "Err".to_string(),
                field_count: 1,
            },
        );

        self.enums.insert(
            "Option".to_string(),
            EnumDecl {
                name: "Option".to_string(),
                type_params: vec![],
                variants: vec![
                    Variant {
                        name: "Some".to_string(),
                        fields: vec![Field {
                            name: "value".to_string(),
                            name_span: Span::new(0, 0),
                            ty: "T".to_string(),
                            is_pub: false,
                        }],
                    },
                    Variant {
                        name: "None".to_string(),
                        fields: vec![],
                    },
                ],
                methods: vec![],
                is_pub: true,
            },
        );
        self.enums.insert(
            "Result".to_string(),
            EnumDecl {
                name: "Result".to_string(),
                type_params: vec![],
                variants: vec![
                    Variant {
                        name: "Ok".to_string(),
                        fields: vec![Field {
                            name: "value".to_string(),
                            name_span: Span::new(0, 0),
                            ty: "T".to_string(),
                            is_pub: false,
                        }],
                    },
                    Variant {
                        name: "Err".to_string(),
                        fields: vec![Field {
                            name: "error".to_string(),
                            name_span: Span::new(0, 0),
                            ty: "E".to_string(),
                            is_pub: false,
                        }],
                    },
                ],
                methods: vec![],
                is_pub: true,
            },
        );
        self.enums.insert(
            "Ordering".to_string(),
            EnumDecl {
                name: "Ordering".to_string(),
                type_params: vec![],
                variants: vec![
                    Variant {
                        name: "Less".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "Equal".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "Greater".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "Relaxed".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "Acquire".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "Release".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "AcqRel".to_string(),
                        fields: vec![],
                    },
                    Variant {
                        name: "SeqCst".to_string(),
                        fields: vec![],
                    },
                ],
                methods: vec![],
                is_pub: true,
            },
        );

        for (name, kind) in imports {
            self.env.define(name, Value::Module(kind));
        }

        // Evaluate top-level const declarations after builtins are registered
        for c in &top_level_consts {
            let value = self.eval_expr(&c.init)
                .map_err(|diag| RuntimeError::Generic(format!("Error evaluating const {}: {}", c.name, diag.error)))?;
            self.env.define(c.name.clone(), value);
        }

        let entry = entry_fn.or_else(|| self.functions.get("main").cloned());

        Ok(RegisteredProgram {
            entry_fn: entry,
            tests,
            benchmarks,
            test_fns,
        })
    }

    /// Run a single test block with isolation and check-continuation.
    pub(super) fn run_single_test(&mut self, name: &str, body: &[Stmt]) -> TestResult {
        let start = std::time::Instant::now();
        let mut errors: Vec<String> = Vec::new();
        let mut ensures: Vec<&Stmt> = Vec::new();

        self.env.push_scope();

        for stmt in body {
            if matches!(&stmt.kind, StmtKind::Ensure { .. }) {
                ensures.push(stmt);
            } else {
                match self.exec_stmt(stmt) {
                    Ok(_) => {}
                    Err(diag) if matches!(&diag.error, RuntimeError::CheckFailed(_)) => {
                        if let RuntimeError::CheckFailed(msg) = diag.error {
                            errors.push(msg);
                        }
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::AssertionFailed(_)) => {
                        if let RuntimeError::AssertionFailed(msg) = diag.error {
                            errors.push(msg);
                        }
                        break;
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::Return(_)) => {
                        break;
                    }
                    Err(e) => {
                        errors.push(format!("{}", e));
                        break;
                    }
                }
            }
        }

        self.run_ensures(&ensures);
        self.env.pop_scope();

        TestResult {
            name: name.to_string(),
            passed: errors.is_empty(),
            duration: start.elapsed(),
            errors,
        }
    }

    /// Run an @test function.
    pub(super) fn run_test_function(&mut self, func: &FnDecl) -> TestResult {
        let start = std::time::Instant::now();
        let mut errors: Vec<String> = Vec::new();

        match self.call_function(func, vec![]) {
            Ok(_) => {}
            Err(diag) if matches!(&diag.error, RuntimeError::Return(_)) => {}
            Err(diag) if matches!(&diag.error, RuntimeError::CheckFailed(_) | RuntimeError::AssertionFailed(_)) => {
                let msg = match diag.error {
                    RuntimeError::CheckFailed(m) | RuntimeError::AssertionFailed(m) => m,
                    _ => unreachable!(),
                };
                errors.push(msg);
            }
            Err(e) => {
                errors.push(format!("{}", e));
            }
        }

        TestResult {
            name: func.name.clone(),
            passed: errors.is_empty(),
            duration: start.elapsed(),
            errors,
        }
    }

    /// Run a single benchmark with warmup and auto-calibrated iterations.
    pub(super) fn run_single_benchmark(&mut self, name: &str, body: &[Stmt]) -> BenchmarkResult {
        // Warmup: 3 iterations
        for _ in 0..3 {
            self.env.push_scope();
            let _ = self.exec_stmts(body);
            self.env.pop_scope();
        }

        // Calibrate: find iteration count that takes >100ms total
        let mut iterations: u64 = 10;
        loop {
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                self.env.push_scope();
                let _ = self.exec_stmts(body);
                self.env.pop_scope();
            }
            let elapsed = start.elapsed();
            if elapsed.as_millis() >= 100 || iterations >= 10_000 {
                break;
            }
            iterations *= 2;
        }

        // Measure
        let mut timings: Vec<std::time::Duration> = Vec::with_capacity(iterations as usize);
        for _ in 0..iterations {
            self.env.push_scope();
            let start = std::time::Instant::now();
            let _ = self.exec_stmts(body);
            let elapsed = start.elapsed();
            self.env.pop_scope();
            timings.push(elapsed);
        }

        timings.sort();
        let total: std::time::Duration = timings.iter().sum();
        let min = timings[0];
        let max = timings[timings.len() - 1];
        let mean = total / iterations as u32;
        let median = timings[timings.len() / 2];

        BenchmarkResult {
            name: name.to_string(),
            iterations,
            total,
            min,
            max,
            mean,
            median,
        }
    }
}


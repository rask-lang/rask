//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl, StructDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::env::Environment;
use crate::resource::ResourceTracker;
use crate::value::{BuiltinKind, ModuleKind, PoolTask, ThreadHandleInner, ThreadPoolInner, TypeConstructorKind, Value};

/// The tree-walk interpreter.
pub struct Interpreter {
    /// Variable bindings (scoped).
    pub(crate) env: Environment,
    /// Function declarations by name.
    functions: HashMap<String, FnDecl>,
    /// Enum declarations by name.
    enums: HashMap<String, EnumDecl>,
    /// Struct declarations by name (for @resource checking).
    struct_decls: HashMap<String, StructDecl>,
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
    /// Create a new interpreter.
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            methods: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: None,
            cli_args: vec![],
        }
    }

    /// Create a new interpreter with command-line arguments.
    pub fn with_args(args: Vec<String>) -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            methods: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: None,
            cli_args: args,
        }
    }

    /// Create an interpreter with output capture enabled.
    /// Returns the interpreter and a reference to the output buffer.
    pub fn with_captured_output() -> (Self, Arc<Mutex<String>>) {
        let buffer = Arc::new(Mutex::new(String::new()));
        let interp = Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            struct_decls: HashMap::new(),
            methods: HashMap::new(),
            resource_tracker: ResourceTracker::new(),
            output_buffer: Some(buffer.clone()),
            cli_args: vec![],
        };
        (interp, buffer)
    }

    /// Create a child interpreter for running in a spawned thread.
    /// Clones function/enum/method tables and captured environment variables.
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

    /// Write to output (buffer or stdout).
    fn write_output(&self, s: &str) {
        if let Some(buf) = &self.output_buffer {
            buf.lock().unwrap().push_str(s);
        } else {
            print!("{}", s);
        }
    }

    /// Write a newline to output (buffer or stdout).
    fn write_output_ln(&self) {
        if let Some(buf) = &self.output_buffer {
            buf.lock().unwrap().push('\n');
        } else {
            println!();
        }
    }

    /// Check if a type name is a resource type (@resource attribute or built-in File).
    fn is_resource_type(&self, name: &str) -> bool {
        if name == "File" {
            return true;
        }
        self.struct_decls
            .get(name)
            .map(|s| s.attrs.iter().any(|a| a == "resource"))
            .unwrap_or(false)
    }

    /// Get the resource ID from a Value, checking both struct resource_id and File Arc pointer.
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

    /// Transfer a resource (if present in the value) to a different scope depth.
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

    /// Run a program (list of declarations).
    ///
    /// This:
    /// 1. Registers all function declarations
    /// 2. Registers built-in functions (println, print, panic)
    /// 3. Registers imported modules
    /// 4. Finds and calls the @entry function
    pub fn run(&mut self, decls: &[Decl]) -> Result<Value, RuntimeError> {
        // Pass 1: Register all function, enum declarations, and collect imports
        let mut entry_fn: Option<FnDecl> = None;
        let mut imports: Vec<(String, ModuleKind)> = Vec::new();

        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    // Check for @entry attribute
                    if f.attrs.iter().any(|a| a == "entry") {
                        if entry_fn.is_some() {
                            return Err(RuntimeError::MultipleEntryPoints);
                        }
                        entry_fn = Some(f.clone());
                    }
                    self.functions.insert(f.name.clone(), f.clone());
                }
                DeclKind::Enum(e) => {
                    self.enums.insert(e.name.clone(), e.clone());
                }
                DeclKind::Impl(impl_decl) => {
                    // Register methods from extend block
                    let type_methods = self.methods.entry(impl_decl.target_ty.clone()).or_default();
                    for method in &impl_decl.methods {
                        type_methods.insert(method.name.clone(), method.clone());
                    }
                }
                DeclKind::Import(import) => {
                    // Handle module imports: import fs, import io as input, etc.
                    if let Some(module_name) = import.path.first() {
                        let alias = import.alias.clone().unwrap_or_else(|| module_name.clone());
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
                            _ => None, // Unknown module, ignore for now
                        };
                        if let Some(kind) = module_kind {
                            imports.push((alias, kind));
                        }
                    }
                }
                DeclKind::Struct(s) => {
                    self.struct_decls.insert(s.name.clone(), s.clone());
                    // Register methods from struct's inline methods
                    if !s.methods.is_empty() {
                        let type_methods = self.methods.entry(s.name.clone()).or_default();
                        for method in &s.methods {
                            type_methods.insert(method.name.clone(), method.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // Register built-in functions in the global scope
        // Global functions (no module prefix)
        self.env
            .define("print".to_string(), Value::Builtin(BuiltinKind::Print));
        self.env
            .define("println".to_string(), Value::Builtin(BuiltinKind::Println));
        self.env
            .define("panic".to_string(), Value::Builtin(BuiltinKind::Panic));
        self.env
            .define("format".to_string(), Value::Builtin(BuiltinKind::Format));

        // Register shorthand constructors for Option and Result
        // Some(x), None, Ok(x), Err(x)
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

        // Register built-in enums (Option, Result, Ordering)
        use rask_ast::decl::{EnumDecl, Field, Variant};
        self.enums.insert(
            "Option".to_string(),
            EnumDecl {
                name: "Option".to_string(),
                variants: vec![
                    Variant {
                        name: "Some".to_string(),
                        fields: vec![Field {
                            name: "value".to_string(),
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
                variants: vec![
                    Variant {
                        name: "Ok".to_string(),
                        fields: vec![Field {
                            name: "value".to_string(),
                            ty: "T".to_string(),
                            is_pub: false,
                        }],
                    },
                    Variant {
                        name: "Err".to_string(),
                        fields: vec![Field {
                            name: "error".to_string(),
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
                variants: vec![
                    // Comparison ordering (cmp)
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
                    // Memory ordering (atomics)
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

        // Register only imported modules
        for (name, kind) in imports {
            self.env.define(name, Value::Module(kind));
        }

        // Pass 2: Call the @entry function
        if let Some(entry) = entry_fn {
            self.call_function(&entry, vec![])
        } else {
            // No @entry function - error
            Err(RuntimeError::NoEntryPoint)
        }
    }

    /// Call a user-defined function with arguments.
    pub(crate) fn call_function(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
        // Check arity
        if args.len() != func.params.len() {
            return Err(RuntimeError::ArityMismatch {
                expected: func.params.len(),
                got: args.len(),
            });
        }

        // Create new scope for function body
        self.env.push_scope();

        // Bind parameters to arguments
        // Handle projection types: if param type is "Type.{field}", extract field from struct arg
        for (param, arg) in func.params.iter().zip(args.into_iter()) {
            if let Some(proj_start) = param.ty.find(".{") {
                // Projection parameter — extract named field from struct
                let proj_fields_str = &param.ty[proj_start + 2..param.ty.len() - 1];
                let proj_fields: Vec<&str> = proj_fields_str.split(',').map(|s| s.trim()).collect();
                if proj_fields.len() == 1 && param.name == proj_fields[0] {
                    // Single field projection: bind the field value directly
                    if let Value::Struct { fields, .. } = &arg {
                        if let Some(field_val) = fields.get(proj_fields[0]) {
                            self.env.define(param.name.clone(), field_val.clone());
                        } else {
                            return Err(RuntimeError::TypeError(format!(
                                "struct has no field '{}' for projection", proj_fields[0]
                            )));
                        }
                    } else {
                        return Err(RuntimeError::TypeError(format!(
                            "projection parameter expects struct, got {}", arg.type_name()
                        )));
                    }
                } else {
                    self.env.define(param.name.clone(), arg);
                }
            } else {
                self.env.define(param.name.clone(), arg);
            }
        }

        // Execute function body (ensures run inside here via exec_stmts)
        let result = self.exec_stmts(&func.body);

        // Before popping scope, transfer any resources in the return value to caller scope
        let scope_depth = self.env.scope_depth();
        let caller_depth = scope_depth.saturating_sub(1);
        match &result {
            Err(RuntimeError::Return(v)) | Ok(v) => {
                self.transfer_resource_to_scope(v, caller_depth);
            }
            Err(RuntimeError::TryError(v)) => {
                // Error propagation — the Err value may contain a resource
                self.transfer_resource_to_scope(v, caller_depth);
            }
            _ => {}
        }

        // Check for unconsumed resources at this scope depth
        if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
            self.env.pop_scope();
            return Err(RuntimeError::Panic(msg));
        }

        // Pop function scope
        self.env.pop_scope();

        // Handle return value:
        // - Ok(_) means function completed without explicit return → return Unit
        //   (Rask requires explicit `return` to produce values from functions)
        // - Err(Return(v)) means explicit return → return v
        // - Err(TryError(v)) means `try` propagated error → return Err value
        // - Err(other) means actual error → propagate
        let value = match result {
            Ok(_) => Value::Unit,
            Err(RuntimeError::Return(v)) => v,
            Err(RuntimeError::TryError(v)) => v, // Return the Err value
            Err(e) => return Err(e),
        };

        // Auto-Ok wrapping: if return type is Result<T, E> (from `T or E`)
        // and the value isn't already a Result, wrap it in Ok
        let returns_result = func.ret_ty.as_ref()
            .map(|t| t.starts_with("Result<"))
            .unwrap_or(false);
        if returns_result {
            match &value {
                Value::Enum { name, .. } if name == "Result" => Ok(value),
                _ => Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![value],
                }),
            }
        } else {
            Ok(value)
        }
    }

    /// Execute a list of statements, returning the value of the last expression.
    ///
    /// Collects `ensure` blocks encountered during execution and runs them in
    /// LIFO order when the block exits (normal completion or any error).
    /// This implements block-scoped deferred cleanup per the ensure spec.
    fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<Value, RuntimeError> {
        let mut last_value = Value::Unit;
        let mut ensures: Vec<&Stmt> = Vec::new();
        let mut exit_error: Option<RuntimeError> = None;

        for stmt in stmts {
            if matches!(&stmt.kind, StmtKind::Ensure { .. }) {
                ensures.push(stmt);
            } else {
                match self.exec_stmt(stmt) {
                    Ok(v) => last_value = v,
                    Err(e) => {
                        exit_error = Some(e);
                        break;
                    }
                }
            }
        }

        // Run ensures in LIFO order before returning
        let ensure_fatal = self.run_ensures(&ensures);

        // Original exit reason takes priority; ensure panics only matter on normal exit
        if let Some(e) = exit_error {
            Err(e)
        } else if let Some(fatal) = ensure_fatal {
            Err(fatal)
        } else {
            Ok(last_value)
        }
    }

    /// Run ensure blocks in LIFO order. Returns a fatal error (Panic/Exit) if one occurs.
    /// Non-fatal errors from ensure bodies are silently ignored or passed to catch handlers.
    fn run_ensures(&mut self, ensures: &[&Stmt]) -> Option<RuntimeError> {
        for ensure_stmt in ensures.iter().rev() {
            if let StmtKind::Ensure { body, catch } = &ensure_stmt.kind {
                // Execute the ensure body (simple sequential execution, no ensure collection)
                let result = self.exec_ensure_body(body);

                match result {
                    Ok(value) => {
                        // Check if value is a Result::Err (Rask-level error from cleanup)
                        if let Value::Enum { name, variant, fields } = &value {
                            if name == "Result" && variant == "Err" {
                                let err_val = fields.first().cloned().unwrap_or(Value::Unit);
                                self.handle_ensure_error(err_val, catch);
                            }
                        }
                    }
                    Err(RuntimeError::Panic(msg)) => return Some(RuntimeError::Panic(msg)),
                    Err(RuntimeError::Exit(code)) => return Some(RuntimeError::Exit(code)),
                    Err(RuntimeError::TryError(val)) => {
                        // try used inside ensure (spec forbids this, handle gracefully)
                        self.handle_ensure_error(val, catch);
                    }
                    Err(_) => {
                        // Other runtime errors in ensure body: silently ignore
                    }
                }
            }
        }
        None
    }

    /// Execute statements in an ensure body (no ensure collection — simple sequential execution).
    fn exec_ensure_body(&mut self, body: &[Stmt]) -> Result<Value, RuntimeError> {
        let mut last_value = Value::Unit;
        for stmt in body {
            last_value = self.exec_stmt(stmt)?;
        }
        Ok(last_value)
    }

    /// Handle an error from an ensure body: pass to catch handler or silently ignore.
    fn handle_ensure_error(&mut self, error_value: Value, catch: &Option<(String, Vec<Stmt>)>) {
        if let Some((name, handler)) = catch {
            self.env.push_scope();
            self.env.define(name.clone(), error_value);
            let _ = self.exec_ensure_body(handler);
            self.env.pop_scope();
        }
        // Without catch: error is silently ignored per spec
    }

    /// Execute a single statement.
    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, RuntimeError> {
        match &stmt.kind {
            // Expression statement - evaluate and return the value
            StmtKind::Expr(expr) => self.eval_expr(expr),

            // Const binding (immutable) - evaluate init and bind
            StmtKind::Const { name, init, .. } => {
                let value = self.eval_expr(init)?;
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            // Let binding (mutable) - same as const for now (mutability checked earlier)
            StmtKind::Let { name, init, .. } => {
                let value = self.eval_expr(init)?;
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            // Let tuple destructuring: let (a, b) = expr
            StmtKind::LetTuple { names, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple(names, value)?;
                Ok(Value::Unit)
            }

            // Const tuple destructuring: const (a, b) = expr
            StmtKind::ConstTuple { names, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple(names, value)?;
                Ok(Value::Unit)
            }

            // Assignment - evaluate and update existing binding
            StmtKind::Assign { target, value } => {
                let val = self.eval_expr(value)?;
                self.assign_target(target, val)?;
                Ok(Value::Unit)
            }

            // Return - wrap value in error for control flow
            StmtKind::Return(expr) => {
                let value = if let Some(e) = expr {
                    self.eval_expr(e)?
                } else {
                    Value::Unit
                };
                Err(RuntimeError::Return(value))
            }

            // While loop
            StmtKind::While { cond, body } => {
                loop {
                    let cond_val = self.eval_expr(cond)?;
                    if !self.is_truthy(&cond_val) {
                        break;
                    }
                    self.env.push_scope();
                    match self.exec_stmts(body) {
                        Ok(_) => {}
                        Err(RuntimeError::Break) => {
                            self.env.pop_scope();
                            break;
                        }
                        Err(RuntimeError::Continue) => {
                            self.env.pop_scope();
                            continue;
                        }
                        Err(e) => {
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                    self.env.pop_scope();
                }
                Ok(Value::Unit)
            }

            // While-let pattern matching loop (while expr is Pattern)
            StmtKind::WhileLet {
                pattern,
                expr,
                body,
            } => {
                loop {
                    let value = self.eval_expr(expr)?;

                    // Try to match the pattern
                    if let Some(bindings) = self.match_pattern(pattern, &value) {
                        self.env.push_scope();
                        // Bind pattern variables
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        match self.exec_stmts(body) {
                            Ok(_) => {}
                            Err(RuntimeError::Break) => {
                                self.env.pop_scope();
                                break;
                            }
                            Err(RuntimeError::Continue) => {
                                self.env.pop_scope();
                                continue;
                            }
                            Err(e) => {
                                self.env.pop_scope();
                                return Err(e);
                            }
                        }
                        self.env.pop_scope();
                    } else {
                        // Pattern didn't match, exit loop
                        break;
                    }
                }
                Ok(Value::Unit)
            }

            // Infinite loop
            StmtKind::Loop { body, .. } => loop {
                self.env.push_scope();
                match self.exec_stmts(body) {
                    Ok(_) => {}
                    Err(RuntimeError::Break) => {
                        self.env.pop_scope();
                        break Ok(Value::Unit);
                    }
                    Err(RuntimeError::Continue) => {
                        self.env.pop_scope();
                        continue;
                    }
                    Err(e) => {
                        self.env.pop_scope();
                        break Err(e);
                    }
                }
                self.env.pop_scope();
            },

            // Break
            StmtKind::Break(_) => Err(RuntimeError::Break),

            // Continue
            StmtKind::Continue(_) => Err(RuntimeError::Continue),

            // For-in loop (basic implementation for ranges)
            StmtKind::For {
                binding,
                iter,
                body,
                ..
            } => {
                // Evaluate the iterator expression
                let iter_val = self.eval_expr(iter)?;

                // Handle Range values (from a..b expressions)
                match iter_val {
                    Value::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let end_val = if inclusive { end + 1 } else { end };
                        for i in start..end_val {
                            self.env.push_scope();
                            self.env.define(binding.clone(), Value::Int(i));
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(RuntimeError::Break) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(RuntimeError::Continue) => {
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    Value::Vec(v) => {
                        // Clone vec items to avoid borrow issues during iteration
                        let items: Vec<Value> = v.lock().unwrap().clone();
                        for item in items {
                            self.env.push_scope();
                            self.env.define(binding.clone(), item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(RuntimeError::Break) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(RuntimeError::Continue) => {
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    _ => Err(RuntimeError::TypeError(format!(
                        "cannot iterate over {}",
                        iter_val.type_name()
                    ))),
                }
            }

            // Ensure block - collected by exec_stmts, not executed here directly
            StmtKind::Ensure { .. } => Ok(Value::Unit),

            // Other statements not yet implemented
            _ => Ok(Value::Unit),
        }
    }

    /// Assign a value to a target expression.
    /// Destructure a tuple/vec/struct into named bindings.
    fn destructure_tuple(&mut self, names: &[String], value: Value) -> Result<(), RuntimeError> {
        match value {
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                if vec.len() != names.len() {
                    return Err(RuntimeError::TypeError(format!(
                        "tuple destructuring: expected {} elements, got {}",
                        names.len(), vec.len()
                    )));
                }
                for (name, val) in names.iter().zip(vec.iter()) {
                    self.env.define(name.clone(), val.clone());
                }
            }
            Value::Struct { fields, .. } => {
                // Destructure struct by field names
                for name in names {
                    let val = fields.get(name).cloned().unwrap_or(Value::Unit);
                    self.env.define(name.clone(), val);
                }
            }
            _ => {
                return Err(RuntimeError::TypeError(format!(
                    "cannot destructure {} into tuple", value.type_name()
                )));
            }
        }
        Ok(())
    }

    /// Assign a value through a chain of field accesses.
    /// field_chain is [field1, field2, ..., fieldN] for target.field1.field2...fieldN = value.
    fn assign_nested_field(obj: &mut Value, field_chain: &[String], value: Value) -> Result<(), RuntimeError> {
        if field_chain.is_empty() {
            *obj = value;
            return Ok(());
        }
        let mut current = obj;
        for (i, field) in field_chain.iter().enumerate() {
            if i == field_chain.len() - 1 {
                // Last field — assign the value
                match current {
                    Value::Struct { fields, .. } => {
                        fields.insert(field.clone(), value);
                        return Ok(());
                    }
                    _ => return Err(RuntimeError::TypeError(format!(
                        "cannot assign field '{}' on {}", field, current.type_name()
                    ))),
                }
            } else {
                // Intermediate field — navigate deeper
                current = match current {
                    Value::Struct { fields, .. } => {
                        fields.get_mut(field).ok_or_else(|| {
                            RuntimeError::TypeError(format!("no field '{}' on struct", field))
                        })?
                    }
                    _ => return Err(RuntimeError::TypeError(format!(
                        "cannot access field '{}' on {}", field, current.type_name()
                    ))),
                };
            }
        }
        unreachable!()
    }

    fn assign_target(&mut self, target: &Expr, value: Value) -> Result<(), RuntimeError> {
        match &target.kind {
            ExprKind::Ident(name) => {
                if !self.env.assign(name, value) {
                    return Err(RuntimeError::UndefinedVariable(name.clone()));
                }
                Ok(())
            }
            // Field assignment: obj.field = value (supports arbitrary nesting)
            ExprKind::Field { .. } => {
                // Collect the chain of field accesses from outermost to base
                let mut field_chain = Vec::new();
                let mut current = target;
                while let ExprKind::Field { object, field: f } = &current.kind {
                    field_chain.push(f.clone());
                    current = object;
                }
                field_chain.reverse(); // Now: [outer_field, ..., inner_field]

                match &current.kind {
                    // Simple: var.field1.field2...fieldN = value
                    ExprKind::Ident(var_name) => {
                        if let Some(obj) = self.env.get_mut(var_name) {
                            Self::assign_nested_field(obj, &field_chain, value)
                        } else {
                            Err(RuntimeError::UndefinedVariable(var_name.clone()))
                        }
                    }
                    // Indexed: container[idx].field1.field2...fieldN = value
                    ExprKind::Index { object: idx_obj, index: idx_expr } => {
                        let idx_val = self.eval_expr(idx_expr)?;
                        if let ExprKind::Ident(var_name) = &idx_obj.kind {
                            if let Some(container) = self.env.get(var_name).cloned() {
                                match container {
                                    Value::Pool(p) => {
                                        if let Value::Handle { pool_id, index, generation } = idx_val {
                                            let mut pool = p.lock().unwrap();
                                            let slot_idx = pool.validate(pool_id, index, generation)
                                                .map_err(|e| RuntimeError::Panic(e))?;
                                            if let Some(ref mut elem) = pool.slots[slot_idx].1 {
                                                Self::assign_nested_field(elem, &field_chain, value)
                                            } else {
                                                Err(RuntimeError::TypeError("pool slot is empty".to_string()))
                                            }
                                        } else {
                                            Err(RuntimeError::TypeError("Pool index must be a Handle".to_string()))
                                        }
                                    }
                                    Value::Vec(v) => {
                                        if let Value::Int(i) = idx_val {
                                            let i = i as usize;
                                            let mut vec = v.lock().unwrap();
                                            if i < vec.len() {
                                                Self::assign_nested_field(&mut vec[i], &field_chain, value)
                                            } else {
                                                Err(RuntimeError::TypeError(format!("index {} out of bounds", i)))
                                            }
                                        } else {
                                            Err(RuntimeError::TypeError("Vec index must be integer".to_string()))
                                        }
                                    }
                                    _ => Err(RuntimeError::TypeError(format!(
                                        "cannot field-assign on indexed {}", container.type_name()
                                    ))),
                                }
                            } else {
                                Err(RuntimeError::UndefinedVariable(var_name.clone()))
                            }
                        } else {
                            // Nested indexing on non-ident (e.g., complex expressions)
                            Err(RuntimeError::TypeError("complex nested assignment not yet supported".to_string()))
                        }
                    }
                    _ => Err(RuntimeError::TypeError("unsupported assignment target".to_string())),
                }
            }
            // Index assignment: collection[idx] = value
            ExprKind::Index { object, index } => {
                let idx = self.eval_expr(index)?;
                if let ExprKind::Ident(var_name) = &object.kind {
                    if let Some(obj) = self.env.get(var_name).cloned() {
                        match obj {
                            Value::Vec(v) => {
                                if let Value::Int(i) = idx {
                                    let i = i as usize;
                                    let mut vec = v.lock().unwrap();
                                    if i < vec.len() {
                                        vec[i] = value;
                                        Ok(())
                                    } else {
                                        Err(RuntimeError::TypeError(format!(
                                            "index {} out of bounds (len {})",
                                            i,
                                            vec.len()
                                        )))
                                    }
                                } else {
                                    Err(RuntimeError::TypeError(
                                        "index must be integer".to_string(),
                                    ))
                                }
                            }
                            Value::Pool(p) => {
                                if let Value::Handle {
                                    pool_id,
                                    index,
                                    generation,
                                } = idx
                                {
                                    let mut pool = p.lock().unwrap();
                                    let slot_idx = pool
                                        .validate(pool_id, index, generation)
                                        .map_err(|e| RuntimeError::Panic(e))?;
                                    pool.slots[slot_idx].1 = Some(value);
                                    Ok(())
                                } else {
                                    Err(RuntimeError::TypeError(
                                        "Pool index must be a Handle".to_string(),
                                    ))
                                }
                            }
                            _ => Err(RuntimeError::TypeError(format!(
                                "cannot index-assign on {}",
                                obj.type_name()
                            ))),
                        }
                    } else {
                        Err(RuntimeError::UndefinedVariable(var_name.clone()))
                    }
                } else {
                    Err(RuntimeError::TypeError(
                        "complex index assignment not yet supported".to_string(),
                    ))
                }
            }
            _ => Err(RuntimeError::TypeError(
                "invalid assignment target".to_string(),
            )),
        }
    }

    /// Evaluate an expression and return its value.
    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            // Literals - just wrap in Value
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(n) => Ok(Value::Float(*n)),
            ExprKind::String(s) => {
                // String interpolation: replace {name} with variable values
                if s.contains('{') {
                    let interpolated = self.interpolate_string(s)?;
                    Ok(Value::String(Arc::new(Mutex::new(interpolated))))
                } else {
                    Ok(Value::String(Arc::new(Mutex::new(s.clone()))))
                }
            }
            ExprKind::Char(c) => Ok(Value::Char(*c)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),

            // Identifier lookup
            ExprKind::Ident(name) => {
                // First check local/global variables
                if let Some(val) = self.env.get(name) {
                    return Ok(val.clone());
                }
                // Then check if it's a function name
                if self.functions.contains_key(name) {
                    return Ok(Value::Function { name: name.clone() });
                }
                // Check type constructors (Vec, Map, string, etc.)
                match name.as_str() {
                    "Vec" => return Ok(Value::TypeConstructor(TypeConstructorKind::Vec)),
                    "Map" => return Ok(Value::TypeConstructor(TypeConstructorKind::Map)),
                    "string" => return Ok(Value::TypeConstructor(TypeConstructorKind::String)),
                    "Pool" => return Ok(Value::TypeConstructor(TypeConstructorKind::Pool)),
                    "Channel" => return Ok(Value::TypeConstructor(TypeConstructorKind::Channel)),
                    _ => {}
                }
                Err(RuntimeError::UndefinedVariable(name.clone()))
            }

            // Function call
            ExprKind::Call { func, args } => {
                // Special case: OptionalField used as function (e.g., foo()?.bar())
                // This happens when `?.` is lexed as a single token, creating
                // Call { func: OptionalField { object, field }, args }
                // We treat this as: try on object, then method call on unwrapped value
                if let ExprKind::OptionalField { object, field } = &func.kind {
                    let obj_val = self.eval_expr(object)?;
                    let arg_vals: Vec<Value> = args
                        .iter()
                        .map(|a| self.eval_expr(a))
                        .collect::<Result<_, _>>()?;

                    // Handle Result: unwrap Ok or propagate Err
                    if let Value::Enum {
                        name,
                        variant,
                        fields,
                    } = &obj_val
                    {
                        if name == "Result" {
                            match variant.as_str() {
                                "Ok" => {
                                    let inner = fields.first().cloned().unwrap_or(Value::Unit);
                                    return self.call_method(inner, field, arg_vals);
                                }
                                "Err" => {
                                    return Err(RuntimeError::TryError(obj_val));
                                }
                                _ => {}
                            }
                        } else if name == "Option" {
                            match variant.as_str() {
                                "Some" => {
                                    let inner = fields.first().cloned().unwrap_or(Value::Unit);
                                    let result = self.call_method(inner, field, arg_vals)?;
                                    // Wrap result in Some for optional chaining
                                    return Ok(Value::Enum {
                                        name: "Option".to_string(),
                                        variant: "Some".to_string(),
                                        fields: vec![result],
                                    });
                                }
                                "None" => {
                                    return Ok(Value::Enum {
                                        name: "Option".to_string(),
                                        variant: "None".to_string(),
                                        fields: vec![],
                                    });
                                }
                                _ => {}
                            }
                        }
                    }

                    // Fallback: try to call the field as a method
                    return self.call_method(obj_val, field, arg_vals);
                }

                let func_val = self.eval_expr(func)?;
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a))
                    .collect::<Result<_, _>>()?;
                self.call_value(func_val, arg_vals)
            }

            // Method call (handles desugared operators like a.add(b))
            ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } => {
                // Check if this is an enum variant constructor (e.g., Option.Some(42))
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(name).cloned() {
                        if let Some(variant) = enum_decl.variants.iter().find(|v| &v.name == method)
                        {
                            let field_count = variant.fields.len();
                            let arg_vals: Vec<Value> = args
                                .iter()
                                .map(|a| self.eval_expr(a))
                                .collect::<Result<_, _>>()?;
                            if arg_vals.len() != field_count {
                                return Err(RuntimeError::ArityMismatch {
                                    expected: field_count,
                                    got: arg_vals.len(),
                                });
                            }
                            return Ok(Value::Enum {
                                name: name.clone(),
                                variant: method.clone(),
                                fields: arg_vals,
                            });
                        }
                    }

                    // Check for static method on type (e.g., Parser.new(), Lexer.new())
                    // These are methods in extend blocks that don't take self
                    if let Some(type_methods) = self.methods.get(name).cloned() {
                        if let Some(method_fn) = type_methods.get(method) {
                            // Check if it's a static method (first param is not "self")
                            let is_static = method_fn
                                .params
                                .first()
                                .map(|p| p.name != "self")
                                .unwrap_or(true);
                            if is_static {
                                let arg_vals: Vec<Value> = args
                                    .iter()
                                    .map(|a| self.eval_expr(a))
                                    .collect::<Result<_, _>>()?;
                                return self.call_function(method_fn, arg_vals);
                            }
                        }
                    }
                }

                // Regular method call
                let receiver = self.eval_expr(object)?;
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a))
                    .collect::<Result<_, _>>()?;

                // Handle type static methods (e.g., Instant.now(), Duration.seconds(5))
                if let Value::Type(type_name) = &receiver {
                    return self.call_type_method(type_name, method, arg_vals);
                }

                self.call_method(receiver, method, arg_vals)
            }

            // Binary operators (only && and || remain after desugaring)
            ExprKind::Binary { op, left, right } => match op {
                BinOp::And => {
                    // Short-circuit: if left is false, don't evaluate right
                    let l = self.eval_expr(left)?;
                    if !self.is_truthy(&l) {
                        Ok(Value::Bool(false))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(self.is_truthy(&r)))
                    }
                }
                BinOp::Or => {
                    // Short-circuit: if left is true, don't evaluate right
                    let l = self.eval_expr(left)?;
                    if self.is_truthy(&l) {
                        Ok(Value::Bool(true))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(self.is_truthy(&r)))
                    }
                }
                _ => {
                    // Other operators should have been desugared
                    Err(RuntimeError::TypeError(format!(
                        "unexpected binary op {:?} - should be desugared to method call",
                        op
                    )))
                }
            },

            // Unary operators
            ExprKind::Unary { op, operand } => {
                let val = self.eval_expr(operand)?;
                match op {
                    UnaryOp::Not => match val {
                        Value::Bool(b) => Ok(Value::Bool(!b)),
                        _ => Err(RuntimeError::TypeError(format!(
                            "! requires bool, got {}",
                            val.type_name()
                        ))),
                    },
                    UnaryOp::Neg => match val {
                        Value::Int(n) => Ok(Value::Int(-n)),
                        Value::Float(n) => Ok(Value::Float(-n)),
                        _ => Err(RuntimeError::TypeError(format!(
                            "- requires number, got {}",
                            val.type_name()
                        ))),
                    },
                    _ => Err(RuntimeError::TypeError(format!(
                        "unhandled unary op {:?}",
                        op
                    ))),
                }
            }

            // Block expression - execute statements and return last value
            ExprKind::Block(stmts) => {
                self.env.push_scope();
                let result = self.exec_stmts(stmts);
                self.env.pop_scope();
                result
            }

            // If expression
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.eval_expr(cond)?;
                if self.is_truthy(&cond_val) {
                    self.eval_expr(then_branch)
                } else if let Some(else_br) = else_branch {
                    self.eval_expr(else_br)
                } else {
                    Ok(Value::Unit)
                }
            }

            // Range expression (a..b or a..=b)
            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_val = if let Some(s) = start {
                    match self.eval_expr(s)? {
                        Value::Int(n) => n,
                        v => {
                            return Err(RuntimeError::TypeError(format!(
                                "range start must be int, got {}",
                                v.type_name()
                            )))
                        }
                    }
                } else {
                    0 // Default start
                };
                let end_val = if let Some(e) = end {
                    match self.eval_expr(e)? {
                        Value::Int(n) => n,
                        v => {
                            return Err(RuntimeError::TypeError(format!(
                                "range end must be int, got {}",
                                v.type_name()
                            )))
                        }
                    }
                } else {
                    i64::MAX // No end (open range)
                };
                Ok(Value::Range {
                    start: start_val,
                    end: end_val,
                    inclusive: *inclusive,
                })
            }

            // Struct literal
            ExprKind::StructLit { name, fields, spread } => {
                let mut field_values = HashMap::new();

                // Handle spread first if present
                if let Some(spread_expr) = spread {
                    if let Value::Struct {
                        fields: base_fields,
                        ..
                    } = self.eval_expr(spread_expr)?
                    {
                        field_values.extend(base_fields);
                    }
                }

                // Evaluate and set explicit fields
                for field in fields {
                    let value = self.eval_expr(&field.value)?;
                    field_values.insert(field.name.clone(), value);
                }

                let resource_id = if self.is_resource_type(name) {
                    Some(self.resource_tracker.register(name, self.env.scope_depth()))
                } else {
                    None
                };

                Ok(Value::Struct {
                    name: name.clone(),
                    fields: field_values,
                    resource_id,
                })
            }

            // Field access
            ExprKind::Field { object, field } => {
                // Check if this is an enum variant access (e.g., Option.Some)
                if let ExprKind::Ident(enum_name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(enum_name).cloned() {
                        // Find the variant
                        if let Some(variant) =
                            enum_decl.variants.iter().find(|v| &v.name == field)
                        {
                            let field_count = variant.fields.len();
                            if field_count == 0 {
                                // Unit variant - return the enum value directly
                                return Ok(Value::Enum {
                                    name: enum_name.clone(),
                                    variant: field.clone(),
                                    fields: vec![],
                                });
                            } else {
                                // Constructor - return callable
                                return Ok(Value::EnumConstructor {
                                    enum_name: enum_name.clone(),
                                    variant_name: field.clone(),
                                    field_count,
                                });
                            }
                        }
                    }
                }

                // Fall through to struct field access or module field access
                let obj = self.eval_expr(object)?;
                match obj {
                    Value::Struct { fields, .. } => {
                        Ok(fields.get(field).cloned().unwrap_or(Value::Unit))
                    }
                    Value::Module(ModuleKind::Time) => {
                        // time.Instant, time.Duration → return type values
                        match field.as_str() {
                            "Instant" => Ok(Value::Type("Instant".to_string())),
                            "Duration" => Ok(Value::Type("Duration".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "time module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Math) => {
                        // math.PI, math.E, etc. → return constant values
                        self.get_math_field(field)
                    }
                    Value::Module(ModuleKind::Path) => {
                        // path.Path → return type value for Path.new() etc.
                        match field.as_str() {
                            "Path" => Ok(Value::Type("Path".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "path module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Random) => {
                        // random.Rng → return type value for Rng.new() etc.
                        match field.as_str() {
                            "Rng" => Ok(Value::Type("Rng".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "random module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Json) => {
                        // json.JsonValue → return type for constructors
                        match field.as_str() {
                            "JsonValue" => Ok(Value::Type("JsonValue".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "json module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Cli) => {
                        // cli.Parser → return type for builder
                        match field.as_str() {
                            "Parser" => Ok(Value::Type("Parser".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "cli module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    _ => Err(RuntimeError::TypeError(format!(
                        "cannot access field on {}",
                        obj.type_name()
                    ))),
                }
            }

            // Index access
            ExprKind::Index { object, index } => {
                let obj = self.eval_expr(object)?;
                let idx = self.eval_expr(index)?;

                match (&obj, &idx) {
                    (Value::Vec(v), Value::Int(i)) => {
                        let vec = v.lock().unwrap();
                        Ok(vec.get(*i as usize).cloned().unwrap_or(Value::Unit))
                    }
                    (Value::Vec(v), Value::Range { start, end, inclusive }) => {
                        let vec = v.lock().unwrap();
                        let len = vec.len() as i64;
                        let start_idx = (*start).max(0).min(len) as usize;
                        let end_idx = if *end == i64::MAX {
                            vec.len()
                        } else {
                            let e = if *inclusive { *end + 1 } else { *end };
                            e.max(0).min(len) as usize
                        };
                        let slice: Vec<Value> = vec[start_idx..end_idx].to_vec();
                        Ok(Value::Vec(Arc::new(Mutex::new(slice))))
                    }
                    (Value::String(s), Value::Int(i)) => Ok(s
                        .lock().unwrap()
                        .chars()
                        .nth(*i as usize)
                        .map(Value::Char)
                        .unwrap_or(Value::Unit)),
                    (
                        Value::Pool(p),
                        Value::Handle {
                            pool_id,
                            index,
                            generation,
                        },
                    ) => {
                        let pool = p.lock().unwrap();
                        let idx = pool
                            .validate(*pool_id, *index, *generation)
                            .map_err(|e| RuntimeError::Panic(e))?;
                        Ok(pool.slots[idx].1.as_ref().unwrap().clone())
                    }
                    _ => Err(RuntimeError::TypeError(format!(
                        "cannot index {} with {}",
                        obj.type_name(),
                        idx.type_name()
                    ))),
                }
            }

            // Array literal
            ExprKind::Array(elements) => {
                let values: Vec<Value> = elements
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            // Tuple literal — evaluated as Vec for pattern matching
            ExprKind::Tuple(elements) => {
                let values: Vec<Value> = elements
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            // Match expression
            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;

                for arm in arms {
                    if let Some(bindings) = self.match_pattern(&arm.pattern, &value) {
                        // Check guard if present
                        if let Some(guard) = &arm.guard {
                            self.env.push_scope();
                            for (name, val) in &bindings {
                                self.env.define(name.clone(), val.clone());
                            }
                            let guard_result = self.eval_expr(guard)?;
                            self.env.pop_scope();
                            if !self.is_truthy(&guard_result) {
                                continue;
                            }
                        }

                        // Execute arm body with bindings
                        self.env.push_scope();
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        let result = self.eval_expr(&arm.body);
                        self.env.pop_scope();
                        return result;
                    }
                }

                // No arm matched
                Err(RuntimeError::NoMatchingArm)
            }

            // If-let pattern matching
            ExprKind::IfLet {
                expr,
                pattern,
                then_branch,
                else_branch,
            } => {
                let value = self.eval_expr(expr)?;

                if let Some(bindings) = self.match_pattern(pattern, &value) {
                    self.env.push_scope();
                    for (name, val) in bindings {
                        self.env.define(name, val);
                    }
                    let result = self.eval_expr(then_branch);
                    self.env.pop_scope();
                    result
                } else if let Some(else_br) = else_branch {
                    self.eval_expr(else_br)
                } else {
                    Ok(Value::Unit)
                }
            }

            // Try operator (?) - unwrap Result/Option or propagate error
            // Works with any enum that has Ok/Some (success) or Err/None (failure) variants
            ExprKind::Try(inner) => {
                let val = self.eval_expr(inner)?;
                match &val {
                    Value::Enum {
                        variant, fields, ..
                    } => match variant.as_str() {
                        "Ok" | "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                        "Err" | "None" => Err(RuntimeError::TryError(val)),
                        _ => Err(RuntimeError::TypeError(format!(
                            "? operator requires Ok/Some or Err/None variant, got {}",
                            variant
                        ))),
                    },
                    _ => Err(RuntimeError::TypeError(format!(
                        "? operator requires Result or Option, got {}",
                        val.type_name()
                    ))),
                }
            }

            // Closure expression (|x, y| body)
            ExprKind::Closure { params, body } => {
                let captured = self.env.capture();
                Ok(Value::Closure {
                    params: params.iter().map(|p| p.name.clone()).collect(),
                    body: (**body).clone(),
                    captured_env: captured,
                })
            }

            // Type cast (x as i32)
            ExprKind::Cast { expr, ty } => {
                let val = self.eval_expr(expr)?;
                match (val, ty.as_str()) {
                    (Value::Int(n), "f64" | "f32" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Float(n), "i64" | "i32" | "int" | "i16" | "i8") => {
                        Ok(Value::Int(n as i64))
                    }
                    (Value::Float(n), "u64" | "u32" | "u16" | "u8" | "usize") => {
                        Ok(Value::Int(n as i64))
                    }
                    (Value::Int(n), "i64" | "i32" | "int" | "i16" | "i8" | "u64" | "u32"
                        | "u16" | "u8" | "usize") => Ok(Value::Int(n)),
                    (Value::Int(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Float(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Char(c), "i32" | "i64" | "int" | "u32" | "u8") => {
                        Ok(Value::Int(c as i64))
                    }
                    (Value::Int(n), "char") => {
                        Ok(Value::Char(char::from_u32(n as u32).unwrap_or('\0')))
                    }
                    (v, _) => Ok(v), // no-op for unrecognized casts
                }
            }

            // Null coalescing (a ?? b)
            ExprKind::NullCoalesce { value, default } => {
                let val = self.eval_expr(value)?;
                match &val {
                    Value::Enum { name, variant, fields, .. }
                        if name == "Option" && variant == "Some" =>
                    {
                        Ok(fields.first().cloned().unwrap_or(Value::Unit))
                    }
                    Value::Enum { name, variant, .. }
                        if name == "Option" && variant == "None" =>
                    {
                        self.eval_expr(default)
                    }
                    _ => Ok(val),
                }
            }

            // spawn_raw { body } - raw OS thread
            ExprKind::BlockCall { name, body } if name == "spawn_raw" => {
                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                let join_handle = std::thread::spawn(move || {
                    let mut interp = child;
                    let mut result = Value::Unit;
                    for stmt in &body {
                        match interp.exec_stmt(stmt) {
                            Ok(val) => result = val,
                            Err(e) => return Err(format!("{}", e)),
                        }
                    }
                    Ok(result)
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                })))
            }

            // spawn_thread { body } - thread from pool
            ExprKind::BlockCall { name, body } if name == "spawn_thread" => {
                let pool = self.env.get("__thread_pool").cloned();
                let pool = match pool {
                    Some(Value::ThreadPool(p)) => p,
                    _ => {
                        return Err(RuntimeError::TypeError(
                            "spawn_thread requires `with threading { }` context".to_string(),
                        ))
                    }
                };

                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                // Create a oneshot channel for the result
                let (result_tx, result_rx) = mpsc::sync_channel::<Result<Value, String>>(1);

                let task = PoolTask {
                    work: Box::new(move || {
                        let mut interp = child;
                        let mut result = Value::Unit;
                        for stmt in &body {
                            match interp.exec_stmt(stmt) {
                                Ok(val) => result = val,
                                Err(e) => {
                                    let _ = result_tx.send(Err(format!("{}", e)));
                                    return;
                                }
                            }
                        }
                        let _ = result_tx.send(Ok(result));
                    }),
                };

                // Send work to pool
                let sender = pool.sender.lock().unwrap();
                if let Some(ref tx) = *sender {
                    tx.send(task).map_err(|_| {
                        RuntimeError::TypeError("thread pool is shut down".to_string())
                    })?;
                } else {
                    return Err(RuntimeError::TypeError(
                        "thread pool is shut down".to_string(),
                    ));
                }

                // Wrap the result receiver as a thread handle
                let join_handle = std::thread::spawn(move || {
                    result_rx
                        .recv()
                        .unwrap_or(Err("thread pool task dropped".to_string()))
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                })))
            }

            // with threading(n) { body }
            ExprKind::WithBlock { name, args, body } if name == "threading" => {
                let num_threads = if args.is_empty() {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(4)
                } else {
                    self.eval_expr(&args[0])?.as_int()
                        .map_err(|e| RuntimeError::TypeError(e))? as usize
                };

                // Create thread pool
                let (tx, rx) = mpsc::channel::<PoolTask>();
                let rx = Arc::new(Mutex::new(rx));
                let mut workers = Vec::with_capacity(num_threads);

                for _ in 0..num_threads {
                    let rx = Arc::clone(&rx);
                    workers.push(std::thread::spawn(move || {
                        loop {
                            let task = {
                                let rx = rx.lock().unwrap();
                                rx.recv()
                            };
                            match task {
                                Ok(task) => (task.work)(),
                                Err(_) => break, // Channel closed, exit
                            }
                        }
                    }));
                }

                let pool = Arc::new(ThreadPoolInner {
                    sender: Mutex::new(Some(tx)),
                    workers: Mutex::new(Vec::new()),
                    size: num_threads,
                });

                // Store pool in environment and execute body
                self.env.push_scope();
                self.env.define("__thread_pool".to_string(), Value::ThreadPool(pool.clone()));

                let mut result = Value::Unit;
                for stmt in body {
                    match self.exec_stmt(stmt) {
                        Ok(val) => result = val,
                        Err(e) => {
                            // Shut down pool on error
                            *pool.sender.lock().unwrap() = None;
                            for w in workers {
                                let _ = w.join();
                            }
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                }

                // Shut down pool: drop sender so workers exit
                *pool.sender.lock().unwrap() = None;
                for w in workers {
                    let _ = w.join();
                }
                self.env.pop_scope();
                Ok(result)
            }

            // Spawn (green task) - not yet implemented, needs M:N scheduler
            ExprKind::Spawn { body } => {
                // For now, treat like spawn_raw (OS thread)
                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                let join_handle = std::thread::spawn(move || {
                    let mut interp = child;
                    let mut result = Value::Unit;
                    for stmt in &body {
                        match interp.exec_stmt(stmt) {
                            Ok(val) => result = val,
                            Err(e) => return Err(format!("{}", e)),
                        }
                    }
                    Ok(result)
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                })))
            }

            // Other expressions not yet implemented
            _ => Ok(Value::Unit),
        }
    }

    /// Match a pattern against a value, returning bindings if successful.
    fn match_pattern(&self, pattern: &Pattern, value: &Value) -> Option<HashMap<String, Value>> {
        match pattern {
            Pattern::Wildcard => Some(HashMap::new()),

            Pattern::Ident(name) => {
                // Check if this identifier is a unit enum variant
                // If so, match against the enum value instead of binding
                if let Value::Enum {
                    variant,
                    fields,
                    ..
                } = value
                {
                    // Check if this name is a known unit variant
                    let is_unit_variant = self.enums.values().any(|e| {
                        e.variants.iter().any(|v| v.name == *name && v.fields.is_empty())
                    });
                    if is_unit_variant {
                        // Match as enum variant, not binding
                        if variant == name && fields.is_empty() {
                            return Some(HashMap::new());
                        } else {
                            return None;
                        }
                    }
                }
                // Not a unit variant - treat as variable binding
                let mut bindings = HashMap::new();
                bindings.insert(name.clone(), value.clone());
                Some(bindings)
            }

            Pattern::Literal(lit_expr) => {
                // Compare value to literal
                if self.values_equal(value, lit_expr) {
                    Some(HashMap::new())
                } else {
                    None
                }
            }

            Pattern::Constructor { name, fields } => {
                if let Value::Enum {
                    variant,
                    fields: enum_fields,
                    ..
                } = value
                {
                    if variant == name && fields.len() == enum_fields.len() {
                        let mut bindings = HashMap::new();
                        for (pat, val) in fields.iter().zip(enum_fields.iter()) {
                            if let Some(sub_bindings) = self.match_pattern(pat, val) {
                                bindings.extend(sub_bindings);
                            } else {
                                return None;
                            }
                        }
                        return Some(bindings);
                    }
                }
                None
            }

            Pattern::Struct {
                name: pat_name,
                fields: pat_fields,
                rest: _,
            } => {
                if let Value::Struct { name, fields, .. } = value {
                    if name == pat_name {
                        let mut bindings = HashMap::new();
                        for (field_name, field_pattern) in pat_fields {
                            if let Some(field_val) = fields.get(field_name) {
                                if let Some(sub_bindings) =
                                    self.match_pattern(field_pattern, field_val)
                                {
                                    bindings.extend(sub_bindings);
                                } else {
                                    return None;
                                }
                            } else {
                                return None;
                            }
                        }
                        return Some(bindings);
                    }
                }
                None
            }

            Pattern::Tuple(patterns) => {
                // For now, treat tuple as an array/vec
                if let Value::Vec(v) = value {
                    let vec = v.lock().unwrap();
                    if patterns.len() == vec.len() {
                        let mut bindings = HashMap::new();
                        for (pat, val) in patterns.iter().zip(vec.iter()) {
                            if let Some(sub_bindings) = self.match_pattern(pat, val) {
                                bindings.extend(sub_bindings);
                            } else {
                                return None;
                            }
                        }
                        return Some(bindings);
                    }
                }
                None
            }

            Pattern::Or(patterns) => {
                for pat in patterns {
                    if let Some(bindings) = self.match_pattern(pat, value) {
                        return Some(bindings);
                    }
                }
                None
            }
        }
    }

    /// Compare a value to a literal expression for pattern matching.
    fn values_equal(&self, value: &Value, lit_expr: &Expr) -> bool {
        match (&value, &lit_expr.kind) {
            (Value::Int(a), ExprKind::Int(b)) => *a == *b,
            (Value::Float(a), ExprKind::Float(b)) => *a == *b,
            (Value::Bool(a), ExprKind::Bool(b)) => *a == *b,
            (Value::Char(a), ExprKind::Char(b)) => *a == *b,
            (Value::String(a), ExprKind::String(b)) => *a.lock().unwrap() == *b,
            _ => false,
        }
    }

    /// Compare two runtime values for equality.
    pub(crate) fn value_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::String(a), Value::String(b)) => *a.lock().unwrap() == *b.lock().unwrap(),
            (Value::Enum { name: n1, variant: v1, fields: f1 },
             Value::Enum { name: n2, variant: v2, fields: f2 }) => {
                n1 == n2 && v1 == v2 && f1.len() == f2.len()
                    && f1.iter().zip(f2.iter()).all(|(a, b)| Self::value_eq(a, b))
            }
            (Value::Handle { pool_id: p1, index: i1, generation: g1 },
             Value::Handle { pool_id: p2, index: i2, generation: g2 }) => {
                p1 == p2 && i1 == i2 && g1 == g2
            }
            _ => false,
        }
    }

    /// Call a value (function or builtin).
    pub(crate) fn call_value(&mut self, func: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match func {
            Value::Function { name } => {
                if let Some(decl) = self.functions.get(&name).cloned() {
                    self.call_function(&decl, args)
                } else {
                    Err(RuntimeError::UndefinedFunction(name))
                }
            }
            Value::Builtin(kind) => self.call_builtin(kind, args),
            Value::EnumConstructor {
                enum_name,
                variant_name,
                field_count,
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
                })
            }
            Value::Closure {
                params,
                body,
                captured_env,
            } => {
                self.env.push_scope();
                // Restore captured environment
                for (name, val) in captured_env {
                    self.env.define(name, val);
                }
                // Bind parameters
                for (param, arg) in params.iter().zip(args.into_iter()) {
                    self.env.define(param.clone(), arg);
                }
                let result = self.eval_expr(&body);
                self.env.pop_scope();
                match result {
                    Ok(v) => Ok(v),
                    Err(RuntimeError::Return(v)) => Ok(v),
                    Err(e) => Err(e),
                }
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
            BuiltinKind::Println => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write_output(" ");
                    }
                    self.write_output(&format!("{}", arg));
                }
                self.write_output_ln();
                Ok(Value::Unit)
            }
            BuiltinKind::Print => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write_output(" ");
                    }
                    self.write_output(&format!("{}", arg));
                }
                Ok(Value::Unit)
            }
            BuiltinKind::Panic => {
                let msg = args
                    .first()
                    .map(|v| format!("{}", v))
                    .unwrap_or_else(|| "panic".to_string());
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
        }
    }

    /// Format a string with positional/named placeholders and format specifiers.
    fn format_string(&self, template: &str, args: &[Value]) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = template.chars().peekable();
        let mut arg_index = 0usize;

        while let Some(c) = chars.next() {
            if c == '{' {
                // Check for escaped brace {{
                if chars.peek() == Some(&'{') {
                    chars.next();
                    result.push('{');
                    continue;
                }
                // Collect everything until '}'
                let mut spec_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        break;
                    }
                    spec_str.push(chars.next().unwrap());
                }
                // Parse: [arg_id][:format_spec]
                let (arg_id, fmt_spec) = if let Some(colon_pos) = spec_str.find(':') {
                    let id_part = &spec_str[..colon_pos];
                    let spec_part = &spec_str[colon_pos + 1..];
                    (id_part.to_string(), Some(spec_part.to_string()))
                } else {
                    (spec_str, None)
                };

                // Resolve the value
                let value = if arg_id.is_empty() {
                    // Positional: next arg
                    if arg_index < args.len() {
                        let v = args[arg_index].clone();
                        arg_index += 1;
                        v
                    } else {
                        // Fall back to environment lookup (empty name — just use next arg)
                        return Err(RuntimeError::TypeError(format!(
                            "format() not enough arguments (expected at least {})",
                            arg_index + 1
                        )));
                    }
                } else if let Ok(idx) = arg_id.parse::<usize>() {
                    // Explicit positional: {0}, {1}, etc.
                    if idx < args.len() {
                        args[idx].clone()
                    } else {
                        return Err(RuntimeError::TypeError(format!(
                            "format() argument index {} out of range (have {} args)",
                            idx,
                            args.len()
                        )));
                    }
                } else {
                    // Named: look up variable in environment, supporting dotted access
                    self.resolve_named_placeholder(&arg_id)?
                };

                // Apply format spec
                match fmt_spec {
                    Some(spec) => {
                        let formatted = self.apply_format_spec(&value, &spec)?;
                        result.push_str(&formatted);
                    }
                    None => {
                        result.push_str(&format!("{}", value));
                    }
                }
            } else if c == '}' {
                // Check for escaped brace }}
                if chars.peek() == Some(&'}') {
                    chars.next();
                    result.push('}');
                } else {
                    result.push('}');
                }
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }

    /// Resolve a named placeholder like "name" or "obj.field" from the environment.
    fn resolve_named_placeholder(&self, name: &str) -> Result<Value, RuntimeError> {
        let parts: Vec<&str> = name.split('.').collect();
        if let Some(val) = self.env.get(parts[0]) {
            let mut current = val.clone();
            for &part in &parts[1..] {
                match current {
                    Value::Struct { fields, .. } => {
                        current = fields.get(part).cloned().unwrap_or(Value::Unit);
                    }
                    _ => {
                        return Err(RuntimeError::TypeError(format!(
                            "cannot access field '{}' on {}",
                            part,
                            current.type_name()
                        )));
                    }
                }
            }
            Ok(current)
        } else {
            Err(RuntimeError::UndefinedVariable(parts[0].to_string()))
        }
    }

    /// Apply a format specifier to a value.
    fn apply_format_spec(&self, value: &Value, spec: &str) -> Result<String, RuntimeError> {
        // Parse: [[fill]align][width][.precision][type]
        let mut fill = ' ';
        let mut align = None; // None means default (right for numbers, left for strings)
        let mut width = 0usize;
        let mut precision = None;
        let mut format_type = ' '; // ' ' = display, '?' = debug, 'x'/'X' = hex, 'b' = binary, 'o' = octal, 'e' = scientific

        // Check for [fill]align — align is <, >, ^
        // If second char is an align char, first char is fill
        let spec_chars: Vec<char> = spec.chars().collect();
        let mut pos = 0;

        if spec_chars.len() >= 2 && matches!(spec_chars[1], '<' | '>' | '^') {
            fill = spec_chars[0];
            align = Some(spec_chars[1]);
            pos = 2;
        } else if !spec_chars.is_empty() && matches!(spec_chars[0], '<' | '>' | '^') {
            align = Some(spec_chars[0]);
            pos = 1;
        }

        // Parse width
        let mut width_str = String::new();
        while pos < spec_chars.len() && spec_chars[pos].is_ascii_digit() {
            width_str.push(spec_chars[pos]);
            pos += 1;
        }
        if !width_str.is_empty() {
            width = width_str.parse().unwrap_or(0);
        }

        // Parse .precision
        if pos < spec_chars.len() && spec_chars[pos] == '.' {
            pos += 1;
            let mut prec_str = String::new();
            while pos < spec_chars.len() && spec_chars[pos].is_ascii_digit() {
                prec_str.push(spec_chars[pos]);
                pos += 1;
            }
            precision = Some(prec_str.parse::<usize>().unwrap_or(0));
        }

        // Parse type
        if pos < spec_chars.len() {
            format_type = spec_chars[pos];
        }

        // Format the value based on type
        let formatted = match format_type {
            '?' => {
                // Debug representation
                self.debug_format(value)
            }
            'x' => {
                // Hex lowercase
                match value {
                    Value::Int(n) => format!("{:x}", n),
                    _ => format!("{}", value),
                }
            }
            'X' => {
                // Hex uppercase
                match value {
                    Value::Int(n) => format!("{:X}", n),
                    _ => format!("{}", value),
                }
            }
            'b' => {
                // Binary
                match value {
                    Value::Int(n) => format!("{:b}", n),
                    _ => format!("{}", value),
                }
            }
            'o' => {
                // Octal
                match value {
                    Value::Int(n) => format!("{:o}", n),
                    _ => format!("{}", value),
                }
            }
            'e' => {
                // Scientific notation
                match value {
                    Value::Float(n) => format!("{:e}", n),
                    Value::Int(n) => format!("{:e}", *n as f64),
                    _ => format!("{}", value),
                }
            }
            _ => {
                // Display (default)
                match precision {
                    Some(prec) => match value {
                        Value::Float(n) => format!("{:.prec$}", n, prec = prec),
                        _ => format!("{}", value),
                    },
                    None => format!("{}", value),
                }
            }
        };

        // Apply width and alignment
        if width > 0 && formatted.len() < width {
            let padding = width - formatted.len();
            let effective_align = align.unwrap_or('>');
            match effective_align {
                '<' => {
                    let mut s = formatted;
                    for _ in 0..padding {
                        s.push(fill);
                    }
                    Ok(s)
                }
                '^' => {
                    let left_pad = padding / 2;
                    let right_pad = padding - left_pad;
                    let mut s = String::new();
                    for _ in 0..left_pad {
                        s.push(fill);
                    }
                    s.push_str(&formatted);
                    for _ in 0..right_pad {
                        s.push(fill);
                    }
                    Ok(s)
                }
                _ => {
                    // '>' or default: right-align
                    let mut s = String::new();
                    for _ in 0..padding {
                        s.push(fill);
                    }
                    s.push_str(&formatted);
                    Ok(s)
                }
            }
        } else {
            Ok(formatted)
        }
    }

    /// Debug format a value (shows type structure).
    fn debug_format(&self, value: &Value) -> String {
        match value {
            Value::String(s) => format!("\"{}\"", s.lock().unwrap()),
            Value::Char(c) => format!("'{}'", c),
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                let items: Vec<String> = vec.iter().map(|v| self.debug_format(v)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Struct { name, fields, .. } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, self.debug_format(v)))
                    .collect();
                format!("{} {{ {} }}", name, field_strs.join(", "))
            }
            Value::Enum { name, variant, fields } => {
                if fields.is_empty() {
                    format!("{}.{}", name, variant)
                } else {
                    let field_strs: Vec<String> =
                        fields.iter().map(|v| self.debug_format(v)).collect();
                    format!("{}.{}({})", name, variant, field_strs.join(", "))
                }
            }
            // For primitives, Display and Debug are the same
            _ => format!("{}", value),
        }
    }

    /// Interpolate a string, replacing {name} or {obj.field} with variable values.
    fn interpolate_string(&self, s: &str) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                // Escaped brace {{ — pass through literally for format()
                if chars.peek() == Some(&'{') {
                    result.push('{');
                    result.push('{');
                    chars.next();
                    continue;
                }
                // Collect expression until '}'
                let mut expr_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next(); // consume '}'
                        break;
                    }
                    expr_str.push(chars.next().unwrap());
                }
                // Empty braces {} or format specifiers {:x} — keep literal for format()
                if expr_str.is_empty() || expr_str.starts_with(':') {
                    result.push('{');
                    result.push_str(&expr_str);
                    result.push('}');
                    continue;
                }
                // Separate format specifier: {expr:spec}
                let (expr_part, fmt_spec) = if let Some(colon_pos) = expr_str.find(':') {
                    (&expr_str[..colon_pos], Some(&expr_str[colon_pos..]))
                } else {
                    (expr_str.as_str(), None)
                };
                let value = self.eval_interpolation_expr(expr_part)?;
                if let Some(spec) = fmt_spec {
                    // Re-wrap with format specifier for Display
                    result.push_str(&Self::format_value_with_spec(&value, spec));
                } else {
                    result.push_str(&format!("{}", value));
                }
            } else if c == '}' && chars.peek() == Some(&'}') {
                // Escaped brace }} — pass through literally for format()
                result.push('}');
                result.push('}');
                chars.next();
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }

    /// Evaluate a simple expression inside string interpolation.
    /// Supports: variable, dotted field access, and simple binary ops (+, -, *, /).
    fn eval_interpolation_expr(&self, expr: &str) -> Result<Value, RuntimeError> {
        let expr = expr.trim();
        // Try binary operators (lowest precedence first)
        for op_str in &[" + ", " - ", " * ", " / "] {
            if let Some(pos) = expr.find(op_str) {
                let left = self.eval_interpolation_expr(&expr[..pos])?;
                let right = self.eval_interpolation_expr(&expr[pos + op_str.len()..])?;
                return match (*op_str, &left, &right) {
                    (" + ", Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
                    (" - ", Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
                    (" * ", Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
                    (" / ", Value::Int(a), Value::Int(b)) => {
                        if *b == 0 { return Err(RuntimeError::DivisionByZero); }
                        Ok(Value::Int(a / b))
                    }
                    (" + ", Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                    (" - ", Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                    (" * ", Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                    (" / ", Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                    _ => Err(RuntimeError::TypeError(format!(
                        "unsupported interpolation operation: {} {:?} {}", left.type_name(), op_str.trim(), right.type_name()
                    ))),
                };
            }
        }
        // Try integer literal
        if let Ok(n) = expr.parse::<i64>() {
            return Ok(Value::Int(n));
        }
        // Try float literal
        if let Ok(f) = expr.parse::<f64>() {
            return Ok(Value::Float(f));
        }
        // Dotted field access (e.g., "state.score" or just "x")
        let parts: Vec<&str> = expr.split('.').collect();
        if let Some(val) = self.env.get(parts[0]) {
            let mut current = val.clone();
            for &part in &parts[1..] {
                match current {
                    Value::Struct { fields, .. } => {
                        current = fields.get(part).cloned().unwrap_or(Value::Unit);
                    }
                    _ => {
                        return Err(RuntimeError::TypeError(format!(
                            "cannot access field '{}' on {}",
                            part,
                            current.type_name()
                        )));
                    }
                }
            }
            Ok(current)
        } else {
            Err(RuntimeError::UndefinedVariable(parts[0].to_string()))
        }
    }

    /// Format a value with a format specifier like :.2, :.1, :b, :x, etc.
    fn format_value_with_spec(value: &Value, spec: &str) -> String {
        let spec = &spec[1..]; // strip leading ':'
        match value {
            Value::Float(f) => {
                if let Some(precision) = spec.strip_prefix('.') {
                    if let Ok(p) = precision.parse::<usize>() {
                        return format!("{:.*}", p, f);
                    }
                }
                format!("{}", f)
            }
            Value::Int(n) => {
                match spec {
                    "b" => format!("{:b}", n),
                    "x" => format!("{:x}", n),
                    "X" => format!("{:X}", n),
                    "o" => format!("{:o}", n),
                    _ => format!("{}", n),
                }
            }
            _ => format!("{}", value),
        }
    }

    /// Call a method on a value. Dispatches to builtins or stdlib.
    fn call_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match &receiver {
            // Stdlib: module methods (fs.read_file, io.read_line, etc.)
            Value::Module(module) => self.call_module_method(module, method, args),
            // Stdlib: File instance methods
            Value::File(f) => self.call_file_method(f, method, args),
            // Stdlib: Duration/Instant instance methods
            Value::Duration(nanos) => self.call_duration_method(*nanos, method),
            Value::Instant(instant) => self.call_instant_method(instant, method, args),
            // Stdlib: Metadata struct methods
            Value::Struct { name, fields, .. } if name == "Metadata" => {
                self.call_metadata_method(fields, method)
            }
            // Stdlib: Path struct methods
            Value::Struct { name, fields, .. } if name == "Path" => {
                self.call_path_instance_method(fields, method, args)
            }
            // Stdlib: Args struct methods (from cli.parse())
            Value::Struct { name, fields, .. } if name == "Args" => {
                self.call_args_method(fields, method, args)
            }
            // Stdlib: JsonValue enum methods
            Value::Enum { name, variant, fields } if name == "JsonValue" => {
                self.call_json_value_method(variant, fields, method)
            }
            // Everything else: builtins (primitives, string, vec, etc.) + user-defined
            _ => self.call_builtin_method(receiver, method, args),
        }
    }
    /// Helper to extract an integer from args.
    pub(crate) fn expect_int(&self, args: &[Value], idx: usize) -> Result<i64, RuntimeError> {
        match args.get(idx) {
            Some(Value::Int(n)) => Ok(*n),
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
            Some(Value::Float(n)) => Ok(*n),
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

    /// Check if a value is truthy.
    fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Bool(b) => *b,
            Value::Unit => false,
            Value::Int(0) => false,
            _ => true,
        }
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
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),

    #[error("undefined function: {0}")]
    UndefinedFunction(String),

    #[error("type error: {0}")]
    TypeError(String),

    #[error("division by zero")]
    DivisionByZero,

    #[error("arity mismatch: expected {expected}, got {got}")]
    ArityMismatch { expected: usize, got: usize },

    #[error("no such method '{method}' on type {ty}")]
    NoSuchMethod { ty: String, method: String },

    #[error("panic: {0}")]
    Panic(String),

    #[error("no matching arm in match expression")]
    NoMatchingArm,

    #[error("multiple @entry functions found (only one allowed per program)")]
    MultipleEntryPoints,

    #[error("no @entry function found (add @entry to mark the program entry point)")]
    NoEntryPoint,

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
}

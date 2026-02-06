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
    env: Environment,
    /// Function declarations by name.
    functions: HashMap<String, FnDecl>,
    /// Enum declarations by name.
    enums: HashMap<String, EnumDecl>,
    /// Struct declarations by name (for @resource checking).
    struct_decls: HashMap<String, StructDecl>,
    /// Methods from extend blocks (type_name -> method_name -> FnDecl).
    methods: HashMap<String, HashMap<String, FnDecl>>,
    /// Linear resource tracker.
    resource_tracker: ResourceTracker,
    /// Optional output buffer for capturing stdout (used in tests).
    output_buffer: Option<Arc<Mutex<String>>>,
    /// Command-line arguments passed to the program.
    cli_args: Vec<String>,
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
    fn get_resource_id(&self, value: &Value) -> Option<u64> {
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
    fn call_function(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
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
        for (param, arg) in func.params.iter().zip(args.into_iter()) {
            self.env.define(param.name.clone(), arg);
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
    fn assign_target(&mut self, target: &Expr, value: Value) -> Result<(), RuntimeError> {
        match &target.kind {
            ExprKind::Ident(name) => {
                if !self.env.assign(name, value) {
                    return Err(RuntimeError::UndefinedVariable(name.clone()));
                }
                Ok(())
            }
            // Field assignment: obj.field = value
            ExprKind::Field { object, field } => {
                if let ExprKind::Ident(var_name) = &object.kind {
                    if let Some(obj) = self.env.get_mut(var_name) {
                        match obj {
                            Value::Struct { fields, .. } => {
                                fields.insert(field.clone(), value);
                                Ok(())
                            }
                            _ => Err(RuntimeError::TypeError(format!(
                                "cannot assign field on {}",
                                obj.type_name()
                            ))),
                        }
                    } else {
                        Err(RuntimeError::UndefinedVariable(var_name.clone()))
                    }
                } else if let ExprKind::Index {
                    object: idx_obj,
                    index: idx_expr,
                } = &object.kind
                {
                    // Nested field assignment: collection[idx].field = value
                    // Handles: pool[h].field = value, vec[i].field = value
                    let idx_val = self.eval_expr(idx_expr)?;
                    if let ExprKind::Ident(var_name) = &idx_obj.kind {
                        if let Some(container) = self.env.get(var_name).cloned() {
                            match container {
                                Value::Pool(p) => {
                                    if let Value::Handle {
                                        pool_id,
                                        index,
                                        generation,
                                    } = idx_val
                                    {
                                        let mut pool = p.lock().unwrap();
                                        let slot_idx = pool
                                            .validate(pool_id, index, generation)
                                            .map_err(|e| RuntimeError::Panic(e))?;
                                        if let Some(Value::Struct {
                                            ref mut fields, ..
                                        }) = pool.slots[slot_idx].1
                                        {
                                            fields.insert(field.clone(), value);
                                            Ok(())
                                        } else {
                                            Err(RuntimeError::TypeError(
                                                "pool element is not a struct".to_string(),
                                            ))
                                        }
                                    } else {
                                        Err(RuntimeError::TypeError(
                                            "Pool index must be a Handle".to_string(),
                                        ))
                                    }
                                }
                                Value::Vec(v) => {
                                    if let Value::Int(i) = idx_val {
                                        let i = i as usize;
                                        let mut vec = v.lock().unwrap();
                                        if i < vec.len() {
                                            if let Value::Struct {
                                                ref mut fields, ..
                                            } = vec[i]
                                            {
                                                fields.insert(field.clone(), value);
                                                Ok(())
                                            } else {
                                                Err(RuntimeError::TypeError(
                                                    "vec element is not a struct".to_string(),
                                                ))
                                            }
                                        } else {
                                            Err(RuntimeError::TypeError(format!(
                                                "index {} out of bounds",
                                                i
                                            )))
                                        }
                                    } else {
                                        Err(RuntimeError::TypeError(
                                            "Vec index must be integer".to_string(),
                                        ))
                                    }
                                }
                                _ => Err(RuntimeError::TypeError(format!(
                                    "cannot field-assign on indexed {}",
                                    container.type_name()
                                ))),
                            }
                        } else {
                            Err(RuntimeError::UndefinedVariable(var_name.clone()))
                        }
                    } else {
                        Err(RuntimeError::TypeError(
                            "nested field assignment not yet supported".to_string(),
                        ))
                    }
                } else {
                    Err(RuntimeError::TypeError(
                        "nested field assignment not yet supported".to_string(),
                    ))
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
    fn value_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::String(a), Value::String(b)) => *a.lock().unwrap() == *b.lock().unwrap(),
            _ => false,
        }
    }

    /// Call a value (function or builtin).
    fn call_value(&mut self, func: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
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
                    match arg {
                        Value::String(s) => {
                            // Handle string interpolation
                            let output = self.interpolate_string(&s.lock().unwrap())?;
                            self.write_output(&output);
                        }
                        _ => self.write_output(&format!("{}", arg)),
                    }
                }
                self.write_output_ln();
                Ok(Value::Unit)
            }
            BuiltinKind::Print => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.write_output(" ");
                    }
                    match arg {
                        Value::String(s) => {
                            let output = self.interpolate_string(&s.lock().unwrap())?;
                            self.write_output(&output);
                        }
                        _ => self.write_output(&format!("{}", arg)),
                    }
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
        }
    }

    /// Interpolate a string, replacing {name} or {obj.field} with variable values.
    fn interpolate_string(&self, s: &str) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                // Collect expression until '}'
                let mut expr_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next(); // consume '}'
                        break;
                    }
                    expr_str.push(chars.next().unwrap());
                }
                // Handle dotted field access (e.g., "opts.verbose")
                let parts: Vec<&str> = expr_str.split('.').collect();
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
                    result.push_str(&format!("{}", current));
                } else {
                    return Err(RuntimeError::UndefinedVariable(parts[0].to_string()));
                }
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }

    /// Call a method on a value (handles desugared operators).
    fn call_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match (&receiver, method) {
            // Integer arithmetic methods
            (Value::Int(a), "add") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a + b))
            }
            (Value::Int(a), "sub") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a - b))
            }
            (Value::Int(a), "mul") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a * b))
            }
            (Value::Int(a), "div") => {
                let b = self.expect_int(&args, 0)?;
                if b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                Ok(Value::Int(a / b))
            }
            (Value::Int(a), "rem") => {
                let b = self.expect_int(&args, 0)?;
                if b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                Ok(Value::Int(a % b))
            }
            (Value::Int(a), "neg") => Ok(Value::Int(-a)),

            // Integer comparison methods
            (Value::Int(a), "eq") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Bool(*a == b))
            }
            (Value::Int(a), "lt") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Bool(*a < b))
            }
            (Value::Int(a), "le") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Bool(*a <= b))
            }
            (Value::Int(a), "gt") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Bool(*a > b))
            }
            (Value::Int(a), "ge") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Bool(*a >= b))
            }

            // Integer bitwise methods
            (Value::Int(a), "bit_and") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a & b))
            }
            (Value::Int(a), "bit_or") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a | b))
            }
            (Value::Int(a), "bit_xor") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a ^ b))
            }
            (Value::Int(a), "shl") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a << b))
            }
            (Value::Int(a), "shr") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int(a >> b))
            }
            (Value::Int(a), "bit_not") => Ok(Value::Int(!a)),
            (Value::Int(a), "abs") => Ok(Value::Int(a.abs())),
            (Value::Int(a), "min") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int((*a).min(b)))
            }
            (Value::Int(a), "max") => {
                let b = self.expect_int(&args, 0)?;
                Ok(Value::Int((*a).max(b)))
            }
            (Value::Int(a), "to_string") => {
                Ok(Value::String(Arc::new(Mutex::new(a.to_string()))))
            }
            (Value::Int(a), "to_float") => Ok(Value::Float(*a as f64)),

            // Float arithmetic methods
            (Value::Float(a), "add") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a + b))
            }
            (Value::Float(a), "sub") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a - b))
            }
            (Value::Float(a), "mul") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a * b))
            }
            (Value::Float(a), "div") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a / b))
            }
            (Value::Float(a), "neg") => Ok(Value::Float(-a)),

            // Float comparison methods
            (Value::Float(a), "eq") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Bool(*a == b))
            }
            (Value::Float(a), "lt") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Bool(*a < b))
            }
            (Value::Float(a), "le") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Bool(*a <= b))
            }
            (Value::Float(a), "gt") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Bool(*a > b))
            }
            (Value::Float(a), "ge") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Bool(*a >= b))
            }
            (Value::Float(a), "abs") => Ok(Value::Float(a.abs())),
            (Value::Float(a), "floor") => Ok(Value::Float(a.floor())),
            (Value::Float(a), "ceil") => Ok(Value::Float(a.ceil())),
            (Value::Float(a), "round") => Ok(Value::Float(a.round())),
            (Value::Float(a), "sqrt") => Ok(Value::Float(a.sqrt())),
            (Value::Float(a), "min") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a.min(b)))
            }
            (Value::Float(a), "max") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a.max(b)))
            }
            (Value::Float(a), "to_string") => {
                Ok(Value::String(Arc::new(Mutex::new(a.to_string()))))
            }
            (Value::Float(a), "to_int") => Ok(Value::Int(*a as i64)),
            (Value::Float(a), "pow") => {
                let b = self.expect_float(&args, 0)?;
                Ok(Value::Float(a.powf(b)))
            }

            // Bool comparison
            (Value::Bool(a), "eq") => {
                let b = self.expect_bool(&args, 0)?;
                Ok(Value::Bool(*a == b))
            }

            // Char methods
            (Value::Char(c), "is_whitespace") => Ok(Value::Bool(c.is_whitespace())),
            (Value::Char(c), "is_alphabetic") => Ok(Value::Bool(c.is_alphabetic())),
            (Value::Char(c), "is_alphanumeric") => Ok(Value::Bool(c.is_alphanumeric())),
            (Value::Char(c), "is_digit") => Ok(Value::Bool(c.is_ascii_digit())),
            (Value::Char(c), "is_uppercase") => Ok(Value::Bool(c.is_uppercase())),
            (Value::Char(c), "is_lowercase") => Ok(Value::Bool(c.is_lowercase())),
            (Value::Char(c), "to_uppercase") => {
                Ok(Value::Char(c.to_uppercase().next().unwrap_or(*c)))
            }
            (Value::Char(c), "to_lowercase") => {
                Ok(Value::Char(c.to_lowercase().next().unwrap_or(*c)))
            }
            (Value::Char(c), "eq") => {
                let other = self.expect_char(&args, 0)?;
                Ok(Value::Bool(*c == other))
            }

            // String methods
            (Value::String(s), "len") => Ok(Value::Int(s.lock().unwrap().len() as i64)),
            (Value::String(s), "is_empty") => Ok(Value::Bool(s.lock().unwrap().is_empty())),
            (Value::String(s), "clone") => Ok(Value::String(Arc::clone(s))),
            (Value::String(s), "starts_with") => {
                let prefix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().starts_with(&prefix)))
            }
            (Value::String(s), "ends_with") => {
                let suffix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().ends_with(&suffix)))
            }
            (Value::String(s), "contains") => {
                let pattern = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().contains(&pattern)))
            }
            (Value::String(s), "push") => {
                let c = self.expect_char(&args, 0)?;
                s.lock().unwrap().push(c);
                Ok(Value::Unit)
            }
            (Value::String(s), "trim") => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim().to_string()))))
            }
            (Value::String(s), "trim_start") => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim_start().to_string()))))
            }
            (Value::String(s), "trim_end") => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim_end().to_string()))))
            }
            (Value::String(s), "to_string") => Ok(Value::String(Arc::clone(s))),
            (Value::String(s), "to_owned") => {
                // Clone the string to create an owned copy
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone()))))
            }
            (Value::String(s), "to_uppercase") => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().to_uppercase()))))
            }
            (Value::String(s), "to_lowercase") => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().to_lowercase()))))
            }
            (Value::String(s), "split") => {
                let delimiter = self.expect_string(&args, 0)?;
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split(&delimiter)
                    .map(|p| Value::String(Arc::new(Mutex::new(p.to_string()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(parts))))
            }
            (Value::String(s), "split_whitespace") => {
                // Split on any Unicode whitespace, skip empty
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split_whitespace()
                    .map(|part| Value::String(Arc::new(Mutex::new(part.to_string()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(parts))))
            }
            (Value::String(s), "chars") => {
                let chars: Vec<Value> = s.lock().unwrap().chars().map(Value::Char).collect();
                Ok(Value::Vec(Arc::new(Mutex::new(chars))))
            }
            (Value::String(s), "lines") => {
                let lines: Vec<Value> = s
                    .lock().unwrap()
                    .lines()
                    .map(|l| Value::String(Arc::new(Mutex::new(l.to_string()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(lines))))
            }
            (Value::String(s), "replace") => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().replace(&from, &to)))))
            }
            (Value::String(s), "substring") => {
                let sb = s.lock().unwrap();
                let start = self.expect_int(&args, 0)? as usize;
                let end = args
                    .get(1)
                    .map(|v| match v {
                        Value::Int(i) => *i as usize,
                        _ => sb.len(),
                    })
                    .unwrap_or(sb.len());
                let substring: String = sb.chars().skip(start).take(end - start).collect();
                Ok(Value::String(Arc::new(Mutex::new(substring))))
            }
            (Value::String(s), "parse_int") | (Value::String(s), "parse") => {
                // parse<i32>() and parse_int() both parse as integer
                match s.lock().unwrap().trim().parse::<i64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Int(n)],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "invalid integer".to_string(),
                        )))],
                    }),
                }
            }
            (Value::String(s), "char_at") => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.lock().unwrap().chars().nth(idx) {
                    Some(c) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Char(c)],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            (Value::String(s), "byte_at") => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.lock().unwrap().as_bytes().get(idx) {
                    Some(&b) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Int(b as i64)],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            (Value::String(s), "parse_float") => {
                match s.lock().unwrap().trim().parse::<f64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Float(n)],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "invalid float".to_string(),
                        )))],
                    }),
                }
            }
            (Value::String(s), "index_of") => {
                let pattern = self.expect_string(&args, 0)?;
                match s.lock().unwrap().find(&pattern) {
                    Some(idx) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Int(idx as i64)],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            (Value::String(s), "repeat") => {
                let n = self.expect_int(&args, 0)? as usize;
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().repeat(n)))))
            }
            (Value::String(s), "reverse") => {
                Ok(Value::String(Arc::new(Mutex::new(
                    s.lock().unwrap().chars().rev().collect(),
                ))))
            }

            // Type constructor static methods (Vec.new(), string.new(), etc.)
            (Value::TypeConstructor(TypeConstructorKind::Vec), "new") => {
                Ok(Value::Vec(Arc::new(Mutex::new(Vec::new()))))
            }
            (Value::TypeConstructor(TypeConstructorKind::Vec), "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::Vec(Arc::new(Mutex::new(Vec::with_capacity(cap)))))
            }
            (Value::TypeConstructor(TypeConstructorKind::String), "new") => {
                Ok(Value::String(Arc::new(Mutex::new(String::new()))))
            }

            // Pool constructor static methods
            (Value::TypeConstructor(TypeConstructorKind::Pool), "new") => {
                use crate::value::PoolData;
                Ok(Value::Pool(Arc::new(Mutex::new(PoolData::new()))))
            }
            (Value::TypeConstructor(TypeConstructorKind::Pool), "with_capacity") => {
                use crate::value::PoolData;
                let cap = self.expect_int(&args, 0)? as usize;
                let mut pool = PoolData::new();
                pool.slots.reserve(cap);
                Ok(Value::Pool(Arc::new(Mutex::new(pool))))
            }

            // Pool instance methods
            (Value::Pool(p), "insert") => {
                let item = args.into_iter().next().unwrap_or(Value::Unit);
                let mut pool = p.lock().unwrap();
                let pool_id = pool.pool_id;
                let (index, generation) = pool.insert(item);
                let handle = Value::Handle {
                    pool_id,
                    index,
                    generation,
                };
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![handle],
                })
            }
            (Value::Pool(p), "get") => {
                if let Some(Value::Handle {
                    pool_id,
                    index,
                    generation,
                }) = args.first()
                {
                    let pool = p.lock().unwrap();
                    match pool.validate(*pool_id, *index, *generation) {
                        Ok(idx) => {
                            let val = pool.slots[idx].1.as_ref().unwrap().clone();
                            Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![val],
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                        }),
                    }
                } else {
                    Err(RuntimeError::TypeError(
                        "pool.get() requires a Handle argument".to_string(),
                    ))
                }
            }
            (Value::Pool(p), "remove") => {
                if let Some(Value::Handle {
                    pool_id,
                    index,
                    generation,
                }) = args.first()
                {
                    let mut pool = p.lock().unwrap();
                    match pool.validate(*pool_id, *index, *generation) {
                        Ok(idx) => {
                            let val = pool.remove_at(idx).unwrap();
                            Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "Some".to_string(),
                                fields: vec![val],
                            })
                        }
                        Err(_) => Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                        }),
                    }
                } else {
                    Err(RuntimeError::TypeError(
                        "pool.remove() requires a Handle argument".to_string(),
                    ))
                }
            }
            (Value::Pool(p), "len") => Ok(Value::Int(p.lock().unwrap().len as i64)),
            (Value::Pool(p), "is_empty") => Ok(Value::Bool(p.lock().unwrap().len == 0)),
            (Value::Pool(p), "contains") => {
                if let Some(Value::Handle {
                    pool_id,
                    index,
                    generation,
                }) = args.first()
                {
                    let pool = p.lock().unwrap();
                    Ok(Value::Bool(
                        pool.validate(*pool_id, *index, *generation).is_ok(),
                    ))
                } else {
                    Err(RuntimeError::TypeError(
                        "pool.contains() requires a Handle argument".to_string(),
                    ))
                }
            }
            (Value::Pool(p), "clear") => {
                let mut pool = p.lock().unwrap();
                let slot_count = pool.slots.len();
                for (_i, (gen, slot)) in pool.slots.iter_mut().enumerate() {
                    if slot.is_some() {
                        *slot = None;
                        *gen = gen.saturating_add(1);
                    }
                }
                pool.free_list.clear();
                for i in 0..slot_count {
                    pool.free_list.push(i as u32);
                }
                pool.len = 0;
                Ok(Value::Unit)
            }
            (Value::Pool(p), "handles") | (Value::Pool(p), "cursor") => {
                let pool = p.lock().unwrap();
                let pool_id = pool.pool_id;
                let handles: Vec<Value> = pool
                    .valid_handles()
                    .iter()
                    .map(|(idx, gen)| Value::Handle {
                        pool_id,
                        index: *idx,
                        generation: *gen,
                    })
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(handles))))
            }

            // Handle methods
            (Value::Handle { pool_id, index, generation, .. }, "eq") => {
                if let Some(Value::Handle {
                    pool_id: p2,
                    index: i2,
                    generation: g2,
                }) = args.first()
                {
                    Ok(Value::Bool(
                        *pool_id == *p2 && *index == *i2 && *generation == *g2,
                    ))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            (Value::Handle { .. }, "ne") => {
                let eq_result = self.call_method(receiver.clone(), "eq", args)?;
                if let Value::Bool(b) = eq_result {
                    Ok(Value::Bool(!b))
                } else {
                    Ok(Value::Bool(true))
                }
            }

            // Vec instance methods
            (Value::Vec(v), "push") => {
                let item = args.into_iter().next().unwrap_or(Value::Unit);
                v.lock().unwrap().push(item);
                // Return Ok(()) wrapped as Result enum
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                })
            }
            (Value::Vec(v), "pop") => {
                Ok(v.lock().unwrap().pop().unwrap_or(Value::Unit))
            }
            (Value::Vec(v), "len") => {
                Ok(Value::Int(v.lock().unwrap().len() as i64))
            }
            (Value::Vec(v), "get") => {
                let idx = self.expect_int(&args, 0)? as usize;
                Ok(v.lock().unwrap().get(idx).cloned().unwrap_or(Value::Unit))
            }
            (Value::Vec(v), "is_empty") => {
                Ok(Value::Bool(v.lock().unwrap().is_empty()))
            }
            (Value::Vec(v), "clear") => {
                v.lock().unwrap().clear();
                Ok(Value::Unit)
            }
            (Value::Vec(v), "iter") => {
                // Return a copy of the Vec for iteration
                // TODO: Implement proper iterator type with skip(), take(), etc.
                Ok(Value::Vec(Arc::clone(v)))
            }
            (Value::Vec(v), "skip") => {
                let n = self.expect_int(&args, 0)? as usize;
                let skipped: Vec<Value> = v.lock().unwrap().iter().skip(n).cloned().collect();
                Ok(Value::Vec(Arc::new(Mutex::new(skipped))))
            }
            (Value::Vec(v), "first") => {
                match v.lock().unwrap().first().cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            (Value::Vec(v), "last") => {
                match v.lock().unwrap().last().cloned() {
                    Some(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![val],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            (Value::Vec(v), "contains") => {
                if let Some(needle) = args.first() {
                    let found = v.lock().unwrap().iter().any(|item| Self::value_eq(item, needle));
                    Ok(Value::Bool(found))
                } else {
                    Err(RuntimeError::ArityMismatch { expected: 1, got: 0 })
                }
            }
            (Value::Vec(v), "reverse") => {
                v.lock().unwrap().reverse();
                Ok(Value::Unit)
            }
            (Value::Vec(v), "join") => {
                let sep = self.expect_string(&args, 0)?;
                let joined: String = v
                    .lock().unwrap()
                    .iter()
                    .map(|item| format!("{}", item))
                    .collect::<Vec<_>>()
                    .join(&sep);
                Ok(Value::String(Arc::new(Mutex::new(joined))))
            }

            // Module methods (fs.read_file, io.read_line, etc.)
            (Value::Module(ModuleKind::Fs), "read_file") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(content)))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "read_lines") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let lines: Vec<Value> = content
                            .lines()
                            .map(|l| Value::String(Arc::new(Mutex::new(l.to_string()))))
                            .collect();
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Vec(Arc::new(Mutex::new(lines)))],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "write_file") => {
                let path = self.expect_string(&args, 0)?;
                let content = self.expect_string(&args, 1)?;
                match std::fs::write(&path, &content) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "append_file") => {
                use std::io::Write;
                let path = self.expect_string(&args, 0)?;
                let content = self.expect_string(&args, 1)?;
                let result = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| f.write_all(content.as_bytes()));
                match result {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "exists") => {
                let path = self.expect_string(&args, 0)?;
                Ok(Value::Bool(std::path::Path::new(&path).exists()))
            }
            (Value::Module(ModuleKind::Fs), "open") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::File::open(&path) {
                    Ok(file) => {
                        let arc = Arc::new(Mutex::new(Some(file)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker.register_file(ptr, self.env.scope_depth());
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::File(arc)],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "create") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::File::create(&path) {
                    Ok(file) => {
                        let arc = Arc::new(Mutex::new(Some(file)));
                        let ptr = Arc::as_ptr(&arc) as usize;
                        self.resource_tracker.register_file(ptr, self.env.scope_depth());
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::File(arc)],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "canonicalize") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::canonicalize(&path) {
                    Ok(p) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            p.to_string_lossy().to_string(),
                        )))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "metadata") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::metadata(&path) {
                    Ok(meta) => {
                        let mut fields = HashMap::new();
                        fields.insert("size".to_string(), Value::Int(meta.len() as i64));
                        // Add timestamps as unix seconds (i64)
                        if let Ok(accessed) = meta.accessed() {
                            if let Ok(dur) = accessed.duration_since(std::time::UNIX_EPOCH) {
                                fields.insert("accessed".to_string(), Value::Int(dur.as_secs() as i64));
                            }
                        }
                        if let Ok(modified) = meta.modified() {
                            if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                                fields.insert("modified".to_string(), Value::Int(dur.as_secs() as i64));
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Struct {
                                name: "Metadata".to_string(),
                                fields,
                                resource_id: None,
                            }],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "remove") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "remove_dir") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::remove_dir(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "create_dir") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::create_dir(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "create_dir_all") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::create_dir_all(&path) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "rename") => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                match std::fs::rename(&from, &to) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "copy") => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                match std::fs::copy(&from, &to) {
                    Ok(bytes) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Int(bytes as i64)],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Io), "read_line") => {
                use std::io::{self, BufRead};
                let mut line = String::new();
                match io::stdin().lock().read_line(&mut line) {
                    Ok(_) => {
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(line)))],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Cli), "args") => {
                let args_vec: Vec<Value> = self
                    .cli_args
                    .iter()
                    .map(|s| Value::String(Arc::new(Mutex::new(s.clone()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(args_vec))))
            }
            (Value::Module(ModuleKind::Std), "exit") => {
                let code = args
                    .first()
                    .map(|v| match v {
                        Value::Int(n) => *n as i32,
                        _ => 1,
                    })
                    .unwrap_or(0);
                Err(RuntimeError::Exit(code))
            }
            (Value::Module(ModuleKind::Env), "var") => {
                let name = self.expect_string(&args, 0)?;
                match std::env::var(&name) {
                    Ok(val) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(val)))],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            (Value::Module(ModuleKind::Env), "vars") => {
                let vars: Vec<Value> = std::env::vars()
                    .map(|(k, v)| {
                        Value::Vec(Arc::new(Mutex::new(vec![
                            Value::String(Arc::new(Mutex::new(k))),
                            Value::String(Arc::new(Mutex::new(v))),
                        ])))
                    })
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(vars))))
            }

            // random.f32() -> f32 in [0.0, 1.0)
            (Value::Module(ModuleKind::Random), "f32") => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                // xorshift64 step for better distribution
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let f = (x as f64) / (u64::MAX as f64);
                Ok(Value::Float(f))
            }

            // random.f64() -> f64 in [0.0, 1.0)
            (Value::Module(ModuleKind::Random), "f64") => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let f = (x as f64) / (u64::MAX as f64);
                Ok(Value::Float(f))
            }

            // random.i64() -> random i64
            (Value::Module(ModuleKind::Random), "i64") => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                Ok(Value::Int(x as i64))
            }

            // random.bool() -> bool
            (Value::Module(ModuleKind::Random), "bool") => {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                Ok(Value::Bool(hash & 1 == 1))
            }

            // random.range(low, high) -> i64 in [low, high)
            (Value::Module(ModuleKind::Random), "range") => {
                if args.len() != 2 {
                    return Err(RuntimeError::ArityMismatch { expected: 2, got: args.len() });
                }
                let low = args[0].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                let high = args[1].as_int().map_err(|e| RuntimeError::TypeError(e))?;
                if low >= high {
                    return Err(RuntimeError::TypeError(
                        format!("random.range: low ({}) must be less than high ({})", low, high)
                    ));
                }
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now().hash(&mut hasher);
                std::thread::current().id().hash(&mut hasher);
                let hash = hasher.finish();
                let mut x = hash;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let range = (high - low) as u64;
                let value = low + (x % range) as i64;
                Ok(Value::Int(value))
            }

            // time.sleep(duration)
            (Value::Module(ModuleKind::Time), "sleep") => {
                let duration_nanos = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_duration()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                let duration = std::time::Duration::from_nanos(duration_nanos);
                std::thread::sleep(duration);
                // Return Ok(())
                Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![Value::Unit],
                })
            }

            // Duration instance methods
            (Value::Duration(nanos), "as_secs") => Ok(Value::Int((nanos / 1_000_000_000) as i64)),
            (Value::Duration(nanos), "as_millis") => Ok(Value::Int((nanos / 1_000_000) as i64)),
            (Value::Duration(nanos), "as_micros") => Ok(Value::Int((nanos / 1_000) as i64)),
            (Value::Duration(nanos), "as_nanos") => Ok(Value::Int(*nanos as i64)),
            (Value::Duration(nanos), "as_secs_f32") => {
                Ok(Value::Float(*nanos as f64 / 1_000_000_000.0))
            }
            (Value::Duration(nanos), "as_secs_f64") => {
                Ok(Value::Float(*nanos as f64 / 1_000_000_000.0))
            }

            // Instant instance methods
            (Value::Instant(instant), "duration_since") => {
                let other_instant = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_instant()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                let duration = instant.duration_since(other_instant);
                Ok(Value::Duration(duration.as_nanos() as u64))
            }
            (Value::Instant(instant), "elapsed") => {
                let duration = instant.elapsed();
                Ok(Value::Duration(duration.as_nanos() as u64))
            }

            // File instance methods
            (Value::File(f), "close") => {
                // If already closed, return silently (e.g., ensure after explicit close)
                if f.lock().unwrap().is_none() {
                    return Ok(Value::Unit);
                }
                let ptr = Arc::as_ptr(&f) as usize;
                if let Some(id) = self.resource_tracker.lookup_file_id(ptr) {
                    self.resource_tracker.mark_consumed(id)
                        .map_err(|msg| RuntimeError::Panic(msg))?;
                }
                let _ = f.lock().unwrap().take();
                Ok(Value::Unit)
            }
            (Value::File(f), "read_all") => {
                use std::io::Read;
                let mut file_opt = f.lock().unwrap();
                let file = file_opt.as_mut().ok_or_else(|| {
                    RuntimeError::TypeError("file is closed".to_string())
                })?;
                let mut content = String::new();
                match file.read_to_string(&mut content) {
                    Ok(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(content)))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::File(f), "write") => {
                use std::io::Write;
                let mut file_opt = f.lock().unwrap();
                let file = file_opt.as_mut().ok_or_else(|| {
                    RuntimeError::TypeError("file is closed".to_string())
                })?;
                let content = self.expect_string(&args, 0)?;
                match file.write_all(content.as_bytes()) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(e.to_string())))],
                    }),
                }
            }
            (Value::File(f), "lines") => {
                use std::io::{BufRead, BufReader};
                let file_opt = f.lock().unwrap();
                let file = file_opt.as_ref().ok_or_else(|| {
                    RuntimeError::TypeError("file is closed".to_string())
                })?;
                // Read all lines eagerly (file handle is borrowed, can't return iterator)
                let reader = BufReader::new(file);
                let lines: Vec<Value> = reader
                    .lines()
                    .filter_map(|r| r.ok())
                    .map(|l| Value::String(Arc::new(Mutex::new(l))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(lines))))
            }

            // Metadata methods
            (Value::Struct { name, fields, .. }, "size") if name == "Metadata" => {
                Ok(fields.get("size").cloned().unwrap_or(Value::Int(0)))
            }
            (Value::Struct { name, fields, .. }, "accessed") if name == "Metadata" => {
                Ok(fields.get("accessed").cloned().unwrap_or(Value::Int(0)))
            }
            (Value::Struct { name, fields, .. }, "modified") if name == "Metadata" => {
                Ok(fields.get("modified").cloned().unwrap_or(Value::Int(0)))
            }

            // Struct clone
            (Value::Struct { .. }, "clone") => Ok(receiver.clone()),

            // Result methods
            (Value::Enum { name, variant, fields }, "map_err")
                if name == "Result" =>
            {
                match variant.as_str() {
                    "Ok" => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: fields.clone(),
                    }),
                    "Err" => {
                        let closure = args.into_iter().next().ok_or_else(|| {
                            RuntimeError::ArityMismatch { expected: 1, got: 0 }
                        })?;
                        let err_val = fields.first().cloned().unwrap_or(Value::Unit);
                        let mapped = self.call_value(closure, vec![err_val])?;
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![mapped],
                        })
                    }
                    _ => Err(RuntimeError::TypeError("invalid Result variant".to_string())),
                }
            }
            (Value::Enum { name, variant, fields }, "map")
                if name == "Result" =>
            {
                match variant.as_str() {
                    "Ok" => {
                        let closure = args.into_iter().next().ok_or_else(|| {
                            RuntimeError::ArityMismatch { expected: 1, got: 0 }
                        })?;
                        let ok_val = fields.first().cloned().unwrap_or(Value::Unit);
                        let mapped = self.call_value(closure, vec![ok_val])?;
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![mapped],
                        })
                    }
                    "Err" => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: fields.clone(),
                    }),
                    _ => Err(RuntimeError::TypeError("invalid Result variant".to_string())),
                }
            }
            (Value::Enum { name, variant, fields }, "ok")
                if name == "Result" =>
            {
                match variant.as_str() {
                    "Ok" => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: fields.clone(),
                    }),
                    "Err" => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                    _ => Err(RuntimeError::TypeError("invalid Result variant".to_string())),
                }
            }
            (Value::Enum { name, variant, fields }, "unwrap_or")
                if name == "Result" =>
            {
                match variant.as_str() {
                    "Ok" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                    "Err" => Ok(args.into_iter().next().unwrap_or(Value::Unit)),
                    _ => Err(RuntimeError::TypeError("invalid Result variant".to_string())),
                }
            }
            (Value::Enum { name, variant, .. }, "is_ok")
                if name == "Result" =>
            {
                Ok(Value::Bool(variant == "Ok"))
            }
            (Value::Enum { name, variant, .. }, "is_err")
                if name == "Result" =>
            {
                Ok(Value::Bool(variant == "Err"))
            }
            (Value::Enum { name, variant, fields }, "unwrap")
                if name == "Result" =>
            {
                match variant.as_str() {
                    "Ok" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                    "Err" => Err(RuntimeError::Panic(format!(
                        "called unwrap on Err: {}",
                        fields.first().map(|v| format!("{}", v)).unwrap_or_default()
                    ))),
                    _ => Err(RuntimeError::TypeError("invalid Result variant".to_string())),
                }
            }

            // Option methods
            (Value::Enum { name, variant, fields }, "unwrap_or")
                if name == "Option" =>
            {
                match variant.as_str() {
                    "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                    "None" => Ok(args.into_iter().next().unwrap_or(Value::Unit)),
                    _ => Err(RuntimeError::TypeError("invalid Option variant".to_string())),
                }
            }
            (Value::Enum { name, variant, .. }, "is_some")
                if name == "Option" =>
            {
                Ok(Value::Bool(variant == "Some"))
            }
            (Value::Enum { name, variant, .. }, "is_none")
                if name == "Option" =>
            {
                Ok(Value::Bool(variant == "None"))
            }
            (Value::Enum { name, variant, fields }, "map")
                if name == "Option" =>
            {
                match variant.as_str() {
                    "Some" => {
                        let closure = args.into_iter().next().ok_or_else(|| {
                            RuntimeError::ArityMismatch { expected: 1, got: 0 }
                        })?;
                        let inner = fields.first().cloned().unwrap_or(Value::Unit);
                        let mapped = self.call_value(closure, vec![inner])?;
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![mapped],
                        })
                    }
                    "None" => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                    _ => Err(RuntimeError::TypeError("invalid Option variant".to_string())),
                }
            }
            (Value::Enum { name, variant, fields }, "unwrap")
                if name == "Option" =>
            {
                match variant.as_str() {
                    "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                    "None" => Err(RuntimeError::Panic("called unwrap on None".to_string())),
                    _ => Err(RuntimeError::TypeError("invalid Option variant".to_string())),
                }
            }

            // Vec clone
            (Value::Vec(v), "clone") => {
                let cloned = v.lock().unwrap().clone();
                Ok(Value::Vec(Arc::new(Mutex::new(cloned))))
            }

            // String ne (not-equal)
            (Value::String(a), "ne") => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*a.lock().unwrap() != b))
            }

            // ThreadHandle methods
            (Value::ThreadHandle(handle), "join") => {
                let jh = handle.handle.lock().unwrap().take();
                match jh {
                    Some(jh) => match jh.join() {
                        Ok(Ok(val)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![val],
                        }),
                        Ok(Err(msg)) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(msg)))],
                        }),
                        Err(_) => Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(
                                "thread panicked".to_string(),
                            )))],
                        }),
                    },
                    None => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "handle already joined".to_string(),
                        )))],
                    }),
                }
            }
            (Value::ThreadHandle(handle), "detach") => {
                let _ = handle.handle.lock().unwrap().take();
                Ok(Value::Unit)
            }

            // Sender methods
            (Value::Sender(tx), "send") => {
                let val = args.into_iter().next().unwrap_or(Value::Unit);
                let tx = tx.lock().unwrap();
                match tx.send(val) {
                    Ok(()) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Unit],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "channel closed".to_string(),
                        )))],
                    }),
                }
            }

            // Receiver methods
            (Value::Receiver(rx), "recv") => {
                let rx = rx.lock().unwrap();
                match rx.recv() {
                    Ok(val) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![val],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "channel closed".to_string(),
                        )))],
                    }),
                }
            }
            (Value::Receiver(rx), "try_recv") => {
                let rx = rx.lock().unwrap();
                match rx.try_recv() {
                    Ok(val) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![val],
                    }),
                    Err(mpsc::TryRecvError::Empty) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new("empty".to_string())))],
                    }),
                    Err(mpsc::TryRecvError::Disconnected) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(
                            "channel closed".to_string(),
                        )))],
                    }),
                }
            }

            // Channel constructor methods
            (Value::TypeConstructor(TypeConstructorKind::Channel), "buffered") => {
                let cap = self.expect_int(&args, 0)? as usize;
                let (tx, rx) = mpsc::sync_channel::<Value>(cap);
                let mut fields = HashMap::new();
                fields.insert("sender".to_string(), Value::Sender(Arc::new(Mutex::new(tx))));
                fields.insert("receiver".to_string(), Value::Receiver(Arc::new(Mutex::new(rx))));
                Ok(Value::Struct {
                    name: "ChannelPair".to_string(),
                    fields,
                    resource_id: None,
                })
            }
            (Value::TypeConstructor(TypeConstructorKind::Channel), "unbuffered") => {
                let (tx, rx) = mpsc::sync_channel::<Value>(0);
                let mut fields = HashMap::new();
                fields.insert("sender".to_string(), Value::Sender(Arc::new(Mutex::new(tx))));
                fields.insert("receiver".to_string(), Value::Receiver(Arc::new(Mutex::new(rx))));
                Ok(Value::Struct {
                    name: "ChannelPair".to_string(),
                    fields,
                    resource_id: None,
                })
            }

            // Generic to_string
            (_, "to_string") => {
                Ok(Value::String(Arc::new(Mutex::new(format!("{}", receiver)))))
            }

            // Check user-defined methods from extend blocks
            _ => {
                // Get the type name for looking up user methods
                let type_name = match &receiver {
                    Value::Struct { name, .. } => name.clone(),
                    Value::Enum { name, .. } => name.clone(),
                    _ => receiver.type_name().to_string(),
                };

                // Look up method in extend blocks
                if let Some(type_methods) = self.methods.get(&type_name) {
                    if let Some(method_fn) = type_methods.get(method).cloned() {
                        // Check if this method consumes self (take self)
                        let consumes_self = method_fn.params.first()
                            .map(|p| p.name == "self" && p.is_take)
                            .unwrap_or(false);
                        if consumes_self {
                            if let Some(id) = self.get_resource_id(&receiver) {
                                self.resource_tracker.mark_consumed(id)
                                    .map_err(|msg| RuntimeError::Panic(msg))?;
                            }
                        }

                        // Call user-defined method with self as first argument
                        let mut all_args = vec![receiver];
                        all_args.extend(args);
                        return self.call_function(&method_fn, all_args);
                    }
                }

                Err(RuntimeError::NoSuchMethod {
                    ty: type_name,
                    method: method.to_string(),
                })
            }
        }
    }

    /// Call a static method on a type (e.g., Instant.now(), Duration.seconds(5)).
    fn call_type_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match (type_name, method) {
            // Instant.now() -> Instant
            ("Instant", "now") => {
                if !args.is_empty() {
                    return Err(RuntimeError::ArityMismatch {
                        expected: 0,
                        got: args.len(),
                    });
                }
                Ok(Value::Instant(std::time::Instant::now()))
            }

            // Duration.seconds(n: u64) -> Duration
            ("Duration", "seconds") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000_000_000))
            }

            // Duration.millis(n: u64) -> Duration
            ("Duration", "millis") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000_000))
            }

            // Duration.micros(n: u64) -> Duration
            ("Duration", "micros") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000))
            }

            // Duration.nanos(n: u64) -> Duration
            ("Duration", "nanos") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n))
            }

            // Duration.from_secs_f64(secs: f64) -> Duration
            ("Duration", "from_secs_f64") => {
                let secs = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_f64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                let nanos = (secs * 1_000_000_000.0) as u64;
                Ok(Value::Duration(nanos))
            }

            // Duration.from_millis(n: u64) -> Duration (alias for millis)
            ("Duration", "from_millis") => {
                let n = args.first()
                    .ok_or_else(|| RuntimeError::ArityMismatch { expected: 1, got: 0 })?
                    .as_u64()
                    .map_err(|e| RuntimeError::TypeError(e))?;
                Ok(Value::Duration(n * 1_000_000))
            }

            _ => Err(RuntimeError::TypeError(format!(
                "type {} has no method '{}'",
                type_name, method
            ))),
        }
    }

    /// Helper to extract an integer from args.
    fn expect_int(&self, args: &[Value], idx: usize) -> Result<i64, RuntimeError> {
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
    fn expect_float(&self, args: &[Value], idx: usize) -> Result<f64, RuntimeError> {
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
    fn expect_bool(&self, args: &[Value], idx: usize) -> Result<bool, RuntimeError> {
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
    fn expect_string(&self, args: &[Value], idx: usize) -> Result<String, RuntimeError> {
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
    fn expect_char(&self, args: &[Value], idx: usize) -> Result<char, RuntimeError> {
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

    /// Error propagation via ? operator
    #[error("try error")]
    TryError(Value),
}

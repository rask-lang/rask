// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use rask_ast::decl::{Decl, DeclKind, EnumDecl, Field, FnDecl, StructDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_types::GenericArg;

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
        let mut entry_fn: Option<FnDecl> = None;
        let mut imports: Vec<(String, ModuleKind)> = Vec::new();

        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
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
                    let type_methods = self.methods.entry(impl_decl.target_ty.clone()).or_default();
                    for method in &impl_decl.methods {
                        type_methods.insert(method.name.clone(), method.clone());
                    }
                }
                DeclKind::Import(import) => {
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
                            "net" => Some(ModuleKind::Net),
                            _ => None,
                        };
                        if let Some(kind) = module_kind {
                            imports.push((alias, kind));
                        }
                    }
                }
                DeclKind::Struct(s) => {
                    self.struct_decls.insert(s.name.clone(), s.clone());
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

        use rask_ast::decl::{EnumDecl, Field, Variant};
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

        // Fall back to func main() if no @entry attribute found
        let entry = entry_fn.or_else(|| self.functions.get("main").cloned());

        if let Some(entry) = entry {
            self.call_function(&entry, vec![])
        } else {
            Err(RuntimeError::NoEntryPoint)
        }
    }

    /// Parse a generic struct name like "Buffer<i32, 256>" and monomorphize it.
    fn monomorphize_struct_from_name(&mut self, full_name: &str) -> Result<String, RuntimeError> {
        // Extract base name and args from "Buffer<i32, 256>"
        let lt_pos = full_name.find('<').ok_or_else(|| {
            RuntimeError::Generic(format!("Expected generic arguments in '{}'", full_name))
        })?;

        let base_name = &full_name[..lt_pos];
        let args_str = &full_name[lt_pos + 1..full_name.len() - 1]; // Remove < and >

        // Parse generic arguments
        let args = self.parse_generic_args(args_str)?;

        // Monomorphize
        self.monomorphize_struct(base_name, &args)
    }

    /// Parse generic arguments from a string like "i32, 256".
    fn parse_generic_args(&self, args_str: &str) -> Result<Vec<GenericArg>, RuntimeError> {
        let mut args = Vec::new();
        let parts: Vec<&str> = args_str.split(',').map(|s| s.trim()).collect();

        for part in parts {
            // Try to parse as usize (const generic)
            if let Ok(n) = part.parse::<usize>() {
                args.push(GenericArg::ConstUsize(n));
            } else {
                // It's a type - for now just store as Type with a placeholder
                // In a real implementation, we'd parse the type properly
                use rask_types::Type;
                let ty = match part {
                    "i32" => Type::I32,
                    "i64" => Type::I64,
                    "f32" => Type::F32,
                    "f64" => Type::F64,
                    "bool" => Type::Bool,
                    "string" => Type::String,
                    "usize" => Type::U64, // Map usize to u64 for now
                    _ => Type::UnresolvedNamed(part.to_string()),
                };
                args.push(GenericArg::Type(Box::new(ty)));
            }
        }

        Ok(args)
    }

    /// Monomorphize a generic struct by substituting type and const parameters.
    /// Returns the monomorphized struct name (e.g., "Buffer<i32, 256>").
    fn monomorphize_struct(&mut self, base_name: &str, args: &[GenericArg]) -> Result<String, RuntimeError> {
        // Build the full instantiated name
        let mut full_name = base_name.to_string();
        full_name.push('<');
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                full_name.push_str(", ");
            }
            match arg {
                GenericArg::Type(ty) => full_name.push_str(&ty.to_string()),
                GenericArg::ConstUsize(n) => full_name.push_str(&n.to_string()),
            }
        }
        full_name.push('>');

        // Check if already monomorphized
        if self.monomorphized_structs.contains_key(&full_name) {
            return Ok(full_name);
        }

        // Get the generic struct declaration
        // Look for a struct whose name is either exactly base_name or starts with "base_name<"
        let base_decl = self.struct_decls.get(base_name)
            .or_else(|| {
                // Search for generic version: "Pair<T, U>"
                self.struct_decls.iter()
                    .find(|(k, _)| k.starts_with(base_name) && k.contains('<'))
                    .map(|(_, v)| v)
            })
            .ok_or_else(|| RuntimeError::UndefinedVariable(base_name.to_string()))?
            .clone();

        // Verify argument count matches parameter count
        if base_decl.type_params.len() != args.len() {
            return Err(RuntimeError::Generic(format!(
                "Wrong number of generic arguments for {}: expected {}, got {}",
                base_name, base_decl.type_params.len(), args.len()
            )));
        }

        // Build substitution map: param name -> GenericArg
        let mut subst_map = HashMap::new();
        for (param, arg) in base_decl.type_params.iter().zip(args.iter()) {
            subst_map.insert(param.name.clone(), arg.clone());
        }

        // Substitute in field types
        let mut new_fields = Vec::new();
        for field in &base_decl.fields {
            let new_ty = self.substitute_type(&field.ty, &subst_map)?;
            new_fields.push(Field {
                name: field.name.clone(),
                ty: new_ty,
                is_pub: field.is_pub,
            });
        }

        // Create monomorphized struct declaration
        let mono_decl = StructDecl {
            name: full_name.clone(),
            type_params: vec![], // Monomorphized structs have no params
            fields: new_fields,
            methods: base_decl.methods.clone(),
            is_pub: base_decl.is_pub,
            attrs: base_decl.attrs.clone(),
        };

        // Cache it
        self.monomorphized_structs.insert(full_name.clone(), mono_decl.clone());
        self.struct_decls.insert(full_name.clone(), mono_decl);

        Ok(full_name)
    }

    /// Substitute type parameters in a type string.
    fn substitute_type(&self, ty: &str, subst_map: &HashMap<String, GenericArg>) -> Result<String, RuntimeError> {
        // Handle array types [T; N]
        if ty.starts_with('[') && ty.ends_with(']') {
            if let Some(semi_pos) = ty.find(';') {
                let elem_ty = ty[1..semi_pos].trim();
                let size_expr = ty[semi_pos + 1..ty.len() - 1].trim();

                let new_elem = self.substitute_type(elem_ty, subst_map)?;
                let new_size = if let Some(arg) = subst_map.get(size_expr) {
                    match arg {
                        GenericArg::ConstUsize(n) => n.to_string(),
                        GenericArg::Type(_) => {
                            return Err(RuntimeError::Generic(format!(
                                "Expected const value for array size, got type"
                            )));
                        }
                    }
                } else {
                    size_expr.to_string()
                };

                return Ok(format!("[{}; {}]", new_elem, new_size));
            }
        }

        // Simple type parameter substitution
        if let Some(arg) = subst_map.get(ty) {
            match arg {
                GenericArg::Type(t) => Ok(t.to_string()),
                GenericArg::ConstUsize(n) => Ok(n.to_string()),
            }
        } else {
            Ok(ty.to_string())
        }
    }

    pub(crate) fn call_function(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if args.len() != func.params.len() {
            return Err(RuntimeError::ArityMismatch {
                expected: func.params.len(),
                got: args.len(),
            });
        }

        self.env.push_scope();

        for (param, arg) in func.params.iter().zip(args.into_iter()) {
            if let Some(proj_start) = param.ty.find(".{") {
                let proj_fields_str = &param.ty[proj_start + 2..param.ty.len() - 1];
                let proj_fields: Vec<&str> = proj_fields_str.split(',').map(|s| s.trim()).collect();
                if proj_fields.len() == 1 && param.name == proj_fields[0] {
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

        let result = self.exec_stmts(&func.body);

        let scope_depth = self.env.scope_depth();
        let caller_depth = scope_depth.saturating_sub(1);
        match &result {
            Err(RuntimeError::Return(v)) | Ok(v) => {
                self.transfer_resource_to_scope(v, caller_depth);
            }
            Err(RuntimeError::TryError(v)) => {
                self.transfer_resource_to_scope(v, caller_depth);
            }
            _ => {}
        }

        if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
            self.env.pop_scope();
            return Err(RuntimeError::Panic(msg));
        }

        self.env.pop_scope();

        let value = match result {
            Ok(_) => Value::Unit,
            Err(RuntimeError::Return(v)) => v,
            Err(RuntimeError::TryError(v)) => v,
            Err(e) => return Err(e),
        };

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

    /// Runs ensure blocks in LIFO order on block exit.
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

        let ensure_fatal = self.run_ensures(&ensures);

        if let Some(e) = exit_error {
            Err(e)
        } else if let Some(fatal) = ensure_fatal {
            Err(fatal)
        } else {
            Ok(last_value)
        }
    }

    /// Returns fatal error (Panic/Exit) if one occurs; non-fatal errors passed to catch handlers.
    fn run_ensures(&mut self, ensures: &[&Stmt]) -> Option<RuntimeError> {
        for ensure_stmt in ensures.iter().rev() {
            if let StmtKind::Ensure { body, catch } = &ensure_stmt.kind {
                let result = self.exec_ensure_body(body);

                match result {
                    Ok(value) => {
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
                        self.handle_ensure_error(val, catch);
                    }
                    Err(_) => {}
                }
            }
        }
        None
    }

    fn exec_ensure_body(&mut self, body: &[Stmt]) -> Result<Value, RuntimeError> {
        let mut last_value = Value::Unit;
        for stmt in body {
            last_value = self.exec_stmt(stmt)?;
        }
        Ok(last_value)
    }

    fn handle_ensure_error(&mut self, error_value: Value, catch: &Option<(String, Vec<Stmt>)>) {
        if let Some((name, handler)) = catch {
            self.env.push_scope();
            self.env.define(name.clone(), error_value);
            let _ = self.exec_ensure_body(handler);
            self.env.pop_scope();
        }
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, RuntimeError> {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.eval_expr(expr),

            StmtKind::Const { name, init, .. } => {
                let value = self.eval_expr(init)?;
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::Let { name, init, .. } => {
                let value = self.eval_expr(init)?;
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::LetTuple { names, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple(names, value)?;
                Ok(Value::Unit)
            }

            StmtKind::ConstTuple { names, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple(names, value)?;
                Ok(Value::Unit)
            }

            StmtKind::Assign { target, value } => {
                let val = self.eval_expr(value)?;
                self.assign_target(target, val)?;
                Ok(Value::Unit)
            }

            StmtKind::Return(expr) => {
                let value = if let Some(e) = expr {
                    self.eval_expr(e)?
                } else {
                    Value::Unit
                };
                Err(RuntimeError::Return(value))
            }

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

            StmtKind::WhileLet {
                pattern,
                expr,
                body,
            } => {
                loop {
                    let value = self.eval_expr(expr)?;

                    if let Some(bindings) = self.match_pattern(pattern, &value) {
                        self.env.push_scope();
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
                        break;
                    }
                }
                Ok(Value::Unit)
            }

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

            StmtKind::Break(_) => Err(RuntimeError::Break),

            StmtKind::Continue(_) => Err(RuntimeError::Continue),

            StmtKind::For {
                binding,
                iter,
                body,
                ..
            } => {
                let iter_val = self.eval_expr(iter)?;

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
                    Value::Pool(p) => {
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
                        drop(pool);

                        for handle in handles {
                            self.env.push_scope();
                            self.env.define(binding.clone(), handle);
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

            StmtKind::Ensure { .. } => Ok(Value::Unit),

            _ => Ok(Value::Unit),
        }
    }

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

    fn assign_nested_field(obj: &mut Value, field_chain: &[String], value: Value) -> Result<(), RuntimeError> {
        if field_chain.is_empty() {
            *obj = value;
            return Ok(());
        }
        let mut current = obj;
        for (i, field) in field_chain.iter().enumerate() {
            if i == field_chain.len() - 1 {
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
            ExprKind::Field { .. } => {
                let mut field_chain = Vec::new();
                let mut current = target;
                while let ExprKind::Field { object, field: f } = &current.kind {
                    field_chain.push(f.clone());
                    current = object;
                }
                field_chain.reverse();

                match &current.kind {
                    ExprKind::Ident(var_name) => {
                        if let Some(obj) = self.env.get_mut(var_name) {
                            Self::assign_nested_field(obj, &field_chain, value)
                        } else {
                            Err(RuntimeError::UndefinedVariable(var_name.clone()))
                        }
                    }
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
                            Err(RuntimeError::TypeError("complex nested assignment not yet supported".to_string()))
                        }
                    }
                    _ => Err(RuntimeError::TypeError("unsupported assignment target".to_string())),
                }
            }
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

    pub(crate) fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            ExprKind::Int(n, suffix) => {
                use rask_ast::token::IntSuffix;
                match suffix {
                    Some(IntSuffix::I128) => Ok(Value::Int128(*n as i128)),
                    Some(IntSuffix::U128) => Ok(Value::Uint128(*n as u128)),
                    _ => Ok(Value::Int(*n)),
                }
            }
            ExprKind::Float(n, _) => Ok(Value::Float(*n)),
            ExprKind::String(s) => {
                if s.contains('{') {
                    let interpolated = self.interpolate_string(s)?;
                    Ok(Value::String(Arc::new(Mutex::new(interpolated))))
                } else {
                    Ok(Value::String(Arc::new(Mutex::new(s.clone()))))
                }
            }
            ExprKind::Char(c) => Ok(Value::Char(*c)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),

            ExprKind::Ident(name) => {
                if let Some(val) = self.env.get(name) {
                    return Ok(val.clone());
                }
                if self.functions.contains_key(name) {
                    return Ok(Value::Function { name: name.clone() });
                }
                match name.as_str() {
                    "Vec" => return Ok(Value::TypeConstructor(TypeConstructorKind::Vec)),
                    "Map" => return Ok(Value::TypeConstructor(TypeConstructorKind::Map)),
                    "string" => return Ok(Value::TypeConstructor(TypeConstructorKind::String)),
                    "Pool" => return Ok(Value::TypeConstructor(TypeConstructorKind::Pool)),
                    "Channel" => return Ok(Value::TypeConstructor(TypeConstructorKind::Channel)),
                    "Shared" => return Ok(Value::TypeConstructor(TypeConstructorKind::Shared)),
                    "Atomic" => return Ok(Value::TypeConstructor(TypeConstructorKind::Atomic)),
                    "Ordering" => return Ok(Value::TypeConstructor(TypeConstructorKind::Ordering)),
                    _ => {}
                }
                Err(RuntimeError::UndefinedVariable(name.clone()))
            }

            ExprKind::Call { func, args } => {
                if let ExprKind::OptionalField { object, field } = &func.kind {
                    let obj_val = self.eval_expr(object)?;
                    let arg_vals: Vec<Value> = args
                        .iter()
                        .map(|a| self.eval_expr(a))
                        .collect::<Result<_, _>>()?;

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

                    return self.call_method(obj_val, field, arg_vals);
                }

                let func_val = self.eval_expr(func)?;
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a))
                    .collect::<Result<_, _>>()?;
                self.call_value(func_val, arg_vals)
            }

            ExprKind::MethodCall {
                object,
                method,
                type_args,
                args,
            } => {
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

                    if let Some(type_methods) = self.methods.get(name).cloned() {
                        if let Some(method_fn) = type_methods.get(method) {
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

                let receiver = self.eval_expr(object)?;
                let mut arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a))
                    .collect::<Result<_, _>>()?;

                // Inject type_args for generic methods (e.g. json.decode<T>)
                if let Some(ta) = type_args {
                    if let Some(first_type) = ta.first() {
                        if let Value::Module(ModuleKind::Json) = &receiver {
                            if method == "decode" || method == "from_value" {
                                arg_vals.insert(
                                    0,
                                    Value::String(Arc::new(Mutex::new(first_type.clone()))),
                                );
                            }
                        }
                    }
                }

                if let Value::Type(type_name) = &receiver {
                    return self.call_type_method(type_name, method, arg_vals);
                }

                self.call_method(receiver, method, arg_vals)
            }

            ExprKind::Binary { op, left, right } => match op {
                BinOp::And => {
                    let l = self.eval_expr(left)?;
                    if !self.is_truthy(&l) {
                        Ok(Value::Bool(false))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(self.is_truthy(&r)))
                    }
                }
                BinOp::Or => {
                    let l = self.eval_expr(left)?;
                    if self.is_truthy(&l) {
                        Ok(Value::Bool(true))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(self.is_truthy(&r)))
                    }
                }
                _ => {
                    Err(RuntimeError::TypeError(format!(
                        "unexpected binary op {:?} - should be desugared to method call",
                        op
                    )))
                }
            },

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

            ExprKind::Block(stmts) => {
                self.env.push_scope();
                let result = self.exec_stmts(stmts);
                self.env.pop_scope();
                result
            }

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
                    0
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
                    i64::MAX
                };
                Ok(Value::Range {
                    start: start_val,
                    end: end_val,
                    inclusive: *inclusive,
                })
            }

            ExprKind::StructLit { name, fields, spread } => {
                // Check if this is a generic instantiation
                let concrete_name = if name.contains('<') {
                    // Parse generic arguments and monomorphize
                    self.monomorphize_struct_from_name(name)?
                } else {
                    name.clone()
                };

                let mut field_values = HashMap::new();

                if let Some(spread_expr) = spread {
                    if let Value::Struct {
                        fields: base_fields,
                        ..
                    } = self.eval_expr(spread_expr)?
                    {
                        field_values.extend(base_fields);
                    }
                }

                for field in fields {
                    let value = self.eval_expr(&field.value)?;
                    field_values.insert(field.name.clone(), value);
                }

                let resource_id = if self.is_resource_type(&concrete_name) {
                    Some(self.resource_tracker.register(&concrete_name, self.env.scope_depth()))
                } else {
                    None
                };

                Ok(Value::Struct {
                    name: concrete_name,
                    fields: field_values,
                    resource_id,
                })
            }

            ExprKind::Field { object, field } => {
                if let ExprKind::Ident(enum_name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(enum_name).cloned() {
                        if let Some(variant) =
                            enum_decl.variants.iter().find(|v| &v.name == field)
                        {
                            let field_count = variant.fields.len();
                            if field_count == 0 {
                                return Ok(Value::Enum {
                                    name: enum_name.clone(),
                                    variant: field.clone(),
                                    fields: vec![],
                                });
                            } else {
                                return Ok(Value::EnumConstructor {
                                    enum_name: enum_name.clone(),
                                    variant_name: field.clone(),
                                    field_count,
                                });
                            }
                        }
                    }
                }

                let obj = self.eval_expr(object)?;
                match obj {
                    Value::Struct { fields, .. } => {
                        Ok(fields.get(field).cloned().unwrap_or(Value::Unit))
                    }
                    Value::Module(ModuleKind::Time) => {
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
                        self.get_math_field(field)
                    }
                    Value::Module(ModuleKind::Path) => {
                        match field.as_str() {
                            "Path" => Ok(Value::Type("Path".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "path module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Random) => {
                        match field.as_str() {
                            "Rng" => Ok(Value::Type("Rng".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "random module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Json) => {
                        match field.as_str() {
                            "JsonValue" => Ok(Value::Type("JsonValue".to_string())),
                            _ => Err(RuntimeError::TypeError(format!(
                                "json module has no member '{}'",
                                field
                            ))),
                        }
                    }
                    Value::Module(ModuleKind::Cli) => {
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
                    (Value::String(s), Value::Range { start, end, inclusive }) => {
                        let str_val = s.lock().unwrap();
                        let len = str_val.len() as i64;
                        let start_idx = (*start).max(0).min(len) as usize;
                        let end_idx = if *end == i64::MAX {
                            str_val.len()
                        } else {
                            let e = if *inclusive { *end + 1 } else { *end };
                            e.max(0).min(len) as usize
                        };
                        let slice = &str_val[start_idx..end_idx];
                        Ok(Value::String(Arc::new(Mutex::new(slice.to_string()))))
                    }
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

            ExprKind::Array(elements) => {
                let values: Vec<Value> = elements
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            ExprKind::Tuple(elements) => {
                let values: Vec<Value> = elements
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;

                for arm in arms {
                    if let Some(bindings) = self.match_pattern(&arm.pattern, &value) {
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

                        self.env.push_scope();
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        let result = self.eval_expr(&arm.body);
                        self.env.pop_scope();
                        return result;
                    }
                }

                Err(RuntimeError::NoMatchingArm)
            }

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

            ExprKind::Closure { params, body } => {
                let captured = self.env.capture();
                Ok(Value::Closure {
                    params: params.iter().map(|p| p.name.clone()).collect(),
                    body: (**body).clone(),
                    captured_env: captured,
                })
            }

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
                    // i128 conversions
                    (Value::Int(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Int(n), "u128") => Ok(Value::Uint128(n as u128)),
                    (Value::Int128(n), "i64" | "i32" | "int" | "i16" | "i8") => Ok(Value::Int(n as i64)),
                    (Value::Int128(n), "u64" | "u32" | "u16" | "u8" | "usize" | "u128") => Ok(Value::Uint128(n as u128)),
                    (Value::Int128(n), "f64" | "f32" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Int128(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    // u128 conversions
                    (Value::Uint128(n), "i64" | "i32" | "int" | "i16" | "i8") => Ok(Value::Int(n as i64)),
                    (Value::Uint128(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Uint128(n), "f64" | "f32" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Uint128(n), "u128" | "u64" | "u32" | "u16" | "u8" | "usize") => Ok(Value::Uint128(n)),
                    (Value::Uint128(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Float(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Float(n), "u128") => Ok(Value::Uint128(n as u128)),
                    (v, _) => Ok(v),
                }
            }

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

                let join_handle = std::thread::spawn(move || {
                    result_rx
                        .recv()
                        .unwrap_or(Err("thread pool task dropped".to_string()))
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                })))
            }

            ExprKind::WithBlock { name, args, body }
                if name == "threading" || name == "multitasking" =>
            {
                let num_threads = if args.is_empty() {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(4)
                } else {
                    self.eval_expr(&args[0])?.as_int()
                        .map_err(|e| RuntimeError::TypeError(e))? as usize
                };

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
                                Err(_) => break,
                            }
                        }
                    }));
                }

                let pool = Arc::new(ThreadPoolInner {
                    sender: Mutex::new(Some(tx)),
                    workers: Mutex::new(Vec::new()),
                    size: num_threads,
                });

                self.env.push_scope();
                self.env.define("__thread_pool".to_string(), Value::ThreadPool(pool.clone()));

                let mut result = Value::Unit;
                for stmt in body {
                    match self.exec_stmt(stmt) {
                        Ok(val) => result = val,
                        Err(e) => {
                            *pool.sender.lock().unwrap() = None;
                            for w in workers {
                                let _ = w.join();
                            }
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                }

                *pool.sender.lock().unwrap() = None;
                for w in workers {
                    let _ = w.join();
                }
                self.env.pop_scope();
                Ok(result)
            }

            ExprKind::Spawn { body } => {
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

            _ => Ok(Value::Unit),
        }
    }

    fn match_pattern(&self, pattern: &Pattern, value: &Value) -> Option<HashMap<String, Value>> {
        match pattern {
            Pattern::Wildcard => Some(HashMap::new()),

            Pattern::Ident(name) => {
                if let Value::Enum {
                    variant,
                    fields,
                    ..
                } = value
                {
                    let is_unit_variant = self.enums.values().any(|e| {
                        e.variants.iter().any(|v| v.name == *name && v.fields.is_empty())
                    });
                    if is_unit_variant {
                        if variant == name && fields.is_empty() {
                            return Some(HashMap::new());
                        } else {
                            return None;
                        }
                    }
                }
                let mut bindings = HashMap::new();
                bindings.insert(name.clone(), value.clone());
                Some(bindings)
            }

            Pattern::Literal(lit_expr) => {
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

    fn values_equal(&self, value: &Value, lit_expr: &Expr) -> bool {
        match (&value, &lit_expr.kind) {
            (Value::Int(a), ExprKind::Int(b, _)) => *a == *b,
            (Value::Int128(a), ExprKind::Int(b, _)) => *a == *b as i128,
            (Value::Uint128(a), ExprKind::Int(b, _)) => *a == *b as u128,
            (Value::Float(a), ExprKind::Float(b, _)) => *a == *b,
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

    /// Compare two runtime values for ordering.
    /// Returns None if the values are not comparable.
    pub(crate) fn value_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        match (a, b) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::String(a), Value::String(b)) => {
                Some(a.lock().unwrap().cmp(&*b.lock().unwrap()))
            }
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)), // false < true
            (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
            _ => None, // Other types are not comparable
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
                for (name, val) in captured_env {
                    self.env.define(name, val);
                }
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
                if chars.peek() == Some(&'{') {
                    chars.next();
                    result.push('{');
                    continue;
                }
                let mut spec_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        break;
                    }
                    spec_str.push(chars.next().unwrap());
                }
                let (arg_id, fmt_spec) = if let Some(colon_pos) = spec_str.find(':') {
                    let id_part = &spec_str[..colon_pos];
                    let spec_part = &spec_str[colon_pos + 1..];
                    (id_part.to_string(), Some(spec_part.to_string()))
                } else {
                    (spec_str, None)
                };

                let value = if arg_id.is_empty() {
                    if arg_index < args.len() {
                        let v = args[arg_index].clone();
                        arg_index += 1;
                        v
                    } else {
                        return Err(RuntimeError::TypeError(format!(
                            "format() not enough arguments (expected at least {})",
                            arg_index + 1
                        )));
                    }
                } else if let Ok(idx) = arg_id.parse::<usize>() {
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
                    self.resolve_named_placeholder(&arg_id)?
                };

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

    fn apply_format_spec(&self, value: &Value, spec: &str) -> Result<String, RuntimeError> {
        let mut fill = ' ';
        let mut align = None;
        let mut width = 0usize;
        let mut precision = None;
        let mut format_type = ' ';

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

        let mut width_str = String::new();
        while pos < spec_chars.len() && spec_chars[pos].is_ascii_digit() {
            width_str.push(spec_chars[pos]);
            pos += 1;
        }
        if !width_str.is_empty() {
            width = width_str.parse().unwrap_or(0);
        }

        if pos < spec_chars.len() && spec_chars[pos] == '.' {
            pos += 1;
            let mut prec_str = String::new();
            while pos < spec_chars.len() && spec_chars[pos].is_ascii_digit() {
                prec_str.push(spec_chars[pos]);
                pos += 1;
            }
            precision = Some(prec_str.parse::<usize>().unwrap_or(0));
        }

        if pos < spec_chars.len() {
            format_type = spec_chars[pos];
        }

        let formatted = match format_type {
            '?' => {
                self.debug_format(value)
            }
            'x' => {
                match value {
                    Value::Int(n) => format!("{:x}", n),
                    _ => format!("{}", value),
                }
            }
            'X' => {
                match value {
                    Value::Int(n) => format!("{:X}", n),
                    _ => format!("{}", value),
                }
            }
            'b' => {
                match value {
                    Value::Int(n) => format!("{:b}", n),
                    _ => format!("{}", value),
                }
            }
            'o' => {
                match value {
                    Value::Int(n) => format!("{:o}", n),
                    _ => format!("{}", value),
                }
            }
            'e' => {
                match value {
                    Value::Float(n) => format!("{:e}", n),
                    Value::Int(n) => format!("{:e}", *n as f64),
                    _ => format!("{}", value),
                }
            }
            _ => {
                match precision {
                    Some(prec) => match value {
                        Value::Float(n) => format!("{:.prec$}", n, prec = prec),
                        _ => format!("{}", value),
                    },
                    None => format!("{}", value),
                }
            }
        };

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
            _ => format!("{}", value),
        }
    }

    fn interpolate_string(&self, s: &str) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                if chars.peek() == Some(&'{') {
                    result.push('{');
                    result.push('{');
                    chars.next();
                    continue;
                }
                let mut expr_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        break;
                    }
                    expr_str.push(chars.next().unwrap());
                }
                if expr_str.is_empty() || expr_str.starts_with(':') {
                    result.push('{');
                    result.push_str(&expr_str);
                    result.push('}');
                    continue;
                }
                let (expr_part, fmt_spec) = if let Some(colon_pos) = expr_str.find(':') {
                    (&expr_str[..colon_pos], Some(&expr_str[colon_pos..]))
                } else {
                    (expr_str.as_str(), None)
                };
                let value = self.eval_interpolation_expr(expr_part)?;
                if let Some(spec) = fmt_spec {
                    result.push_str(&Self::format_value_with_spec(&value, spec));
                } else {
                    result.push_str(&format!("{}", value));
                }
            } else if c == '}' && chars.peek() == Some(&'}') {
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
        if let Ok(n) = expr.parse::<i64>() {
            return Ok(Value::Int(n));
        }
        if let Ok(f) = expr.parse::<f64>() {
            return Ok(Value::Float(f));
        }
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

    fn call_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match &receiver {
            Value::Module(module) => self.call_module_method(module, method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::File(f) => self.call_file_method(f, method, args),
            Value::Duration(nanos) => self.call_duration_method(*nanos, method),
            Value::Instant(instant) => self.call_instant_method(instant, method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::Struct { name, fields, .. } if name == "Metadata" => {
                self.call_metadata_method(fields, method)
            }
            Value::Struct { name, fields, .. } if name == "Path" => {
                self.call_path_instance_method(fields, method, args)
            }
            Value::Struct { name, fields, .. } if name == "Args" => {
                self.call_args_method(fields, method, args)
            }
            Value::Enum { name, variant, fields } if name == "JsonValue" => {
                self.call_json_value_method(variant, fields, method)
            }
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

    #[error("no entry point found (add func main() or use @entry)")]
    NoEntryPoint,

    #[error("generic error: {0}")]
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
}

//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::env::Environment;
use crate::value::{BuiltinKind, ModuleKind, TypeConstructorKind, Value};

/// The tree-walk interpreter.
pub struct Interpreter {
    /// Variable bindings (scoped).
    env: Environment,
    /// Function declarations by name.
    functions: HashMap<String, FnDecl>,
    /// Enum declarations by name.
    enums: HashMap<String, EnumDecl>,
    /// Methods from extend blocks (type_name -> method_name -> FnDecl).
    methods: HashMap<String, HashMap<String, FnDecl>>,
    /// Optional output buffer for capturing stdout (used in tests).
    output_buffer: Option<Rc<RefCell<String>>>,
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
            methods: HashMap::new(),
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
            methods: HashMap::new(),
            output_buffer: None,
            cli_args: args,
        }
    }

    /// Create an interpreter with output capture enabled.
    /// Returns the interpreter and a reference to the output buffer.
    pub fn with_captured_output() -> (Self, Rc<RefCell<String>>) {
        let buffer = Rc::new(RefCell::new(String::new()));
        let interp = Self {
            env: Environment::new(),
            functions: HashMap::new(),
            enums: HashMap::new(),
            methods: HashMap::new(),
            output_buffer: Some(buffer.clone()),
            cli_args: vec![],
        };
        (interp, buffer)
    }

    /// Write to output (buffer or stdout).
    fn write_output(&self, s: &str) {
        if let Some(buf) = &self.output_buffer {
            buf.borrow_mut().push_str(s);
        } else {
            print!("{}", s);
        }
    }

    /// Write a newline to output (buffer or stdout).
    fn write_output_ln(&self) {
        if let Some(buf) = &self.output_buffer {
            buf.borrow_mut().push('\n');
        } else {
            println!();
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
                            _ => None, // Unknown module, ignore for now
                        };
                        if let Some(kind) = module_kind {
                            imports.push((alias, kind));
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

        // Execute function body
        let result = self.exec_stmts(&func.body);

        // Pop function scope
        self.env.pop_scope();

        // Handle return value:
        // - Ok(()) means function completed normally → return Unit
        // - Err(Return(v)) means explicit return → return v
        // - Err(TryError(v)) means ? propagated error → return Err value
        // - Err(other) means actual error → propagate
        match result {
            Ok(value) => Ok(value),
            Err(RuntimeError::Return(v)) => Ok(v),
            Err(RuntimeError::TryError(v)) => Ok(v), // Return the Err value
            Err(e) => Err(e),
        }
    }

    /// Execute a list of statements, returning the value of the last expression.
    fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<Value, RuntimeError> {
        let mut last_value = Value::Unit;

        for stmt in stmts {
            last_value = self.exec_stmt(stmt)?;
        }

        Ok(last_value)
    }

    /// Execute a single statement.
    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, RuntimeError> {
        match &stmt.kind {
            // Expression statement - evaluate and return the value
            StmtKind::Expr(expr) => self.eval_expr(expr),

            // Const binding (immutable) - evaluate init and bind
            StmtKind::Const { name, init, .. } => {
                let value = self.eval_expr(init)?;
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            // Let binding (mutable) - same as const for now (mutability checked earlier)
            StmtKind::Let { name, init, .. } => {
                let value = self.eval_expr(init)?;
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
                        let items: Vec<Value> = v.borrow().clone();
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

            // Ensure block (deferred cleanup - in interpreter, resources are cleaned up
            // automatically via Rc drop, so this is a no-op)
            StmtKind::Ensure(_stmts) => Ok(Value::Unit),

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
                } else {
                    Err(RuntimeError::TypeError(
                        "nested field assignment not yet supported".to_string(),
                    ))
                }
            }
            // Index assignment: vec[i] = value
            ExprKind::Index { object, index } => {
                let idx = self.eval_expr(index)?;
                if let ExprKind::Ident(var_name) = &object.kind {
                    // Get the value - for Vec (Rc<RefCell>), we can modify through the shared reference
                    if let Some(obj) = self.env.get(var_name).cloned() {
                        match obj {
                            Value::Vec(v) => {
                                if let Value::Int(i) = idx {
                                    let i = i as usize;
                                    let mut vec = v.borrow_mut();
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
                    Ok(Value::String(Rc::new(RefCell::new(interpolated))))
                } else {
                    Ok(Value::String(Rc::new(RefCell::new(s.clone()))))
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

                Ok(Value::Struct {
                    name: name.clone(),
                    fields: field_values,
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

                // Fall through to struct field access
                let obj = self.eval_expr(object)?;
                match obj {
                    Value::Struct { fields, .. } => {
                        Ok(fields.get(field).cloned().unwrap_or(Value::Unit))
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
                        let vec = v.borrow();
                        Ok(vec.get(*i as usize).cloned().unwrap_or(Value::Unit))
                    }
                    (Value::String(s), Value::Int(i)) => Ok(s
                        .borrow()
                        .chars()
                        .nth(*i as usize)
                        .map(Value::Char)
                        .unwrap_or(Value::Unit)),
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
                Ok(Value::Vec(Rc::new(RefCell::new(values))))
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
                        Ok(Value::String(Rc::new(RefCell::new(n.to_string()))))
                    }
                    (Value::Float(n), "string") => {
                        Ok(Value::String(Rc::new(RefCell::new(n.to_string()))))
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
                if let Value::Struct { name, fields } = value {
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
                    let vec = v.borrow();
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
            (Value::String(a), ExprKind::String(b)) => *a.borrow() == *b,
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
            (Value::String(a), Value::String(b)) => *a.borrow() == *b.borrow(),
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
                            let output = self.interpolate_string(&s.borrow())?;
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
                            let output = self.interpolate_string(&s.borrow())?;
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
                Ok(Value::String(Rc::new(RefCell::new(a.to_string()))))
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
                Ok(Value::String(Rc::new(RefCell::new(a.to_string()))))
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
            (Value::String(s), "len") => Ok(Value::Int(s.borrow().len() as i64)),
            (Value::String(s), "is_empty") => Ok(Value::Bool(s.borrow().is_empty())),
            (Value::String(s), "clone") => Ok(Value::String(Rc::clone(s))),
            (Value::String(s), "starts_with") => {
                let prefix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.borrow().starts_with(&prefix)))
            }
            (Value::String(s), "ends_with") => {
                let suffix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.borrow().ends_with(&suffix)))
            }
            (Value::String(s), "contains") => {
                let pattern = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.borrow().contains(&pattern)))
            }
            (Value::String(s), "push") => {
                let c = self.expect_char(&args, 0)?;
                s.borrow_mut().push(c);
                Ok(Value::Unit)
            }
            (Value::String(s), "trim") => {
                Ok(Value::String(Rc::new(RefCell::new(s.borrow().trim().to_string()))))
            }
            (Value::String(s), "to_string") => Ok(Value::String(Rc::clone(s))),
            (Value::String(s), "to_uppercase") => {
                Ok(Value::String(Rc::new(RefCell::new(s.borrow().to_uppercase()))))
            }
            (Value::String(s), "to_lowercase") => {
                Ok(Value::String(Rc::new(RefCell::new(s.borrow().to_lowercase()))))
            }
            (Value::String(s), "split") => {
                let delimiter = self.expect_string(&args, 0)?;
                let parts: Vec<Value> = s
                    .borrow()
                    .split(&delimiter)
                    .map(|p| Value::String(Rc::new(RefCell::new(p.to_string()))))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(parts))))
            }
            (Value::String(s), "chars") => {
                let chars: Vec<Value> = s.borrow().chars().map(Value::Char).collect();
                Ok(Value::Vec(Rc::new(RefCell::new(chars))))
            }
            (Value::String(s), "lines") => {
                let lines: Vec<Value> = s
                    .borrow()
                    .lines()
                    .map(|l| Value::String(Rc::new(RefCell::new(l.to_string()))))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(lines))))
            }
            (Value::String(s), "replace") => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                Ok(Value::String(Rc::new(RefCell::new(s.borrow().replace(&from, &to)))))
            }
            (Value::String(s), "substring") => {
                let sb = s.borrow();
                let start = self.expect_int(&args, 0)? as usize;
                let end = args
                    .get(1)
                    .map(|v| match v {
                        Value::Int(i) => *i as usize,
                        _ => sb.len(),
                    })
                    .unwrap_or(sb.len());
                let substring: String = sb.chars().skip(start).take(end - start).collect();
                Ok(Value::String(Rc::new(RefCell::new(substring))))
            }
            (Value::String(s), "parse_int") => {
                match s.borrow().trim().parse::<i64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Int(n)],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(
                            "invalid integer".to_string(),
                        )))],
                    }),
                }
            }
            (Value::String(s), "char_at") => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.borrow().chars().nth(idx) {
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
                match s.borrow().as_bytes().get(idx) {
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
                match s.borrow().trim().parse::<f64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Float(n)],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(
                            "invalid float".to_string(),
                        )))],
                    }),
                }
            }
            (Value::String(s), "index_of") => {
                let pattern = self.expect_string(&args, 0)?;
                match s.borrow().find(&pattern) {
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
                Ok(Value::String(Rc::new(RefCell::new(s.borrow().repeat(n)))))
            }
            (Value::String(s), "reverse") => {
                Ok(Value::String(Rc::new(RefCell::new(
                    s.borrow().chars().rev().collect(),
                ))))
            }

            // Type constructor static methods (Vec.new(), string.new(), etc.)
            (Value::TypeConstructor(TypeConstructorKind::Vec), "new") => {
                Ok(Value::Vec(Rc::new(RefCell::new(Vec::new()))))
            }
            (Value::TypeConstructor(TypeConstructorKind::Vec), "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::Vec(Rc::new(RefCell::new(Vec::with_capacity(cap)))))
            }
            (Value::TypeConstructor(TypeConstructorKind::String), "new") => {
                Ok(Value::String(Rc::new(RefCell::new(String::new()))))
            }

            // Vec instance methods
            (Value::Vec(v), "push") => {
                let item = args.into_iter().next().unwrap_or(Value::Unit);
                v.borrow_mut().push(item);
                Ok(Value::Unit)
            }
            (Value::Vec(v), "pop") => {
                Ok(v.borrow_mut().pop().unwrap_or(Value::Unit))
            }
            (Value::Vec(v), "len") => {
                Ok(Value::Int(v.borrow().len() as i64))
            }
            (Value::Vec(v), "get") => {
                let idx = self.expect_int(&args, 0)? as usize;
                Ok(v.borrow().get(idx).cloned().unwrap_or(Value::Unit))
            }
            (Value::Vec(v), "is_empty") => {
                Ok(Value::Bool(v.borrow().is_empty()))
            }
            (Value::Vec(v), "clear") => {
                v.borrow_mut().clear();
                Ok(Value::Unit)
            }
            (Value::Vec(v), "iter") => {
                // Return a copy of the Vec for iteration
                // TODO: Implement proper iterator type with skip(), take(), etc.
                Ok(Value::Vec(Rc::clone(v)))
            }
            (Value::Vec(v), "skip") => {
                let n = self.expect_int(&args, 0)? as usize;
                let skipped: Vec<Value> = v.borrow().iter().skip(n).cloned().collect();
                Ok(Value::Vec(Rc::new(RefCell::new(skipped))))
            }
            (Value::Vec(v), "first") => {
                match v.borrow().first().cloned() {
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
                match v.borrow().last().cloned() {
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
                    let found = v.borrow().iter().any(|item| Self::value_eq(item, needle));
                    Ok(Value::Bool(found))
                } else {
                    Err(RuntimeError::ArityMismatch { expected: 1, got: 0 })
                }
            }
            (Value::Vec(v), "reverse") => {
                v.borrow_mut().reverse();
                Ok(Value::Unit)
            }
            (Value::Vec(v), "join") => {
                let sep = self.expect_string(&args, 0)?;
                let joined: String = v
                    .borrow()
                    .iter()
                    .map(|item| format!("{}", item))
                    .collect::<Vec<_>>()
                    .join(&sep);
                Ok(Value::String(Rc::new(RefCell::new(joined))))
            }

            // Module methods (fs.read_file, io.read_line, etc.)
            (Value::Module(ModuleKind::Fs), "read_file") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(content)))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "read_lines") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let lines: Vec<Value> = content
                            .lines()
                            .map(|l| Value::String(Rc::new(RefCell::new(l.to_string()))))
                            .collect();
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::Vec(Rc::new(RefCell::new(lines)))],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                    Ok(file) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::File(Rc::new(RefCell::new(Some(file))))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "create") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::File::create(&path) {
                    Ok(file) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::File(Rc::new(RefCell::new(Some(file))))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Fs), "canonicalize") => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::canonicalize(&path) {
                    Ok(p) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(
                            p.to_string_lossy().to_string(),
                        )))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                            }],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
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
                            fields: vec![Value::String(Rc::new(RefCell::new(line)))],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
                    }),
                }
            }
            (Value::Module(ModuleKind::Cli), "args") => {
                let args_vec: Vec<Value> = self
                    .cli_args
                    .iter()
                    .map(|s| Value::String(Rc::new(RefCell::new(s.clone()))))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(args_vec))))
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
                        fields: vec![Value::String(Rc::new(RefCell::new(val)))],
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
                        Value::Vec(Rc::new(RefCell::new(vec![
                            Value::String(Rc::new(RefCell::new(k))),
                            Value::String(Rc::new(RefCell::new(v))),
                        ])))
                    })
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(vars))))
            }

            // File instance methods
            (Value::File(f), "close") => {
                let _ = f.borrow_mut().take();
                Ok(Value::Unit)
            }
            (Value::File(f), "read_all") => {
                use std::io::Read;
                let mut file_opt = f.borrow_mut();
                let file = file_opt.as_mut().ok_or_else(|| {
                    RuntimeError::TypeError("file is closed".to_string())
                })?;
                let mut content = String::new();
                match file.read_to_string(&mut content) {
                    Ok(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(content)))],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
                    }),
                }
            }
            (Value::File(f), "write") => {
                use std::io::Write;
                let mut file_opt = f.borrow_mut();
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
                        fields: vec![Value::String(Rc::new(RefCell::new(e.to_string())))],
                    }),
                }
            }
            (Value::File(f), "lines") => {
                use std::io::{BufRead, BufReader};
                let file_opt = f.borrow();
                let file = file_opt.as_ref().ok_or_else(|| {
                    RuntimeError::TypeError("file is closed".to_string())
                })?;
                // Read all lines eagerly (file handle is borrowed, can't return iterator)
                let reader = BufReader::new(file);
                let lines: Vec<Value> = reader
                    .lines()
                    .filter_map(|r| r.ok())
                    .map(|l| Value::String(Rc::new(RefCell::new(l))))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(lines))))
            }

            // Metadata methods
            (Value::Struct { name, fields }, "size") if name == "Metadata" => {
                Ok(fields.get("size").cloned().unwrap_or(Value::Int(0)))
            }
            (Value::Struct { name, fields }, "accessed") if name == "Metadata" => {
                Ok(fields.get("accessed").cloned().unwrap_or(Value::Int(0)))
            }
            (Value::Struct { name, fields }, "modified") if name == "Metadata" => {
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
                let cloned = v.borrow().clone();
                Ok(Value::Vec(Rc::new(RefCell::new(cloned))))
            }

            // String ne (not-equal)
            (Value::String(a), "ne") => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*a.borrow() != b))
            }

            // Generic to_string
            (_, "to_string") => {
                Ok(Value::String(Rc::new(RefCell::new(format!("{}", receiver)))))
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
            Some(Value::String(s)) => Ok(s.borrow().clone()),
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

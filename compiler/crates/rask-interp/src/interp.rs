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
use crate::value::{BuiltinKind, TypeConstructorKind, Value};

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
    /// 3. Finds and calls the @entry function
    pub fn run(&mut self, decls: &[Decl]) -> Result<Value, RuntimeError> {
        // Pass 1: Register all function and enum declarations, find @entry
        let mut entry_fn: Option<FnDecl> = None;
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
                _ => {}
            }
        }

        // Register built-in functions in the global scope
        self.env
            .define("print".to_string(), Value::Builtin(BuiltinKind::Print));
        self.env
            .define("println".to_string(), Value::Builtin(BuiltinKind::Println));
        self.env
            .define("panic".to_string(), Value::Builtin(BuiltinKind::Panic));
        self.env
            .define("cli_args".to_string(), Value::Builtin(BuiltinKind::CliArgs));
        self.env
            .define("std_exit".to_string(), Value::Builtin(BuiltinKind::StdExit));
        self.env
            .define("fs_read_file".to_string(), Value::Builtin(BuiltinKind::FsReadFile));
        self.env
            .define("fs_read_lines".to_string(), Value::Builtin(BuiltinKind::FsReadLines));
        self.env
            .define("read_line".to_string(), Value::Builtin(BuiltinKind::ReadLine));

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
            // TODO: Field assignment, index assignment
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
            ExprKind::String(s) => Ok(Value::String(s.clone())),
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
                // Check type constructors (Vec, Map, etc.)
                match name.as_str() {
                    "Vec" => return Ok(Value::TypeConstructor(TypeConstructorKind::Vec)),
                    "Map" => return Ok(Value::TypeConstructor(TypeConstructorKind::Map)),
                    _ => {}
                }
                Err(RuntimeError::UndefinedVariable(name.clone()))
            }

            // Function call
            ExprKind::Call { func, args } => {
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
            (Value::String(a), ExprKind::String(b)) => a == b,
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
                            let output = self.interpolate_string(s)?;
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
                            let output = self.interpolate_string(s)?;
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
            BuiltinKind::CliArgs => {
                let args_vec: Vec<Value> = self
                    .cli_args
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(args_vec))))
            }
            BuiltinKind::StdExit => {
                let code = args
                    .first()
                    .map(|v| match v {
                        Value::Int(n) => *n as i32,
                        _ => 1,
                    })
                    .unwrap_or(0);
                Err(RuntimeError::Exit(code))
            }
            BuiltinKind::FsReadFile => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::String(content)],
                    }),
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(e.to_string())],
                    }),
                }
            }
            BuiltinKind::FsReadLines => {
                let path = self.expect_string(&args, 0)?;
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let lines: Vec<Value> = content
                            .lines()
                            .map(|l| Value::String(l.to_string()))
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
                        fields: vec![Value::String(e.to_string())],
                    }),
                }
            }
            BuiltinKind::ReadLine => {
                use std::io::{self, BufRead};
                let mut line = String::new();
                match io::stdin().lock().read_line(&mut line) {
                    Ok(_) => {
                        // Remove trailing newline
                        if line.ends_with('\n') {
                            line.pop();
                            if line.ends_with('\r') {
                                line.pop();
                            }
                        }
                        Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Ok".to_string(),
                            fields: vec![Value::String(line)],
                        })
                    }
                    Err(e) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String(e.to_string())],
                    }),
                }
            }
        }
    }

    /// Interpolate a string, replacing {name} with variable values.
    fn interpolate_string(&self, s: &str) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                // Collect identifier until '}'
                let mut ident = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next(); // consume '}'
                        break;
                    }
                    ident.push(chars.next().unwrap());
                }
                // Look up variable
                if let Some(val) = self.env.get(&ident) {
                    result.push_str(&format!("{}", val));
                } else {
                    return Err(RuntimeError::UndefinedVariable(ident));
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
            (Value::String(s), "len") => Ok(Value::Int(s.len() as i64)),
            (Value::String(s), "is_empty") => Ok(Value::Bool(s.is_empty())),
            (Value::String(s), "clone") => Ok(Value::String(s.clone())),
            (Value::String(s), "starts_with") => {
                let prefix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.starts_with(&prefix)))
            }
            (Value::String(s), "ends_with") => {
                let suffix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.ends_with(&suffix)))
            }
            (Value::String(s), "contains") => {
                let pattern = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.contains(&pattern)))
            }
            (Value::String(s), "trim") => Ok(Value::String(s.trim().to_string())),
            (Value::String(s), "to_uppercase") => Ok(Value::String(s.to_uppercase())),
            (Value::String(s), "to_lowercase") => Ok(Value::String(s.to_lowercase())),
            (Value::String(s), "split") => {
                let delimiter = self.expect_string(&args, 0)?;
                let parts: Vec<Value> = s
                    .split(&delimiter)
                    .map(|p| Value::String(p.to_string()))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(parts))))
            }
            (Value::String(s), "chars") => {
                let chars: Vec<Value> = s.chars().map(Value::Char).collect();
                Ok(Value::Vec(Rc::new(RefCell::new(chars))))
            }
            (Value::String(s), "lines") => {
                let lines: Vec<Value> = s
                    .lines()
                    .map(|l| Value::String(l.to_string()))
                    .collect();
                Ok(Value::Vec(Rc::new(RefCell::new(lines))))
            }
            (Value::String(s), "replace") => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                Ok(Value::String(s.replace(&from, &to)))
            }
            (Value::String(s), "substring") => {
                let start = self.expect_int(&args, 0)? as usize;
                let end = args
                    .get(1)
                    .map(|v| match v {
                        Value::Int(i) => *i as usize,
                        _ => s.len(),
                    })
                    .unwrap_or(s.len());
                let substring: String = s.chars().skip(start).take(end - start).collect();
                Ok(Value::String(substring))
            }
            (Value::String(s), "parse_int") => {
                match s.trim().parse::<i64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Int(n)],
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![Value::String("invalid integer".to_string())],
                    }),
                }
            }
            (Value::String(s), "char_at") => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.chars().nth(idx) {
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
                match s.as_bytes().get(idx) {
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

            // Type constructor static methods (Vec.new(), etc.)
            (Value::TypeConstructor(TypeConstructorKind::Vec), "new") => {
                Ok(Value::Vec(Rc::new(RefCell::new(Vec::new()))))
            }
            (Value::TypeConstructor(TypeConstructorKind::Vec), "with_capacity") => {
                let cap = self.expect_int(&args, 0)? as usize;
                Ok(Value::Vec(Rc::new(RefCell::new(Vec::with_capacity(cap)))))
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
            Some(Value::String(s)) => Ok(s.clone()),
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

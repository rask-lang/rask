//! The interpreter implementation.
//!
//! This is a tree-walk interpreter that directly evaluates the AST.
//! After desugaring, arithmetic operators become method calls (a + b → a.add(b)),
//! so the interpreter implements these methods on primitive types.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::env::Environment;
use crate::value::{BuiltinKind, Value};

/// The tree-walk interpreter.
pub struct Interpreter {
    /// Variable bindings (scoped).
    env: Environment,
    /// Function declarations by name.
    functions: HashMap<String, FnDecl>,
}

impl Interpreter {
    /// Create a new interpreter.
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
        }
    }

    /// Run a program (list of declarations).
    ///
    /// This:
    /// 1. Registers all function declarations
    /// 2. Registers built-in functions (println, print, panic)
    /// 3. Calls main() if it exists
    pub fn run(&mut self, decls: &[Decl]) -> Result<Value, RuntimeError> {
        // Pass 1: Register all function declarations
        for decl in decls {
            if let DeclKind::Fn(f) = &decl.kind {
                self.functions.insert(f.name.clone(), f.clone());
            }
        }

        // Register built-in functions in the global scope
        self.env
            .define("print".to_string(), Value::Builtin(BuiltinKind::Print));
        self.env
            .define("println".to_string(), Value::Builtin(BuiltinKind::Println));
        self.env
            .define("panic".to_string(), Value::Builtin(BuiltinKind::Panic));

        // Pass 2: Call main() if it exists
        if let Some(main_fn) = self.functions.get("main").cloned() {
            self.call_function(&main_fn, vec![])
        } else {
            // No main function - just return Unit
            Ok(Value::Unit)
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
        // - Err(other) means actual error → propagate
        match result {
            Ok(value) => Ok(value),
            Err(RuntimeError::Return(v)) => Ok(v),
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

            // Other expressions not yet implemented
            _ => Ok(Value::Unit),
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
                        print!(" ");
                    }
                    match arg {
                        Value::String(s) => {
                            // Handle string interpolation
                            let output = self.interpolate_string(s)?;
                            print!("{}", output);
                        }
                        _ => print!("{}", arg),
                    }
                }
                println!();
                Ok(Value::Unit)
            }
            BuiltinKind::Print => {
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        print!(" ");
                    }
                    match arg {
                        Value::String(s) => {
                            let output = self.interpolate_string(s)?;
                            print!("{}", output);
                        }
                        _ => print!("{}", arg),
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

            // String methods
            (Value::String(s), "len") => Ok(Value::Int(s.len() as i64)),

            // Unknown method
            _ => Err(RuntimeError::NoSuchMethod {
                ty: receiver.type_name().to_string(),
                method: method.to_string(),
            }),
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

    // Control flow (not actual errors)
    #[error("return")]
    Return(Value),

    #[error("break")]
    Break,

    #[error("continue")]
    Continue,
}

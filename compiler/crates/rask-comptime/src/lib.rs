// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Compile-time execution for Rask.
//!
//! Evaluates `comptime` blocks and functions at compile time.
//! Subject to restrictions: no I/O, no pools, no concurrency.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use std::collections::HashMap;
use thiserror::Error;

// ============================================================================
// Comptime Values
// ============================================================================

/// A value that exists at compile time.
#[derive(Debug, Clone, PartialEq)]
pub enum ComptimeValue {
    Unit,
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    Array(Vec<ComptimeValue>),
    Tuple(Vec<ComptimeValue>),
    Struct {
        name: String,
        fields: HashMap<String, ComptimeValue>,
    },
    Enum {
        name: String,
        variant: String,
        data: Option<Box<ComptimeValue>>,
    },
}

impl ComptimeValue {
    /// Get the type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            ComptimeValue::Unit => "()",
            ComptimeValue::Bool(_) => "bool",
            ComptimeValue::I8(_) => "i8",
            ComptimeValue::I16(_) => "i16",
            ComptimeValue::I32(_) => "i32",
            ComptimeValue::I64(_) => "i64",
            ComptimeValue::U8(_) => "u8",
            ComptimeValue::U16(_) => "u16",
            ComptimeValue::U32(_) => "u32",
            ComptimeValue::U64(_) => "u64",
            ComptimeValue::F32(_) => "f32",
            ComptimeValue::F64(_) => "f64",
            ComptimeValue::Char(_) => "char",
            ComptimeValue::String(_) => "String",
            ComptimeValue::Array(_) => "Array",
            ComptimeValue::Tuple(_) => "Tuple",
            ComptimeValue::Struct { .. } => "Struct",
            ComptimeValue::Enum { .. } => "Enum",
        }
    }

    /// Convert to bool if possible.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ComptimeValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Convert to i64 (widening conversion for integers).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ComptimeValue::I8(v) => Some(*v as i64),
            ComptimeValue::I16(v) => Some(*v as i64),
            ComptimeValue::I32(v) => Some(*v as i64),
            ComptimeValue::I64(v) => Some(*v),
            ComptimeValue::U8(v) => Some(*v as i64),
            ComptimeValue::U16(v) => Some(*v as i64),
            ComptimeValue::U32(v) => Some(*v as i64),
            ComptimeValue::U64(v) => Some(*v as i64), // May overflow
            _ => None,
        }
    }

    /// Convert to f64 (widening conversion for floats).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ComptimeValue::F32(v) => Some(*v as f64),
            ComptimeValue::F64(v) => Some(*v),
            _ => None,
        }
    }
}

// ============================================================================
// Comptime Errors
// ============================================================================

/// Errors that can occur during comptime evaluation.
#[derive(Debug, Error)]
pub enum ComptimeError {
    #[error("comptime exceeded backwards branch quota ({0}); increase with @branch_quota")]
    BranchQuotaExceeded(usize),

    #[error("comptime exceeded time limit; simplify the expression or increase the limit")]
    TimeoutExceeded,

    #[error("comptime exceeded memory limit; reduce allocations in comptime block")]
    MemoryLimitExceeded,

    #[error("undefined variable `{0}` in comptime context")]
    UndefinedVariable(String),

    #[error("undefined function `{0}` in comptime context")]
    UndefinedFunction(String),

    #[error("type mismatch: expected `{expected}`, found `{found}`")]
    TypeMismatch { expected: String, found: String },

    #[error("division by zero in comptime evaluation")]
    DivisionByZero,

    #[error("index {index} out of bounds (length is {len})")]
    IndexOutOfBounds { index: usize, len: usize },

    #[error("cannot call runtime function `{0}` at comptime; mark it `comptime func` or restructure")]
    RuntimeFunctionCall(String),

    #[error("I/O not allowed at comptime; use runtime code for file/network operations")]
    IoNotAllowed,

    #[error("pools and handles not allowed at comptime; use Vec or arrays instead")]
    PoolsNotAllowed,

    #[error("concurrency not allowed at comptime; spawn/channels require runtime")]
    ConcurrencyNotAllowed,

    #[error("unsafe blocks not allowed at comptime; raw pointers require runtime")]
    UnsafeNotAllowed,

    #[error("comptime panic: {0}")]
    Panic(String),

    #[error("no field `{field}` on type `{ty}`")]
    NoSuchField { ty: String, field: String },

    #[error("`{0}` is not a struct")]
    NotAStruct(String),

    #[error("break outside of loop")]
    BreakOutsideLoop,

    #[error("continue outside of loop")]
    ContinueOutsideLoop,

    #[error("return outside of function")]
    ReturnOutsideFunction,

    #[error("not supported at comptime: {0}")]
    NotSupported(String),
}

/// Result type for comptime operations.
pub type ComptimeResult<T> = Result<T, ComptimeError>;

// ============================================================================
// Comptime Environment
// ============================================================================

/// The comptime execution environment.
#[derive(Debug, Default)]
pub struct ComptimeEnv {
    /// Variable bindings in scope stack.
    scopes: Vec<HashMap<String, ComptimeValue>>,
    /// Comptime function definitions.
    functions: HashMap<String, FnDecl>,
    /// Backwards branch counter (loops + recursion).
    branch_count: usize,
    /// Maximum allowed backwards branches.
    branch_quota: usize,
}

impl ComptimeEnv {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            branch_count: 0,
            branch_quota: 1000, // Default
        }
    }

    pub fn with_quota(quota: usize) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            branch_count: 0,
            branch_quota: quota,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: String, value: ComptimeValue) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn get(&self, name: &str) -> Option<&ComptimeValue> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Some(value);
            }
        }
        None
    }

    fn assign(&mut self, name: &str, value: ComptimeValue) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return true;
            }
        }
        false
    }

    fn register_function(&mut self, name: String, func: FnDecl) {
        self.functions.insert(name, func);
    }

    fn get_function(&self, name: &str) -> Option<&FnDecl> {
        self.functions.get(name)
    }

    fn count_branch(&mut self) -> ComptimeResult<()> {
        self.branch_count += 1;
        if self.branch_count > self.branch_quota {
            Err(ComptimeError::BranchQuotaExceeded(self.branch_quota))
        } else {
            Ok(())
        }
    }
}

// ============================================================================
// Control Flow
// ============================================================================

/// Control flow signals during evaluation.
#[derive(Debug)]
enum ControlFlow {
    /// Normal execution continues.
    Normal(ComptimeValue),
    /// Break statement encountered.
    Break(Option<ComptimeValue>),
    /// Continue statement encountered.
    Continue,
    /// Return statement encountered.
    Return(ComptimeValue),
}

impl ControlFlow {
    fn value(self) -> ComptimeValue {
        match self {
            ControlFlow::Normal(v) | ControlFlow::Break(Some(v)) | ControlFlow::Return(v) => v,
            ControlFlow::Break(None) | ControlFlow::Continue => ComptimeValue::Unit,
        }
    }

}

// ============================================================================
// Comptime Interpreter
// ============================================================================

/// The compile-time interpreter.
pub struct ComptimeInterpreter {
    env: ComptimeEnv,
}

impl ComptimeInterpreter {
    pub fn new() -> Self {
        Self {
            env: ComptimeEnv::new(),
        }
    }

    pub fn with_quota(quota: usize) -> Self {
        Self {
            env: ComptimeEnv::with_quota(quota),
        }
    }

    /// Register comptime functions from declarations.
    pub fn register_functions(&mut self, decls: &[Decl]) {
        for decl in decls {
            if let DeclKind::Fn(f) = &decl.kind {
                if f.is_comptime {
                    self.env.register_function(f.name.clone(), f.clone());
                }
            }
        }
    }

    /// Evaluate a comptime expression.
    pub fn eval_expr(&mut self, expr: &Expr) -> ComptimeResult<ComptimeValue> {
        match self.eval_expr_cf(expr)? {
            ControlFlow::Normal(v) => Ok(v),
            ControlFlow::Return(v) => Ok(v),
            ControlFlow::Break(_) => Err(ComptimeError::BreakOutsideLoop),
            ControlFlow::Continue => Err(ComptimeError::ContinueOutsideLoop),
        }
    }

    fn eval_expr_cf(&mut self, expr: &Expr) -> ComptimeResult<ControlFlow> {
        let value = match &expr.kind {
            // Literals
            ExprKind::Int(v, _) => ComptimeValue::I64(*v),
            ExprKind::Float(v, _) => ComptimeValue::F64(*v),
            ExprKind::String(s) => ComptimeValue::String(s.clone()),
            ExprKind::Char(c) => ComptimeValue::Char(*c),
            ExprKind::Bool(b) => ComptimeValue::Bool(*b),

            // Identifier
            ExprKind::Ident(name) => {
                self.env
                    .get(name)
                    .cloned()
                    .ok_or_else(|| ComptimeError::UndefinedVariable(name.clone()))?
            }

            // Binary operation (after desugaring, most become method calls)
            // But logical operators remain as binary
            ExprKind::Binary { op, left, right } => {
                self.eval_binary(*op, left, right)?
            }

            // Unary operation
            ExprKind::Unary { op, operand } => {
                self.eval_unary(*op, operand)?
            }

            // Function call
            ExprKind::Call { func, args } => {
                self.eval_call(func, args)?
            }

            // Method call (from desugared operators)
            ExprKind::MethodCall { object, method, args, .. } => {
                self.eval_method_call(object, method, args)?
            }

            // Field access
            ExprKind::Field { object, field } => {
                let obj = self.eval_expr(object)?;
                match obj {
                    ComptimeValue::Struct { name, fields } => {
                        fields.get(field).cloned().ok_or_else(|| {
                            ComptimeError::NoSuchField {
                                ty: name,
                                field: field.clone(),
                            }
                        })?
                    }
                    other => return Err(ComptimeError::NotAStruct(other.type_name().to_string())),
                }
            }

            // Index access
            ExprKind::Index { object, index } => {
                let obj = self.eval_expr(object)?;
                let idx = self.eval_expr(index)?;
                let idx_val = idx.as_i64().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "integer".to_string(),
                    found: idx.type_name().to_string(),
                })? as usize;

                match obj {
                    ComptimeValue::Array(arr) => {
                        if idx_val >= arr.len() {
                            return Err(ComptimeError::IndexOutOfBounds {
                                index: idx_val,
                                len: arr.len(),
                            });
                        }
                        arr[idx_val].clone()
                    }
                    ComptimeValue::String(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        if idx_val >= chars.len() {
                            return Err(ComptimeError::IndexOutOfBounds {
                                index: idx_val,
                                len: chars.len(),
                            });
                        }
                        ComptimeValue::Char(chars[idx_val])
                    }
                    _ => return Err(ComptimeError::TypeMismatch {
                        expected: "Array or String".to_string(),
                        found: obj.type_name().to_string(),
                    }),
                }
            }

            // Block expression
            ExprKind::Block(stmts) => {
                self.env.push_scope();
                let result = self.eval_block(stmts);
                self.env.pop_scope();
                return result;
            }

            // If expression
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.eval_expr(cond)?;
                let cond_bool = cond_val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: cond_val.type_name().to_string(),
                })?;

                if cond_bool {
                    return self.eval_expr_cf(then_branch);
                } else if let Some(else_br) = else_branch {
                    return self.eval_expr_cf(else_br);
                } else {
                    ComptimeValue::Unit
                }
            }

            // Match expression
            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;
                for arm in arms {
                    if self.pattern_matches(&arm.pattern, &value)? {
                        self.env.push_scope();
                        self.bind_pattern(&arm.pattern, &value)?;

                        // Check guard if present
                        if let Some(guard) = &arm.guard {
                            let guard_val = self.eval_expr(guard)?;
                            if !guard_val.as_bool().unwrap_or(false) {
                                self.env.pop_scope();
                                continue;
                            }
                        }

                        let result = self.eval_expr_cf(&arm.body);
                        self.env.pop_scope();
                        return result;
                    }
                }
                // No match - this should be a compile error but for now return unit
                ComptimeValue::Unit
            }

            // Array literal
            ExprKind::Array(elems) => {
                let values: ComptimeResult<Vec<_>> = elems.iter().map(|e| self.eval_expr(e)).collect();
                ComptimeValue::Array(values?)
            }

            // Tuple literal
            ExprKind::Tuple(elems) => {
                let values: ComptimeResult<Vec<_>> = elems.iter().map(|e| self.eval_expr(e)).collect();
                ComptimeValue::Tuple(values?)
            }

            // Struct literal
            ExprKind::StructLit { name, fields, .. } => {
                let mut field_values = HashMap::new();
                for field in fields {
                    let value = self.eval_expr(&field.value)?;
                    field_values.insert(field.name.clone(), value);
                }
                ComptimeValue::Struct {
                    name: name.clone(),
                    fields: field_values,
                }
            }

            // Range expression
            ExprKind::Range { start, end, inclusive } => {
                // For now, just create an array of the range
                let start_val = if let Some(s) = start {
                    self.eval_expr(s)?.as_i64().unwrap_or(0)
                } else {
                    0
                };
                let end_val = if let Some(e) = end {
                    self.eval_expr(e)?.as_i64().unwrap_or(0)
                } else {
                    return Err(ComptimeError::NotSupported("unbounded range".to_string()));
                };

                let values: Vec<ComptimeValue> = if *inclusive {
                    (start_val..=end_val).map(ComptimeValue::I64).collect()
                } else {
                    (start_val..end_val).map(ComptimeValue::I64).collect()
                };
                ComptimeValue::Array(values)
            }

            // Closure - store as value (not supported for execution yet)
            ExprKind::Closure { .. } => {
                return Err(ComptimeError::NotSupported("closures at comptime".to_string()));
            }

            // Spawn - not allowed
            ExprKind::Spawn { .. } => {
                return Err(ComptimeError::ConcurrencyNotAllowed);
            }

            // Unsafe - not allowed
            ExprKind::Unsafe { .. } => {
                return Err(ComptimeError::UnsafeNotAllowed);
            }

            // Other expressions not yet supported
            _ => {
                return Err(ComptimeError::NotSupported(format!("{:?}", expr.kind)));
            }
        };

        Ok(ControlFlow::Normal(value))
    }

    fn eval_block(&mut self, stmts: &[Stmt]) -> ComptimeResult<ControlFlow> {
        let mut last_value = ComptimeValue::Unit;

        for stmt in stmts {
            match self.eval_stmt(stmt)? {
                ControlFlow::Normal(v) => last_value = v,
                cf @ ControlFlow::Return(_) => return Ok(cf),
                cf @ ControlFlow::Break(_) => return Ok(cf),
                cf @ ControlFlow::Continue => return Ok(cf),
            }
        }

        Ok(ControlFlow::Normal(last_value))
    }

    fn eval_stmt(&mut self, stmt: &Stmt) -> ComptimeResult<ControlFlow> {
        match &stmt.kind {
            StmtKind::Expr(e) => self.eval_expr_cf(e),

            StmtKind::Let { name, init, .. } | StmtKind::Const { name, init, .. } => {
                let value = self.eval_expr(init)?;
                self.env.define(name.clone(), value);
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::LetTuple { names, init } | StmtKind::ConstTuple { names, init } => {
                let value = self.eval_expr(init)?;
                if let ComptimeValue::Tuple(values) = value {
                    if values.len() != names.len() {
                        return Err(ComptimeError::TypeMismatch {
                            expected: format!("tuple of {} elements", names.len()),
                            found: format!("tuple of {} elements", values.len()),
                        });
                    }
                    for (name, val) in names.iter().zip(values) {
                        self.env.define(name.clone(), val);
                    }
                } else {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "tuple".to_string(),
                        found: value.type_name().to_string(),
                    });
                }
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::Assign { target, value } => {
                let val = self.eval_expr(value)?;
                if let ExprKind::Ident(name) = &target.kind {
                    if !self.env.assign(name, val) {
                        return Err(ComptimeError::UndefinedVariable(name.clone()));
                    }
                } else {
                    return Err(ComptimeError::NotSupported("complex assignment target".to_string()));
                }
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::Return(expr) => {
                let value = if let Some(e) = expr {
                    self.eval_expr(e)?
                } else {
                    ComptimeValue::Unit
                };
                Ok(ControlFlow::Return(value))
            }

            StmtKind::Break(label) => {
                if label.is_some() {
                    return Err(ComptimeError::NotSupported("labeled break".to_string()));
                }
                Ok(ControlFlow::Break(None))
            }

            StmtKind::Continue(label) => {
                if label.is_some() {
                    return Err(ComptimeError::NotSupported("labeled continue".to_string()));
                }
                Ok(ControlFlow::Continue)
            }

            StmtKind::Deliver { value, .. } => {
                let val = self.eval_expr(value)?;
                Ok(ControlFlow::Break(Some(val)))
            }

            StmtKind::While { cond, body } => {
                loop {
                    self.env.count_branch()?;

                    let cond_val = self.eval_expr(cond)?;
                    let cond_bool = cond_val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                        expected: "bool".to_string(),
                        found: cond_val.type_name().to_string(),
                    })?;

                    if !cond_bool {
                        break;
                    }

                    self.env.push_scope();
                    match self.eval_block(body)? {
                        ControlFlow::Normal(_) | ControlFlow::Continue => {}
                        ControlFlow::Break(v) => {
                            self.env.pop_scope();
                            return Ok(ControlFlow::Normal(v.unwrap_or(ComptimeValue::Unit)));
                        }
                        cf @ ControlFlow::Return(_) => {
                            self.env.pop_scope();
                            return Ok(cf);
                        }
                    }
                    self.env.pop_scope();
                }
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::Loop { body, .. } => {
                loop {
                    self.env.count_branch()?;

                    self.env.push_scope();
                    match self.eval_block(body)? {
                        ControlFlow::Normal(_) | ControlFlow::Continue => {}
                        ControlFlow::Break(v) => {
                            self.env.pop_scope();
                            return Ok(ControlFlow::Normal(v.unwrap_or(ComptimeValue::Unit)));
                        }
                        cf @ ControlFlow::Return(_) => {
                            self.env.pop_scope();
                            return Ok(cf);
                        }
                    }
                    self.env.pop_scope();
                }
            }

            StmtKind::For { binding, iter, body, .. } => {
                let iter_val = self.eval_expr(iter)?;
                let items = match iter_val {
                    ComptimeValue::Array(arr) => arr,
                    ComptimeValue::String(s) => s.chars().map(ComptimeValue::Char).collect(),
                    _ => return Err(ComptimeError::TypeMismatch {
                        expected: "iterable".to_string(),
                        found: iter_val.type_name().to_string(),
                    }),
                };

                for item in items {
                    self.env.count_branch()?;

                    self.env.push_scope();
                    self.env.define(binding.clone(), item);

                    match self.eval_block(body)? {
                        ControlFlow::Normal(_) | ControlFlow::Continue => {}
                        ControlFlow::Break(v) => {
                            self.env.pop_scope();
                            return Ok(ControlFlow::Normal(v.unwrap_or(ComptimeValue::Unit)));
                        }
                        cf @ ControlFlow::Return(_) => {
                            self.env.pop_scope();
                            return Ok(cf);
                        }
                    }
                    self.env.pop_scope();
                }
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::Comptime(body) => {
                // Already at comptime, just evaluate the block
                self.eval_block(body)
            }

            StmtKind::Ensure { .. } => {
                // Ensure blocks are runtime-only
                Err(ComptimeError::NotSupported("ensure blocks at comptime".to_string()))
            }

            StmtKind::WhileLet { .. } => {
                Err(ComptimeError::NotSupported("while-let at comptime".to_string()))
            }
        }
    }

    fn eval_binary(&mut self, op: BinOp, left: &Expr, right: &Expr) -> ComptimeResult<ComptimeValue> {
        // Only logical operators should reach here (And, Or)
        // Other operators are desugared to method calls
        match op {
            BinOp::And => {
                let left_val = self.eval_expr(left)?;
                let left_bool = left_val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: left_val.type_name().to_string(),
                })?;
                if !left_bool {
                    return Ok(ComptimeValue::Bool(false));
                }
                let right_val = self.eval_expr(right)?;
                let right_bool = right_val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: right_val.type_name().to_string(),
                })?;
                Ok(ComptimeValue::Bool(right_bool))
            }
            BinOp::Or => {
                let left_val = self.eval_expr(left)?;
                let left_bool = left_val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: left_val.type_name().to_string(),
                })?;
                if left_bool {
                    return Ok(ComptimeValue::Bool(true));
                }
                let right_val = self.eval_expr(right)?;
                let right_bool = right_val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: right_val.type_name().to_string(),
                })?;
                Ok(ComptimeValue::Bool(right_bool))
            }
            // Other operators should be desugared to method calls
            _ => Err(ComptimeError::NotSupported(format!("binary operator {:?} (should be desugared)", op))),
        }
    }

    fn eval_unary(&mut self, op: UnaryOp, operand: &Expr) -> ComptimeResult<ComptimeValue> {
        let val = self.eval_expr(operand)?;
        match op {
            UnaryOp::Not => {
                let b = val.as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: val.type_name().to_string(),
                })?;
                Ok(ComptimeValue::Bool(!b))
            }
            UnaryOp::Neg => {
                // Should be desugared to .neg() but handle directly for primitives
                match val {
                    ComptimeValue::I8(v) => Ok(ComptimeValue::I8(-v)),
                    ComptimeValue::I16(v) => Ok(ComptimeValue::I16(-v)),
                    ComptimeValue::I32(v) => Ok(ComptimeValue::I32(-v)),
                    ComptimeValue::I64(v) => Ok(ComptimeValue::I64(-v)),
                    ComptimeValue::F32(v) => Ok(ComptimeValue::F32(-v)),
                    ComptimeValue::F64(v) => Ok(ComptimeValue::F64(-v)),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "numeric".to_string(),
                        found: val.type_name().to_string(),
                    }),
                }
            }
            UnaryOp::BitNot => {
                match val {
                    ComptimeValue::I8(v) => Ok(ComptimeValue::I8(!v)),
                    ComptimeValue::I16(v) => Ok(ComptimeValue::I16(!v)),
                    ComptimeValue::I32(v) => Ok(ComptimeValue::I32(!v)),
                    ComptimeValue::I64(v) => Ok(ComptimeValue::I64(!v)),
                    ComptimeValue::U8(v) => Ok(ComptimeValue::U8(!v)),
                    ComptimeValue::U16(v) => Ok(ComptimeValue::U16(!v)),
                    ComptimeValue::U32(v) => Ok(ComptimeValue::U32(!v)),
                    ComptimeValue::U64(v) => Ok(ComptimeValue::U64(!v)),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "integer".to_string(),
                        found: val.type_name().to_string(),
                    }),
                }
            }
            UnaryOp::Ref => {
                Err(ComptimeError::NotSupported("references at comptime".to_string()))
            }
            UnaryOp::Deref => {
                Err(ComptimeError::NotSupported("pointer dereference at comptime".to_string()))
            }
        }
    }

    fn eval_call(&mut self, func: &Expr, args: &[Expr]) -> ComptimeResult<ComptimeValue> {
        // Get function name
        let func_name = if let ExprKind::Ident(name) = &func.kind {
            name.clone()
        } else {
            return Err(ComptimeError::NotSupported("indirect function calls".to_string()));
        };

        // Evaluate arguments
        let arg_values: ComptimeResult<Vec<_>> = args.iter().map(|a| self.eval_expr(a)).collect();
        let arg_values = arg_values?;

        // Look up function
        if let Some(func_decl) = self.env.get_function(&func_name).cloned() {
            self.env.count_branch()?;
            self.call_function(&func_decl, arg_values)
        } else {
            // Check for builtin functions
            self.call_builtin(&func_name, arg_values)
        }
    }

    fn eval_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
    ) -> ComptimeResult<ComptimeValue> {
        let obj = self.eval_expr(object)?;
        let arg_values: ComptimeResult<Vec<_>> = args.iter().map(|a| self.eval_expr(a)).collect();
        let arg_values = arg_values?;

        // Handle primitive methods (from desugared operators)
        self.call_primitive_method(&obj, method, &arg_values)
    }

    fn call_function(
        &mut self,
        func: &FnDecl,
        args: Vec<ComptimeValue>,
    ) -> ComptimeResult<ComptimeValue> {
        if func.params.len() != args.len() {
            return Err(ComptimeError::TypeMismatch {
                expected: format!("{} arguments", func.params.len()),
                found: format!("{} arguments", args.len()),
            });
        }

        self.env.push_scope();

        // Bind parameters
        for (param, value) in func.params.iter().zip(args) {
            self.env.define(param.name.clone(), value);
        }

        // Execute body
        let result = self.eval_block(&func.body)?;
        self.env.pop_scope();

        Ok(result.value())
    }

    fn call_builtin(&mut self, name: &str, args: Vec<ComptimeValue>) -> ComptimeResult<ComptimeValue> {
        match name {
            "panic" => {
                let msg = if args.is_empty() {
                    "explicit panic".to_string()
                } else if let Some(ComptimeValue::String(s)) = args.first() {
                    s.clone()
                } else {
                    format!("{:?}", args.first())
                };
                Err(ComptimeError::Panic(msg))
            }
            "println" | "print" => {
                // At comptime, these are no-ops (or could be @comptime_print)
                Ok(ComptimeValue::Unit)
            }
            "assert" => {
                if args.is_empty() {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "1 argument".to_string(),
                        found: "0 arguments".to_string(),
                    });
                }
                let cond = args[0].as_bool().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "bool".to_string(),
                    found: args[0].type_name().to_string(),
                })?;
                if !cond {
                    Err(ComptimeError::Panic("assertion failed".to_string()))
                } else {
                    Ok(ComptimeValue::Unit)
                }
            }
            _ => Err(ComptimeError::UndefinedFunction(name.to_string())),
        }
    }

    fn call_primitive_method(
        &self,
        obj: &ComptimeValue,
        method: &str,
        args: &[ComptimeValue],
    ) -> ComptimeResult<ComptimeValue> {
        // Handle numeric operations
        match method {
            "add" => self.numeric_binop(obj, args, |a, b| a + b, |a, b| a + b),
            "sub" => self.numeric_binop(obj, args, |a, b| a - b, |a, b| a - b),
            "mul" => self.numeric_binop(obj, args, |a, b| a * b, |a, b| a * b),
            "div" => {
                if let Some(arg) = args.first() {
                    if arg.as_i64() == Some(0) || arg.as_f64() == Some(0.0) {
                        return Err(ComptimeError::DivisionByZero);
                    }
                }
                self.numeric_binop(obj, args, |a, b| a / b, |a, b| a / b)
            }
            "rem" => {
                if let Some(arg) = args.first() {
                    if arg.as_i64() == Some(0) {
                        return Err(ComptimeError::DivisionByZero);
                    }
                }
                self.numeric_binop(obj, args, |a, b| a % b, |a, b| a % b)
            }
            "neg" => {
                match obj {
                    ComptimeValue::I64(v) => Ok(ComptimeValue::I64(-v)),
                    ComptimeValue::F64(v) => Ok(ComptimeValue::F64(-v)),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "numeric".to_string(),
                        found: obj.type_name().to_string(),
                    }),
                }
            }
            "eq" => self.comparison_op(obj, args, |a, b| a == b, |a, b| a == b),
            "lt" => self.comparison_op(obj, args, |a, b| a < b, |a, b| a < b),
            "gt" => self.comparison_op(obj, args, |a, b| a > b, |a, b| a > b),
            "le" => self.comparison_op(obj, args, |a, b| a <= b, |a, b| a <= b),
            "ge" => self.comparison_op(obj, args, |a, b| a >= b, |a, b| a >= b),
            "bit_and" => self.int_binop(obj, args, |a, b| a & b),
            "bit_or" => self.int_binop(obj, args, |a, b| a | b),
            "bit_xor" => self.int_binop(obj, args, |a, b| a ^ b),
            "shl" => self.int_binop(obj, args, |a, b| a << b),
            "shr" => self.int_binop(obj, args, |a, b| a >> b),
            "bit_not" => {
                match obj {
                    ComptimeValue::I64(v) => Ok(ComptimeValue::I64(!v)),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "integer".to_string(),
                        found: obj.type_name().to_string(),
                    }),
                }
            }
            // String methods
            "len" => {
                match obj {
                    ComptimeValue::String(s) => Ok(ComptimeValue::I64(s.len() as i64)),
                    ComptimeValue::Array(arr) => Ok(ComptimeValue::I64(arr.len() as i64)),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "String or Array".to_string(),
                        found: obj.type_name().to_string(),
                    }),
                }
            }
            _ => Err(ComptimeError::NotSupported(format!("method {} on {}", method, obj.type_name()))),
        }
    }

    fn numeric_binop<Fi, Ff>(
        &self,
        obj: &ComptimeValue,
        args: &[ComptimeValue],
        int_op: Fi,
        float_op: Ff,
    ) -> ComptimeResult<ComptimeValue>
    where
        Fi: Fn(i64, i64) -> i64,
        Ff: Fn(f64, f64) -> f64,
    {
        let arg = args.first().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "1 argument".to_string(),
            found: "0 arguments".to_string(),
        })?;

        match (obj, arg) {
            (ComptimeValue::I64(a), ComptimeValue::I64(b)) => Ok(ComptimeValue::I64(int_op(*a, *b))),
            (ComptimeValue::F64(a), ComptimeValue::F64(b)) => Ok(ComptimeValue::F64(float_op(*a, *b))),
            (obj, arg) => {
                // Try to coerce to common type
                if let (Some(a), Some(b)) = (obj.as_i64(), arg.as_i64()) {
                    Ok(ComptimeValue::I64(int_op(a, b)))
                } else if let (Some(a), Some(b)) = (obj.as_f64(), arg.as_f64()) {
                    Ok(ComptimeValue::F64(float_op(a, b)))
                } else {
                    Err(ComptimeError::TypeMismatch {
                        expected: "matching numeric types".to_string(),
                        found: format!("{} and {}", obj.type_name(), arg.type_name()),
                    })
                }
            }
        }
    }

    fn int_binop<F>(
        &self,
        obj: &ComptimeValue,
        args: &[ComptimeValue],
        op: F,
    ) -> ComptimeResult<ComptimeValue>
    where
        F: Fn(i64, i64) -> i64,
    {
        let arg = args.first().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "1 argument".to_string(),
            found: "0 arguments".to_string(),
        })?;

        let a = obj.as_i64().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "integer".to_string(),
            found: obj.type_name().to_string(),
        })?;
        let b = arg.as_i64().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "integer".to_string(),
            found: arg.type_name().to_string(),
        })?;

        Ok(ComptimeValue::I64(op(a, b)))
    }

    fn comparison_op<Fi, Ff>(
        &self,
        obj: &ComptimeValue,
        args: &[ComptimeValue],
        int_op: Fi,
        float_op: Ff,
    ) -> ComptimeResult<ComptimeValue>
    where
        Fi: Fn(i64, i64) -> bool,
        Ff: Fn(f64, f64) -> bool,
    {
        let arg = args.first().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "1 argument".to_string(),
            found: "0 arguments".to_string(),
        })?;

        match (obj, arg) {
            (ComptimeValue::Bool(a), ComptimeValue::Bool(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::Char(a), ComptimeValue::Char(b)) => Ok(ComptimeValue::Bool(a == b)),
            (ComptimeValue::String(a), ComptimeValue::String(b)) => Ok(ComptimeValue::Bool(a == b)),
            (obj, arg) => {
                if let (Some(a), Some(b)) = (obj.as_i64(), arg.as_i64()) {
                    Ok(ComptimeValue::Bool(int_op(a, b)))
                } else if let (Some(a), Some(b)) = (obj.as_f64(), arg.as_f64()) {
                    Ok(ComptimeValue::Bool(float_op(a, b)))
                } else {
                    Err(ComptimeError::TypeMismatch {
                        expected: "comparable types".to_string(),
                        found: format!("{} and {}", obj.type_name(), arg.type_name()),
                    })
                }
            }
        }
    }

    fn pattern_matches(&self, pattern: &Pattern, value: &ComptimeValue) -> ComptimeResult<bool> {
        match pattern {
            Pattern::Wildcard => Ok(true),
            Pattern::Ident(_) => Ok(true), // Binds anything
            Pattern::Literal(_lit) => {
                // Compare literal value - need to evaluate it
                // For now, just return true (simplified)
                Ok(true)
            }
            Pattern::Constructor { name, fields } => {
                if let ComptimeValue::Enum { variant, data, .. } = value {
                    if variant != name {
                        return Ok(false);
                    }
                    // Check fields match
                    match (fields.len(), data) {
                        (0, None) => Ok(true),
                        (1, Some(d)) => self.pattern_matches(&fields[0], d),
                        (n, Some(d)) if n > 1 => {
                            if let ComptimeValue::Tuple(vals) = d.as_ref() {
                                if vals.len() != n {
                                    return Ok(false);
                                }
                                for (p, v) in fields.iter().zip(vals.iter()) {
                                    if !self.pattern_matches(p, v)? {
                                        return Ok(false);
                                    }
                                }
                                Ok(true)
                            } else {
                                Ok(false)
                            }
                        }
                        _ => Ok(false),
                    }
                } else {
                    Ok(false)
                }
            }
            Pattern::Tuple(patterns) => {
                if let ComptimeValue::Tuple(values) = value {
                    if patterns.len() != values.len() {
                        return Ok(false);
                    }
                    for (p, v) in patterns.iter().zip(values.iter()) {
                        if !self.pattern_matches(p, v)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Pattern::Struct { name, fields, rest } => {
                if let ComptimeValue::Struct { name: sname, fields: sfields } = value {
                    if name != sname {
                        return Ok(false);
                    }
                    for (fname, fpat) in fields {
                        if let Some(fval) = sfields.get(fname) {
                            if !self.pattern_matches(fpat, fval)? {
                                return Ok(false);
                            }
                        } else if !*rest {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Pattern::Or(patterns) => {
                for p in patterns {
                    if self.pattern_matches(p, value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn bind_pattern(&mut self, pattern: &Pattern, value: &ComptimeValue) -> ComptimeResult<()> {
        match pattern {
            Pattern::Wildcard => Ok(()),
            Pattern::Ident(name) => {
                self.env.define(name.clone(), value.clone());
                Ok(())
            }
            Pattern::Literal(_) => Ok(()),
            Pattern::Constructor { fields, .. } => {
                if let ComptimeValue::Enum { data, .. } = value {
                    match (fields.len(), data) {
                        (0, _) => Ok(()),
                        (1, Some(d)) => self.bind_pattern(&fields[0], d),
                        (n, Some(d)) if n > 1 => {
                            if let ComptimeValue::Tuple(vals) = d.as_ref() {
                                for (p, v) in fields.iter().zip(vals.iter()) {
                                    self.bind_pattern(p, v)?;
                                }
                            }
                            Ok(())
                        }
                        _ => Ok(()),
                    }
                } else {
                    Ok(())
                }
            }
            Pattern::Tuple(patterns) => {
                if let ComptimeValue::Tuple(values) = value {
                    for (p, v) in patterns.iter().zip(values.iter()) {
                        self.bind_pattern(p, v)?;
                    }
                }
                Ok(())
            }
            Pattern::Struct { fields, .. } => {
                if let ComptimeValue::Struct { fields: sfields, .. } = value {
                    for (fname, fpat) in fields {
                        if let Some(fval) = sfields.get(fname) {
                            self.bind_pattern(fpat, fval)?;
                        }
                    }
                }
                Ok(())
            }
            Pattern::Or(patterns) => {
                // Bind from first matching pattern
                for p in patterns {
                    if self.pattern_matches(p, value)? {
                        return self.bind_pattern(p, value);
                    }
                }
                Ok(())
            }
        }
    }
}

impl Default for ComptimeInterpreter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arithmetic() {
        let mut interp = ComptimeInterpreter::new();

        // We'd need to construct AST nodes for proper testing
        // For now, just verify the interpreter can be created
        assert_eq!(interp.env.branch_quota, 1000);
    }
}

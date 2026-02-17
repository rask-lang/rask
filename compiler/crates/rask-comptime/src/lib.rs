// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Compile-time execution for Rask.
//!
//! Evaluates `comptime` blocks and functions at compile time.
//! Subject to restrictions: no I/O, no pools, no concurrency.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use std::collections::HashMap;
use thiserror::Error;

// ============================================================================
// Comptime Values
// ============================================================================

/// A value that exists at compile time.
#[derive(Debug, Clone)]
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
    Closure {
        params: Vec<String>,
        body: Box<Expr>,
        /// Captured environment at time of closure creation.
        captures: Vec<HashMap<String, ComptimeValue>>,
    },
}

impl PartialEq for ComptimeValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ComptimeValue::Unit, ComptimeValue::Unit) => true,
            (ComptimeValue::Bool(a), ComptimeValue::Bool(b)) => a == b,
            (ComptimeValue::I8(a), ComptimeValue::I8(b)) => a == b,
            (ComptimeValue::I16(a), ComptimeValue::I16(b)) => a == b,
            (ComptimeValue::I32(a), ComptimeValue::I32(b)) => a == b,
            (ComptimeValue::I64(a), ComptimeValue::I64(b)) => a == b,
            (ComptimeValue::U8(a), ComptimeValue::U8(b)) => a == b,
            (ComptimeValue::U16(a), ComptimeValue::U16(b)) => a == b,
            (ComptimeValue::U32(a), ComptimeValue::U32(b)) => a == b,
            (ComptimeValue::U64(a), ComptimeValue::U64(b)) => a == b,
            (ComptimeValue::F32(a), ComptimeValue::F32(b)) => a == b,
            (ComptimeValue::F64(a), ComptimeValue::F64(b)) => a == b,
            (ComptimeValue::Char(a), ComptimeValue::Char(b)) => a == b,
            (ComptimeValue::String(a), ComptimeValue::String(b)) => a == b,
            (ComptimeValue::Array(a), ComptimeValue::Array(b)) => a == b,
            (ComptimeValue::Tuple(a), ComptimeValue::Tuple(b)) => a == b,
            (
                ComptimeValue::Struct { name: n1, fields: f1 },
                ComptimeValue::Struct { name: n2, fields: f2 },
            ) => n1 == n2 && f1 == f2,
            (
                ComptimeValue::Enum { name: n1, variant: v1, data: d1 },
                ComptimeValue::Enum { name: n2, variant: v2, data: d2 },
            ) => n1 == n2 && v1 == v2 && d1 == d2,
            // Closures are never equal (identity semantics)
            (ComptimeValue::Closure { .. }, ComptimeValue::Closure { .. }) => false,
            _ => false,
        }
    }
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
            ComptimeValue::Closure { .. } => "Closure",
        }
    }

    /// Type prefix for method dispatch when embedded as a comptime global.
    pub fn type_prefix(&self) -> &'static str {
        match self {
            ComptimeValue::Array(_) => "Vec",
            _ => self.type_name(),
        }
    }

    /// Element count for Array/Vec values.
    pub fn elem_count(&self) -> usize {
        match self {
            ComptimeValue::Array(elems) => elems.len(),
            _ => 0,
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

    /// Serialize to a flat byte array for embedding in Cranelift data sections.
    /// Only supports primitive arrays — the main use case for comptime globals.
    pub fn serialize(&self) -> Option<Vec<u8>> {
        match self {
            ComptimeValue::Array(elems) => {
                let mut bytes = Vec::new();
                for elem in elems {
                    bytes.extend(elem.serialize_element()?);
                }
                Some(bytes)
            }
            _ => self.serialize_element().map(|b| b.to_vec()),
        }
    }

    /// Serialize a single element to its native byte representation.
    fn serialize_element(&self) -> Option<Vec<u8>> {
        Some(match self {
            ComptimeValue::Bool(b) => vec![*b as u8],
            ComptimeValue::I8(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::I16(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::I32(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::I64(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::U8(v) => vec![*v],
            ComptimeValue::U16(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::U32(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::U64(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::F32(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::F64(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::Char(c) => (*c as u32).to_le_bytes().to_vec(),
            _ => return None,
        })
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

    #[error("non-exhaustive match at comptime; no arm matched the scrutinee value")]
    NonExhaustiveMatch,

    #[error("not supported at comptime: {0}")]
    NotSupported(String),
}

/// Result type for comptime operations.
pub type ComptimeResult<T> = Result<T, ComptimeError>;

/// Check if a name is a known type for static method dispatch at comptime.
fn is_comptime_type(name: &str) -> bool {
    matches!(name, "Vec" | "Map" | "string")
}

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
                let arg_exprs: Vec<_> = args.iter().map(|a| &a.expr).collect();
                self.eval_call(func, &arg_exprs)?
            }

            // Method call (from desugared operators)
            ExprKind::MethodCall { object, method, args, .. } => {
                let arg_exprs: Vec<_> = args.iter().map(|a| &a.expr).collect();
                self.eval_method_call(object, method, &arg_exprs)?
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
                return Err(ComptimeError::NonExhaustiveMatch);
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

            // Nested comptime — already in comptime context, just evaluate the body
            ExprKind::Comptime { body } => {
                return self.eval_block(body);
            }

            // Closure — capture current environment and store for later call
            ExprKind::Closure { params, body, .. } => {
                let param_names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
                let captures = self.env.scopes.clone();
                ComptimeValue::Closure {
                    params: param_names,
                    body: body.clone(),
                    captures,
                }
            }

            // Spawn - not allowed
            ExprKind::Spawn { .. } => {
                return Err(ComptimeError::ConcurrencyNotAllowed);
            }

            // Unsafe - not allowed
            ExprKind::Unsafe { .. } => {
                return Err(ComptimeError::UnsafeNotAllowed);
            }

            // If-let pattern match: if expr is Pattern { then } else { else }
            ExprKind::IfLet { expr, pattern, then_branch, else_branch } => {
                let value = self.eval_expr(expr)?;
                if self.pattern_matches(pattern, &value)? {
                    self.env.push_scope();
                    self.bind_pattern(pattern, &value)?;
                    let result = self.eval_expr_cf(then_branch);
                    self.env.pop_scope();
                    return result;
                } else if let Some(else_br) = else_branch {
                    return self.eval_expr_cf(else_br);
                } else {
                    ComptimeValue::Unit
                }
            }

            // Type cast: expr as Type
            ExprKind::Cast { expr, ty } => {
                let val = self.eval_expr(expr)?;
                match (&val, ty.as_str()) {
                    // int → int
                    (ComptimeValue::I64(n), "i8") => ComptimeValue::I8(*n as i8),
                    (ComptimeValue::I64(n), "i16") => ComptimeValue::I16(*n as i16),
                    (ComptimeValue::I64(n), "i32") => ComptimeValue::I32(*n as i32),
                    (ComptimeValue::I64(n), "i64") => ComptimeValue::I64(*n),
                    (ComptimeValue::I64(n), "u8") => ComptimeValue::U8(*n as u8),
                    (ComptimeValue::I64(n), "u16") => ComptimeValue::U16(*n as u16),
                    (ComptimeValue::I64(n), "u32") => ComptimeValue::U32(*n as u32),
                    (ComptimeValue::I64(n), "u64") => ComptimeValue::U64(*n as u64),
                    (ComptimeValue::I64(n), "f64") => ComptimeValue::F64(*n as f64),
                    (ComptimeValue::I64(n), "f32") => ComptimeValue::F32(*n as f32),
                    // char → int
                    (ComptimeValue::Char(c), "i64" | "usize") => ComptimeValue::I64(*c as i64),
                    (ComptimeValue::Char(c), "u32") => ComptimeValue::U32(*c as u32),
                    (ComptimeValue::Char(c), "u8") => ComptimeValue::U8(*c as u8),
                    // int → char
                    (ComptimeValue::I64(n), "char") => {
                        char::from_u32(*n as u32)
                            .map(ComptimeValue::Char)
                            .unwrap_or(ComptimeValue::Char('\0'))
                    }
                    (ComptimeValue::U32(n), "char") => {
                        char::from_u32(*n)
                            .map(ComptimeValue::Char)
                            .unwrap_or(ComptimeValue::Char('\0'))
                    }
                    // float → int
                    (ComptimeValue::F64(f), "i64") => ComptimeValue::I64(*f as i64),
                    (ComptimeValue::F64(f), "i32") => ComptimeValue::I32(*f as i32),
                    (ComptimeValue::F64(f), "i16") => ComptimeValue::I16(*f as i16),
                    // int → int (small widths)
                    (ComptimeValue::I32(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::I16(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::I8(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::U8(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::U16(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::U32(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::U64(n), "i64") => ComptimeValue::I64(*n as i64),
                    (ComptimeValue::U8(n), "u32") => ComptimeValue::U32(*n as u32),
                    (ComptimeValue::I32(n), "i16") => ComptimeValue::I16(*n as i16),
                    // Identity / pass-through
                    _ => val,
                }
            }

            // Other expressions not yet supported
            _ => {
                let kind_name = match &expr.kind {
                    ExprKind::BlockCall { name, .. } => format!("`{name} {{ }}`"),
                    _ => format!("{:?}", std::mem::discriminant(&expr.kind)),
                };
                return Err(ComptimeError::NotSupported(kind_name));
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

            StmtKind::Break { label, value } => {
                if label.is_some() {
                    return Err(ComptimeError::NotSupported("labeled break".to_string()));
                }
                let val = if let Some(v) = value {
                    Some(self.eval_expr(v)?)
                } else {
                    None
                };
                Ok(ControlFlow::Break(val))
            }

            StmtKind::Continue(label) => {
                if label.is_some() {
                    return Err(ComptimeError::NotSupported("labeled continue".to_string()));
                }
                Ok(ControlFlow::Continue)
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
                    match binding {
                        ForBinding::Single(name) => self.env.define(name.clone(), item),
                        ForBinding::Tuple(names) => {
                            if let ComptimeValue::Array(fields) = item {
                                for (i, name) in names.iter().enumerate() {
                                    let val = fields.get(i).cloned().unwrap_or(ComptimeValue::Unit);
                                    self.env.define(name.clone(), val);
                                }
                            }
                        }
                    }

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

    fn eval_call(&mut self, func: &Expr, args: &[&Expr]) -> ComptimeResult<ComptimeValue> {
        // Evaluate arguments first
        let arg_values: ComptimeResult<Vec<_>> = args.iter().map(|a| self.eval_expr(a)).collect();
        let arg_values = arg_values?;

        // If the callee is an identifier, check named functions/builtins first,
        // then fall back to variable lookup (could be a closure).
        if let ExprKind::Ident(name) = &func.kind {
            if let Some(func_decl) = self.env.get_function(name).cloned() {
                self.env.count_branch()?;
                return self.call_function(&func_decl, arg_values);
            }

            // Check if it's a closure stored in a variable
            if let Some(val) = self.env.get(name).cloned() {
                if let ComptimeValue::Closure { params, body, captures } = val {
                    self.env.count_branch()?;
                    return self.call_closure(&params, &body, &captures, arg_values);
                }
            }

            // Check for builtin functions
            return self.call_builtin(name, arg_values);
        }

        // Static method call: Type.method(args) — e.g. Vec.new()
        if let ExprKind::Field { object, field } = &func.kind {
            if let ExprKind::Ident(type_name) = &object.kind {
                return self.call_static_method(type_name, field, arg_values);
            }
        }

        // Non-ident callee — evaluate it; if it produces a closure, call it
        let callee = self.eval_expr(func)?;
        if let ComptimeValue::Closure { params, body, captures } = callee {
            self.env.count_branch()?;
            self.call_closure(&params, &body, &captures, arg_values)
        } else {
            Err(ComptimeError::NotSupported("indirect function calls".to_string()))
        }
    }

    fn eval_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[&Expr],
    ) -> ComptimeResult<ComptimeValue> {
        // Static method call on a type: Vec.new(), Map.new()
        if let ExprKind::Ident(name) = &object.kind {
            if !self.env.get(name).is_some() && is_comptime_type(name) {
                let arg_values: ComptimeResult<Vec<_>> = args.iter().map(|a| self.eval_expr(a)).collect();
                let arg_values = arg_values?;
                return self.call_static_method(name, method, arg_values);
            }
        }

        // Mutating Vec methods: push, pop — need to update the variable in-place
        if matches!(method, "push" | "pop" | "insert" | "remove" | "clear") {
            if let ExprKind::Ident(var_name) = &object.kind {
                let arg_values: ComptimeResult<Vec<_>> = args.iter().map(|a| self.eval_expr(a)).collect();
                let arg_values = arg_values?;
                return self.call_mutating_vec_method(var_name, method, &arg_values);
            }
        }

        let obj = self.eval_expr(object)?;
        let arg_values: ComptimeResult<Vec<_>> = args.iter().map(|a| self.eval_expr(a)).collect();
        let arg_values = arg_values?;

        // Handle primitive methods (from desugared operators) + Vec read methods
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

    fn call_closure(
        &mut self,
        params: &[String],
        body: &Expr,
        captures: &[HashMap<String, ComptimeValue>],
        args: Vec<ComptimeValue>,
    ) -> ComptimeResult<ComptimeValue> {
        if params.len() != args.len() {
            return Err(ComptimeError::TypeMismatch {
                expected: format!("{} arguments", params.len()),
                found: format!("{} arguments", args.len()),
            });
        }

        // Swap in the captured environment, preserving current env
        let saved_scopes = std::mem::replace(&mut self.env.scopes, captures.to_vec());

        // New scope for parameters
        self.env.push_scope();
        for (name, value) in params.iter().zip(args) {
            self.env.define(name.clone(), value);
        }

        let result = self.eval_expr_cf(body);

        // Restore original environment
        self.env.scopes = saved_scopes;

        Ok(result?.value())
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

    /// Handle static method calls on types: Vec.new(), Map.new(), etc.
    fn call_static_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: Vec<ComptimeValue>,
    ) -> ComptimeResult<ComptimeValue> {
        match (type_name, method) {
            ("Vec", "new") => Ok(ComptimeValue::Array(Vec::new())),
            ("Vec", "from") if args.len() == 1 => {
                // Vec.from(array) — clone the array
                match &args[0] {
                    ComptimeValue::Array(arr) => Ok(ComptimeValue::Array(arr.clone())),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "Array".to_string(),
                        found: args[0].type_name().to_string(),
                    }),
                }
            }
            _ => Err(ComptimeError::NotSupported(
                format!("static method {}.{}", type_name, method),
            )),
        }
    }

    /// Handle mutating Vec methods that update the variable in the environment.
    fn call_mutating_vec_method(
        &mut self,
        var_name: &str,
        method: &str,
        args: &[ComptimeValue],
    ) -> ComptimeResult<ComptimeValue> {
        let val = self.env.get(var_name)
            .ok_or_else(|| ComptimeError::UndefinedVariable(var_name.to_string()))?
            .clone();

        let mut arr = match val {
            ComptimeValue::Array(arr) => arr,
            _ => return Err(ComptimeError::TypeMismatch {
                expected: "Vec/Array".to_string(),
                found: val.type_name().to_string(),
            }),
        };

        let result = match method {
            "push" => {
                if args.len() != 1 {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "1 argument".to_string(),
                        found: format!("{} arguments", args.len()),
                    });
                }
                arr.push(args[0].clone());
                ComptimeValue::Unit
            }
            "pop" => {
                arr.pop()
                    .map(|v| v)
                    .unwrap_or(ComptimeValue::Unit)
            }
            "insert" => {
                if args.len() != 2 {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "2 arguments".to_string(),
                        found: format!("{} arguments", args.len()),
                    });
                }
                let idx = args[0].as_i64().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "integer index".to_string(),
                    found: args[0].type_name().to_string(),
                })? as usize;
                if idx > arr.len() {
                    return Err(ComptimeError::IndexOutOfBounds { index: idx, len: arr.len() });
                }
                arr.insert(idx, args[1].clone());
                ComptimeValue::Unit
            }
            "remove" => {
                if args.len() != 1 {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "1 argument".to_string(),
                        found: format!("{} arguments", args.len()),
                    });
                }
                let idx = args[0].as_i64().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "integer index".to_string(),
                    found: args[0].type_name().to_string(),
                })? as usize;
                if idx >= arr.len() {
                    return Err(ComptimeError::IndexOutOfBounds { index: idx, len: arr.len() });
                }
                arr.remove(idx);
                ComptimeValue::Unit
            }
            "clear" => {
                arr.clear();
                ComptimeValue::Unit
            }
            _ => return Err(ComptimeError::NotSupported(
                format!("mutating method .{}", method),
            )),
        };

        // Write back the modified array
        if !self.env.assign(var_name, ComptimeValue::Array(arr)) {
            return Err(ComptimeError::UndefinedVariable(var_name.to_string()));
        }
        Ok(result)
    }

    fn call_primitive_method(
        &self,
        obj: &ComptimeValue,
        method: &str,
        args: &[ComptimeValue],
    ) -> ComptimeResult<ComptimeValue> {
        // Vec/Array read methods
        if let ComptimeValue::Array(arr) = obj {
            match method {
                "get" => {
                    let idx = args.first()
                        .and_then(|a| a.as_i64())
                        .ok_or_else(|| ComptimeError::TypeMismatch {
                            expected: "integer index".to_string(),
                            found: args.first().map(|a| a.type_name()).unwrap_or("nothing").to_string(),
                        })? as usize;
                    return arr.get(idx).cloned().ok_or(ComptimeError::IndexOutOfBounds {
                        index: idx,
                        len: arr.len(),
                    });
                }
                "is_empty" => return Ok(ComptimeValue::Bool(arr.is_empty())),
                "contains" => {
                    let needle = args.first().ok_or_else(|| ComptimeError::TypeMismatch {
                        expected: "1 argument".to_string(),
                        found: "0 arguments".to_string(),
                    })?;
                    return Ok(ComptimeValue::Bool(arr.contains(needle)));
                }
                _ => {} // fall through to numeric/string methods
            }
        }

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

    fn pattern_matches(&mut self, pattern: &Pattern, value: &ComptimeValue) -> ComptimeResult<bool> {
        match pattern {
            Pattern::Wildcard => Ok(true),
            Pattern::Ident(_) => Ok(true), // Binds anything
            Pattern::Literal(lit) => {
                let lit_val = self.eval_expr(lit)?;
                Ok(lit_val == *value)
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

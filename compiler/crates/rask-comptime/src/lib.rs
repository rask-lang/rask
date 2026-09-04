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
// Build Configuration (CT11-CT16)
// ============================================================================

/// Build configuration for conditional compilation.
///
/// Provides `cfg.os`, `cfg.arch`, `cfg.env`, `cfg.profile`, `cfg.debug`,
/// and `cfg.features` for use in `comptime if` blocks.
#[derive(Debug, Clone)]
pub struct CfgConfig {
    pub os: String,
    pub arch: String,
    pub env: String,
    pub profile: String,
    pub features: Vec<String>,
}

impl CfgConfig {
    /// Detect from the current host platform.
    pub fn from_host(profile: &str, features: Vec<String>) -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            env: detect_env(),
            profile: profile.to_string(),
            features,
        }
    }

    /// Parse from a target triple (e.g. "x86_64-linux-musl").
    pub fn from_target(target: &str, profile: &str, features: Vec<String>) -> Self {
        let parts: Vec<&str> = target.splitn(3, '-').collect();
        Self {
            arch: parts.first().unwrap_or(&"unknown").to_string(),
            os: parts.get(1).unwrap_or(&"unknown").to_string(),
            env: parts.get(2).unwrap_or(&"gnu").to_string(),
            profile: profile.to_string(),
            features,
        }
    }

    /// Dispatch based on whether a target triple is provided.
    pub fn from_target_or_host(target: Option<&str>, profile: &str, features: Vec<String>) -> Self {
        match target {
            Some(t) => Self::from_target(t, profile, features),
            None => Self::from_host(profile, features),
        }
    }

    /// Convert to a flat map of field name → value for the resolver's
    /// dead branch elimination in `comptime if`.
    pub fn to_cfg_values(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("os".to_string(), self.os.clone());
        m.insert("arch".to_string(), self.arch.clone());
        m.insert("env".to_string(), self.env.clone());
        m.insert("profile".to_string(), self.profile.clone());
        m
    }

    /// Convert to a `ComptimeValue::Struct` for injection into the comptime environment.
    pub fn to_comptime_value(&self) -> ComptimeValue {
        let mut fields = HashMap::new();
        fields.insert("os".to_string(), ComptimeValue::String(self.os.clone()));
        fields.insert("arch".to_string(), ComptimeValue::String(self.arch.clone()));
        fields.insert("env".to_string(), ComptimeValue::String(self.env.clone()));
        fields.insert("profile".to_string(), ComptimeValue::String(self.profile.clone()));
        fields.insert("debug".to_string(), ComptimeValue::Bool(self.profile == "debug"));
        fields.insert(
            "features".to_string(),
            ComptimeValue::Array(
                self.features.iter().map(|f| ComptimeValue::String(f.clone())).collect(),
            ),
        );
        ComptimeValue::Struct {
            name: "Cfg".to_string(),
            fields,
        }
    }
}

fn detect_env() -> String {
    if cfg!(target_env = "musl") {
        "musl".to_string()
    } else if cfg!(target_env = "msvc") {
        "msvc".to_string()
    } else if cfg!(target_os = "linux") {
        "gnu".to_string()
    } else {
        "unknown".to_string()
    }
}

// ============================================================================
// Conditional Compilation — Dead Branch Elimination (CC1)
// ============================================================================

/// Eliminate dead branches in `comptime if cfg.*` blocks.
///
/// Walks the AST and replaces `comptime { if cfg.field == "value" { A } else { B } }`
/// with either `A` or `B` statements. Runs before desugar so `==` is still `Binary { Eq }`.
pub fn eliminate_comptime_if(decls: &mut [Decl], cfg: &CfgConfig) {
    let cfg_values = cfg.to_cfg_values();
    for decl in decls {
        eliminate_in_decl(decl, &cfg_values);
    }
}

fn eliminate_in_decl(decl: &mut Decl, cfg_values: &HashMap<String, String>) {
    match &mut decl.kind {
        DeclKind::Fn(f) => eliminate_in_fn_body(&mut f.body, cfg_values),
        DeclKind::Struct(s) => {
            for m in &mut s.methods { eliminate_in_fn_body(&mut m.body, cfg_values); }
        }
        DeclKind::Enum(e) => {
            for m in &mut e.methods { eliminate_in_fn_body(&mut m.body, cfg_values); }
        }
        DeclKind::Trait(t) => {
            for m in &mut t.methods { eliminate_in_fn_body(&mut m.body, cfg_values); }
        }
        DeclKind::Impl(i) => {
            for m in &mut i.methods { eliminate_in_fn_body(&mut m.body, cfg_values); }
        }
        DeclKind::Const(c) => eliminate_in_expr(&mut c.init, cfg_values),
        DeclKind::Test(t) => eliminate_in_stmts(&mut t.body, cfg_values),
        DeclKind::Benchmark(b) => eliminate_in_stmts(&mut b.body, cfg_values),
        _ => {}
    }
}

fn eliminate_in_fn_body(body: &mut Vec<Stmt>, cfg_values: &HashMap<String, String>) {
    eliminate_in_stmts(body, cfg_values);
}

fn eliminate_in_stmts(stmts: &mut Vec<Stmt>, cfg_values: &HashMap<String, String>) {
    let mut i = 0;
    while i < stmts.len() {
        if let StmtKind::Comptime(body) = &stmts[i].kind {
            if let Some(replacement) = try_eval_comptime_if_stmts(body, cfg_values) {
                // Replace the comptime stmt with the taken branch's stmts
                stmts.splice(i..=i, replacement);
                continue; // Re-check at same index (new stmts may contain comptime)
            }
        }
        // Recurse into the statement
        eliminate_in_stmt(&mut stmts[i], cfg_values);
        i += 1;
    }
}

fn eliminate_in_stmt(stmt: &mut Stmt, cfg_values: &HashMap<String, String>) {
    match &mut stmt.kind {
        StmtKind::Expr(e) => eliminate_in_expr(e, cfg_values),
        StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => eliminate_in_expr(init, cfg_values),
        StmtKind::MutTuple { init, .. }
        | StmtKind::LetTuple { init, .. }
        | StmtKind::LetStruct { init, .. } => eliminate_in_expr(init, cfg_values),
        StmtKind::Assign { target, value, .. } => {
            eliminate_in_expr(target, cfg_values);
            eliminate_in_expr(value, cfg_values);
        }
        StmtKind::Return(Some(e)) => eliminate_in_expr(e, cfg_values),
        StmtKind::While { cond, body, .. } => {
            eliminate_in_expr(cond, cfg_values);
            eliminate_in_stmts(body, cfg_values);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            eliminate_in_expr(expr, cfg_values);
            eliminate_in_stmts(body, cfg_values);
        }
        StmtKind::Loop { body, .. } => eliminate_in_stmts(body, cfg_values),
        StmtKind::For { iter, body, .. } => {
            eliminate_in_expr(iter, cfg_values);
            eliminate_in_stmts(body, cfg_values);
        }
        StmtKind::Ensure { body, else_handler } => {
            eliminate_in_stmts(body, cfg_values);
            if let Some((_, handler)) = else_handler {
                eliminate_in_stmts(handler, cfg_values);
            }
        }
        StmtKind::Comptime(body) => eliminate_in_stmts(body, cfg_values),
        StmtKind::ComptimeFor { iter, body, .. } => {
            eliminate_in_expr(iter, cfg_values);
            eliminate_in_stmts(body, cfg_values);
        }
        _ => {}
    }
}

fn eliminate_in_expr(expr: &mut Expr, cfg_values: &HashMap<String, String>) {
    match &mut expr.kind {
        ExprKind::Binary { left, right, .. } => {
            eliminate_in_expr(left, cfg_values);
            eliminate_in_expr(right, cfg_values);
        }
        ExprKind::Unary { operand, .. } => eliminate_in_expr(operand, cfg_values),
        ExprKind::Call { func, args } => {
            eliminate_in_expr(func, cfg_values);
            for arg in args { eliminate_in_expr(&mut arg.expr, cfg_values); }
        }
        ExprKind::MethodCall { object, args, .. } => {
            eliminate_in_expr(object, cfg_values);
            for arg in args { eliminate_in_expr(&mut arg.expr, cfg_values); }
        }
        ExprKind::Field { object, .. } => eliminate_in_expr(object, cfg_values),
        ExprKind::Index { object, index } => {
            eliminate_in_expr(object, cfg_values);
            eliminate_in_expr(index, cfg_values);
        }
        ExprKind::Block(stmts) => eliminate_in_stmts(stmts, cfg_values),
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            eliminate_in_expr(cond, cfg_values);
            eliminate_in_expr(then_branch, cfg_values);
            if let Some(e) = else_branch { eliminate_in_expr(e, cfg_values); }
        }
        ExprKind::Match { scrutinee, arms } => {
            eliminate_in_expr(scrutinee, cfg_values);
            for arm in arms { eliminate_in_expr(&mut arm.body, cfg_values); }
        }
        ExprKind::Closure { body, .. } => eliminate_in_expr(body, cfg_values),
        ExprKind::Comptime { body } => {
            // Check if this is `comptime if cfg.* { ... } else { ... }` in expression context
            if let Some(replacement) = try_eval_comptime_if_stmts(body, cfg_values) {
                // Replace the comptime expression with a block containing the taken branch
                expr.kind = ExprKind::Block(replacement);
                return;
            }
            eliminate_in_stmts(body, cfg_values);
        }
        ExprKind::Unsafe { body } => eliminate_in_stmts(body, cfg_values),
        ExprKind::Spawn { body } => eliminate_in_stmts(body, cfg_values),
        ExprKind::Loop { body, .. } => eliminate_in_stmts(body, cfg_values),
        ExprKind::StructLit { fields, .. } => {
            for f in fields { eliminate_in_expr(&mut f.value, cfg_values); }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems { eliminate_in_expr(e, cfg_values); }
        }
        _ => {}
    }
}

/// Try to evaluate a comptime if cfg condition and return the taken branch.
fn try_eval_comptime_if_stmts(stmts: &[Stmt], cfg_values: &HashMap<String, String>) -> Option<Vec<Stmt>> {
    if stmts.len() != 1 {
        return None;
    }
    let inner = match &stmts[0].kind {
        StmtKind::Expr(e) => e,
        _ => return None,
    };
    let (cond, then_branch, else_branch) = match &inner.kind {
        ExprKind::If { cond, then_branch, else_branch, .. } => (cond, then_branch, else_branch),
        _ => return None,
    };

    let taken = eval_cfg_condition(cond, cfg_values)?;
    if taken {
        if let ExprKind::Block(block_stmts) = &then_branch.kind {
            Some(block_stmts.clone())
        } else {
            None
        }
    } else if let Some(else_br) = else_branch {
        if let ExprKind::Block(block_stmts) = &else_br.kind {
            Some(block_stmts.clone())
        } else {
            None
        }
    } else {
        Some(vec![])
    }
}

/// Evaluate a cfg condition at compile time.
/// Supports: `cfg.field == "value"`, `cfg.field != "value"`,
/// `!expr`, `expr && expr`, `expr || expr`.
fn eval_cfg_condition(expr: &Expr, cfg_values: &HashMap<String, String>) -> Option<bool> {
    match &expr.kind {
        ExprKind::Binary { op, left, right } => {
            match op {
                BinOp::Eq | BinOp::Ne => {
                    let (field, value) = extract_cfg_comparison(left, right)?;
                    let cfg_val = cfg_values.get(field)?;
                    let result = cfg_val.as_str() == value;
                    Some(if *op == BinOp::Eq { result } else { !result })
                }
                BinOp::And => {
                    Some(eval_cfg_condition(left, cfg_values)? && eval_cfg_condition(right, cfg_values)?)
                }
                BinOp::Or => {
                    Some(eval_cfg_condition(left, cfg_values)? || eval_cfg_condition(right, cfg_values)?)
                }
                _ => None,
            }
        }
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            Some(!eval_cfg_condition(operand, cfg_values)?)
        }
        _ => None,
    }
}

fn extract_cfg_comparison<'a>(left: &'a Expr, right: &'a Expr) -> Option<(&'a str, &'a str)> {
    if let Some(field) = extract_cfg_field(left) {
        if let ExprKind::String(val) = &right.kind { return Some((field, val)); }
    }
    if let Some(field) = extract_cfg_field(right) {
        if let ExprKind::String(val) = &left.kind { return Some((field, val)); }
    }
    None
}

fn extract_cfg_field(expr: &Expr) -> Option<&str> {
    if let ExprKind::Field { object, field } = &expr.kind {
        if let ExprKind::Ident(name) = &object.kind {
            if name == "cfg" { return Some(field); }
        }
    }
    None
}

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
    I128(i128),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
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
            (ComptimeValue::I128(a), ComptimeValue::I128(b)) => a == b,
            (ComptimeValue::U8(a), ComptimeValue::U8(b)) => a == b,
            (ComptimeValue::U16(a), ComptimeValue::U16(b)) => a == b,
            (ComptimeValue::U32(a), ComptimeValue::U32(b)) => a == b,
            (ComptimeValue::U64(a), ComptimeValue::U64(b)) => a == b,
            (ComptimeValue::U128(a), ComptimeValue::U128(b)) => a == b,
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
            ComptimeValue::I128(_) => "i128",
            ComptimeValue::U8(_) => "u8",
            ComptimeValue::U16(_) => "u16",
            ComptimeValue::U32(_) => "u32",
            ComptimeValue::U64(_) => "u64",
            ComptimeValue::U128(_) => "u128",
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

    /// Type prefix for method dispatch when embedded as a comptime global — the
    /// Rask type name, which is what a method call is prefixed with
    /// (`i64_to_string`, `string_to_uppercase`, `Vec_get`).
    ///
    /// `type_name` spells a string `String`, for error messages; the dispatch
    /// prefix is `string`. Keep this in step with `MiriValue::type_prefix`,
    /// which writes the same field (#824).
    pub fn type_prefix(&self) -> &'static str {
        match self {
            ComptimeValue::Array(_) => "Vec",
            ComptimeValue::String(_) => "string",
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

    /// The Rask type name of what an Array holds, spelled the way a type
    /// annotation would be.
    ///
    /// A comptime global keeps its bytes and its length but used to drop what
    /// those bytes *are*, so `SQUARES[i]` on a `const SQUARES = comptime { … }`
    /// had nothing to go on and lowering guessed i64 — right for these, wrong
    /// the moment a comptime block builds floats or strings.
    pub fn elem_type_name(&self) -> Option<&'static str> {
        let ComptimeValue::Array(elems) = self else { return None };
        let first = elems.first()?;
        Some(match first {
            // `type_name` spells this one for error messages, not as a type.
            ComptimeValue::String(_) => "string",
            ComptimeValue::Array(_) | ComptimeValue::Tuple(_)
            | ComptimeValue::Struct { .. } | ComptimeValue::Enum { .. }
            | ComptimeValue::Closure { .. } | ComptimeValue::Unit => return None,
            other => other.type_name(),
        })
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
            ComptimeValue::I128(v) => Some(*v as i64),  // May overflow
            ComptimeValue::U128(v) => Some(*v as i64),  // May overflow
            _ => None,
        }
    }

    /// The logical value and width kind of an integer variant.
    fn as_int(&self) -> Option<(CtNum, CtInt)> {
        Some(match self {
            ComptimeValue::I8(v) => (CtNum::Signed(*v as i128), CtInt::I8),
            ComptimeValue::I16(v) => (CtNum::Signed(*v as i128), CtInt::I16),
            ComptimeValue::I32(v) => (CtNum::Signed(*v as i128), CtInt::I32),
            ComptimeValue::I64(v) => (CtNum::Signed(*v as i128), CtInt::I64),
            ComptimeValue::U8(v) => (CtNum::Signed(*v as i128), CtInt::U8),
            ComptimeValue::U16(v) => (CtNum::Signed(*v as i128), CtInt::U16),
            ComptimeValue::U32(v) => (CtNum::Signed(*v as i128), CtInt::U32),
            ComptimeValue::U64(v) => (CtNum::Signed(*v as i128), CtInt::U64),
            ComptimeValue::I128(v) => (CtNum::Signed(*v), CtInt::I128),
            ComptimeValue::U128(v) => (CtNum::Big(*v), CtInt::U128),
            _ => return None,
        })
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
            ComptimeValue::I128(v) => v.to_le_bytes().to_vec(),
            ComptimeValue::U128(v) => v.to_le_bytes().to_vec(),
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
    #[error("comptime exceeded backwards branch quota ({0}); raise it with @comptime_quota(N) on the const")]
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

    #[error("integer overflow in comptime evaluation: {0}")]
    IntegerOverflow(String),

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

    /// `todo()` reached at comptime. Separate from `Panic` because `todo()`
    /// means "not written yet" — a program full of them still has to compile,
    /// which is the whole point of the placeholder. The value falls back to
    /// runtime, where the same `todo()` panics.
    #[error("not yet implemented at comptime: {0}")]
    Unimplemented(String),

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

    #[error("comptime stack overflow (depth {0}); reduce recursion or increase limit")]
    StackOverflow(usize),
}

impl ComptimeError {
    /// Hard errors are genuine compile errors — the evaluator ran the code and
    /// the code, or what it asked for, is the problem. There is no answer to
    /// fall back to: `const X = comptime { … }` promises a value computed at
    /// compile time (CT2), and running the block again at runtime would only
    /// reach the same panic or the same limit.
    ///
    /// The spec's own error table says as much: quota (CT35), panic and
    /// out-of-bounds (CT46), I/O (CT7), pools (CT31), concurrency (CT33),
    /// memory (CT37) are all listed as compile errors.
    ///
    /// Everything else is the evaluator's own gap — a construct it can't model
    /// yet — and that's soft: the const runs at runtime and the caller warns.
    pub fn is_hard(&self) -> bool {
        use ComptimeError::*;
        matches!(
            self,
            IntegerOverflow(_)
                | DivisionByZero
                | BranchQuotaExceeded(_)
                | TimeoutExceeded
                | MemoryLimitExceeded
                | StackOverflow(_)
                | IndexOutOfBounds { .. }
                | NonExhaustiveMatch
                | Panic(_)
                | IoNotAllowed
                | PoolsNotAllowed
                | ConcurrencyNotAllowed
                | UnsafeNotAllowed
        )
    }
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
    /// Maximum allowed backwards branches (CT35: default 1,000).
    branch_quota: usize,
    /// Current call stack depth (CT29).
    call_depth: usize,
    /// Maximum allowed call depth (CT29).
    max_call_depth: usize,
}

impl ComptimeEnv {
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            branch_count: 0,
            branch_quota: 1_000, // CT35: default 1,000
            call_depth: 0,
            max_call_depth: 256, // CT29: stack depth limit
        }
    }

    pub fn with_quota(quota: usize) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            branch_count: 0,
            branch_quota: quota,
            call_depth: 0,
            max_call_depth: 256,
        }
    }

    /// Reset branch counter between independent comptime evaluations.
    pub fn reset_branch_count(&mut self) {
        self.branch_count = 0;
    }

    /// CT35: raise or lower the quota for the next evaluation (`@comptime_quota`).
    pub fn set_quota(&mut self, quota: usize) {
        self.branch_quota = quota;
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

    /// CT29: track call depth to prevent stack overflow.
    fn push_call(&mut self) -> ComptimeResult<()> {
        self.call_depth += 1;
        if self.call_depth > self.max_call_depth {
            Err(ComptimeError::StackOverflow(self.max_call_depth))
        } else {
            Ok(())
        }
    }

    fn pop_call(&mut self) {
        self.call_depth = self.call_depth.saturating_sub(1);
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
/// Integer width of a comptime value, for width-aware overflow checks (CT1).
/// I64 doubles as the unsuffixed-literal default.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CtInt { I8, I16, I32, I64, I128, U8, U16, U32, U64, U128 }

#[derive(Clone, Copy)]
pub(crate) enum CtOp { Add, Sub, Mul, Div, Rem, Shl, Shr, BitAnd, BitOr, BitXor }

/// A comptime integer operand.
///
/// An `i128` carries every width exactly except the top half of `u128`. That half
/// used to have nowhere to go: `as_int` refused it, so a fold involving a large
/// `u128` silently didn't happen and the const was computed at run time instead —
/// no diagnostic, just no constant folding (#802). Two variants rather than one
/// wider carrier, because a signed 256-bit type would only exist to hold values no
/// Rask type has.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CtNum {
    Signed(i128),
    Big(u128),
}

impl CtNum {
    /// `None` when the value is a `u128` past `i128::MAX` — the caller is on the
    /// signed path and has to report a range error rather than truncate.
    fn to_i128(self) -> Option<i128> {
        match self {
            CtNum::Signed(v) => Some(v),
            CtNum::Big(v) => i128::try_from(v).ok(),
        }
    }

    /// `None` for a negative value: nothing negative is a `u128`.
    fn to_u128(self) -> Option<u128> {
        match self {
            CtNum::Signed(v) => u128::try_from(v).ok(),
            CtNum::Big(v) => Some(v),
        }
    }

    /// Ordering across the two carriers. A negative is below every `Big`, and a
    /// non-negative signed value compares as the unsigned number it is.
    fn cmp(self, other: CtNum) -> std::cmp::Ordering {
        match (self.to_u128(), other.to_u128()) {
            (Some(a), Some(b)) => a.cmp(&b),
            // Only a negative fails the conversion, and a negative is smaller.
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, None) => {
                let a = if let CtNum::Signed(v) = self { v } else { 0 };
                let b = if let CtNum::Signed(v) = other { v } else { 0 };
                a.cmp(&b)
            }
        }
    }
}

impl std::fmt::Display for CtNum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CtNum::Signed(v) => write!(f, "{}", v),
            CtNum::Big(v) => write!(f, "{}", v),
        }
    }
}

impl CtInt {
    fn signed(self) -> bool { matches!(self, CtInt::I8 | CtInt::I16 | CtInt::I32 | CtInt::I64 | CtInt::I128) }
    fn bits(self) -> u32 {
        match self {
            CtInt::I8 | CtInt::U8 => 8,
            CtInt::I16 | CtInt::U16 => 16,
            CtInt::I32 | CtInt::U32 => 32,
            CtInt::I64 | CtInt::U64 => 64,
            CtInt::I128 | CtInt::U128 => 128,
        }
    }
    /// The kind a type annotation names, or `None` if it isn't an integer width.
    fn from_name(name: &str) -> Option<CtInt> {
        Some(match name.trim() {
            "i8" => CtInt::I8, "i16" => CtInt::I16, "i32" => CtInt::I32,
            "i64" => CtInt::I64, "i128" => CtInt::I128,
            "u8" => CtInt::U8, "u16" => CtInt::U16, "u32" => CtInt::U32,
            "u64" => CtInt::U64, "u128" => CtInt::U128,
            // P2: pointer-sized, decided in one place.
            "isize" => if rask_ast::primitives::pointer_bits() == 32 { CtInt::I32 } else { CtInt::I64 },
            "usize" => if rask_ast::primitives::pointer_bits() == 32 { CtInt::U32 } else { CtInt::U64 },
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            CtInt::I8 => "i8", CtInt::I16 => "i16", CtInt::I32 => "i32", CtInt::I64 => "i64",
            CtInt::I128 => "i128",
            CtInt::U8 => "u8", CtInt::U16 => "u16", CtInt::U32 => "u32", CtInt::U64 => "u64",
            CtInt::U128 => "u128",
        }
    }
    fn min(self) -> i128 {
        match self {
            CtInt::I128 => i128::MIN,
            _ if self.signed() => -(1i128 << (self.bits() - 1)),
            _ => 0,
        }
    }
    /// The type's own maximum. `u128`'s doesn't fit an `i128`, so it answers on
    /// the unsigned path instead — see `max_u128` (#802).
    fn max(self) -> i128 {
        match self {
            CtInt::I128 | CtInt::U128 => i128::MAX,
            _ if self.signed() => (1i128 << (self.bits() - 1)) - 1,
            _ => (1i128 << self.bits()) - 1,
        }
    }
    /// The maximum as an unsigned number, for the widths that reach past
    /// `i128::MAX`. Only meaningful for unsigned kinds.
    fn max_u128(self) -> u128 {
        match self {
            CtInt::U128 => u128::MAX,
            _ => self.max() as u128,
        }
    }
    /// The range, spelled for a diagnostic. `u128` can't say its own top end in
    /// an `i128`, so the two paths print it differently.
    fn range_text(self) -> String {
        if self == CtInt::U128 {
            format!("[0, {}]", u128::MAX)
        } else {
            format!("[{}, {}]", self.min(), self.max())
        }
    }
    /// Pick the more specific kind. I64 is the untyped default and yields.
    fn unify(self, other: CtInt) -> CtInt {
        match (self, other) {
            (CtInt::I64, k) | (k, CtInt::I64) => k,
            (a, _) => a,
        }
    }
    fn make(self, v: i128) -> ComptimeValue {
        match self {
            CtInt::I8 => ComptimeValue::I8(v as i8),
            CtInt::I16 => ComptimeValue::I16(v as i16),
            CtInt::I32 => ComptimeValue::I32(v as i32),
            CtInt::I64 => ComptimeValue::I64(v as i64),
            CtInt::I128 => ComptimeValue::I128(v),
            CtInt::U8 => ComptimeValue::U8(v as u8),
            CtInt::U16 => ComptimeValue::U16(v as u16),
            CtInt::U32 => ComptimeValue::U32(v as u32),
            CtInt::U64 => ComptimeValue::U64(v as u64),
            CtInt::U128 => ComptimeValue::U128(v as u128),
        }
    }
    /// Build from an unsigned value. The signed `make` can't carry the top half of
    /// a `u128`, and casting through `i128` there would flip the sign bit.
    fn make_u128(self, v: u128) -> ComptimeValue {
        match self {
            CtInt::U128 => ComptimeValue::U128(v),
            _ => self.make(v as i128),
        }
    }
    fn wrap(self, v: i128) -> i128 {
        let bits = self.bits();
        if bits >= 128 { return v; }
        let mask = (1i128 << bits) - 1;
        let masked = v & mask;
        if self.signed() && (masked & (1i128 << (bits - 1))) != 0 { masked - (1i128 << bits) } else { masked }
    }
}

fn ct_op_symbol(op: CtOp) -> &'static str {
    match op {
        CtOp::Add => "+", CtOp::Sub => "-", CtOp::Mul => "*",
        CtOp::Div => "/", CtOp::Rem => "%", CtOp::Shl => "<<", CtOp::Shr => ">>",
        CtOp::BitAnd => "&", CtOp::BitOr => "|", CtOp::BitXor => "^",
    }
}

fn ct_overflow_of(kind: CtInt, op: CtOp, a: impl std::fmt::Display, b: impl std::fmt::Display) -> ComptimeError {
    ComptimeError::IntegerOverflow(format!(
        "{} {} {} exceeds {} range {}", a, ct_op_symbol(op), b, kind.name(), kind.range_text()
    ))
}

fn ct_overflow(kind: CtInt, op: CtOp, a: i128, b: i128) -> ComptimeError {
    ct_overflow_of(kind, op, a, b)
}

/// Width-aware checked comptime integer arithmetic (CT1).
///
/// Splits on whether the result width is `u128`: that one is computed in a `u128`,
/// because half its range has no `i128` to be computed in. Every other width fits
/// the signed carrier exactly (#802).
pub(crate) fn ct_checked_binop(kind: CtInt, op: CtOp, a: CtNum, b: CtNum) -> ComptimeResult<ComptimeValue> {
    if kind == CtInt::U128 {
        return ct_checked_binop_u128(op, a, b);
    }
    let range = |v: CtNum| ComptimeError::IntegerOverflow(format!(
        "{} is outside {} range {}", v, kind.name(), kind.range_text()
    ));
    let a = a.to_i128().ok_or_else(|| range(a))?;
    let b = b.to_i128().ok_or_else(|| range(b))?;
    ct_checked_binop_i128(kind, op, a, b)
}

/// The `u128` half. Checked `u128` arithmetic throughout, so the top of the range
/// is reachable and an underflow past zero is an overflow error rather than a
/// wrapped answer.
fn ct_checked_binop_u128(op: CtOp, a: CtNum, b: CtNum) -> ComptimeResult<ComptimeValue> {
    let kind = CtInt::U128;
    let range = |v: CtNum| ComptimeError::IntegerOverflow(format!(
        "{} is outside u128 range {}", v, kind.range_text()
    ));
    let au = a.to_u128().ok_or_else(|| range(a))?;
    // A shift amount is a count, not a `u128` value, so it comes off the operand
    // as written rather than through the unsigned conversion.
    if matches!(op, CtOp::Shl | CtOp::Shr) {
        let amount = match b {
            CtNum::Signed(v) if (0..128).contains(&v) => v as u32,
            CtNum::Big(v) if v < 128 => v as u32,
            _ => return Err(ComptimeError::IntegerOverflow(format!(
                "shift amount {} exceeds u128 bit width (128)", b
            ))),
        };
        let shifted = match op {
            CtOp::Shl => au << amount,
            _ => au >> amount,
        };
        return Ok(ComptimeValue::U128(shifted));
    }
    let bu = b.to_u128().ok_or_else(|| range(b))?;
    match op {
        CtOp::BitAnd => return Ok(ComptimeValue::U128(au & bu)),
        CtOp::BitOr => return Ok(ComptimeValue::U128(au | bu)),
        CtOp::BitXor => return Ok(ComptimeValue::U128(au ^ bu)),
        CtOp::Div | CtOp::Rem if bu == 0 => return Err(ComptimeError::DivisionByZero),
        _ => {}
    }
    let result = match op {
        CtOp::Add => au.checked_add(bu),
        CtOp::Sub => au.checked_sub(bu),
        CtOp::Mul => au.checked_mul(bu),
        CtOp::Div => Some(au / bu),
        CtOp::Rem => Some(au % bu),
        _ => unreachable!(),
    };
    result
        .map(ComptimeValue::U128)
        .ok_or_else(|| ct_overflow_of(kind, op, au, bu))
}

fn ct_checked_binop_i128(kind: CtInt, op: CtOp, a: i128, b: i128) -> ComptimeResult<ComptimeValue> {
    match op {
        CtOp::BitAnd => return Ok(kind.make(a & b)),
        CtOp::BitOr => return Ok(kind.make(a | b)),
        CtOp::BitXor => return Ok(kind.make(a ^ b)),
        CtOp::Shl | CtOp::Shr => {
            if b < 0 || b >= kind.bits() as i128 {
                return Err(ComptimeError::IntegerOverflow(format!(
                    "shift amount {} exceeds {} bit width ({})", b, kind.name(), kind.bits()
                )));
            }
            let shifted = match op {
                CtOp::Shl => a << (b as u32),
                CtOp::Shr if kind.signed() => a >> (b as u32),
                CtOp::Shr => ((a as u128) >> (b as u32)) as i128,
                _ => unreachable!(),
            };
            return Ok(kind.make(kind.wrap(shifted)));
        }
        _ => {}
    }
    if matches!(op, CtOp::Div | CtOp::Rem) {
        if b == 0 { return Err(ComptimeError::DivisionByZero); }
        if kind.signed() && a == kind.min() && b == -1 {
            return Err(ct_overflow(kind, op, a, b));
        }
    }
    let result = match op {
        CtOp::Add => a.checked_add(b),
        CtOp::Sub => a.checked_sub(b),
        CtOp::Mul => a.checked_mul(b),
        CtOp::Div => Some(a / b),
        CtOp::Rem => Some(a % b),
        _ => unreachable!(),
    };
    match result {
        Some(r) if r >= kind.min() && r <= kind.max() => Ok(kind.make(r)),
        _ => Err(ct_overflow(kind, op, a, b)),
    }
}

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

    /// Reset branch counter between independent comptime evaluations.
    pub fn reset_branch_count(&mut self) {
        self.env.reset_branch_count();
    }

    /// CT35: set the backwards-branch quota for the next evaluation.
    pub fn set_quota(&mut self, quota: usize) {
        self.env.set_quota(quota);
    }

    /// Inject the `cfg` build configuration into the comptime environment.
    pub fn inject_cfg(&mut self, cfg: &CfgConfig) {
        self.env.define("cfg".to_string(), cfg.to_comptime_value());
    }

    /// Register comptime functions from declarations.
    /// Every function with a body, not only the `comptime func`s.
    ///
    /// CT6 is explicit that a call in comptime position is legal iff the
    /// callee evaluates within CT7/CT8 — "No `comptime` marking required on
    /// the callee". Registering only the marked ones meant an ordinary `func`
    /// came back "undefined function in comptime context", the const fell out
    /// of folding, and the block ran at runtime instead (#1072). The compiler's
    /// own `ctrl.comptime/CT6 — unmarked func called only at comptime` test
    /// passed the whole time, because the runtime answer is the same number.
    ///
    /// Registering one doesn't promise it evaluates. A body that does I/O or
    /// touches something the evaluator has no answer for fails the same way it
    /// would have; this only stops the lookup failing first.
    pub fn register_functions(&mut self, decls: &[Decl]) {
        for decl in decls {
            if let DeclKind::Fn(f) = &decl.kind {
                if f.is_comptime || !f.body.is_empty() {
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
            // Literals. An explicit width suffix picks the variant so arithmetic
            // is checked at that width (type.overflow); unsuffixed defaults to
            // i64 (checked at the i64 boundary).
            ExprKind::Int(v, suffix) => {
                use rask_ast::token::IntSuffix;
                match suffix {
                    Some(IntSuffix::I8) => ComptimeValue::I8(*v as i8),
                    Some(IntSuffix::I16) => ComptimeValue::I16(*v as i16),
                    Some(IntSuffix::I32) => ComptimeValue::I32(*v as i32),
                    Some(IntSuffix::I64) => ComptimeValue::I64(*v as i64),
                    Some(IntSuffix::Isize) => if rask_ast::primitives::pointer_bits() == 32 {
                        ComptimeValue::I32(*v as i32)
                    } else {
                        ComptimeValue::I64(*v as i64)
                    },
                    Some(IntSuffix::U8) => ComptimeValue::U8(*v as u8),
                    Some(IntSuffix::U16) => ComptimeValue::U16(*v as u16),
                    Some(IntSuffix::U32) => ComptimeValue::U32(*v as u32),
                    Some(IntSuffix::U64) | Some(IntSuffix::U64ByMagnitude) => {
                        ComptimeValue::U64(*v as u64)
                    }
                    Some(IntSuffix::Usize) => if rask_ast::primitives::pointer_bits() == 32 {
                        ComptimeValue::U32(*v as u32)
                    } else {
                        ComptimeValue::U64(*v as u64)
                    },
                    Some(IntSuffix::I128) | Some(IntSuffix::I128ByMagnitude) => {
                        ComptimeValue::I128(*v)
                    }
                    // Above `i128::MAX` the token carries a bit pattern, so
                    // read it back as the unsigned value it stands for.
                    Some(IntSuffix::U128) | Some(IntSuffix::U128ByMagnitude) => {
                        ComptimeValue::U128(*v as u128)
                    }
                    None => ComptimeValue::I64(*v as i64),
                }
            }
            ExprKind::Float(v, _) => ComptimeValue::F64(*v),
            ExprKind::String(s) => ComptimeValue::String(s.clone()),
            ExprKind::Char(c) => ComptimeValue::Char(*c),
            ExprKind::Bool(b) => ComptimeValue::Bool(*b),

            // `none`. The evaluator's optional is the shape `eval_presence_if`
            // already reads — an `Option` enum, `Some` with a payload or `None`
            // without. Nothing produced one before, so a `comptime func`
            // returning `T?` fell out of folding and its const ran at runtime
            // (#1072).
            ExprKind::None => Self::ct_none(),

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
            // `else_binding` is dropped here. Unlike the `IfLet` form below, the
            // scrutinee isn't in hand — the condition is a presence or `is` test
            // wrapping it — and binding the wrong thing is worse than binding
            // nothing. Filed rather than guessed (#808).
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
                else_binding,
            } => {
                // OPT19/ER22: `if x? as v { … } else as e { … }` evaluates the
                // scrutinee *once* and binds its payload — `v` in the then branch,
                // `e` in the else. Comptime dropped `else_binding` on the floor, so
                // a body reading `e` failed with "undefined variable" (#808).
                //
                // Same rule as the interpreter's arm, including its restraint: the
                // else binds only when there's a payload to bind. An Option's
                // absence carries nothing, and inventing a `Unit` for it would put
                // a wrong value where a missing one at least reports itself.
                if let Some(flow) =
                    self.eval_presence_if(cond, then_branch, else_branch.as_deref(), else_binding)?
                {
                    return Ok(flow);
                }

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
            ExprKind::IfLet { expr, pattern, then_branch, else_branch, else_binding } => {
                let value = self.eval_expr(expr)?;
                if self.pattern_matches(pattern, &value)? {
                    self.env.push_scope();
                    self.bind_pattern(pattern, &value)?;
                    let result = self.eval_expr_cf(then_branch);
                    self.env.pop_scope();
                    return result;
                } else if let Some(else_br) = else_branch {
                    // ER22: `else as e` binds the branch the test ruled out.
                    // Comptime ignored the binding, so a body that used it failed
                    // with "undefined variable" — the same field the formatter
                    // was dropping (#805), one pass over. Same payload rule as
                    // the interpreter's.
                    self.env.push_scope();
                    if let Some(name) = else_binding {
                        let payload = match &value {
                            ComptimeValue::Enum { data: Some(inner), .. } => (**inner).clone(),
                            ComptimeValue::Enum { data: None, .. } => ComptimeValue::Unit,
                            other => other.clone(),
                        };
                        self.env.define(name.clone(), payload);
                    }
                    let result = self.eval_expr_cf(else_br);
                    self.env.pop_scope();
                    return result;
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

            // `a ?? b` — the payload when there is one, `b` when there isn't.
            ExprKind::NullCoalesce { value, default } => {
                let v = self.eval_expr(value)?;
                match Self::ct_payload(&v) {
                    Some(payload) => payload,
                    None => self.eval_expr(default)?,
                }
            }

            // Other expressions not yet supported
            _ => {
                let kind_name = match &expr.kind {
                    ExprKind::BlockCall { name, .. } => format!("`{name} {{ }}`"),
                    // `Discriminant(26)` was what this said before, in a
                    // message a person reads. `expr_kind_name` is exhaustive
                    // on purpose, so a new variant can't quietly go unnamed.
                    other => rask_ast::expr::expr_kind_name(other).to_string(),
                };
                return Err(ComptimeError::NotSupported(kind_name));
            }
        };

        Ok(ControlFlow::Normal(value))
    }

    /// Evaluate a block of statements and return the final value.
    pub fn eval_block_to_value(&mut self, stmts: &[Stmt]) -> ComptimeResult<ComptimeValue> {
        match self.eval_block(stmts)? {
            ControlFlow::Normal(v) | ControlFlow::Return(v) => Ok(v),
            ControlFlow::Break(_) => Err(ComptimeError::BreakOutsideLoop),
            ControlFlow::Continue => Err(ComptimeError::ContinueOutsideLoop),
        }
    }

    pub(crate) fn eval_block(&mut self, stmts: &[Stmt]) -> ComptimeResult<ControlFlow> {
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

    /// Retype an integer value to a declared width, refusing rather than
    /// truncating if it doesn't fit (CT1).
    ///
    /// A non-integer value passes through: an annotation this doesn't recognise
    /// isn't an integer width, and nothing else here needs re-widening.
    fn coerce_int_width(value: ComptimeValue, kind: CtInt) -> ComptimeResult<ComptimeValue> {
        let Some((num, _)) = value.as_int() else { return Ok(value) };
        if kind == CtInt::U128 {
            return num
                .to_u128()
                .map(ComptimeValue::U128)
                .ok_or_else(|| ComptimeError::IntegerOverflow(format!(
                    "{} is outside u128 range {}", num, kind.range_text()
                )));
        }
        let n = num.to_i128().ok_or_else(|| ComptimeError::IntegerOverflow(format!(
            "{} is outside {} range {}", num, kind.name(), kind.range_text()
        )))?;
        if n < kind.min() || n > kind.max() {
            return Err(ComptimeError::IntegerOverflow(format!(
                "{} is outside {} range {}", n, kind.name(), kind.range_text()
            )));
        }
        Ok(kind.make(n))
    }

    /// `if x? { … }`, `if x? as v { … }`, `if x? as v { … } else as e { … }`.
    ///
    /// `Ok(None)` when the condition isn't a presence test with a name to bind, so
    /// the caller falls through to evaluating it as an ordinary bool. Otherwise the
    /// scrutinee is evaluated once — not the condition, which would evaluate it a
    /// second time — and the branch runs with its payload bound.
    fn eval_presence_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        else_binding: &Option<String>,
    ) -> ComptimeResult<Option<ControlFlow>> {
        let ExprKind::IsPresent { expr: inner, binding } = &cond.kind else {
            return Ok(None);
        };
        // A bare `if x?` narrows `x` itself; `as v` names the payload instead.
        let then_name = match (binding, &inner.kind) {
            (Some(v), _) => Some(v.clone()),
            (None, ExprKind::Ident(n)) => Some(n.clone()),
            _ => None,
        };
        let else_name = else_binding.clone().or_else(|| then_name.clone());
        if then_name.is_none() && else_name.is_none() {
            return Ok(None);
        }

        let value = self.eval_expr(inner)?;
        let (variant, payload) = match &value {
            ComptimeValue::Enum { variant, data, .. } => {
                (variant.clone(), data.as_ref().map(|d| (**d).clone()))
            }
            // Not a wrapper at all — the condition isn't the shape this handles.
            _ => return Ok(None),
        };

        if matches!(variant.as_str(), "Some" | "Ok") {
            self.env.push_scope();
            if let Some(name) = then_name {
                self.env.define(name, payload.unwrap_or(ComptimeValue::Unit));
            }
            let result = self.eval_expr_cf(then_branch);
            self.env.pop_scope();
            return result.map(Some);
        }

        let Some(else_br) = else_branch else {
            return Ok(Some(ControlFlow::Normal(ComptimeValue::Unit)));
        };
        // A Result's error branch carries E; an Option's absence carries nothing,
        // and there is no name to give nothing.
        match (else_name, payload) {
            (Some(name), Some(p)) => {
                self.env.push_scope();
                self.env.define(name, p);
                let result = self.eval_expr_cf(else_br);
                self.env.pop_scope();
                result.map(Some)
            }
            _ => self.eval_expr_cf(else_br).map(Some),
        }
    }

    fn eval_stmt(&mut self, stmt: &Stmt) -> ComptimeResult<ControlFlow> {
        match &stmt.kind {
            StmtKind::Expr(e) => self.eval_expr_cf(e),

            StmtKind::Mut { name, ty, init, .. } | StmtKind::Let { name, ty, init, .. } => {
                let value = self.eval_expr(init)?;
                // The annotation decides the width. Without this the value kept
                // whatever width its literal evaluated to, so `let a: u128 = <fits
                // in u64>` bound a `u64` — and a fold of it came out `u64`-wide,
                // which printed the same digits and compared unequal against a
                // `u128` (#826).
                let value = match ty.as_deref().and_then(CtInt::from_name) {
                    Some(kind) => Self::coerce_int_width(value, kind)?,
                    None => value,
                };
                self.env.define(name.clone(), value);
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::MutTuple { patterns, init } | StmtKind::LetTuple { patterns, init } => {
                let value = self.eval_expr(init)?;
                if let ComptimeValue::Tuple(values) = value {
                    let names: Vec<&str> = rask_ast::stmt::tuple_pats_flat_names(patterns);
                    if values.len() != names.len() {
                        return Err(ComptimeError::TypeMismatch {
                            expected: format!("tuple of {} elements", names.len()),
                            found: format!("tuple of {} elements", values.len()),
                        });
                    }
                    for (name, val) in names.iter().zip(values) {
                        self.env.define(name.to_string(), val);
                    }
                } else {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "tuple".to_string(),
                        found: value.type_name().to_string(),
                    });
                }
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            // `let Point { x, .. } = p` — bind the fields the pattern names.
            StmtKind::LetStruct { pattern, init, .. } => {
                let value = self.eval_expr(init)?;
                let Pattern::Struct { fields: pat_fields, .. } = pattern else {
                    return Err(ComptimeError::NotSupported(
                        "destructuring binding on a non-struct pattern".to_string(),
                    ));
                };
                let ComptimeValue::Struct { fields, .. } = value else {
                    return Err(ComptimeError::TypeMismatch {
                        expected: "struct".to_string(),
                        found: value.type_name().to_string(),
                    });
                };
                for (field_name, pat) in pat_fields {
                    let Some(val) = fields.get(field_name) else {
                        return Err(ComptimeError::NotSupported(format!(
                            "no field `{}` to bind", field_name
                        )));
                    };
                    let Pattern::Ident(binding) = pat else {
                        return Err(ComptimeError::NotSupported(
                            "only a name can bind a field at comptime".to_string(),
                        ));
                    };
                    self.env.define(binding.clone(), val.clone());
                }
                Ok(ControlFlow::Normal(ComptimeValue::Unit))
            }

            StmtKind::Assign { target, value, .. } => {
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

            StmtKind::While { cond, body, .. } => {
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

            StmtKind::ComptimeFor { binding, iter, body } => {
                // CT48: Evaluate the iterable and unroll
                let iter_val = self.eval_expr(iter)?;
                match iter_val {
                    ComptimeValue::Array(items) => {
                        for item in items {
                            self.env.push_scope();
                            match binding {
                                rask_ast::stmt::ForBinding::Single(name) => {
                                    self.env.define(name.clone(), item);
                                }
                                rask_ast::stmt::ForBinding::Tuple(names) => {
                                    if let ComptimeValue::Array(elems) = item {
                                        for (i, name) in names.iter().enumerate() {
                                            let v = elems.get(i).cloned()
                                                .unwrap_or(ComptimeValue::Unit);
                                            self.env.define(name.clone(), v);
                                        }
                                    }
                                }
                            }
                            match self.eval_block(body)? {
                                ControlFlow::Normal(_) => {}
                                cf => {
                                    self.env.pop_scope();
                                    return Ok(cf);
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(ControlFlow::Normal(ComptimeValue::Unit))
                    }
                    _ => Err(ComptimeError::NotSupported(
                        "comptime for requires a comptime-known iterable".to_string()
                    )),
                }
            }

            StmtKind::Ensure { .. } => {
                // Ensure blocks are runtime-only
                Err(ComptimeError::NotSupported("ensure blocks at comptime".to_string()))
            }

            StmtKind::WhileLet { .. } => {
                Err(ComptimeError::NotSupported("while-let at comptime".to_string()))
            }

            StmtKind::Discard { .. } => Ok(ControlFlow::Normal(ComptimeValue::Unit)),
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
            // No heap at comptime — `own` is transparent, same as OW5 at runtime,
            // so the operand's value *is* the answer.
            UnaryOp::Heap => Ok(val),
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

        self.env.push_call()?; // CT29: stack depth check
        self.env.push_scope();

        // Bind parameters at their declared widths, same rule as a `let`
        // annotation (#826): the value keeps whatever width the argument
        // expression evaluated to otherwise, which is a different type from the
        // one the signature promises.
        for (param, value) in func.params.iter().zip(args) {
            let value = match CtInt::from_name(&param.ty) {
                Some(kind) => Self::coerce_int_width(value, kind)?,
                None => value,
            };
            self.env.define(param.name.clone(), value);
        }

        // Execute body
        let result = self.eval_block(&func.body);
        self.env.pop_scope();
        self.env.pop_call();

        let value = result?.value();
        // The declared return type decides the width too. Without it
        // `comptime func big() -> i32 { return 2147483647 }` handed back an
        // `i64`, so `big() + 1` was i64 arithmetic — no overflow — and the
        // out-of-range 2147483648 went into an `i32` const with no diagnostic,
        // where CT1 says comptime overflow is a compile error (#325).
        // A `-> T?` hands back an optional, so a bare `T` is wrapped here
        // rather than at every `return` in the body. Already-optional values
        // (a `none`, or a result passed straight through) go as they are.
        let ret = func.ret_ty.as_deref();
        if ret.map(rask_ast::type_str::is_optional).unwrap_or(false) {
            return Ok(Self::ct_some(value));
        }
        match ret.and_then(CtInt::from_name) {
            Some(kind) => Self::coerce_int_width(value, kind),
            None => Ok(value),
        }
    }

    /// The absent optional.
    fn ct_none() -> ComptimeValue {
        ComptimeValue::Enum {
            name: "Option".to_string(),
            variant: "None".to_string(),
            data: None,
        }
    }

    /// `value` as a present optional, unless it already is one.
    fn ct_some(value: ComptimeValue) -> ComptimeValue {
        if let ComptimeValue::Enum { name, .. } = &value {
            if name == "Option" {
                return value;
            }
        }
        ComptimeValue::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            data: Some(Box::new(value)),
        }
    }

    /// What's inside a present optional or result, or `None` for an absent
    /// one. A value that isn't a wrapper at all is its own payload — that's
    /// what makes `x ?? y` work on something already unwrapped.
    fn ct_payload(value: &ComptimeValue) -> Option<ComptimeValue> {
        match value {
            ComptimeValue::Enum { variant, data, .. } if variant == "None" => {
                let _ = data;
                None
            }
            ComptimeValue::Enum { variant, data, .. } if variant == "Some" || variant == "Ok" => {
                Some(data.as_ref().map(|d| (**d).clone()).unwrap_or(ComptimeValue::Unit))
            }
            other => Some(other.clone()),
        }
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

        self.env.push_call()?; // CT29: stack depth check

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
        self.env.pop_call();

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
            "todo" => {
                let msg = if let Some(ComptimeValue::String(s)) = args.first() {
                    s.clone()
                } else {
                    "no message".to_string()
                };
                Err(ComptimeError::Unimplemented(msg))
            }
            "unreachable" => {
                let msg = if let Some(ComptimeValue::String(s)) = args.first() {
                    format!("entered unreachable code: {}", s)
                } else {
                    "entered unreachable code".to_string()
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

        // Handle numeric operations. CT1: overflow at comptime is a compile
        // error (CT1), never a silent wrap. Width-aware: the operand variants
        // carry the type, so `200u8 + 100u8` overflows at u8, not just i64.
        match method {
            "add" => self.ct_arith(obj, args, CtOp::Add, |a, b| a + b, "+"),
            "sub" => self.ct_arith(obj, args, CtOp::Sub, |a, b| a - b, "-"),
            "mul" => self.ct_arith(obj, args, CtOp::Mul, |a, b| a * b, "*"),
            "div" => {
                if args.first().and_then(|a| a.as_f64()) == Some(0.0) && obj.as_int().is_none() {
                    return Err(ComptimeError::DivisionByZero);
                }
                self.ct_arith(obj, args, CtOp::Div, |a, b| a / b, "/")
            }
            "rem" => self.ct_arith(obj, args, CtOp::Rem, |a, b| a % b, "%"),
            "neg" => match obj.as_int() {
                Some((v, kind)) => {
                    // Nothing unsigned has a negation but zero, and the `u128`
                    // half of the range has no signed carrier to negate in — so
                    // the check is on the value, not on a computed result (#802).
                    let out_of_range = ComptimeError::IntegerOverflow(format!(
                        "negating {} exceeds {} range {}", v, kind.name(), kind.range_text()
                    ));
                    match v.to_i128().map(|n| -n) {
                        Some(r) if r >= kind.min() && r <= kind.max() => Ok(kind.make(r)),
                        _ => Err(out_of_range),
                    }
                }
                None => match obj {
                    ComptimeValue::F64(v) => Ok(ComptimeValue::F64(-v)),
                    ComptimeValue::F32(v) => Ok(ComptimeValue::F32(-v)),
                    _ => Err(ComptimeError::TypeMismatch {
                        expected: "numeric".to_string(),
                        found: obj.type_name().to_string(),
                    }),
                },
            },
            "eq" => self.comparison_op(obj, args, |a, b| a == b, |a, b| a == b),
            "lt" => self.comparison_op(obj, args, |a, b| a < b, |a, b| a < b),
            "gt" => self.comparison_op(obj, args, |a, b| a > b, |a, b| a > b),
            "le" => self.comparison_op(obj, args, |a, b| a <= b, |a, b| a <= b),
            "ge" => self.comparison_op(obj, args, |a, b| a >= b, |a, b| a >= b),
            "bit_and" => self.ct_int_only(obj, args, CtOp::BitAnd),
            "bit_or" => self.ct_int_only(obj, args, CtOp::BitOr),
            "bit_xor" => self.ct_int_only(obj, args, CtOp::BitXor),
            "shl" => self.ct_int_only(obj, args, CtOp::Shl),
            "shr" => self.ct_int_only(obj, args, CtOp::Shr),
            "bit_not" => match obj.as_int() {
                Some((v, kind)) if kind == CtInt::U128 => {
                    let u = v.to_u128().ok_or_else(|| ComptimeError::IntegerOverflow(format!(
                        "{} is outside u128 range {}", v, kind.range_text()
                    )))?;
                    Ok(ComptimeValue::U128(!u))
                }
                Some((v, kind)) => {
                    let n = v.to_i128().ok_or_else(|| ComptimeError::IntegerOverflow(format!(
                        "{} is outside {} range {}", v, kind.name(), kind.range_text()
                    )))?;
                    Ok(kind.make(kind.wrap(!n)))
                }
                None => Err(ComptimeError::TypeMismatch {
                    expected: "integer".to_string(),
                    found: obj.type_name().to_string(),
                }),
            },
            // String methods
            // A string's own `to_string` is the identity. Without it a
            // `const GREETING = comptime { … "{name}".to_string() }` fell out
            // of folding and ran at runtime (#1072).
            "to_string" if matches!(obj, ComptimeValue::String(_)) => Ok(obj.clone()),
            // What string interpolation desugars to — there is no public
            // `concat`, so this is the one way two strings join (std.strings).
            "__concat" => match (obj, args.first()) {
                (ComptimeValue::String(a), Some(ComptimeValue::String(b))) => {
                    Ok(ComptimeValue::String(format!("{a}{b}")))
                }
                _ => Err(ComptimeError::TypeMismatch {
                    expected: "two strings".to_string(),
                    found: obj.type_name().to_string(),
                }),
            },
            // The checked hatches (type.overflow). They answer `T?`, which the
            // evaluator can represent now, so overflow is the absent case
            // rather than a comptime error — the whole point of reaching for
            // one of these instead of `+`.
            "checked_add" | "checked_sub" | "checked_mul" => {
                let (a, ka) = obj.as_int().ok_or_else(|| ComptimeError::TypeMismatch {
                    expected: "integer".to_string(),
                    found: obj.type_name().to_string(),
                })?;
                let (b, _) = args
                    .first()
                    .and_then(|v| v.as_int())
                    .ok_or_else(|| ComptimeError::TypeMismatch {
                        expected: "integer".to_string(),
                        found: "non-integer argument".to_string(),
                    })?;
                let op = match method {
                    "checked_add" => CtOp::Add,
                    "checked_sub" => CtOp::Sub,
                    _ => CtOp::Mul,
                };
                Ok(match ct_checked_binop(ka, op, a, b) {
                    Ok(v) => Self::ct_some(v),
                    Err(_) => Self::ct_none(),
                })
            }
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
            // `freeze` ends a `comptime` block to say the Vec it built is the
            // constant's value. Everything here is already a compile-time
            // value, so there is nothing to do but hand it back. It was
            // declared `comptime func` with an empty body and nothing
            // implemented it, so every `const X = comptime { … v.freeze() }`
            // reached codegen as a call to `Vec_freeze` (#1069).
            // A `Vec` is the only thing that reaches here: `Map.new` isn't
            // supported at comptime, so a map-valued const never folds and its
            // `freeze` is handled at runtime instead.
            "freeze" => match obj {
                ComptimeValue::Array(arr) => Ok(ComptimeValue::Array(arr.clone())),
                _ => Err(ComptimeError::TypeMismatch {
                    expected: "Vec".to_string(),
                    found: obj.type_name().to_string(),
                }),
            },
            _ => Err(ComptimeError::NotSupported(format!("method {} on {}", method, obj.type_name()))),
        }
    }

    /// Width-aware checked integer arithmetic on comptime values (CT1). Reads
    /// the width from the operand variants (i64 is the unsuffixed default and
    /// is checked at the i64 boundary), so overflow at any width is a compile
    /// error. Falls back to `checked_numeric_binop` for float operands.
    fn ct_int_binop(
        &self,
        obj: &ComptimeValue,
        arg: &ComptimeValue,
        op: CtOp,
    ) -> Option<ComptimeResult<ComptimeValue>> {
        let (a, ka) = obj.as_int()?;
        let (b, kb) = arg.as_int()?;
        let k = ka.unify(kb);
        Some(ct_checked_binop(k, op, a, b))
    }

    /// Like `numeric_binop` but the integer op is checked (CT1). A `None`
    /// result is an overflow and becomes a compile error.
    /// Arithmetic that may be integer or float. Integers go through the
    /// width-aware checked path; floats use the supplied op.
    fn ct_arith<Ff>(
        &self,
        obj: &ComptimeValue,
        args: &[ComptimeValue],
        op: CtOp,
        float_op: Ff,
        _sym: &str,
    ) -> ComptimeResult<ComptimeValue>
    where
        Ff: Fn(f64, f64) -> f64,
    {
        let arg = args.first().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "1 argument".to_string(),
            found: "0 arguments".to_string(),
        })?;
        if let Some(res) = self.ct_int_binop(obj, arg, op) {
            return res;
        }
        if let (Some(a), Some(b)) = (obj.as_f64(), arg.as_f64()) {
            Ok(ComptimeValue::F64(float_op(a, b)))
        } else {
            Err(ComptimeError::TypeMismatch {
                expected: "matching numeric types".to_string(),
                found: format!("{} and {}", obj.type_name(), arg.type_name()),
            })
        }
    }

    /// Integer-only operations (bitwise, shifts), width-aware.
    fn ct_int_only(
        &self,
        obj: &ComptimeValue,
        args: &[ComptimeValue],
        op: CtOp,
    ) -> ComptimeResult<ComptimeValue> {
        let arg = args.first().ok_or_else(|| ComptimeError::TypeMismatch {
            expected: "1 argument".to_string(),
            found: "0 arguments".to_string(),
        })?;
        self.ct_int_binop(obj, arg, op).unwrap_or_else(|| Err(ComptimeError::TypeMismatch {
            expected: "integer".to_string(),
            found: format!("{} and {}", obj.type_name(), arg.type_name()),
        }))
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
                // Wide integers first: `as_i64` can't see past `i64::MAX`, so two
                // large `u128` values fell through to the float path and then to a
                // type error (#802). The comparison itself is exact — an ordering
                // needs no arithmetic and no carrier wide enough to hold both.
                if let (Some((a, _)), Some((b, _))) = (obj.as_int(), arg.as_int()) {
                    let ord = a.cmp(b);
                    // The caller's predicate is written on i64s; reuse it by
                    // feeding it the ordering as -1/0/1, which every comparison
                    // operator reads the same way.
                    let (l, r) = match ord {
                        std::cmp::Ordering::Less => (-1i64, 0i64),
                        std::cmp::Ordering::Equal => (0, 0),
                        std::cmp::Ordering::Greater => (1, 0),
                    };
                    Ok(ComptimeValue::Bool(int_op(l, r)))
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
            Pattern::Range { start, end } => {
                let start_val = self.eval_expr(start)?;
                let end_val = self.eval_expr(end)?;
                Ok(match (value, &start_val, &end_val) {
                    (ComptimeValue::Char(c), ComptimeValue::Char(s), ComptimeValue::Char(e)) => {
                        c >= s && c <= e
                    }
                    (ComptimeValue::I64(n), ComptimeValue::I64(s), ComptimeValue::I64(e)) => {
                        n >= s && n <= e
                    }
                    (ComptimeValue::I32(n), ComptimeValue::I32(s), ComptimeValue::I32(e)) => {
                        n >= s && n <= e
                    }
                    _ => false,
                })
            }
            Pattern::TypePat { ty_name, .. } => {
                // Match when the value's enum tag matches the named type.
                Ok(matches!(value, ComptimeValue::Enum { variant, .. } if variant == ty_name))
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
            Pattern::Range { .. } => Ok(()),
            Pattern::TypePat { binding, .. } => {
                if let Some(name) = binding {
                    self.env.define(name.clone(), value.clone());
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
        assert_eq!(interp.env.branch_quota, 1_000); // CT35: default 1,000
    }
}

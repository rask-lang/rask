// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Expression AST nodes.

use crate::token::{FloatSuffix, IntSuffix};
use crate::{NodeId, Span};

/// An expression in the AST.
#[derive(Debug, Clone)]
pub struct Expr {
    pub id: NodeId,
    pub kind: ExprKind,
    pub span: Span,
}

/// The kind of expression.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// Integer literal
    Int(i64, Option<IntSuffix>),
    /// Float literal
    Float(f64, Option<FloatSuffix>),
    /// String literal
    String(String),
    /// Character literal
    Char(char),
    /// Boolean literal
    Bool(bool),
    /// Identifier
    Ident(String),
    /// Binary operation
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Unary operation
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    /// Function call
    Call {
        func: Box<Expr>,
        args: Vec<Expr>,
    },
    /// Method call (syntactic sugar for field access + call)
    MethodCall {
        object: Box<Expr>,
        method: String,
        type_args: Option<Vec<String>>,
        args: Vec<Expr>,
    },
    /// Field access
    Field {
        object: Box<Expr>,
        field: String,
    },
    /// Optional chaining field access (a?.b)
    OptionalField {
        object: Box<Expr>,
        field: String,
    },
    /// Index access
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// Block expression
    Block(Vec<super::stmt::Stmt>),
    /// If expression
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    /// If-is pattern matching expression (if expr is Pattern { })
    IfLet {
        expr: Box<Expr>,
        pattern: Pattern,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    /// Match expression
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// Try expression (try prefix or postfix ?)
    Try(Box<Expr>),
    /// Null coalescing (a ?? b)
    NullCoalesce {
        value: Box<Expr>,
        default: Box<Expr>,
    },
    /// Range expression (a..b or a..=b)
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    /// Struct literal (Point { x: 1, y: 2 })
    StructLit {
        name: String,
        fields: Vec<FieldInit>,
        spread: Option<Box<Expr>>,
    },
    /// Array/list literal ([1, 2, 3])
    Array(Vec<Expr>),
    /// Array repeat expression ([value; count])
    ArrayRepeat {
        value: Box<Expr>,
        count: Box<Expr>,
    },
    /// Tuple literal ((a, b, c))
    Tuple(Vec<Expr>),
    /// With block expression (with name { body } or with name(args) { body })
    WithBlock {
        name: String,
        args: Vec<Expr>,
        body: Vec<super::stmt::Stmt>,
    },
    /// Closure (|x, y| x + y)
    Closure {
        params: Vec<ClosureParam>,
        body: Box<Expr>,
    },
    /// Type cast (x as i32)
    Cast {
        expr: Box<Expr>,
        ty: String,
    },
    /// Spawn expression (spawn { body })
    Spawn {
        body: Vec<super::stmt::Stmt>,
    },
    /// Block call expression (identifier { body }) like spawn_raw { ... }
    BlockCall {
        name: String,
        body: Vec<super::stmt::Stmt>,
    },
    /// Unsafe block expression
    Unsafe {
        body: Vec<super::stmt::Stmt>,
    },
    /// Comptime expression (computed at compile time)
    Comptime {
        body: Vec<super::stmt::Stmt>,
    },
    /// Assert expression (assert condition, "message")
    Assert {
        condition: Box<Expr>,
        message: Option<Box<Expr>>,
    },
    /// Check expression (check condition, "message") - continues on failure
    Check {
        condition: Box<Expr>,
        message: Option<Box<Expr>>,
    },
}

/// A field initializer in a struct literal.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
}

/// A closure parameter.
#[derive(Debug, Clone)]
pub struct ClosureParam {
    pub name: String,
    pub ty: Option<String>,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // Comparison
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    // Logical
    And,
    Or,
    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Negation (-)
    Neg,
    /// Logical not (!)
    Not,
    /// Bitwise not (~)
    BitNot,
    /// Reference (&)
    Ref,
    /// Dereference (*)
    Deref,
}

/// A match arm.
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Box<Expr>>,
    pub body: Box<Expr>,
}

/// A pattern for matching.
#[derive(Debug, Clone)]
pub enum Pattern {
    /// Wildcard `_`
    Wildcard,
    /// Binding `name`
    Ident(String),
    /// Literal
    Literal(Box<Expr>),
    /// Constructor `Name(patterns...)`
    Constructor {
        name: String,
        fields: Vec<Pattern>,
    },
    /// Struct pattern `Name { field: pattern, ... }`
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
        rest: bool,
    },
    /// Tuple pattern `(a, b, c)`
    Tuple(Vec<Pattern>),
    /// Or pattern `a | b`
    Or(Vec<Pattern>),
}

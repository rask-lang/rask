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
    /// String with interpolation: "hello {name}, age {age}"
    /// Segments alternate between literal strings and expressions.
    StringInterp(Vec<StringSegment>),
    /// Character literal
    Char(char),
    /// Boolean literal
    Bool(bool),
    /// Null pointer literal
    Null,
    /// OPT3: absent sentinel for `T?`. Dedicated literal — not tied to the
    /// `None` enum variant. Context infers the inner type `T`.
    None,
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
        args: Vec<CallArg>,
    },
    /// Method call (syntactic sugar for field access + call)
    MethodCall {
        object: Box<Expr>,
        method: String,
        type_args: Option<Vec<String>>,
        args: Vec<CallArg>,
    },
    /// Field access
    Field {
        object: Box<Expr>,
        field: String,
    },
    /// Dynamic field access: value.(expr) — comptime field name
    DynamicField {
        object: Box<Expr>,
        field_expr: Box<Expr>,
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
    /// If expression. `else_binding` (ER22) is the optional `as e` on the else
    /// clause that binds the error value from a `IsPresent` cond on a Result.
    /// `if r? { … } else as e { use(e) }`.
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
        else_binding: Option<String>,
    },
    /// If-is pattern matching expression (if expr is Pattern { })
    IfLet {
        expr: Box<Expr>,
        pattern: Pattern,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    /// Guard pattern (const v = expr is Pattern else { diverge })
    GuardPattern {
        expr: Box<Expr>,
        pattern: Pattern,
        else_branch: Box<Expr>,
    },
    /// Pattern test expression (expr is Pattern) — evaluates to bool
    IsPattern {
        expr: Box<Expr>,
        pattern: Pattern,
    },
    /// Match expression
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// Try expression (prefix `try expr` for propagation; optional `else |e| handler`)
    Try {
        expr: Box<Expr>,
        else_clause: Option<TryElse>,
    },
    /// Presence predicate (postfix `expr?`) — evaluates to bool.
    /// `true` if scrutinee is `Some`/`Ok`, `false` if `None`/`Err`.
    /// `binding` (OPT20/ER20) is the optional `as v` that binds the inner
    /// payload as a fresh const in the then-branch — `if x? as v { ... v ... }`.
    IsPresent {
        expr: Box<Expr>,
        binding: Option<String>,
    },
    /// Unwrap expression (postfix !) - panics if None/Err
    Unwrap {
        expr: Box<Expr>,
        message: Option<String>,
    },
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
    /// Using block expression (using name { body } or using name(args) { body })
    UsingBlock {
        name: String,
        args: Vec<CallArg>,
        body: Vec<super::stmt::Stmt>,
    },
    /// With-as element binding (with expr as [const] name, ... { body })
    WithAs {
        bindings: Vec<WithBinding>,
        body: Vec<super::stmt::Stmt>,
    },
    /// Closure (|x, y| x + y)
    Closure {
        params: Vec<ClosureParam>,
        ret_ty: Option<String>,
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
    /// Select expression (channel multiplexing)
    Select {
        arms: Vec<SelectArm>,
        is_priority: bool,
    },
    /// Loop expression (loop { ... } with break value)
    Loop {
        label: Option<String>,
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

/// A segment of an interpolated string.
#[derive(Debug, Clone)]
pub enum StringSegment {
    /// Literal text between interpolation braces.
    Literal(String),
    /// An expression inside `{...}`.
    Expr(Box<Expr>),
}

/// How an argument is passed at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgMode {
    /// Default (borrow / read-only)
    Default,
    /// `own expr` — transfers ownership (matches `take` param)
    Own,
    /// `mutate expr` — mutable borrow (matches `mutate` param)
    Mutate,
}

/// A function call argument with optional name label and mode annotation.
#[derive(Debug, Clone)]
pub struct CallArg {
    /// Named argument label (e.g., `timeout:` in `connect(timeout: 60)`).
    pub name: Option<String>,
    pub mode: ArgMode,
    pub expr: Expr,
}

/// A field initializer in a struct literal.
#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
}

/// Error transformation clause for `try...else |e| expr`.
#[derive(Debug, Clone)]
pub struct TryElse {
    pub error_binding: String,
    pub body: Box<Expr>,
}

/// A `with...as` binding: source expression, binding name, and mutability.
/// Mutable by default; `as const name` for read-only.
#[derive(Debug, Clone)]
pub struct WithBinding {
    pub source: Expr,
    pub name: String,
    pub mutable: bool,
}

/// A closure parameter.
#[derive(Debug, Clone)]
pub struct ClosureParam {
    pub name: String,
    pub ty: Option<String>,
    pub is_mutate: bool,
    pub is_take: bool,
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

/// A select arm for channel multiplexing.
#[derive(Debug, Clone)]
pub struct SelectArm {
    pub kind: SelectArmKind,
    pub body: Box<Expr>,
}

/// The kind of select arm.
#[derive(Debug, Clone)]
pub enum SelectArmKind {
    /// Receive: `rx -> v`
    Recv { channel: Expr, binding: String },
    /// Send: `tx <- val`
    Send { channel: Expr, value: Expr },
    /// Default: `_`
    Default,
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
    /// Inclusive range pattern `start..=end`. Both bounds must be literal chars or ints.
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
    },
    /// ER23: type pattern `Type as name` — narrows the scrutinee to `Type`
    /// and binds the value as `name`. Currently supported for `T or E` Result
    /// errors in `if r is E as e { ... }`.
    TypePat {
        ty_name: String,
        binding: Option<String>,
    },
}

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
    Int(i128, Option<IntSuffix>),
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
        /// ER22: `else as e` binds the complement branch. A binding, not a
        /// narrow — the scrutinee keeps its own type throughout.
        else_binding: Option<String>,
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
    /// Propagation (prefix `try expr`) — extracts the success payload, or the
    /// other branch leaves to the caller. Works on both wrapper shapes: the
    /// error of a `T or E`, the `none` of a `T?` (type.errors/ER16).
    /// It has no clause; substituting a value is `??` or `catch` instead.
    Try {
        expr: Box<Expr>,
    },
    /// Failure fallback (`r catch e => body` / `r catch _ => body`, ER14).
    /// Results only — absence uses `??`. The binder is never optional.
    Catch {
        value: Box<Expr>,
        clause: CatchClause,
    },
    /// `take <place>` — move the payload out of a mutable `T?` slot and leave
    /// `none` behind (type.optionals/OPT32). Yields `T?`.
    Take {
        place: Box<Expr>,
    },
    /// Presence predicate (postfix `expr?`) — a plain bool, `true` when the
    /// optional is present. It narrows nothing; reaching the payload is the
    /// `as v` bind in `binding` (OPT19), which introduces `v: T` in the
    /// then-branch.
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
    /// Closure (|x, y| x + y or own |x, y| x + y)
    Closure {
        params: Vec<ClosureParam>,
        ret_ty: Option<String>,
        body: Box<Expr>,
        /// Captures non-Copy values by move; can escape its creation scope.
        /// Without this flag the closure borrows outer variables (scope-limited).
        is_own: bool,
    },
    /// Type cast (x as i32) — lossless widening only (type.primitives CV1).
    Cast {
        expr: Box<Expr>,
        ty: String,
    },
    /// Explicit lossy numeric conversion (type.primitives CV5–CV10):
    /// `x truncate to T`, `x saturate to T`, `try x convert to T`,
    /// `x float to int T`, `x float to int T (saturating)`, `try x float to int T`.
    Convert {
        expr: Box<Expr>,
        /// Target primitive type name (e.g. `i8`, `u32`, `f64`).
        target: String,
        kind: ConvertKind,
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
    /// An expression inside `{...}`, with the `:spec` that followed it.
    Expr(Box<Expr>, Option<crate::fmt_spec::FormatSpec>),
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
    /// `deleting expr` — may have nodes deleted from it (matches `deleting`
    /// param). PM5: the marker follows the signature, and a `deleting` parameter
    /// is a `mutate` parameter that may also delete, so the call site says the
    /// more specific word (analysis.fourth-option).
    Deleting,
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

/// The handler of `r catch <binder> => <body>`. `binder` is `_` for a visible
/// discard; the body is a value or a divergence.
#[derive(Debug, Clone)]
pub struct CatchClause {
    pub binder: String,
    pub body: Box<Expr>,
}

impl CatchClause {
    /// `catch _ =>` drops the error without naming it.
    pub fn is_discard(&self) -> bool {
        self.binder == "_"
    }
}

/// A `with...as` binding: source expression and binding name.
/// Bindings are mutable — `with` exists for multi-statement mutation; reads
/// use inline access, `.read()` locks, or frozen pools. The checker rejects
/// mutation when the source is a shared read lock (conc.sync/R1).
#[derive(Debug, Clone)]
pub struct WithBinding {
    pub source: Expr,
    pub name: String,
}

/// A closure parameter.
#[derive(Debug, Clone)]
pub struct ClosureParam {
    pub name: String,
    pub ty: Option<String>,
    pub is_mutate: bool,
    pub is_take: bool,
}

/// Numeric conversions that can lose something (type.primitives CV11–CV16).
/// Unlike `as` (lossless widening only), each names its data-loss behavior.
///
/// These replaced the phrase-verb family — `truncate to T`, `saturate to T`,
/// `try convert to T` and the three `float to int` forms. Not a rename: one
/// form became three, and `to` answers a result where `try convert to`
/// answered an optional, so an old diagnostic citing CV5 must not resolve to
/// `to` (#790).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvertKind {
    /// CV11: `x.to<T>()` — exact, or `ConvertError`.
    To,
    /// CV12: `x.wrap<T>()` — keeps the low bits. Integers only.
    Wrap,
    /// CV13: `x.clamp<T>()` — pins to the target's range. Integers only.
    Clamp,
    /// CV14: `x.round<T>()` — nearest representable, ties to even. Total to a
    /// float target, fallible to an integer one.
    Round,
    /// CV15: `x.floor<T>()` — toward negative infinity. Float source, integer
    /// target.
    Floor,
    /// CV16: `x.ceil<T>()` — toward positive infinity. Float source, integer
    /// target.
    Ceil,
    /// Compiler-internal: a checked conversion that answers `T?`.
    ///
    /// No surface syntax reaches this — `char.from_u32(n)` lowers to it,
    /// because "is this a valid Unicode scalar" is the same shape as "does this
    /// fit the target" and the codegen path is already written (CH3).
    CheckedOption,
}

impl ConvertKind {
    /// Result is `T?` (optional) rather than `T`.
    pub fn is_optional(self) -> bool {
        matches!(self, ConvertKind::CheckedOption)
    }

    /// CV11/CV14–CV16: does this form yield `T or ConvertError` rather than a
    /// bare `T`? `round` is the one that depends on the target — rounding to a
    /// float can't fail, rounding to an integer can (CV14).
    pub fn yields_result(self, target_is_int: bool) -> bool {
        match self {
            ConvertKind::To | ConvertKind::Floor | ConvertKind::Ceil => true,
            ConvertKind::Round => target_is_int,
            _ => false,
        }
    }

    /// Surface form for diagnostics.
    pub fn surface(self) -> &'static str {
        match self {
            ConvertKind::CheckedOption => "char.from_u32",
            ConvertKind::To => "to",
            ConvertKind::Wrap => "wrap",
            ConvertKind::Clamp => "clamp",
            ConvertKind::Round => "round",
            ConvertKind::Floor => "floor",
            ConvertKind::Ceil => "ceil",
        }
    }
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
    /// Heap-allocate (`Heap(expr)`) — `mem.heap/HP3`. The operand is evaluated
    /// and its value moved to the heap; the result is the pointer, which is also
    /// the value's representation from here on (HP5).
    Heap,
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

//! Statement AST nodes.

use crate::{NodeId, Span};
use crate::expr::Expr;

/// A statement in the AST.
#[derive(Debug, Clone)]
pub struct Stmt {
    pub id: NodeId,
    pub kind: StmtKind,
    pub span: Span,
}

/// The kind of statement.
#[derive(Debug, Clone)]
pub enum StmtKind {
    /// Expression statement
    Expr(Expr),
    /// Let binding (mutable)
    Let {
        name: String,
        ty: Option<String>,
        init: Expr,
    },
    /// Let tuple destructuring
    LetTuple {
        names: Vec<String>,
        init: Expr,
    },
    /// Const binding (immutable)
    Const {
        name: String,
        ty: Option<String>,
        init: Expr,
    },
    /// Const tuple destructuring
    ConstTuple {
        names: Vec<String>,
        init: Expr,
    },
    /// Assignment
    Assign {
        target: Expr,
        value: Expr,
    },
    /// Return statement
    Return(Option<Expr>),
    /// Break statement
    Break(Option<String>),
    /// Continue statement
    Continue(Option<String>),
    /// Deliver statement (loop with value)
    Deliver {
        label: Option<String>,
        value: Expr,
    },
    /// While loop
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// While-let pattern matching loop
    WhileLet {
        pattern: crate::expr::Pattern,
        expr: Expr,
        body: Vec<Stmt>,
    },
    /// Loop (infinite)
    Loop {
        label: Option<String>,
        body: Vec<Stmt>,
    },
    /// For-in loop
    For {
        label: Option<String>,
        binding: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// Ensure block (deferred cleanup)
    Ensure(Vec<Stmt>),
    /// Comptime block (compile-time evaluated)
    Comptime(Vec<Stmt>),
}

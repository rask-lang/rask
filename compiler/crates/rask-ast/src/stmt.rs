// SPDX-License-Identifier: (MIT OR Apache-2.0)
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

/// Binding pattern in a for-in loop.
#[derive(Debug, Clone)]
pub enum ForBinding {
    /// Single variable: `for x in ...`
    Single(String),
    /// Tuple destructuring: `for (h, v) in ...`
    Tuple(Vec<String>),
}

/// Pattern element in tuple destructuring: `const (a, (b, c), _) = ...`
#[derive(Debug, Clone)]
pub enum TuplePat {
    /// Named binding
    Name(String),
    /// Wildcard `_`
    Wildcard,
    /// Nested tuple: `(b, c)`
    Nested(Vec<TuplePat>),
}

impl ForBinding {
    /// Whether the binding is mutable (`for mutate x in ...`).
    /// Stored separately in StmtKind::For.
    pub fn names(&self) -> Vec<&str> {
        match self {
            ForBinding::Single(n) => vec![n.as_str()],
            ForBinding::Tuple(ns) => ns.iter().map(|n| n.as_str()).collect(),
        }
    }
}

impl TuplePat {
    /// Collect all named bindings (flattened), skipping wildcards.
    pub fn flat_names(&self) -> Vec<&str> {
        match self {
            TuplePat::Name(n) => vec![n.as_str()],
            TuplePat::Wildcard => vec![],
            TuplePat::Nested(pats) => pats.iter().flat_map(|p| p.flat_names()).collect(),
        }
    }
}

/// Collect all named bindings from a list of TuplePat elements.
pub fn tuple_pats_flat_names(pats: &[TuplePat]) -> Vec<&str> {
    pats.iter().flat_map(|p| p.flat_names()).collect()
}

/// The kind of statement.
#[derive(Debug, Clone)]
pub enum StmtKind {
    /// Expression statement
    Expr(Expr),
    /// Let binding (mutable)
    Let {
        name: String,
        name_span: Span,
        ty: Option<String>,
        init: Expr,
    },
    /// Let tuple destructuring
    LetTuple {
        patterns: Vec<TuplePat>,
        init: Expr,
    },
    /// Const binding (immutable)
    Const {
        name: String,
        name_span: Span,
        ty: Option<String>,
        init: Expr,
    },
    /// Const tuple destructuring
    ConstTuple {
        patterns: Vec<TuplePat>,
        init: Expr,
    },
    /// Assignment
    Assign {
        target: Expr,
        value: Expr,
    },
    /// Return statement
    Return(Option<Expr>),
    /// Break statement (optional label, optional value for loop-with-value)
    Break {
        label: Option<String>,
        value: Option<Expr>,
    },
    /// Continue statement
    Continue(Option<String>),
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
        binding: ForBinding,
        mutate: bool,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// Ensure block (deferred cleanup)
    Ensure {
        body: Vec<Stmt>,
        /// Optional else clause: (param_name, handler_body)
        else_handler: Option<(String, Vec<Stmt>)>,
    },
    /// Comptime block (compile-time evaluated)
    Comptime(Vec<Stmt>),
    /// Comptime for loop — unrolled at compile/monomorphization time (CT48–CT54)
    ComptimeFor {
        binding: ForBinding,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// Discard statement — explicitly drop a value and invalidate its binding (D1–D3)
    Discard {
        name: String,
        name_span: Span,
    },
}

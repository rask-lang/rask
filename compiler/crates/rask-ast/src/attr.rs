// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Attributes: `@test`, `@native("sym")`, `@rename("user_name")`,
//! `@validate(max: 100)`, `@liba.validate(max: 3)`.
//!
//! The parser splits one into its name and its arguments once, here, and
//! everything after reads those. They used to be stored as the source text,
//! and every reader re-split it — a prefix test for `native(`, a split on
//! commas for `get<A>().field`, a third copy in the checker — so what counted
//! as an attribute's name or its arguments could differ between readers.
//!
//! The name is a path because a user annotation is an ordinary name
//! (type.annotations/AN5): `@liba.validate` names `liba`'s. Package scoping
//! rewrites that, like any other reference to a dependency, to the name the
//! declaration was given, so after it a user annotation is one segment that
//! matches its declaration and whatever `has<A>()` names it with.
//!
//! An argument's value is an expression, parsed like any other: `100`,
//! `"user_name"`, `Color.Red`, `[1, 2]`, `Instant.epoch()`. What it means
//! depends on who reads it — a string to `@rename`, a typed constant to
//! `get<A>()` — and most readers want it as a [`Literal`]. The one exception is
//! `@allow(…)`, whose arguments are lint rule names (`idiom/duck-interface`,
//! `naming/as`) and not expressions at all.

use crate::expr::{Expr, ExprKind, UnaryOp};
use crate::Span;

/// One `@name` or `@name(args)` attached to a declaration, field or variant.
#[derive(Debug, Clone)]
pub struct Attr {
    /// The name, one entry per dotted segment.
    pub path: Vec<String>,
    /// `None` when written without parentheses (`@test`), otherwise the
    /// arguments in the order written.
    pub args: Option<Vec<AttrArg>>,
    pub span: Span,
}

/// One argument: `max: 100`, or `"user_name"` with no label.
#[derive(Debug, Clone)]
pub struct AttrArg {
    pub label: Option<String>,
    pub value: AttrValue,
}

/// What an argument holds.
#[derive(Debug, Clone)]
pub enum AttrValue {
    Expr(Expr),
    /// A lint or warning name inside `@allow(…)`. Its `/`, `-` and keywords
    /// are part of the name, so it isn't parsed as an expression.
    RuleName(String),
}

impl AttrValue {
    /// A bare name the compiler writes itself: the `C` of a synthetic
    /// `@layout(C)`.
    pub fn synthetic_name(name: &str) -> AttrValue {
        AttrValue::Expr(Expr {
            id: crate::NodeId::DUMMY,
            kind: ExprKind::Ident(name.to_string()),
            span: Span::new(0, 0),
        })
    }

    pub fn expr(&self) -> Option<&Expr> {
        match self {
            AttrValue::Expr(e) => Some(e),
            AttrValue::RuleName(_) => None,
        }
    }

    /// The value as a literal constant, when it is one.
    pub fn literal(&self) -> Option<Literal> {
        literal(self.expr()?)
    }

    /// The contents of a string literal: `"user_name"` → `user_name`.
    pub fn string(&self) -> Option<&str> {
        match &self.expr()?.kind {
            ExprKind::String(s) => Some(s),
            _ => None,
        }
    }

    /// A bare name: the `C` of `@layout(C)`.
    pub fn ident(&self) -> Option<&str> {
        match &self.expr()?.kind {
            ExprKind::Ident(s) => Some(s),
            _ => None,
        }
    }

    pub fn rule_name(&self) -> Option<&str> {
        match self {
            AttrValue::RuleName(n) => Some(n),
            AttrValue::Expr(_) => None,
        }
    }
}

/// A constant written as a literal. What a value in an attribute, or a field's
/// declared default, reduces to for the readers that splice it.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i128),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char),
}

/// `expr` as a literal, with a leading `-` folded into a number.
pub fn literal(expr: &Expr) -> Option<Literal> {
    Some(match &expr.kind {
        ExprKind::Int(n, _) => Literal::Int(*n),
        ExprKind::Float(f, _) => Literal::Float(*f),
        ExprKind::Bool(b) => Literal::Bool(*b),
        ExprKind::String(s) => Literal::Str(s.clone()),
        ExprKind::Char(c) => Literal::Char(*c),
        ExprKind::Unary { op: UnaryOp::Neg, operand } => match literal(operand)? {
            Literal::Int(n) => Literal::Int(-n),
            Literal::Float(f) => Literal::Float(-f),
            _ => return None,
        },
        _ => return None,
    })
}

impl Attr {
    /// An attribute the compiler attaches itself (`@entry` on a script's
    /// `main`, `@layout(C)` on an imported C struct).
    pub fn synthetic(name: &str, args: Option<Vec<AttrArg>>) -> Attr {
        Attr { path: vec![name.to_string()], args, span: Span::new(0, 0) }
    }

    /// The name as written, segments joined with `.`.
    pub fn name(&self) -> String {
        self.path.join(".")
    }

    /// Is this the attribute `name`? A dotted path is never a compiler-known
    /// attribute, so it never matches.
    pub fn is(&self, name: &str) -> bool {
        self.path.len() == 1 && self.path[0] == name
    }

    /// The first argument without a label: the `"sym"` of `@native("sym")`.
    pub fn positional(&self) -> Option<&AttrValue> {
        self.args.as_ref()?.iter().find(|a| a.label.is_none()).map(|a| &a.value)
    }

    /// The argument labelled `label`.
    pub fn arg(&self, label: &str) -> Option<&AttrValue> {
        self.labelled().find(|(l, _)| *l == label).map(|(_, v)| v)
    }

    /// Every labelled argument, in written order.
    pub fn labelled(&self) -> impl Iterator<Item = (&str, &AttrValue)> {
        self.args
            .iter()
            .flatten()
            .filter_map(|a| a.label.as_deref().map(|l| (l, &a.value)))
    }
}

/// `@name(label: value)`, for diagnostics and hashing. The formatter prints
/// values with its own expression printer; this one knows the constant forms
/// an attribute holds and writes `…` for anything else.
impl std::fmt::Display for Attr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())?;
        if let Some(args) = &self.args {
            write!(f, "(")?;
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                if let Some(l) = &a.label {
                    write!(f, "{}: ", l)?;
                }
                write!(f, "{}", a.value)?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

impl std::fmt::Display for AttrValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttrValue::RuleName(n) => write!(f, "{}", n),
            AttrValue::Expr(e) => write_const(f, e),
        }
    }
}

fn write_const(f: &mut std::fmt::Formatter<'_>, e: &Expr) -> std::fmt::Result {
    match &e.kind {
        ExprKind::Int(n, _) => write!(f, "{}", n),
        ExprKind::Float(x, _) => write!(f, "{:?}", x),
        ExprKind::Bool(b) => write!(f, "{}", b),
        ExprKind::String(s) => write!(f, "{:?}", s),
        ExprKind::Char(c) => write!(f, "{:?}", c),
        ExprKind::Ident(n) => write!(f, "{}", n),
        ExprKind::Unary { op: UnaryOp::Neg, operand } => {
            write!(f, "-")?;
            write_const(f, operand)
        }
        ExprKind::Field { object, field } => {
            write_const(f, object)?;
            write!(f, ".{}", field)
        }
        ExprKind::Array(items) => {
            write!(f, "[")?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write_const(f, item)?;
            }
            write!(f, "]")
        }
        _ => write!(f, "…"),
    }
}

/// The attribute `name` in `attrs`, if one is there.
pub fn find<'a>(attrs: &'a [Attr], name: &str) -> Option<&'a Attr> {
    attrs.iter().find(|a| a.is(name))
}

/// Whether `attrs` carries the attribute `name`.
pub fn has(attrs: &[Attr], name: &str) -> bool {
    find(attrs, name).is_some()
}

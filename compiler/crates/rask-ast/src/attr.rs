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
//! `"user_name"`, `Color.Red`, `[1, 2]`, `"nf {0}"`. What it means depends on
//! who reads it — a string to `@rename`, a typed constant to `get<A>()` — and
//! most readers want it as a [`Literal`]. The exception is `@allow(…)` and its
//! kin, whose arguments are lint rule names (`idiom/duck-interface`) and not
//! expressions at all; [`COMPILER_ATTRS`] says which those are.

use crate::expr::{Expr, ExprKind, Literal, UnaryOp};
use crate::Span;

/// How a compiler-known attribute's arguments are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgForm {
    /// Expressions: `@rename("id")`, `@layout(C)`, `@comptime_quota(10_000)`.
    Exprs,
    /// Lint or warning names: `@allow(idiom/duck-interface)`.
    RuleNames,
}

/// Every attribute name the compiler owns, and how its arguments are written.
/// The one list: the parser asks it which arguments are rule names, the
/// checker refuses these names to user annotations (type.annotations/AN5), and
/// `every_attribute_the_compiler_reads_is_listed` fails the build when the
/// compiler reads a name that isn't here.
///
/// Names the specs decide but nothing reads yet are listed too, so a user
/// annotation can't take one before it's built. `@embed_file` and the other
/// `@name(…)` forms written in an expression aren't attributes and aren't here.
pub const COMPILER_ATTRS: &[(&str, ArgForm)] = &[
    // Serialization (std.encoding)
    ("rename", ArgForm::Exprs),
    ("no_serialize", ArgForm::Exprs),
    ("skip", ArgForm::Exprs), // only to say it's now `@no_serialize`
    ("default", ArgForm::Exprs),
    ("tag", ArgForm::Exprs),
    ("no_encode", ArgForm::Exprs),
    ("no_decode", ArgForm::Exprs),
    ("binary", ArgForm::Exprs),
    // Types and memory
    ("resource", ArgForm::Exprs),
    ("unique", ArgForm::Exprs),
    ("small", ArgForm::Exprs),
    ("layout", ArgForm::Exprs),
    ("message", ArgForm::Exprs),
    // Functions
    ("entry", ArgForm::Exprs),
    ("no_alloc", ArgForm::Exprs),
    ("no_io", ArgForm::Exprs),
    ("test", ArgForm::Exprs),
    // Stdlib declarations: where the body lives
    ("native", ArgForm::Exprs),
    ("builtin", ArgForm::Exprs),
    ("unimplemented", ArgForm::Exprs),
    // Stdlib declarations: effects a body can't show (comp.advanced/EF1,
    // comp.effects/IO1, AS3)
    ("grows", ArgForm::Exprs),
    ("shrinks", ArgForm::Exprs),
    ("io", ArgForm::Exprs),
    ("waits", ArgForm::Exprs),
    // Stdlib declarations: a one-word value the runtime owns the storage of
    ("handle", ArgForm::Exprs),
    // Attached by the compiler itself, never written
    ("test_body", ArgForm::Exprs),
    // Comptime
    ("comptime_quota", ArgForm::Exprs),
    // Lints and warnings (tool.lint, tool.warnings)
    ("allow", ArgForm::RuleNames),
    ("warn", ArgForm::RuleNames),
    ("deny", ArgForm::RuleNames),
    // Decided, not built yet
    ("inline", ArgForm::Exprs),             // SYNTAX.md
    ("deprecated", ArgForm::Exprs),         // tool.warnings/W5
    ("packed", ArgForm::Exprs),             // type.structs
    ("align", ArgForm::Exprs),              // type.structs
    ("bitfield", ArgForm::Exprs),           // struct.c-interop
    ("export", ArgForm::Exprs),             // compiler.mangling
    ("traced", ArgForm::Exprs),             // type.errors/ER34
    ("pure", ArgForm::Exprs),               // tool.lint/P1
    ("checked_unsafe", ArgForm::Exprs),     // mem.unsafe
    ("unchecked_unsafe", ArgForm::Exprs),   // mem.unsafe
    ("call_text", ArgForm::Exprs),          // ctrl.call-info (proposed)
    ("call_location", ArgForm::Exprs),      // ctrl.call-info (proposed)
];

/// How the compiler-known attribute `name` takes its arguments, or `None` for
/// a name the compiler doesn't own (a user annotation).
pub fn compiler_attr(name: &str) -> Option<ArgForm> {
    COMPILER_ATTRS.iter().find(|(n, _)| *n == name).map(|(_, form)| *form)
}

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
        self.expr()?.as_literal()
    }

    /// The contents of a string literal: `"user_name"` → `user_name`.
    pub fn string(&self) -> Option<String> {
        match self.literal()? {
            Literal::Str(s) => Some(s),
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

/// A constant expression as a diagnostic quotes it: `-1.5`, `Color.Red`.
pub struct ShowConst<'a>(pub &'a Expr);

impl std::fmt::Display for ShowConst<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write_const(f, self.0)
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

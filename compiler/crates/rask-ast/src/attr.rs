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

/// Who may write a compiler-known attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// Any program.
    Anyone,
    /// Only the stdlib's own declarations. These say how a declaration is
    /// implemented, or what it does that its body can't show, and a program
    /// writing one would be believed: `@blocks` on a program function made it
    /// a blocking call (#1593).
    Stdlib,
    /// The stdlib, and any `extern` declaration. A C function has no body to
    /// read its effects from, so its declaration is the only place to say them
    /// (comp.effects/INF5).
    StdlibOrExtern,
}

/// Every attribute name the compiler owns, how its arguments are written, and
/// who may write it. The one list: the parser asks it which arguments are rule
/// names, the checker refuses these names to user annotations
/// (type.annotations/AN5) and the stdlib's to programs, and
/// `every_attribute_the_compiler_reads_is_listed` fails the build when the
/// compiler reads a name that isn't here.
///
/// Names the specs decide but nothing reads yet are listed too, so a user
/// annotation can't take one before it's built. `@embed_file` and the other
/// `@name(…)` forms written in an expression aren't attributes and aren't here.
pub const COMPILER_ATTRS: &[(&str, ArgForm, Writer)] = &[
    // Serialization (std.encoding)
    ("rename", ArgForm::Exprs, Writer::Anyone),
    ("no_serialize", ArgForm::Exprs, Writer::Anyone),
    ("skip", ArgForm::Exprs, Writer::Anyone), // only to say it's now `@no_serialize`
    ("default", ArgForm::Exprs, Writer::Anyone),
    ("tag", ArgForm::Exprs, Writer::Anyone),
    ("no_encode", ArgForm::Exprs, Writer::Anyone),
    ("no_decode", ArgForm::Exprs, Writer::Anyone),
    ("binary", ArgForm::Exprs, Writer::Anyone),
    // Types and memory
    ("resource", ArgForm::Exprs, Writer::Anyone),
    ("unique", ArgForm::Exprs, Writer::Anyone),
    ("small", ArgForm::Exprs, Writer::Anyone),
    ("layout", ArgForm::Exprs, Writer::Anyone),
    ("packed", ArgForm::Exprs, Writer::Anyone),
    ("align", ArgForm::Exprs, Writer::Anyone),
    ("message", ArgForm::Exprs, Writer::Anyone),
    ("reads_as", ArgForm::Exprs, Writer::Anyone),  // type.reads-as
    ("temporary", ArgForm::Exprs, Writer::Anyone), // type.temporary
    // Functions
    ("entry", ArgForm::Exprs, Writer::Anyone),
    ("no_alloc", ArgForm::Exprs, Writer::Anyone),
    ("inline", ArgForm::Exprs, Writer::Anyone),
    // Raw-pointer checks (mem.unsafe, debug-mode safety)
    ("checked_unsafe", ArgForm::Exprs, Writer::Anyone),
    ("unchecked_unsafe", ArgForm::Exprs, Writer::Anyone),
    ("test", ArgForm::Exprs, Writer::Anyone),
    // Stdlib declarations: where the body lives
    ("native", ArgForm::Exprs, Writer::Stdlib),
    ("builtin", ArgForm::Exprs, Writer::Stdlib),
    ("unimplemented", ArgForm::Exprs, Writer::Stdlib),
    // Stdlib declarations: effects a body can't show (comp.advanced/EF1,
    // comp.effects/FX1)
    ("grows", ArgForm::Exprs, Writer::Stdlib),
    ("shrinks", ArgForm::Exprs, Writer::Stdlib),
    ("blocks", ArgForm::Exprs, Writer::Stdlib),
    ("prints", ArgForm::Exprs, Writer::Stdlib),
    ("ambient", ArgForm::Exprs, Writer::StdlibOrExtern),
    // Stdlib declarations: the one half of a `MemoryOrder` an atomic operation
    // can apply. A load only reads, a store only writes (mem.atomics/AT6).
    ("acquires", ArgForm::Exprs, Writer::Stdlib),
    ("releases", ArgForm::Exprs, Writer::Stdlib),
    // Stdlib declarations: a one-word value the runtime owns the storage of
    ("handle", ArgForm::Exprs, Writer::Stdlib),
    // Attached by the compiler itself, never written
    ("test_body", ArgForm::Exprs, Writer::Anyone),
    ("claim", ArgForm::Exprs, Writer::Anyone), // type.claims/CL1
    // Comptime
    ("comptime_quota", ArgForm::Exprs, Writer::Anyone),
    // Claims
    ("claim_budget", ArgForm::Exprs, Writer::Anyone), // type.claims/D2
    // Lints and warnings (tool.lint, tool.warnings)
    ("allow", ArgForm::RuleNames, Writer::Anyone),
    ("warn", ArgForm::RuleNames, Writer::Anyone),
    ("deny", ArgForm::RuleNames, Writer::Anyone),
    // Decided, not built yet
    ("deprecated", ArgForm::Exprs, Writer::Anyone),         // tool.warnings/W5
    ("bits", ArgForm::Exprs, Writer::Anyone),               // struct.c-interop/TM7, on a field
    ("inline_from", ArgForm::Exprs, Writer::Anyone),        // struct.c-interop/IN1, on an extern
    ("context", ArgForm::Exprs, Writer::Anyone),            // struct.c-interop/CB3, on an extern
    ("export", ArgForm::Exprs, Writer::Anyone),             // compiler.mangling
    ("traced", ArgForm::Exprs, Writer::Anyone),             // type.errors/ER34
    ("memory_only", ArgForm::Exprs, Writer::Anyone),        // on an `extern` only, comp.effects/INF5
    ("call_text", ArgForm::Exprs, Writer::Anyone),          // ctrl.call-info (proposed)
    ("call_location", ArgForm::Exprs, Writer::Anyone),      // ctrl.call-info (proposed)
];

/// The largest `@align(N)` honoured, 2^29: LLVM's limit, so a layout this
/// backend accepts is one any backend can. Nothing here needs less. The heap
/// allocator takes any power of two, and a frame slot's spare bytes never
/// exceed the value's own size, which is a multiple of N.
pub const MAX_ALIGN: u32 = 1 << 29;

/// What `@packed` and `@align(N)` ask of a struct's layout (type.structs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LayoutRequest {
    /// No padding: every field at alignment 1, in declaration order.
    pub packed: bool,
    /// At least this alignment, a power of two up to `MAX_ALIGN`.
    pub align: Option<u32>,
}

/// Read `@packed` and `@align(N)` off a struct's attributes, or say what's
/// wrong with them and where.
pub fn layout_request(attrs: &[Attr]) -> Result<LayoutRequest, (Span, String)> {
    let mut req = LayoutRequest::default();
    for a in attrs {
        if a.is("packed") {
            if a.args.as_ref().is_some_and(|args| !args.is_empty()) {
                return Err((a.span, "`@packed` takes no arguments".to_string()));
            }
            req.packed = true;
        } else if a.is("align") {
            let n = match (a.args.as_deref(), a.positional().and_then(AttrValue::literal)) {
                (Some([_]), Some(Literal::Int(n))) => n,
                _ => return Err((a.span, "`@align` takes one integer: `@align(8)`".to_string())),
            };
            if n < 1 || n > MAX_ALIGN as i128 || n & (n - 1) != 0 {
                return Err((
                    a.span,
                    format!("`@align({n})`: the alignment has to be a power of two from 1 to {MAX_ALIGN}"),
                ));
            }
            req.align = Some(n as u32);
        }
    }
    if req.packed && req.align.is_some() {
        return Err((
            attrs.iter().find(|a| a.is("align")).map_or(Span::new(0, 0), |a| a.span),
            "`@packed` and `@align` contradict each other: packed means alignment 1".to_string(),
        ));
    }
    Ok(req)
}

/// How the compiler-known attribute `name` takes its arguments, or `None` for
/// a name the compiler doesn't own (a user annotation).
pub fn compiler_attr(name: &str) -> Option<ArgForm> {
    COMPILER_ATTRS.iter().find(|(n, ..)| *n == name).map(|(_, form, _)| *form)
}

/// Whether only the stdlib may write the attribute `name`.
pub fn stdlib_only(name: &str) -> bool {
    COMPILER_ATTRS
        .iter()
        .any(|(n, _, w)| *n == name && matches!(w, Writer::Stdlib | Writer::StdlibOrExtern))
}

/// Whether a program's `extern` declaration may write the attribute `name`.
pub fn extern_may_write(name: &str) -> bool {
    COMPILER_ATTRS.iter().any(|(n, _, w)| *n == name && *w == Writer::StdlibOrExtern)
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

/// What `extern "C" struct Name` carries: a C type with no body, so no
/// layout. Never written as an attribute; the formatter prints the form back.
pub const OPAQUE: &str = "extern_c_opaque";

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

/// `@context(callback, userdata, at: i)` on an extern (struct.c-interop/CB3):
/// parameter `context` is `callback`'s context pointer, arriving as the
/// callback's `at`th argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSpec {
    pub callback: usize,
    pub context: usize,
    pub at: usize,
}

/// The extern's `@context`, read against its parameter names. `None` when it
/// has none; `Some(Err)` says what's wrong with the one it has.
pub fn context_spec(attrs: &[Attr], params: &[&str]) -> Option<Result<ContextSpec, (Span, String)>> {
    let a = attrs.iter().find(|a| a.is("context"))?;
    let positional: Vec<&AttrValue> = a.args.iter().flatten().filter(|x| x.label.is_none()).map(|x| &x.value).collect();
    let name_of = |v: &AttrValue| v.ident().map(str::to_string);
    let (Some(cb), Some(ctx)) = (positional.first().and_then(|v| name_of(v)), positional.get(1).and_then(|v| name_of(v))) else {
        return Some(Err((a.span, "`@context` names the callback parameter, then the context one: `@context(callback, userdata, at: 0)`".to_string())));
    };
    let at = match a.arg("at").and_then(|v| v.literal()) {
        Some(crate::expr::Literal::Int(n)) if n >= 0 => n as usize,
        _ => return Some(Err((a.span, "`@context` needs `at:`, which argument of the callback the context arrives as".to_string()))),
    };
    let index = |n: &str| params.iter().position(|p| *p == n);
    match (index(&cb), index(&ctx)) {
        (Some(callback), Some(context)) if callback != context => Some(Ok(ContextSpec { callback, context, at })),
        (None, _) => Some(Err((a.span, format!("`@context` names `{}`, which isn't a parameter", cb)))),
        (_, None) => Some(Err((a.span, format!("`@context` names `{}`, which isn't a parameter", ctx)))),
        _ => Some(Err((a.span, "`@context` needs two different parameters".to_string()))),
    }
}

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
//! An argument's value stays its source text: `100`, `"user_name"`,
//! `Color.Red`, `[1, 2]`. What it means depends on who reads it — a string to
//! `@rename`, a typed constant to `get<A>()` (`annotation_value`), a rule id
//! to `@allow` — and that reader turns it into one.

use crate::Span;

/// One `@name` or `@name(args)` attached to a declaration, field or variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Attr {
    /// The name, one entry per dotted segment.
    pub path: Vec<String>,
    /// `None` when written without parentheses (`@test`), otherwise the
    /// arguments in the order written.
    pub args: Option<Vec<AttrArg>>,
    pub span: Span,
}

/// One argument: `max: 100`, or `"user_name"` with no label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttrArg {
    pub label: Option<String>,
    /// The value as written.
    pub value: String,
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
    pub fn positional(&self) -> Option<&str> {
        self.args.as_ref()?.iter().find(|a| a.label.is_none()).map(|a| a.value.as_str())
    }

    /// The argument labelled `label`.
    pub fn arg(&self, label: &str) -> Option<&str> {
        self.labelled().find(|(l, _)| *l == label).map(|(_, v)| v)
    }

    /// Every labelled argument, in written order.
    pub fn labelled(&self) -> impl Iterator<Item = (&str, &str)> {
        self.args
            .iter()
            .flatten()
            .filter_map(|a| a.label.as_deref().map(|l| (l, a.value.as_str())))
    }
}

/// `@name`, `@name(a, b)`, `@name(label: value)` — the form the formatter
/// prints and a diagnostic quotes.
impl std::fmt::Display for Attr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())?;
        if let Some(args) = &self.args {
            write!(f, "(")?;
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                match &a.label {
                    Some(l) => write!(f, "{}: {}", l, a.value)?,
                    None => write!(f, "{}", a.value)?,
                }
            }
            write!(f, ")")?;
        }
        Ok(())
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

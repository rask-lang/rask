// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A type as written in source.
//!
//! The parser builds this tree and every later pass walks it. Types used to be
//! rendered to strings here and parsed back by each consumer, and seven
//! parsers drifted apart doing it. `Display` gives the written spelling for
//! messages and the formatter; nothing reads a type back out of it.

use std::fmt;

/// A written type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeExpr {
    /// A named type with its arguments: `i64`, `Point`, `io.Buffer`,
    /// `Map<K, V>`, `Buffer<u8, 256>`. `path` holds the dotted segments.
    Named { path: Vec<String>, args: Vec<TypeExpr> },
    /// An integer in type position: the `256` of `Buffer<u8, 256>`.
    Int(String),
    /// `T?`
    Optional(Box<TypeExpr>),
    /// `T or E`
    Result { ok: Box<TypeExpr>, err: Box<TypeExpr> },
    /// An error union, `E1 | E2`. Only written in error position.
    Union(Vec<TypeExpr>),
    /// `(A, B)`, arity two or more.
    Tuple(Vec<TypeExpr>),
    /// `void`
    Unit,
    /// `none`
    NoneType,
    /// `[T; N]`, where `N` is a literal or a comptime parameter's name.
    Array { elem: Box<TypeExpr>, len: String },
    /// `[N]T`, the fixed-count form `@binary` layouts use.
    FixedCount { count: String, elem: Box<TypeExpr> },
    /// `func(A, mutate B, take C) -> R` and `|A, mutate B| -> R`. Each
    /// parameter carries its mode: it is part of the type (type.functions/FT1).
    Func { params: Vec<FuncParam>, ret: Box<TypeExpr> },
    /// `*T`
    RawPtr(Box<TypeExpr>),
    /// `any Interface`
    Any(Box<TypeExpr>),
}

/// How a parameter is passed: lent, lent for writing, or handed over.
///
/// A `deleting` parameter is `Mutate` here. The extra promise it makes is
/// about which links survive the call, which the ownership pass reads off the
/// declaration, not off a function's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamMode {
    Borrow,
    Mutate,
    Take,
}

impl ParamMode {
    /// The word written before the parameter, or `None` for a borrow.
    pub fn keyword(self) -> Option<&'static str> {
        match self {
            ParamMode::Borrow => None,
            ParamMode::Mutate => Some("mutate"),
            ParamMode::Take => Some("take"),
        }
    }

    pub fn from_flags(is_take: bool, is_mutate: bool) -> ParamMode {
        if is_take {
            ParamMode::Take
        } else if is_mutate {
            ParamMode::Mutate
        } else {
            ParamMode::Borrow
        }
    }
}

/// One parameter of a function type: its mode and its type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FuncParam {
    pub mode: ParamMode,
    pub ty: TypeExpr,
}

impl FuncParam {
    pub fn borrowed(ty: TypeExpr) -> FuncParam {
        FuncParam { mode: ParamMode::Borrow, ty }
    }

    fn map(&self, f: impl FnOnce(&TypeExpr) -> TypeExpr) -> FuncParam {
        FuncParam { mode: self.mode, ty: f(&self.ty) }
    }

    fn source(&self) -> String {
        match self.mode.keyword() {
            Some(kw) => format!("{} {}", kw, self.ty.source()),
            None => self.ty.source(),
        }
    }
}

impl fmt::Display for FuncParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(kw) = self.mode.keyword() {
            write!(f, "{} ", kw)?;
        }
        write!(f, "{}", self.ty)
    }
}

impl TypeExpr {
    /// A plain name with no arguments.
    pub fn named(name: impl Into<String>) -> TypeExpr {
        TypeExpr::Named { path: vec![name.into()], args: Vec::new() }
    }

    /// A name with arguments.
    pub fn generic(name: impl Into<String>, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Named { path: vec![name.into()], args }
    }

    /// The written name of a named type, dotted path joined: `io.Buffer`.
    /// `None` for anything that isn't a name.
    pub fn name(&self) -> Option<String> {
        match self {
            TypeExpr::Named { path, .. } => Some(path.join(".")),
            _ => None,
        }
    }

    /// The last path segment of a named type: `Buffer` for `io.Buffer<u8>`.
    pub fn last_segment(&self) -> Option<&str> {
        match self {
            TypeExpr::Named { path, .. } => path.last().map(String::as_str),
            _ => None,
        }
    }

    /// A named type's arguments; empty for anything else.
    pub fn args(&self) -> &[TypeExpr] {
        match self {
            TypeExpr::Named { args, .. } => args,
            _ => &[],
        }
    }

    /// The name, when this is a single bare name: `i64`, `T`, `Point`.
    pub fn bare_name(&self) -> Option<&str> {
        match self {
            TypeExpr::Named { path, args } if args.is_empty() && path.len() == 1 => Some(&path[0]),
            _ => None,
        }
    }

    /// Is this the bare name `name`, with no arguments and no path?
    pub fn is_name(&self, name: &str) -> bool {
        matches!(self, TypeExpr::Named { path, args } if args.is_empty() && path.len() == 1 && path[0] == name)
    }

    /// Replace every bare name in `subst` with what it maps to. A name that
    /// carries arguments or a path is a type, not a parameter, and is left
    /// alone; its arguments are still walked.
    pub fn substitute(&self, subst: &dyn Fn(&str) -> Option<TypeExpr>) -> TypeExpr {
        let sub = |t: &TypeExpr| t.substitute(subst);
        let sub_box = |t: &TypeExpr| Box::new(t.substitute(subst));
        match self {
            TypeExpr::Named { path, args } => {
                if args.is_empty() && path.len() == 1 {
                    if let Some(to) = subst(&path[0]) {
                        return to;
                    }
                }
                TypeExpr::Named { path: path.clone(), args: args.iter().map(sub).collect() }
            }
            TypeExpr::Int(n) => match subst(n) {
                Some(to) => to,
                None => self.clone(),
            },
            TypeExpr::Optional(inner) => TypeExpr::Optional(sub_box(inner)),
            TypeExpr::Result { ok, err } => TypeExpr::Result { ok: sub_box(ok), err: sub_box(err) },
            TypeExpr::Union(ts) => TypeExpr::Union(ts.iter().map(sub).collect()),
            TypeExpr::Tuple(ts) => TypeExpr::Tuple(ts.iter().map(sub).collect()),
            TypeExpr::Unit | TypeExpr::NoneType => self.clone(),
            TypeExpr::Array { elem, len } => {
                let len = match subst(len) {
                    Some(TypeExpr::Int(n)) => n,
                    _ => len.clone(),
                };
                TypeExpr::Array { elem: sub_box(elem), len }
            }
            TypeExpr::FixedCount { count, elem } => {
                TypeExpr::FixedCount { count: count.clone(), elem: sub_box(elem) }
            }
            TypeExpr::Func { params, ret } => {
                TypeExpr::Func { params: params.iter().map(|p| p.map(sub)).collect(), ret: sub_box(ret) }
            }
            TypeExpr::RawPtr(inner) => TypeExpr::RawPtr(sub_box(inner)),
            TypeExpr::Any(inner) => TypeExpr::Any(sub_box(inner)),
        }
    }

    /// Replace every two-segment projection `T.Out` that `f` answers for. The
    /// pair is passed split: `("T", "Out")`.
    pub fn substitute_projections(&self, f: &dyn Fn(&str, &str) -> Option<TypeExpr>) -> TypeExpr {
        let mut out = self.clone();
        out.replace_projections(f);
        out
    }

    fn replace_projections(&mut self, f: &dyn Fn(&str, &str) -> Option<TypeExpr>) {
        match self {
            TypeExpr::Named { path, args } => {
                if let ([head, tail], true) = (path.as_slice(), args.is_empty()) {
                    if let Some(to) = f(head, tail) {
                        *self = to;
                        return;
                    }
                }
                args.iter_mut().for_each(|a| a.replace_projections(f));
            }
            TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => {}
            TypeExpr::Optional(inner) | TypeExpr::RawPtr(inner) | TypeExpr::Any(inner) => {
                inner.replace_projections(f)
            }
            TypeExpr::Result { ok, err } => {
                ok.replace_projections(f);
                err.replace_projections(f);
            }
            TypeExpr::Union(ts) | TypeExpr::Tuple(ts) => ts.iter_mut().for_each(|t| t.replace_projections(f)),
            TypeExpr::Array { elem, .. } | TypeExpr::FixedCount { elem, .. } => elem.replace_projections(f),
            TypeExpr::Func { params, ret } => {
                params.iter_mut().for_each(|p| p.ty.replace_projections(f));
                ret.replace_projections(f);
            }
        }
    }

    /// Rename every single-segment type name `f` answers for, with or without
    /// arguments, and walk into everything.
    pub fn rename(&mut self, f: &dyn Fn(&str) -> Option<String>) {
        match self {
            TypeExpr::Named { path, args } => {
                if path.len() == 1 {
                    if let Some(to) = f(&path[0]) {
                        path[0] = to;
                    }
                }
                args.iter_mut().for_each(|a| a.rename(f));
            }
            TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => {}
            TypeExpr::Optional(inner) | TypeExpr::RawPtr(inner) | TypeExpr::Any(inner) => {
                inner.rename(f)
            }
            TypeExpr::Result { ok, err } => {
                ok.rename(f);
                err.rename(f);
            }
            TypeExpr::Union(ts) | TypeExpr::Tuple(ts) => ts.iter_mut().for_each(|t| t.rename(f)),
            TypeExpr::Array { elem, .. } | TypeExpr::FixedCount { elem, .. } => elem.rename(f),
            TypeExpr::Func { params, ret } => {
                params.iter_mut().for_each(|p| p.ty.rename(f));
                ret.rename(f);
            }
        }
    }

    /// Does any named type anywhere in this one satisfy `f`? Asked of the
    /// name without its arguments: `Map` for `Map<K, V>`.
    pub fn mentions(&self, f: &dyn Fn(&str) -> bool) -> bool {
        match self {
            TypeExpr::Named { path, args } => {
                f(&path.join(".")) || args.iter().any(|a| a.mentions(f))
            }
            TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => false,
            TypeExpr::Optional(inner) | TypeExpr::RawPtr(inner) | TypeExpr::Any(inner) => {
                inner.mentions(f)
            }
            TypeExpr::Result { ok, err } => ok.mentions(f) || err.mentions(f),
            TypeExpr::Union(ts) | TypeExpr::Tuple(ts) => ts.iter().any(|t| t.mentions(f)),
            TypeExpr::Array { elem, .. } | TypeExpr::FixedCount { elem, .. } => elem.mentions(f),
            TypeExpr::Func { params, ret } => params.iter().any(|p| p.ty.mentions(f)) || ret.mentions(f),
        }
    }

    /// The same type with every named type's path passed through `f`, which
    /// answers a replacement path or `None` to keep it. Arguments are rewritten
    /// too.
    pub fn substitute_paths(&self, f: &dyn Fn(&[String]) -> Option<Vec<String>>) -> TypeExpr {
        let sub = |t: &TypeExpr| t.substitute_paths(f);
        let sub_box = |t: &TypeExpr| Box::new(t.substitute_paths(f));
        match self {
            TypeExpr::Named { path, args } => TypeExpr::Named {
                path: f(path).unwrap_or_else(|| path.clone()),
                args: args.iter().map(sub).collect(),
            },
            TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => self.clone(),
            TypeExpr::Optional(inner) => TypeExpr::Optional(sub_box(inner)),
            TypeExpr::RawPtr(inner) => TypeExpr::RawPtr(sub_box(inner)),
            TypeExpr::Any(inner) => TypeExpr::Any(sub_box(inner)),
            TypeExpr::Result { ok, err } => TypeExpr::Result { ok: sub_box(ok), err: sub_box(err) },
            TypeExpr::Union(ts) => TypeExpr::Union(ts.iter().map(sub).collect()),
            TypeExpr::Tuple(ts) => TypeExpr::Tuple(ts.iter().map(sub).collect()),
            TypeExpr::Array { elem, len } => TypeExpr::Array { elem: sub_box(elem), len: len.clone() },
            TypeExpr::FixedCount { count, elem } => {
                TypeExpr::FixedCount { count: count.clone(), elem: sub_box(elem) }
            }
            TypeExpr::Func { params, ret } => {
                TypeExpr::Func { params: params.iter().map(|p| p.map(sub)).collect(), ret: sub_box(ret) }
            }
        }
    }

    /// Every named type's path, outermost first: `Vec<time.Duration>?` gives
    /// `["Vec"]` then `["time", "Duration"]`.
    pub fn walk_paths(&self, f: &mut dyn FnMut(&[String])) {
        match self {
            TypeExpr::Named { path, args } => {
                f(path);
                args.iter().for_each(|a| a.walk_paths(f));
            }
            TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => {}
            TypeExpr::Optional(inner) | TypeExpr::RawPtr(inner) | TypeExpr::Any(inner) => {
                inner.walk_paths(f)
            }
            TypeExpr::Result { ok, err } => {
                ok.walk_paths(f);
                err.walk_paths(f);
            }
            TypeExpr::Union(ts) | TypeExpr::Tuple(ts) => ts.iter().for_each(|t| t.walk_paths(f)),
            TypeExpr::Array { elem, .. } | TypeExpr::FixedCount { elem, .. } => elem.walk_paths(f),
            TypeExpr::Func { params, ret } => {
                params.iter().for_each(|p| p.ty.walk_paths(f));
                ret.walk_paths(f);
            }
        }
    }

    /// Every bare name in this type, in written order, duplicates included.
    pub fn walk_names(&self, f: &mut dyn FnMut(&str)) {
        match self {
            TypeExpr::Named { path, args } => {
                if args.is_empty() && path.len() == 1 {
                    f(&path[0]);
                }
                for a in args {
                    a.walk_names(f);
                }
            }
            TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => {}
            TypeExpr::Optional(inner) | TypeExpr::RawPtr(inner) | TypeExpr::Any(inner) => {
                inner.walk_names(f)
            }
            TypeExpr::Result { ok, err } => {
                ok.walk_names(f);
                err.walk_names(f);
            }
            TypeExpr::Union(ts) | TypeExpr::Tuple(ts) => ts.iter().for_each(|t| t.walk_names(f)),
            TypeExpr::Array { elem, .. } | TypeExpr::FixedCount { elem, .. } => elem.walk_names(f),
            TypeExpr::Func { params, ret } => {
                params.iter().for_each(|p| p.ty.walk_names(f));
                ret.walk_names(f);
            }
        }
    }
}

impl TypeExpr {
    /// Would a suffix or an `or` written after this type attach to a part of
    /// it rather than to the whole? True for `T or E`, and for a function type
    /// with a return, whose return runs to the end: `func() -> i64?` returns
    /// `i64?`.
    fn runs_on(&self) -> bool {
        match self {
            TypeExpr::Result { .. } => true,
            TypeExpr::Func { ret, .. } => **ret != TypeExpr::Unit,
            _ => false,
        }
    }

    /// The type as a programmer writes it: `T or E`, `void`, `func(A)` with
    /// no arrow for a `void` return. `Display` is the canonical spelling
    /// diagnostics use, which names the result type `Result<T, E>`.
    pub fn source(&self) -> String {
        let list = |ts: &[TypeExpr], sep: &str| {
            ts.iter().map(TypeExpr::source).collect::<Vec<_>>().join(sep)
        };
        match self {
            TypeExpr::Named { path, args } => {
                let base = path.join(".");
                if args.is_empty() {
                    base
                } else {
                    format!("{}<{}>", base, list(args, ", "))
                }
            }
            TypeExpr::Int(n) => n.clone(),
            // A suffix binds tighter than `or`, and a function's return runs to
            // the end of the type, so either one under a `?` needs its
            // parentheses back.
            TypeExpr::Optional(inner) if inner.runs_on() => format!("({})?", inner.source()),
            TypeExpr::Optional(inner) => format!("{}?", inner.source()),
            TypeExpr::Result { ok, err } => {
                let ok_src = if ok.runs_on() { format!("({})", ok.source()) } else { ok.source() };
                format!("{} or {}", ok_src, err.source())
            }
            TypeExpr::Union(ts) => ts
                .iter()
                .map(|t| if t.runs_on() { format!("({})", t.source()) } else { t.source() })
                .collect::<Vec<_>>()
                .join(" | "),
            TypeExpr::Tuple(ts) => format!("({})", list(ts, ", ")),
            TypeExpr::Unit => "void".to_string(),
            TypeExpr::NoneType => "none".to_string(),
            TypeExpr::Array { elem, len } => format!("[{}; {}]", elem.source(), len),
            TypeExpr::FixedCount { count, elem } => format!("[{}]{}", count, elem.source()),
            // An omitted return type is `void`, so writing it back would add an
            // arrow the source never had. `func(…)` rather than `|…|`: `||` is
            // the or-operator token, so a zero-parameter closure type can't use
            // the other spelling.
            TypeExpr::Func { params, ret } => {
                let params = params.iter().map(FuncParam::source).collect::<Vec<_>>().join(", ");
                match **ret {
                    TypeExpr::Unit => format!("func({})", params),
                    _ => format!("func({}) -> {}", params, ret.source()),
                }
            }
            TypeExpr::RawPtr(inner) => format!("*{}", inner.source()),
            TypeExpr::Any(inner) => format!("any {}", inner.source()),
        }
    }
}

fn join(f: &mut fmt::Formatter<'_>, ts: &[TypeExpr], sep: &str) -> fmt::Result {
    for (i, t) in ts.iter().enumerate() {
        if i > 0 {
            f.write_str(sep)?;
        }
        write!(f, "{}", t)?;
    }
    Ok(())
}

impl fmt::Display for TypeExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeExpr::Named { path, args } => {
                f.write_str(&path.join("."))?;
                if !args.is_empty() {
                    f.write_str("<")?;
                    join(f, args, ", ")?;
                    f.write_str(">")?;
                }
                Ok(())
            }
            TypeExpr::Int(n) => f.write_str(n),
            TypeExpr::Optional(inner) => write!(f, "{}?", inner),
            TypeExpr::Result { ok, err } => write!(f, "Result<{}, {}>", ok, err),
            TypeExpr::Union(ts) => join(f, ts, "|"),
            TypeExpr::Tuple(ts) => {
                f.write_str("(")?;
                join(f, ts, ", ")?;
                f.write_str(")")
            }
            TypeExpr::Unit => f.write_str("()"),
            TypeExpr::NoneType => f.write_str("none"),
            TypeExpr::Array { elem, len } => write!(f, "[{}; {}]", elem, len),
            TypeExpr::FixedCount { count, elem } => write!(f, "[{}]{}", count, elem),
            TypeExpr::Func { params, ret } => {
                f.write_str("func(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            TypeExpr::RawPtr(inner) => write!(f, "*{}", inner),
            TypeExpr::Any(inner) => write!(f, "any {}", inner),
        }
    }
}

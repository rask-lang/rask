// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What a stdlib spec promises, and what the stdlib declares, as comparable
//! lists of functions and types.
//!
//! A spec states its surface in blocks marked `<!-- surface -->`: body-less
//! declarations in the shape `stdlib/*.rk` uses, so module functions sit in
//! `extend time { … }` and methods in `extend Duration { … }`. The same
//! reader turns those blocks and the stdlib's own sources into [`Surface`]s,
//! so both sides are compared on identical terms.

use std::path::Path;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::ty::{ParamMode, TypeExpr};

/// The marker line above a surface block.
pub const SURFACE_MARKER: &str = "<!-- surface -->";

/// One function or method, as declared.
#[derive(Debug, Clone)]
pub struct SurfaceFn {
    /// The `extend` target with its type arguments dropped (`Vec`, `time`),
    /// or empty for a free function.
    pub owner: String,
    pub name: String,
    /// The interface name, for a method a conformance supplies
    /// (`Instant implements Sub<Duration>` gives `Sub`).
    pub via: Option<String>,
    /// `None` for a function without `self`.
    pub self_mode: Option<ParamMode>,
    /// Mode and type of each parameter after `self`, normalized by
    /// [`normalize`]. Names aren't compared.
    pub params: Vec<(ParamMode, TypeExpr)>,
    pub ret: TypeExpr,
    pub is_pub: bool,
    /// Declared `@unimplemented`: there's a signature and nothing behind it.
    pub unimplemented: bool,
    /// A method an interface requires. It has no body of its own to run, so
    /// only its signature is compared.
    pub required: bool,
    /// 1-based line in the file it came from.
    pub line: usize,
}

/// What identifies a function across the spec, the stdlib and a call site.
///
/// Owner and name, plus for a conformance method its interface and the type of
/// its first parameter. `Instant` has two `sub`s, one per right operand, and
/// a conformance written `Duration implements Add` is the same one as
/// `Duration implements Add<Duration>`; keying on the parameter rather than
/// the interface's written arguments treats both alike.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FnKey {
    pub owner: String,
    pub name: String,
    pub via: Option<(String, Option<TypeExpr>)>,
}

impl SurfaceFn {
    pub fn key(&self) -> FnKey {
        FnKey {
            owner: self.owner.clone(),
            name: self.name.clone(),
            via: self.via.clone().map(|i| (i, self.params.first().map(|(_, t)| t.clone()))),
        }
    }

    /// `Vec.push`, `println`, or `Instant.sub [Sub<Duration>]`.
    pub fn qualified(&self) -> String {
        let base = if self.owner.is_empty() { self.name.clone() } else { format!("{}.{}", self.owner, self.name) };
        match (&self.via, self.params.first()) {
            (Some(i), Some((_, rhs))) => format!("{base} [{i}<{}>]", rask_syntax(rhs)),
            (Some(i), None) => format!("{base} [{i}]"),
            (None, _) => base,
        }
    }

    /// The signature in Rask syntax, without parameter names.
    pub fn signature(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(mode) = self.self_mode {
            parts.push(with_mode(mode, "self"));
        }
        for (mode, ty) in &self.params {
            parts.push(with_mode(*mode, &rask_syntax(ty)));
        }
        let ret = if self.ret == TypeExpr::Unit { String::new() } else { format!(" -> {}", rask_syntax(&self.ret)) };
        format!("func {}({}){}", self.name, parts.join(", "), ret)
    }

    /// Same shape: receiver mode, parameter modes and types, return type.
    pub fn matches(&self, other: &SurfaceFn) -> bool {
        self.self_mode == other.self_mode && self.params == other.params && self.ret == other.ret
    }
}

fn with_mode(mode: ParamMode, rest: &str) -> String {
    match mode.keyword() {
        Some(kw) => format!("{kw} {rest}"),
        None => rest.to_string(),
    }
}

/// A struct, enum or interface, as declared.
#[derive(Debug, Clone)]
pub struct SurfaceType {
    pub name: String,
    pub kind: &'static str,
    /// Variant names, for an enum. A spec listing variants is promising them.
    pub variants: Vec<String>,
    pub is_pub: bool,
    pub line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Surface {
    pub fns: Vec<SurfaceFn>,
    pub types: Vec<SurfaceType>,
}

/// A spec's promised surface.
#[derive(Debug, Clone)]
pub struct SpecSurface {
    /// The spec's `<!-- id: … -->`, e.g. `std.time`.
    pub id: String,
    pub surface: Surface,
    /// Number of surface blocks found. Zero means the spec hasn't stated one.
    pub blocks: usize,
    /// A block that doesn't parse, as (line, message).
    pub errors: Vec<(usize, String)>,
}

/// Read every surface block in a spec.
pub fn spec_surface(path: &Path, markdown: &str) -> SpecSurface {
    let id = markdown
        .lines()
        .find_map(|l| l.trim().strip_prefix("<!-- id:")?.strip_suffix("-->").map(|s| s.trim().to_string()))
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());

    let mut out = SpecSurface { id, surface: Surface::default(), blocks: 0, errors: Vec::new() };
    for (first_line, code) in surface_blocks(markdown) {
        out.blocks += 1;
        let lex = rask_lexer::Lexer::new(&code).tokenize();
        if let Some(e) = lex.errors.first() {
            out.errors.push((first_line, format!("{:?}", e)));
            continue;
        }
        let parsed = rask_parser::Parser::new(lex.tokens).parse();
        if let Some(e) = parsed.errors.first() {
            out.errors.push((first_line, e.message.clone()));
            continue;
        }
        let line_of = |offset: usize| first_line + code[..offset.min(code.len())].matches('\n').count();
        collect(&parsed.decls, &line_of, &mut out.surface);
    }
    out
}

/// Each surface block's code and the line its first statement is on.
fn surface_blocks(markdown: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != SURFACE_MARKER {
            i += 1;
            continue;
        }
        i += 1;
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }
        if i < lines.len() && lines[i].trim_start().starts_with("```rask") {
            i += 1;
            let start = i;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                i += 1;
            }
            blocks.push((start + 1, lines[start..i].join("\n")));
        }
    }
    blocks
}

/// What a Rask source declares, numbering lines from 1.
pub fn source_surface(source: &str) -> Result<Surface, String> {
    stdlib_source_surface(source, |_| {})
}

/// The same, with `prepare` applied to the parsed declarations first. The
/// stdlib's own loader rewrites some of what's written (one `extend Integer`
/// block becomes one block per width), and the gate has to read what the
/// checker reads.
pub fn stdlib_source_surface(source: &str, prepare: impl FnOnce(&mut Vec<Decl>)) -> Result<Surface, String> {
    let lex = rask_lexer::Lexer::new(source).tokenize();
    if let Some(e) = lex.errors.first() {
        return Err(format!("{:?}", e));
    }
    let mut parsed = rask_parser::Parser::new(lex.tokens).parse();
    if let Some(e) = parsed.errors.first() {
        return Err(e.message.clone());
    }
    prepare(&mut parsed.decls);
    let line_of = |offset: usize| 1 + source[..offset.min(source.len())].matches('\n').count();
    let mut surface = Surface::default();
    collect(&parsed.decls, &line_of, &mut surface);
    Ok(surface)
}

fn collect(decls: &[Decl], line_of: &dyn Fn(usize) -> usize, out: &mut Surface) {
    for decl in decls {
        let line = line_of(decl.span.start);
        match &decl.kind {
            DeclKind::Fn(f) => out.fns.push(surface_fn("", None, f, line_of)),
            DeclKind::Impl(imp) => {
                let owner = owner_name(&imp.target_ty);
                let via = imp.interface.as_ref().map(owner_name);
                for m in &imp.methods {
                    out.fns.push(surface_fn(&owner, via.clone(), m, line_of));
                }
            }
            DeclKind::Struct(s) => {
                out.types.push(SurfaceType { name: s.name.clone(), kind: "struct", variants: Vec::new(), is_pub: s.is_pub, line });
                for m in &s.methods {
                    out.fns.push(surface_fn(&s.name, None, m, line_of));
                }
            }
            DeclKind::Enum(e) => {
                out.types.push(SurfaceType {
                    name: e.name.clone(),
                    kind: "enum",
                    variants: e.variants.iter().map(|v| v.name.clone()).collect(),
                    is_pub: e.is_pub,
                    line,
                });
                for m in &e.methods {
                    out.fns.push(surface_fn(&e.name, None, m, line_of));
                }
            }
            DeclKind::Interface(i) => {
                out.types.push(SurfaceType { name: i.name.clone(), kind: "interface", variants: Vec::new(), is_pub: i.is_pub, line });
                for m in &i.methods {
                    let mut f = surface_fn(&i.name, None, m, line_of);
                    f.required = true;
                    // An interface member carries the interface's visibility.
                    f.is_pub = i.is_pub;
                    out.fns.push(f);
                }
            }
            _ => {}
        }
    }
}

fn owner_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { path, .. } => path.last().cloned().unwrap_or_default(),
        other => rask_syntax(other),
    }
}

fn surface_fn(owner: &str, via: Option<String>, f: &FnDecl, line_of: &dyn Fn(usize) -> usize) -> SurfaceFn {
    let mut self_mode = None;
    let mut params = Vec::new();
    for p in &f.params {
        let mode = ParamMode::from_flags(p.is_take, p.is_mutate || p.is_deleting);
        if p.name == "self" {
            self_mode = Some(mode);
        } else {
            // Every stdlib and surface parameter is annotated; an inferred one
            // (GC1) can't match anything, and reads as `_` in a message.
            let ty = p.ty.as_ref().map(normalize).unwrap_or_else(|| named("_"));
            params.push((mode, ty));
        }
    }
    SurfaceFn {
        owner: owner.to_string(),
        name: f.name.clone(),
        via,
        self_mode,
        params,
        ret: f.ret_ty.as_ref().map(normalize).unwrap_or(TypeExpr::Unit),
        is_pub: f.is_pub,
        unimplemented: f.attrs.iter().any(|a| a.is("unimplemented")),
        required: false,
        line: line_of(f.span.start),
    }
}

fn named(name: &str) -> TypeExpr {
    TypeExpr::Named { path: vec![name.to_string()], args: Vec::new() }
}

/// One spelling per type, so two declarations compare with `==`. The long
/// forms the stdlib still writes fold into the short ones (`Option<T>` is
/// `T?`, `Result<T, E>` is `T or E`), and a dotted path keeps its last
/// segment, since a spec says `Duration` where the stdlib may say
/// `time.Duration`.
pub fn normalize(ty: &TypeExpr) -> TypeExpr {
    let boxed = |t: &TypeExpr| Box::new(normalize(t));
    match ty {
        TypeExpr::Named { path, args } => {
            let name = path.last().cloned().unwrap_or_default();
            match (name.as_str(), args.as_slice()) {
                ("Option", [inner]) => TypeExpr::Optional(boxed(inner)),
                ("Result", [ok, err]) => TypeExpr::Result { ok: boxed(ok), err: boxed(err) },
                _ => TypeExpr::Named { path: vec![name], args: args.iter().map(normalize).collect() },
            }
        }
        TypeExpr::Optional(inner) => TypeExpr::Optional(boxed(inner)),
        TypeExpr::Result { ok, err } => TypeExpr::Result { ok: boxed(ok), err: boxed(err) },
        TypeExpr::Union(ts) => TypeExpr::Union(ts.iter().map(normalize).collect()),
        TypeExpr::Tuple(ts) => TypeExpr::Tuple(ts.iter().map(normalize).collect()),
        TypeExpr::Array { elem, len } => TypeExpr::Array { elem: boxed(elem), len: len.clone() },
        TypeExpr::FixedCount { count, elem } => TypeExpr::FixedCount { count: count.clone(), elem: boxed(elem) },
        TypeExpr::Func { params, ret } => TypeExpr::Func {
            params: params.iter().map(|p| rask_ast::ty::FuncParam { mode: p.mode, ty: normalize(&p.ty) }).collect(),
            ret: boxed(ret),
        },
        TypeExpr::RawPtr(inner) => TypeExpr::RawPtr(boxed(inner)),
        TypeExpr::Any(inner) => TypeExpr::Any(boxed(inner)),
        TypeExpr::Int(_) | TypeExpr::Unit | TypeExpr::NoneType => ty.clone(),
    }
}

/// A type as a Rask program spells it, for messages. `Display` on `TypeExpr`
/// prints `Result<T, E>` and `()`, which no program writes.
pub fn rask_syntax(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named { path, args } if args.is_empty() => path.join("."),
        TypeExpr::Named { path, args } => {
            format!("{}<{}>", path.join("."), args.iter().map(rask_syntax).collect::<Vec<_>>().join(", "))
        }
        TypeExpr::Int(n) => n.clone(),
        TypeExpr::Optional(inner) => format!("{}?", rask_syntax(inner)),
        TypeExpr::Result { ok, err } => format!("{} or {}", rask_syntax(ok), rask_syntax(err)),
        TypeExpr::Union(ts) => ts.iter().map(rask_syntax).collect::<Vec<_>>().join(" | "),
        TypeExpr::Tuple(ts) => format!("({})", ts.iter().map(rask_syntax).collect::<Vec<_>>().join(", ")),
        TypeExpr::Unit => "void".to_string(),
        TypeExpr::NoneType => "none".to_string(),
        TypeExpr::Array { elem, len } => format!("[{}; {}]", rask_syntax(elem), len),
        TypeExpr::FixedCount { count, elem } => format!("[{}]{}", count, rask_syntax(elem)),
        TypeExpr::Func { params, ret } => {
            let ps: Vec<String> = params.iter().map(|p| with_mode(p.mode, &rask_syntax(&p.ty))).collect();
            format!("func({}) -> {}", ps.join(", "), rask_syntax(ret))
        }
        TypeExpr::RawPtr(inner) => format!("*{}", rask_syntax(inner)),
        TypeExpr::Any(inner) => format!("any {}", rask_syntax(inner)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_surface_blocks_and_folds_long_forms() {
        let md = "<!-- id: std.demo -->\n\
                  # Demo\n\
                  <!-- surface -->\n\
                  ```rask\n\
                  extend Vec<T> {\n    func capacity(self) -> Option<usize>\n    func push(mutate self, take value: T)\n}\n\
                  enum E {\n    A\n    B(x: i64)\n}\n\
                  ```\n\
                  ```rask\nextend Ignored { func nope() }\n```\n";
        let s = spec_surface(Path::new("demo.md"), md);
        assert_eq!(s.id, "std.demo");
        assert_eq!(s.blocks, 1);
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        let names: Vec<String> = s.surface.fns.iter().map(|f| f.qualified()).collect();
        assert_eq!(names, ["Vec.capacity", "Vec.push"]);
        assert_eq!(s.surface.fns[0].signature(), "func capacity(self) -> usize?");
        assert_eq!(s.surface.fns[1].signature(), "func push(mutate self, take T)");
        assert_eq!(s.surface.fns[1].line, 7);
        assert_eq!(s.surface.types[0].variants, ["A", "B"]);
    }

    #[test]
    fn a_conformance_is_keyed_by_its_right_operand() {
        let bare = source_surface("Duration implements Add {\n    func add(self, other: Duration) -> Duration\n}").unwrap();
        let applied = source_surface("Duration implements Add<Duration> {\n    func add(self, d: time.Duration) -> Duration\n}").unwrap();
        let other = source_surface("Instant implements Sub<Instant> {\n    func sub(self, e: Instant) -> Duration\n}\nInstant implements Sub<Duration> {\n    func sub(self, d: Duration) -> Instant\n}").unwrap();
        assert_eq!(bare.fns[0].key(), applied.fns[0].key());
        assert_ne!(other.fns[0].key(), other.fns[1].key());
        assert_eq!(other.fns[1].qualified(), "Instant.sub [Sub<Duration>]");
    }
}

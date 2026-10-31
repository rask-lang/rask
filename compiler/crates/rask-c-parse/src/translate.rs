// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A parsed header as Rask source: the text `rask bindc` writes and
//! `import c` compiles (struct.c-interop/BI1).
//!
//! The translator owns the C-to-Rask spelling. Everything after it is the
//! ordinary Rask pipeline, which is why a header gets whatever Rask
//! declarations get: constants with values, structs inside structs laid out as
//! structs, enumerators usable in expressions.

use std::collections::{HashMap, HashSet};

use crate::consteval::{self, CValue};
use crate::*;

/// What to translate, and how to name what it can't.
pub struct Options<'a> {
    /// The header as the program wrote it, for `@inline_from`.
    pub header: &'a str,
    /// Names `hiding { … }` leaves out (CI5).
    pub hiding: &'a [String],
    /// Whether a file the preprocessor named is the library's own. Its
    /// declarations are translated; another file's only when one of those
    /// needs it.
    pub own: &'a dyn Fn(&str) -> bool,
}

/// The translation of one header.
#[derive(Debug, Clone, Default)]
pub struct Translation {
    pub source: String,
    /// C names left out, and why, for the error a use of one gets.
    pub skipped: HashMap<String, String>,
    pub counts: Counts,
}

#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub functions: usize,
    pub types: usize,
    pub constants: usize,
    pub skipped: usize,
}

/// Rask keywords, which a C name may collide with.
const KEYWORDS: &[&str] = &[
    "func", "let", "mut", "const", "struct", "enum", "interface", "implements", "extend",
    "public", "import", "return", "if", "else", "match", "for", "in", "while", "loop", "break",
    "continue", "select_priority", "select", "with", "ensure", "take", "mutate", "where", "as",
    "is", "true", "false", "unsafe", "extern", "comptime", "type", "none", "nullptr", "using",
    "export", "lazy", "asm", "native", "test", "benchmark", "assert", "check", "try", "catch",
    "or", "private", "union", "discard", "void", "self", "Self", "spawn", "own",
];

/// Names a callback's context parameter goes by (CB4).
const CONTEXT_NAMES: &[&str] = &["userdata", "user_data", "ctx", "context", "arg", "data", "opaque", "udata", "user"];

pub fn translate(parsed: &CParseResult, opts: &Options) -> Translation {
    let mut t = Translator::new(parsed, opts);
    t.select();
    t.emit();
    Translation { source: t.out, skipped: t.skipped, counts: t.counts }
}

/// A C type name as a Rask type name: Rask types start with a capital
/// (type.structs/S1a), so `sqlite3` is `Sqlite3` and `__u_char` is `U_char`.
pub fn type_name(c: &str) -> String {
    // `__u_char` has no first letter to raise; the underscores go.
    let trimmed = c.trim_start_matches('_');
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => first.to_uppercase().chain(chars).collect(),
        _ => format!("C{}", trimmed),
    }
}

/// A C field or parameter name that's safe as a Rask one.
fn value_name(c: &str) -> String {
    if KEYWORDS.contains(&c) { format!("{}_", c) } else { c.to_string() }
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Translator<'a> {
    items: &'a [CItem],
    opts: &'a Options<'a>,
    consts: Constants,
    /// Every function the header declares, so a macro of the same name
    /// doesn't shadow it.
    function_names: HashSet<String>,
    /// Where each struct or union tag is defined, else forward-declared.
    records: HashMap<&'a str, usize>,
    typedefs: HashMap<&'a str, usize>,
    enums: HashMap<&'a str, usize>,
    /// Items to emit, and type names needed only behind a pointer.
    chosen: HashSet<usize>,
    by_value: HashSet<String>,
    opaque: Vec<String>,
    out: String,
    skipped: HashMap<String, String>,
    counts: Counts,
}

impl<'a> Translator<'a> {
    fn new(parsed: &'a CParseResult, opts: &'a Options<'a>) -> Self {
        let items = parsed.decls.as_slice();
        let mut records = HashMap::new();
        let mut typedefs = HashMap::new();
        let mut enums = HashMap::new();
        for (i, item) in items.iter().enumerate() {
            match &item.decl {
                CDecl::Struct(s) | CDecl::Union(s) => {
                    if let Some(tag) = &s.tag {
                        let have_def = records.get(tag.as_str()).is_some_and(|&j: &usize| !is_forward(&items[j].decl));
                        if !have_def {
                            records.insert(tag.as_str(), i);
                        }
                    }
                }
                CDecl::Typedef(td) => {
                    typedefs.entry(td.name.as_str()).or_insert(i);
                }
                CDecl::Enum(e) if !e.is_forward => {
                    if let Some(tag) = &e.tag {
                        enums.entry(tag.as_str()).or_insert(i);
                    }
                }
                _ => {}
            }
        }
        Translator {
            items,
            opts,
            consts: Constants::from_items(items),
            function_names: items
                .iter()
                .filter_map(|i| match &i.decl {
                    CDecl::Function(f) => Some(f.name.clone()),
                    _ => None,
                })
                .collect(),
            records,
            typedefs,
            enums,
            chosen: HashSet::new(),
            by_value: HashSet::new(),
            opaque: Vec::new(),
            out: String::new(),
            skipped: HashMap::new(),
            counts: Counts::default(),
        }
    }

    // -- Selection -----------------------------------------------------------

    /// The library's own declarations, then what they need from elsewhere:
    /// in full where it's used by value, as an opaque type where it's only
    /// pointed at.
    fn select(&mut self) {
        let mut work: Vec<(CType, bool)> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            if !(self.opts.own)(&item.file) {
                continue;
            }
            if let Some(n) = decl_name(&item.decl) {
                if self.opts.hiding.iter().any(|h| h == n) {
                    continue;
                }
            }
            match &item.decl {
                CDecl::Struct(s) | CDecl::Union(s) if is_forward(&item.decl) => {
                    // Defined somewhere? That definition is what to emit.
                    if let Some(tag) = &s.tag {
                        work.push((CType::StructTag(tag.clone()), false));
                    }
                    continue;
                }
                _ => {}
            }
            self.chosen.insert(i);
            uses(&item.decl, &mut work);
        }
        let mut seen: HashSet<(String, bool)> = HashSet::new();
        while let Some((ty, by_value)) = work.pop() {
            let Some(name) = named(&ty) else { continue };
            if !seen.insert((name.to_string(), by_value)) {
                continue;
            }
            let name = name.to_string();
            if by_value {
                self.by_value.insert(name.clone());
            }
            let idx = match &ty {
                CType::Named(n) => self.typedefs.get(n.as_str()).or_else(|| self.records.get(n.as_str())).copied(),
                CType::StructTag(t) | CType::UnionTag(t) => self.records.get(t.as_str()).copied(),
                CType::EnumTag(t) => self.enums.get(t.as_str()).copied(),
                _ => None,
            };
            match idx {
                Some(i) => {
                    let own = (self.opts.own)(&self.items[i].file);
                    let record = matches!(self.items[i].decl, CDecl::Struct(_) | CDecl::Union(_));
                    // A foreign record reached only through pointers stays opaque.
                    if record && !own && !by_value && !self.chosen.contains(&i) {
                        if !self.opaque.contains(&name) {
                            self.opaque.push(name);
                        }
                        continue;
                    }
                    if self.chosen.insert(i) || by_value {
                        let mut more = Vec::new();
                        uses(&self.items[i].decl, &mut more);
                        // A typedef passes on how it's used.
                        if let CDecl::Typedef(_) = &self.items[i].decl {
                            for m in &mut more {
                                m.1 = m.1 && by_value;
                            }
                        }
                        work.extend(more);
                    }
                }
                None => {
                    if !self.opaque.contains(&name) {
                        self.opaque.push(name);
                    }
                }
            }
        }
        // An opaque name that turned out to be defined and chosen isn't opaque.
        let defined: HashSet<String> = self
            .chosen
            .iter()
            .filter_map(|&i| match &self.items[i].decl {
                CDecl::Struct(s) | CDecl::Union(s) if !s.is_forward => s.tag.as_deref().map(type_name),
                CDecl::Typedef(td) if !self.self_typedef(td) => Some(type_name(&td.name)),
                CDecl::Enum(e) => e.tag.as_deref().map(type_name),
                _ => None,
            })
            .collect();
        self.opaque.retain(|n| !defined.contains(&type_name(n)));
    }

    // -- Emission ------------------------------------------------------------

    fn emit(&mut self) {
        self.out.push_str(&format!(
            "// Generated by `rask bindc {}`. Regenerate it; don't edit it.\n",
            self.opts.header
        ));
        let mut emitted: HashSet<String> = HashSet::new();
        let mut externs: Vec<String> = Vec::new();
        for i in 0..self.items.len() {
            if !self.chosen.contains(&i) {
                continue;
            }
            let item = &self.items[i];
            let mut text = String::new();
            match &item.decl {
                CDecl::Function(f) => {
                    match self.function(f) {
                        Ok((attrs, sig)) if attrs.is_empty() => externs.push(sig),
                        Ok((attrs, sig)) => {
                            text.push_str(&format!("{}extern \"C\" {}\n", attrs, sig));
                            self.counts.functions += 1;
                        }
                        Err(why) => text.push_str(&self.skip(&f.name, &why)),
                    }
                }
                CDecl::Struct(s) | CDecl::Union(s) => {
                    let Some(tag) = &s.tag else { continue };
                    let rname = type_name(tag);
                    if !emitted.insert(rname.clone()) {
                        continue;
                    }
                    text.push_str(&self.record(s, matches!(item.decl, CDecl::Union(_))));
                    self.counts.types += 1;
                }
                CDecl::Enum(e) => text.push_str(&self.enumeration(e, &mut emitted)),
                CDecl::Typedef(td) => {
                    let rname = type_name(&td.name);
                    if self.self_typedef(td) || !emitted.insert(rname.clone()) {
                        continue;
                    }
                    text.push_str(&format!("public type alias {} = {}\n", rname, self.ty(&td.target)));
                    self.counts.types += 1;
                }
                CDecl::Define(d) => text.push_str(&self.define(d, &mut emitted)),
                CDecl::Variable(v) => {
                    text.push_str(&self.skip(&v.name, "a C global variable; read it through a function the library provides"));
                }
            }
            if !text.is_empty() {
                self.out.push('\n');
                self.out.push_str(&text);
            }
        }
        if !self.opaque.is_empty() {
            self.out.push('\n');
            let mut names = self.opaque.clone();
            names.sort();
            for n in names {
                let rname = type_name(&n);
                if emitted.insert(rname.clone()) {
                    self.out.push_str(&format!("extern \"C\" struct {}\n", rname));
                    self.counts.types += 1;
                }
            }
        }
        if !externs.is_empty() {
            self.out.push_str("\nextern \"C\" {\n");
            for e in &externs {
                self.out.push_str("    ");
                self.out.push_str(e);
                self.out.push('\n');
            }
            self.out.push_str("}\n");
            self.counts.functions += externs.len();
        }
    }

    /// The comment a skipped declaration leaves (BI5), and the record of why.
    /// A function-like macro, callable from Rask (PP4). Its body is expanded
    /// and written out as self-contained C: other macros expanded, constants
    /// in as typed values, casts to plain C types. Its parameter and result
    /// types come from each call. One that isn't an arithmetic expression
    /// still gets a declaration, carrying the reason, so a call says why
    /// rather than "no such name".
    fn function_macro(&mut self, d: &CDefine, params: &[String], emitted: &mut HashSet<String>) -> String {
        // A function of the same name is the real thing; the macro is often
        // only a shortcut to it.
        if emitted.contains(&d.name) || self.function_names.contains(&d.name) {
            return String::new();
        }
        emitted.insert(d.name.clone());
        let variadic = params.iter().any(|p| p == "...");
        let rparams: Vec<String> = params.iter().filter(|p| *p != "...").map(|p| value_name(p)).collect();
        let parsed = if variadic {
            Err("takes a variable number of arguments".to_string())
        } else {
            crate::cexpr::parse(&d.body, params, &crate::consteval::AsNames(&self.consts))
        };
        let attr = match parsed {
            Ok(e) => format!("@c_macro({})", quote(&crate::cexpr::to_c(&e, &rparams))),
            Err(why) => format!("@c_macro(unusable: {})", quote(&why)),
        };
        let ps: Vec<String> = rparams.iter().map(|p| format!("{}: _", p)).collect();
        self.counts.functions += 1;
        format!("// {}
{}
extern \"C\" func {}({}) -> _\n", d.text.trim(), attr, d.name, ps.join(", "))
    }

    fn skip(&mut self, name: &str, why: &str) -> String {
        self.skipped.insert(name.to_string(), why.to_string());
        self.counts.skipped += 1;
        format!("// skipped `{}`: {}\n", name, why)
    }

    fn self_typedef(&self, td: &CTypedef) -> bool {
        match &td.target {
            CType::StructTag(t) | CType::UnionTag(t) | CType::EnumTag(t) | CType::Named(t) => {
                type_name(t) == type_name(&td.name)
            }
            _ => false,
        }
    }

    fn function(&self, f: &CFuncDecl) -> Result<(String, String), String> {
        if KEYWORDS.contains(&f.name.as_str()) {
            return Err(format!("its name is a Rask keyword"));
        }
        let params: Vec<&CParam> = f
            .params
            .iter()
            .filter(|p| !(f.params.len() == 1 && p.name.is_none() && p.ty == CType::Void))
            .collect();
        let mut names: Vec<String> = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let mut n = p.name.as_deref().map(value_name).unwrap_or_else(|| format!("p{}", i));
            if names.contains(&n) {
                n = format!("{}{}", n, i);
            }
            names.push(n);
        }
        let mut sig = format!("func {}(", f.name);
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                sig.push_str(", ");
            }
            sig.push_str(&format!("{}: {}", names[i], self.ty(&p.ty)));
        }
        if f.is_variadic {
            sig.push_str(if params.is_empty() { "..." } else { ", ..." });
        }
        sig.push(')');
        if strip(&f.ret_ty) != &CType::Void {
            sig.push_str(&format!(" -> {}", self.ty(&f.ret_ty)));
        }
        let mut attrs = String::new();
        if f.is_inline && f.is_static {
            attrs.push_str(&format!("@inline_from({})\n", quote(self.opts.header)));
        }
        let resolve = |t: &CType| self.resolve(t);
        for (cb, ctx, at) in contexts(&params, &resolve) {
            attrs.push_str(&format!("@context({}, {}, at: {})\n", names[cb], names[ctx], at));
        }
        Ok((attrs, sig))
    }

    /// A type with typedefs looked through.
    fn resolve(&self, ty: &CType) -> CType {
        let mut ty = strip(ty).clone();
        for _ in 0..16 {
            let CType::Named(n) = &ty else { break };
            let Some(&i) = self.typedefs.get(n.as_str()) else { break };
            let CDecl::Typedef(td) = &self.items[i].decl else { break };
            ty = strip(&td.target).clone();
        }
        ty
    }

    fn record(&self, s: &CStructDecl, is_union: bool) -> String {
        let name = type_name(s.tag.as_deref().unwrap_or_default());
        if s.is_forward {
            return format!("extern \"C\" struct {}\n", name);
        }
        let mut out = if is_union {
            format!("public union {} {{\n", name)
        } else {
            format!("@layout(C)\npublic struct {} {{\n", name)
        };
        let mut used: Vec<String> = Vec::new();
        for f in &s.fields {
            let mut n = value_name(&f.name);
            if used.contains(&n) {
                n = format!("{}{}", n, used.len());
            }
            used.push(n.clone());
            out.push_str("    ");
            if let Some(w) = f.bit_width {
                out.push_str(&format!("@bits({}) ", w));
            }
            out.push_str(&format!("public {}: {}\n", n, self.ty(&f.ty)));
        }
        out.push_str("}\n");
        out
    }

    /// A C enum is its integer type, and its enumerators are constants (TM6).
    /// The type is the one GCC and Clang give it: `unsigned int` with no
    /// negative enumerator, else `int`, wider when a value needs it. Typed
    /// `int` regardless, `ev.type == SDL_EVENT_QUIT` compared a `Uint32`
    /// field with a `c_int` and needed a conversion C never asks for.
    fn enumeration(&mut self, e: &CEnumDecl, emitted: &mut HashSet<String>) -> String {
        let known: Vec<i64> = e.variants.iter().filter_map(|v| v.value).collect();
        let base = if known.iter().all(|&v| u32::try_from(v).is_ok()) {
            "c_uint"
        } else if known.iter().all(|&v| i32::try_from(v).is_ok()) {
            "c_int"
        } else if known.iter().all(|&v| v >= 0) {
            "c_ulong"
        } else {
            "c_long"
        };
        let mut out = String::new();
        let ty = match &e.tag {
            Some(tag) => {
                let rname = type_name(tag);
                if emitted.insert(rname.clone()) {
                    out.push_str(&format!("public type alias {} = {}\n", rname, base));
                    self.counts.types += 1;
                }
                rname
            }
            // No type to be: the enumerators are C's `int` constants.
            None if known.iter().all(|&v| i32::try_from(v).is_ok()) => "c_int".to_string(),
            None => base.to_string(),
        };
        for v in &e.variants {
            if !emitted.insert(v.name.clone()) {
                continue;
            }
            match v.value {
                Some(x) => {
                    out.push_str(&format!("public const {}: {} = {}\n", v.name, ty, x));
                    self.counts.constants += 1;
                }
                None => out.push_str(&self.skip(&v.name, "an enumerator whose value isn't a constant expression")),
            }
        }
        out
    }

    fn define(&mut self, d: &CDefine, emitted: &mut HashSet<String>) -> String {
        if let Some(params) = &d.params {
            return self.function_macro(d, params, emitted);
        }
        if emitted.contains(&d.name) {
            return String::new();
        }
        let line = match consteval::eval(&d.body, &self.consts) {
            Some(CValue::Int(v, ty)) => {
                let rty = self.ty(&ty);
                format!("public const {}: {} = {}\n", d.name, rty, v)
            }
            Some(CValue::Float(f, t)) => {
                if crate::cexpr::canonical(&t) == CType::Float {
                    format!("public const {}: f32 = {:?}\n", d.name, f as f32)
                } else {
                    format!("public const {}: f64 = {:?}\n", d.name, f)
                }
            }
            Some(CValue::Str(s)) => format!("public const {}: string = {}\n", d.name, quote(&s)),
            // `#define SDL_mutex SDL_Mutex`: another name for something. A
            // library's old spellings come in hundreds and say nothing, so
            // they're kept out of the file; a use still says what it was.
            None if matches!(d.body.as_slice(), [CTokenKind::Ident(_)]) => {
                self.skipped.insert(d.name.clone(), format!("another name for `{}`", d.text.split_whitespace().last().unwrap_or("")));
                return String::new();
            }
            None => return self.skip(&d.name, &format!("not a constant: {}", d.text)),
        };
        emitted.insert(d.name.clone());
        self.counts.constants += 1;
        line
    }

    // -- Types ---------------------------------------------------------------

    fn ty(&self, ty: &CType) -> String {
        match ty {
            CType::Void => "void".into(),
            CType::Char => "c_char".into(),
            CType::SignedChar => "i8".into(),
            CType::UnsignedChar => "u8".into(),
            CType::Short => "c_short".into(),
            CType::UnsignedShort => "c_ushort".into(),
            CType::Int => "c_int".into(),
            CType::UnsignedInt => "c_uint".into(),
            CType::Long => "c_long".into(),
            CType::UnsignedLong => "c_ulong".into(),
            CType::LongLong => "c_longlong".into(),
            CType::UnsignedLongLong => "c_ulonglong".into(),
            CType::Float => "f32".into(),
            CType::Double => "f64".into(),
            CType::Bool => "bool".into(),
            CType::SizeT => "c_size".into(),
            CType::SSizeT => "c_ssize".into(),
            CType::FixedInt { bits, signed } => format!("{}{}", if *signed { 'i' } else { 'u' }, bits),
            CType::IntPtr { signed } => (if *signed { "isize" } else { "usize" }).into(),
            CType::Const(inner) => self.ty(inner),
            // A header doesn't say which of its pointers may be NULL, and a
            // `*T` never is (mem.unsafe/PT6), so a data pointer is `*T?`. A
            // `*T` argument still fills one; a returned one is checked before
            // it's used. An override declaration (BI3) narrows one that the
            // library's docs promise is never NULL.
            CType::Pointer(inner) => match strip(inner) {
                // A C string, by convention.
                CType::Char | CType::SignedChar | CType::UnsignedChar => "*u8?".into(),
                CType::FuncPtr { .. } => self.ty(strip(inner)),
                other => optional_pointer_to(self.ty(other)),
            },
            CType::Array(elem, Some(n)) => format!("[{}; {}]", self.ty(elem), n),
            CType::Array(elem, None) => optional_pointer_to(self.ty(elem)),
            CType::Named(n) => match builtin_typedef(n) {
                Some(t) => self.ty(&t),
                None => type_name(n),
            },
            CType::StructTag(t) | CType::UnionTag(t) => type_name(t),
            CType::EnumTag(t) => match self.enums.get(t.as_str()) {
                Some(_) => type_name(t),
                None => "c_int".into(),
            },
            // Rask function types have no variadic form.
            CType::FuncPtr { is_variadic: true, .. } => "*void".into(),
            CType::FuncPtr { ret, params, .. } => {
                let ps: Vec<String> = params
                    .iter()
                    .filter(|p| !(params.len() == 1 && **p == CType::Void))
                    .map(|p| self.ty(p))
                    .collect();
                let mut s = format!("*func({})", ps.join(", "));
                if strip(ret) != &CType::Void {
                    s.push_str(&format!(" -> {}", self.ty(ret)));
                }
                s
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip(ty: &CType) -> &CType {
    match ty {
        CType::Const(inner) => strip(inner),
        other => other,
    }
}

fn is_forward(d: &CDecl) -> bool {
    matches!(d, CDecl::Struct(s) | CDecl::Union(s) if s.is_forward)
}

fn is_void_ptr(ty: &CType) -> bool {
    matches!(strip(ty), CType::Pointer(inner) if strip(inner) == &CType::Void)
}

/// The callback/context pairs in a parameter list (CB4): `(callback index,
/// context index, position of the context among the callback's arguments)`.
fn contexts(params: &[&CParam], resolve: &dyn Fn(&CType) -> CType) -> Vec<(usize, usize, usize)> {
    let ctx_params: Vec<usize> = (0..params.len()).filter(|&i| is_void_ptr(&params[i].ty)).collect();
    let mut out = Vec::new();
    for (cb, p) in params.iter().enumerate() {
        let resolved = resolve(&p.ty);
        let fp = match &resolved {
            CType::FuncPtr { params, is_variadic: false, .. } => params,
            CType::Pointer(inner) => match strip(inner) {
                CType::FuncPtr { params, is_variadic: false, .. } => params,
                _ => continue,
            },
            _ => continue,
        };
        let slots: Vec<usize> = (0..fp.len()).filter(|&i| is_void_ptr(&fp[i])).collect();
        if slots.len() != 1 {
            continue;
        }
        let ctx = match ctx_params.as_slice() {
            [one] => Some(*one),
            many => {
                let named: Vec<usize> = many
                    .iter()
                    .copied()
                    .filter(|&i| params[i].name.as_deref().is_some_and(|n| CONTEXT_NAMES.contains(&n.to_ascii_lowercase().as_str())))
                    .collect();
                if named.len() == 1 { Some(named[0]) } else { None }
            }
        };
        if let Some(ctx) = ctx {
            if !out.iter().any(|&(_, c, _)| c == ctx) {
                out.push((cb, ctx, slots[0]));
            }
        }
    }
    out
}

/// The type a type expression names, if it names one.
fn named(ty: &CType) -> Option<&str> {
    match ty {
        CType::Named(n) if builtin_typedef(n).is_none() => Some(n),
        CType::StructTag(t) | CType::UnionTag(t) | CType::EnumTag(t) => Some(t),
        _ => None,
    }
}

/// The types a declaration mentions, and whether by value.
fn uses(decl: &CDecl, out: &mut Vec<(CType, bool)>) {
    fn walk(ty: &CType, by_value: bool, out: &mut Vec<(CType, bool)>) {
        match ty {
            CType::Const(inner) => walk(inner, by_value, out),
            CType::Pointer(inner) => walk(inner, false, out),
            CType::Array(elem, Some(_)) => walk(elem, by_value, out),
            CType::Array(elem, None) => walk(elem, false, out),
            CType::FuncPtr { ret, params, .. } => {
                walk(ret, false, out);
                for p in params {
                    walk(p, false, out);
                }
            }
            other if named(other).is_some() => out.push((other.clone(), by_value)),
            _ => {}
        }
    }
    match decl {
        // A parameter or return by value needs the layout to pass it.
        CDecl::Function(f) => {
            walk(&f.ret_ty, true, out);
            for p in &f.params {
                walk(&p.ty, true, out);
            }
        }
        CDecl::Struct(s) | CDecl::Union(s) => {
            for f in &s.fields {
                walk(&f.ty, true, out);
            }
        }
        CDecl::Typedef(td) => walk(&td.target, true, out),
        CDecl::Enum(_) | CDecl::Define(_) | CDecl::Variable(_) => {}
    }
}

/// The name a declaration introduces, for `hiding`.
fn decl_name(decl: &CDecl) -> Option<&str> {
    match decl {
        CDecl::Function(f) => Some(&f.name),
        CDecl::Struct(s) | CDecl::Union(s) => s.tag.as_deref(),
        CDecl::Enum(e) => e.tag.as_deref(),
        CDecl::Typedef(td) => Some(&td.name),
        CDecl::Define(d) => Some(&d.name),
        CDecl::Variable(v) => Some(&v.name),
    }
}

#[cfg(test)]
pub(crate) mod tests;

/// `*T?` for a pointee spelled `pointee`. A `?` after a pointer type is the
/// pointer's own, so a pointee that is itself optional takes parentheses:
/// C's `void **` is `*(*void?)?`, where `**void??` doesn't parse.
fn optional_pointer_to(pointee: String) -> String {
    if pointee.ends_with('?') {
        format!("*({})?", pointee)
    } else {
        format!("*{}?", pointee)
    }
}

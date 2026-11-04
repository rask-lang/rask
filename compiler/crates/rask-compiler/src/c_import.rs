// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `import c "header.h"`: the header's translation, compiled as a package.
//!
//! The translator writes Rask source, the same text `rask bindc` would write
//! (struct.c-interop/BI1). That text is parsed like any file and merged like a
//! dependency: `Point` from a header imported as `c` becomes `Point_c`, and
//! the program's `c.Point` is rewritten to it. From there a header's
//! declarations are ordinary declarations, with nothing downstream that knows
//! they came from C.
//!
//! An extern keeps its name. It's a symbol the linker resolves, so it is
//! global whatever the package, and `c.f(x)` is rewritten to `f(x)`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::rewrite::{self, Rewrite};
use rask_ast::Span;
use rask_diagnostics::{Diagnostic, ToDiagnostic};

use crate::package_scope;

/// What expanding a program's C imports produced.
#[derive(Default)]
pub struct Expanded {
    /// Per import, as a diagnostic spells it (`c`, or `pa.c` for a
    /// sub-package's): each C name and the name the compiler uses for it.
    pub exports: HashMap<String, HashMap<String, String>>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Where the headers live and who reads them.
pub struct Context<'a> {
    /// The directory of each file, by file id.
    pub source_dirs: &'a HashMap<u16, PathBuf>,
    pub arch: &'a str,
    pub os: &'a str,
}

/// One package's declarations. An alias means the header that package
/// imported, so each package's imports are expanded against its own.
pub struct Unit<'a> {
    pub decls: &'a mut Vec<Decl>,
    /// `None` for the root package, or a single file.
    pub package: Option<&'a str>,
}

/// Translate every `import c` in each unit, append the declarations to it, and
/// point its references at them.
///
/// The generated text is added to `source_files` so a diagnostic inside it
/// renders; `next_id` keeps node ids unique across it.
pub fn expand(
    units: &mut [Unit],
    source_files: &mut Vec<(PathBuf, String)>,
    next_id: &mut u32,
    cx: &Context,
) -> Expanded {
    let mut out = Expanded::default();
    for unit in units.iter_mut() {
        out.diagnostics.extend(resolve_macro_headers(unit.decls, cx));
    }

    // An extern the program declares itself replaces the header's (CI4), and
    // one header imported twice, by one package or by two, declares each
    // function once. The stdlib declares a few C functions for its own use
    // (`strlen`, `fread`); its declaration stands too.
    let stdlib = rask_stdlib::StubRegistry::compilable_decls();
    let mut externs: HashSet<String> = units
        .iter()
        .flat_map(|u| u.decls.iter())
        .chain(stdlib.iter())
        .filter_map(|d| match &d.kind {
            // A macro is no symbol, and only its own package's.
            DeclKind::Extern(e) if !is_macro(e) => Some(e.name.clone()),
            _ => None,
        })
        .collect();

    for unit in units.iter_mut() {
        let imports: Vec<(rask_ast::decl::CImportDecl, Span)> = unit
            .decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::CImport(c) => Some((c.clone(), d.span)),
                _ => None,
            })
            .collect();
        if imports.is_empty() {
            continue;
        }
        let mut exports: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut done: HashSet<(PathBuf, String)> = HashSet::new();
        let mut skipped: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut generated: Vec<Decl> = Vec::new();

        for (import, span) in &imports {
            // A sub-package's `c` isn't the root's: its names carry the
            // package too, so two packages' `c.Point` are two types.
            let (qualifier, macro_prefix) = match unit.package {
                None => (import.alias.clone(), import.alias.clone()),
                Some(pkg) => (
                    format!("{}_{}", import.alias, pkg.replace('.', "_")),
                    format!("{}.{}", pkg, import.alias),
                ),
            };
            for header in &import.headers {
                let found = match locate(header, span.file_id, cx) {
                    Ok(p) => p,
                    Err(msg) => {
                        out.diagnostics.push(
                            Diagnostic::error(format!("C header not found: `{}`", header))
                                .with_primary(*span, "imported here")
                                .with_note(msg),
                        );
                        continue;
                    }
                };
                if !done.insert((found.clone(), import.alias.clone())) {
                    continue;
                }
                let found = canonical(&found);
                let text = match translate(&found.display().to_string(), &found, &import.hiding, cx) {
                    Ok(t) => t,
                    Err(msg) => {
                        out.diagnostics.push(
                            Diagnostic::error(format!("couldn't read C header `{}`", header))
                                .with_primary(*span, "imported here")
                                .with_note(msg),
                        );
                        continue;
                    }
                };
                skipped.entry(import.alias.clone()).or_default().extend(text.skipped);

                let file_id = source_files.len() as u16;
                source_files.push((PathBuf::from(format!("{} (as Rask)", found.display())), text.source.clone()));
                let mut lexer = rask_lexer::Lexer::new_with_file_id(&text.source, file_id);
                let lexed = lexer.tokenize();
                let mut parser = rask_parser::Parser::new_with_file_id(lexed.tokens, *next_id, file_id);
                let parsed = parser.parse();
                *next_id = parser.next_node_id();
                if !lexed.errors.is_empty() || !parsed.errors.is_empty() {
                    for e in &lexed.errors {
                        out.diagnostics.push(e.to_diagnostic());
                    }
                    for e in &parsed.errors {
                        out.diagnostics.push(e.to_diagnostic());
                    }
                    out.diagnostics.push(
                        Diagnostic::error(format!("the Rask translation of `{}` doesn't parse", header))
                            .with_primary(*span, "imported here")
                            .with_note("this is a bug in the C translator; `rask bindc` shows the text"),
                    );
                    continue;
                }
                let mut new: Vec<Decl> = parsed
                    .decls
                    .into_iter()
                    .filter(|d| match &d.kind {
                        DeclKind::Extern(e) if !is_macro(e) => externs.insert(e.name.clone()),
                        _ => true,
                    })
                    .collect();
                let mut map = package_scope::qualify_declarations(&mut new, &qualifier);
                for d in &mut new {
                    if let DeclKind::Extern(e) = &mut d.kind {
                        if is_macro(e) {
                            // A macro is no symbol: each header's is its own,
                            // under its import's dotted name, which no Rask
                            // declaration can take. Its C name is kept.
                            let qualified = format!("{}.{}", macro_prefix, e.name);
                            e.attrs.push(c_name_attr(&e.name));
                            map.insert(e.name.clone(), qualified.clone());
                            e.name = qualified;
                        } else {
                            map.insert(e.name.clone(), e.name.clone());
                        }
                    }
                }
                // Externs dropped above as duplicates are still reachable by name.
                for name in &externs {
                    map.entry(name.clone()).or_insert_with(|| name.clone());
                }
                exports.entry(import.alias.clone()).or_default().extend(map);
                generated.extend(new);
            }
        }

        package_scope::qualify_references(unit.decls, &exports, &HashSet::new());
        let mut leftover = Leftover { exports: &exports, found: Vec::new() };
        rewrite::rewrite_decls(unit.decls, &mut leftover);
        for (alias, name, span) in leftover.found {
            let why = skipped.get(&alias).and_then(|s| s.get(&name));
            let d = Diagnostic::error(format!("`{}.{}` isn't in the imported header", alias, name))
                .with_primary(span, "not translated");
            out.diagnostics.push(match why {
                Some(why) => d.with_note(format!("`{}` was skipped: {}", name, why)),
                None => d.with_note(format!("no declaration named `{}`; `rask bindc` shows what the header translates to", name)),
            });
        }
        unit.decls.extend(generated);
        for (alias, map) in exports {
            let spelled = match unit.package {
                None => alias,
                Some(pkg) => format!("{}.{}", pkg, alias),
            };
            out.exports.insert(spelled, map);
        }
    }
    out
}

/// A written `header:` (`@c_macro(…, header: "a.h")`, `@c_name("struct pt",
/// header: "a.h")`) names its header the way `import c` does, beside the file
/// it's written in. Resolved here, once, to the path the C a macro call
/// compiles into includes; a header `import c` translated already names its
/// own path.
fn resolve_macro_headers(decls: &mut [Decl], cx: &Context) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for d in decls.iter_mut() {
        let (attr, what) = match &mut d.kind {
            DeclKind::Extern(e) => {
                let what = format!("the header C macro `{}` is compiled with", e.name);
                (e.attrs.iter_mut().find(|a| a.is("c_macro")), what)
            }
            DeclKind::Struct(s) => {
                let what = format!("the header that declares `{}` for C", s.name);
                (s.attrs.iter_mut().find(|a| a.is("c_name")), what)
            }
            DeclKind::Union(u) => {
                let what = format!("the header that declares `{}` for C", u.name);
                (u.attrs.iter_mut().find(|a| a.is("c_name")), what)
            }
            _ => continue,
        };
        let Some(args) = attr.and_then(|a| a.args.as_mut()) else { continue };
        let Some(arg) = args.iter_mut().find(|a| a.label.as_deref() == Some("header")) else { continue };
        let Some(header) = arg.value.string() else { continue };
        if Path::new(&header).is_absolute() {
            continue;
        }
        match locate(&header, d.span.file_id, cx) {
            Ok(path) => {
                let path = canonical(&path).display().to_string();
                arg.value = rask_ast::attr::AttrValue::Expr(Expr {
                    id: rask_ast::NodeId::DUMMY,
                    kind: ExprKind::String(path),
                    span: d.span,
                });
            }
            Err(msg) => diags.push(
                Diagnostic::error(format!("C header not found: `{}`", header))
                    .with_primary(d.span, what)
                    .with_note(msg),
            ),
        }
    }
    diags
}

/// `@c_name("name")`.
fn c_name_attr(name: &str) -> rask_ast::attr::Attr {
    let spelling = Expr { id: rask_ast::NodeId::DUMMY, kind: ExprKind::String(name.to_string()), span: Span::new(0, 0) };
    let arg = rask_ast::attr::AttrArg { label: None, value: rask_ast::attr::AttrValue::Expr(spelling) };
    rask_ast::attr::Attr::synthetic("c_name", Some(vec![arg]))
}

fn is_macro(e: &rask_ast::decl::ExternDecl) -> bool {
    rask_ast::attr::c_macro(&e.attrs).is_some()
}

/// A reference through a C import's alias to a name it doesn't have.
struct Leftover<'a> {
    exports: &'a HashMap<String, HashMap<String, String>>,
    found: Vec<(String, String, Span)>,
}

impl Rewrite for Leftover<'_> {
    fn expr(&mut self, e: &mut Expr) {
        let (object, name) = match &e.kind {
            ExprKind::MethodCall { object, method, .. } => (object, method),
            ExprKind::Field { object, field } => (object, field),
            _ => return,
        };
        if let ExprKind::Ident(alias) = &object.kind {
            if self.exports.contains_key(alias) {
                self.found.push((alias.clone(), name.clone(), e.span));
            }
        }
    }
}

/// Find a header as C does: beside the importing file, as written, then along
/// the target compiler's search list (struct.c-interop/CI1, XC2).
fn locate(header: &str, file_id: u16, cx: &Context) -> Result<PathBuf, String> {
    if let Some(dir) = cx.source_dirs.get(&file_id) {
        let p = dir.join(header);
        if p.is_file() {
            return Ok(p);
        }
    }
    if Path::new(header).is_file() {
        return Ok(PathBuf::from(header));
    }
    let search = rask_c_parse::toolchain::HeaderSearch::for_target(cx.arch, cx.os);
    for base in &search.dirs {
        let p = base.join(header);
        if p.is_file() {
            return Ok(p);
        }
    }
    let mut looked: Vec<String> = Vec::new();
    if let Some(dir) = cx.source_dirs.get(&file_id) {
        looked.push(dir.display().to_string());
    }
    looked.extend(search.dirs.iter().map(|d| d.display().to_string()));
    let mut msg = format!("looked in: {}", looked.join(", "));
    if let Some(why) = &search.no_system_headers {
        msg.push_str(&format!("\n{}", why));
    }
    Err(msg)
}

/// The header through the target's C preprocessor, translated.
///
/// The preprocessor follows `#include`s and settles `#if`s the way the C
/// compiler will, and `-dD` keeps the `#define`s. Without a compiler for the
/// target the header is parsed as written: its own declarations still
/// translate, and what it includes doesn't.
pub fn translate(
    header: &str,
    path: &Path,
    hiding: &[String],
    cx: &Context,
) -> Result<rask_c_parse::translate::Translation, String> {
    let (parsed, preprocessed) = parse(path, cx)?;
    let own_dir = canonical(&header_dir(path));
    // A header straight in a system include directory (`/usr/include/zlib.h`)
    // shares it with libc's; only the header itself is the library's there,
    // and what it uses from the others comes along as needed.
    let search = rask_c_parse::toolchain::HeaderSearch::for_target(cx.arch, cx.os);
    let in_system_root = search.dirs.iter().any(|d| canonical(d) == own_dir);
    let own_file = canonical(path);
    let mut cache: HashMap<String, bool> = HashMap::new();
    let cache = std::cell::RefCell::new(&mut cache);
    let own = |file: &str| -> bool {
        if !preprocessed || file.is_empty() {
            return true;
        }
        *cache
            .borrow_mut()
            .entry(file.to_string())
            .or_insert_with(|| {
                let f = canonical(Path::new(file));
                if in_system_root { f == own_file } else { f.starts_with(&own_dir) }
            })
    };
    Ok(rask_c_parse::translate::translate(
        &parsed,
        &rask_c_parse::translate::Options { header, hiding, own: &own },
    ))
}

/// A hash of every C header `decls` imports, as the program will see it.
///
/// The build cache keys on this, so editing a header, or anything it
/// includes, rebuilds (#1705). It hashes what the preprocessor produces, which
/// is every included file's text after `#if`s are settled: the input the
/// translation reads. A header that can't be found or preprocessed hashes its
/// error, and the build that follows reports it.
pub fn header_fingerprint(decls: &[Decl], cx: &Context) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for d in decls {
        let DeclKind::CImport(import) = &d.kind else { continue };
        for header in &import.headers {
            header.hash(&mut hasher);
            let text = locate(header, d.span.file_id, cx).and_then(|p| {
                let p = canonical(&p);
                match preprocess(&p, &header_dir(&p), cx) {
                    Some(r) => r,
                    None => std::fs::read_to_string(&p).map_err(|e| e.to_string()),
                }
            });
            match text {
                Ok(t) => t.hash(&mut hasher),
                Err(e) => e.hash(&mut hasher),
            }
        }
    }
    hasher.finish()
}

fn header_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The header parsed, and whether through the preprocessor.
fn parse(path: &Path, cx: &Context) -> Result<(rask_c_parse::CParseResult, bool), String> {
    let dir = header_dir(path);
    let (source, preprocessed) = match preprocess(path, &dir, cx) {
        Some(Ok(text)) => (text, true),
        Some(Err(msg)) => return Err(msg),
        None => (std::fs::read_to_string(path).map_err(|e| e.to_string())?, false),
    };
    let parsed = rask_c_parse::parse_c_header(&source).map_err(|e| e.to_string())?;
    Ok((parsed, preprocessed))
}

/// C source for the wrappers every `@inline_from` extern in `decls` links to
/// (struct.c-interop/IN2), or `None` when there are none.
///
/// A header is found as written, else beside the program's files, else along
/// the target compiler's search list, the way `import c` finds one.
pub fn inline_wrappers(decls: &[Decl], cx: &Context) -> Result<Option<String>, Vec<String>> {
    let mut by_header: Vec<(String, Vec<String>)> = Vec::new();
    for d in decls {
        let DeclKind::Extern(e) = &d.kind else { continue };
        let Some(header) = inline_header(e) else { continue };
        match by_header.iter_mut().find(|(h, _)| *h == header) {
            Some((_, names)) => names.push(e.name.clone()),
            None => by_header.push((header, vec![e.name.clone()])),
        }
    }
    if by_header.is_empty() {
        return Ok(None);
    }
    let mut out = String::new();
    let mut errors = Vec::new();
    for (header, names) in by_header {
        let path = match find_header(&header, cx) {
            Ok(p) => p,
            Err(msg) => {
                errors.push(format!("`@inline_from(\"{}\")`: header not found ({})", header, msg));
                continue;
            }
        };
        let (parsed, _) = match parse(&path, cx) {
            Ok(p) => p,
            Err(msg) => {
                errors.push(format!("`@inline_from(\"{}\")`: {}", header, msg));
                continue;
            }
        };
        let (src, missing) = rask_c_parse::shim::wrapper_source(&path.display().to_string(), &parsed.decls, &names);
        for m in missing {
            errors.push(format!("`{}` is marked `@inline_from(\"{}\")`, and that header has no inline function `{}`", m, header, m));
        }
        out.push_str(&src);
        out.push('\n');
    }
    if errors.is_empty() { Ok(Some(out)) } else { Err(errors) }
}

/// The header an extern's `@inline_from("…")` names.
/// A header a generated declaration names (`@inline_from`, `@c_macro`'s
/// `header:`), found as written, beside the program's files, or along the
/// target compiler's search list, the way `import c` finds one.
pub fn find_header(header: &str, cx: &Context) -> Result<PathBuf, String> {
    let path = if Path::new(header).is_file() {
        PathBuf::from(header)
    } else {
        let found = cx.source_dirs.values().map(|d| d.join(header)).find(|p| p.is_file());
        match found {
            Some(p) => p,
            None => locate(header, u16::MAX, cx)?,
        }
    };
    Ok(canonical(&path))
}

pub fn inline_header(e: &rask_ast::decl::ExternDecl) -> Option<String> {
    e.attrs.iter().find(|a| a.is("inline_from"))?.positional()?.string()
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Every file `path` includes, directly or not, as the target's C
/// preprocessor follows them. `None` when there's no C compiler to ask, or it
/// can't follow them.
pub fn included_files(path: &Path, cx: &Context) -> Option<HashSet<PathBuf>> {
    let cc = rask_c_parse::toolchain::c_compiler(cx.arch, cx.os).ok()?;
    let output = std::process::Command::new(&cc.program)
        .args(&cc.args)
        .args(["-M", "-x", "c"])
        .arg(format!("-I{}", header_dir(path).display()))
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // `x.o: a.h b.h \` and so on: make's rule, a space in a name escaped.
    let text = String::from_utf8_lossy(&output.stdout).replace("\\\n", " ");
    let (_, deps) = text.split_once(": ")?;
    let mut files = HashSet::new();
    let mut current = String::new();
    let mut chars = deps.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' if chars.peek() == Some(&' ') => current.push(chars.next().unwrap()),
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    files.insert(canonical(Path::new(&std::mem::take(&mut current))));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        files.insert(canonical(Path::new(&current)));
    }
    files.remove(&canonical(path));
    Some(files)
}

/// `None` when there's no C compiler to ask.
fn preprocess(path: &Path, dir: &Path, cx: &Context) -> Option<Result<String, String>> {
    let cc = rask_c_parse::toolchain::c_compiler(cx.arch, cx.os).ok()?;
    let output = std::process::Command::new(&cc.program)
        .args(&cc.args)
        .args(["-E", "-dD", "-x", "c"])
        .arg(format!("-I{}", dir.display()))
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let first: Vec<&str> = err.lines().take(6).collect();
        return Some(Err(format!("the C preprocessor ({}) failed:\n{}", cc.program, first.join("\n"))));
    }
    Some(Ok(String::from_utf8_lossy(&output.stdout).into_owned()))
}

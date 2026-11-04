// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A dependency's names carry where they came from.
//!
//! Every package's declarations are merged into one program before resolve,
//! and they all landed in one namespace. So a library's `Cat` and the
//! program's own `Cat` were one name with two declarations behind it, and
//! whichever arrived second lost — in a file the consumer never wrote. Two
//! dependencies that both declare `Helper` collided the same way, and there
//! the wrong one was picked inside a library's own body (#1129).
//!
//! modules/RE2 says those are different types: a type's identity is the
//! package it was declared in plus its name, not the name alone. The way to
//! make that true in a pipeline whose registries, dispatch prefixes and
//! codegen symbols are all keyed by a bare string is to give the name the
//! origin: `Cat` in `libpkg` becomes `Cat_libpkg` everywhere, before anything
//! reads it.
//!
//! An underscore is what the naming conventions leave free. A type is
//! UpperCamel (`style/pascal-case-type`), so `Cat_libpkg` is a spelling no
//! type can legitimately have; a function is snake_case, so `greet_libpkg` is
//! one a function *could* have, and if a program declares it the collision is
//! an ordinary duplicate declaration, reported by name.
//!
//! A subdirectory of the program is a package too (modules/PO1, PO3), and is
//! qualified the same way, by its path from the root: `pa/sub`'s `f` is
//! `f_pa_sub`. What something outside the program calls by name keeps it: a
//! C symbol, a function with a foreign ABI, and the entry point when a
//! sub-package holds it (`src/main.rk`).

use std::collections::HashMap;

use rask_ast::ty::TypeExpr;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_ast::qualify;
use rask_ast::rewrite::{self, Rewrite};

/// `Cat` in `libpkg` → `Cat_libpkg`.
///
/// The origin goes on the end so the name keeps its own first letter. Half the
/// compiler tells a type from a value by capitalization — dispatch decides
/// whether a resolved name is a method prefix that way (type.gradual/PC3 makes
/// it a rule, not a heuristic) — so `libpkg_Cat` reads as a value and
/// `c.bump()` came back "receiver of unresolved type" on a program the checker
/// had accepted. Suffixed, a type stays uppercase and a function stays
/// snake_case, and every one of those tests keeps working untouched.
pub fn qualified(pkg: &str, name: &str) -> String {
    format!("{}_{}", name, pkg)
}

/// Rename everything `decls` declares, and every reference to it inside them.
///
/// Returns the original name of each declaration against its new one, so the
/// resolver's export table and the consumer's own references can be pointed at
/// the same place.
///
/// A name something outside the program calls keeps it, and is in the map as
/// itself, so `pkg.name` still reaches it: a function with a foreign ABI,
/// which C calls, and, in one of the project's own packages, the entry point
/// (`main`, or `@entry`). A `src/` layout puts `main` in a sub-package.
///
/// An extern is the package's own like a function, `labs_pkg`, and links to
/// the C symbol its `@c_name` keeps (#1754). A C macro is no symbol at all:
/// it's `pkg.F`, a dotted name no Rask declaration can take, so the
/// program's own `F_pkg` can't meet it.
pub fn qualify_declarations(decls: &mut [Decl], pkg: &str, keeps_entry: bool) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut kept: HashMap<String, String> = HashMap::new();
    for decl in decls.iter() {
        match &decl.kind {
            DeclKind::Extern(e) if rask_ast::attr::c_macro(&e.attrs).is_some() => {
                map.insert(e.name.clone(), format!("{}.{}", pkg, e.name));
                continue;
            }
            DeclKind::Extern(e) => {
                map.insert(e.name.clone(), qualified(pkg, &e.name));
                continue;
            }
            DeclKind::Fn(f) => {
                let entry = f.name == "main" || f.attrs.iter().any(|a| a.is("entry"));
                if f.abi.is_some() || (keeps_entry && entry) {
                    kept.insert(f.name.clone(), f.name.clone());
                    continue;
                }
            }
            _ => {}
        }
        if let Some(name) = decl.name().map(str::to_string) {
            map.insert(name.clone(), qualified(pkg, &name));
        }
    }
    if map.is_empty() {
        map.extend(kept);
        return map;
    }

    qualify::qualify_in_place(decls, &map);
    for decl in decls.iter_mut() {
        let DeclKind::Extern(e) = &mut decl.kind else { continue };
        let Some(q) = map.get(&e.name) else { continue };
        if rask_ast::attr::c_name(&e.attrs).is_none() {
            e.attrs.push(c_name_attr(&e.name));
        }
        e.name = q.clone();
    }
    // An attachment of the package's own annotation names it too. Only those:
    // `@rename` must stay `@rename` whatever the package calls a function.
    let annotations: HashMap<String, String> = map
        .iter()
        .filter(|(_, q)| decls.iter().any(|d| matches!(&d.kind, DeclKind::Annotation(a) if &a.name == *q)))
        .map(|(n, q)| (n.clone(), q.clone()))
        .collect();
    rename_attachments(decls, &annotations);
    map.extend(kept);
    map
}

/// `@c_name("name")`.
fn c_name_attr(name: &str) -> rask_ast::attr::Attr {
    let spelling = Expr {
        id: rask_ast::NodeId::DUMMY,
        kind: ExprKind::String(name.to_string()),
        span: rask_ast::Span::new(0, 0),
    };
    let arg = rask_ast::attr::AttrArg { label: None, value: rask_ast::attr::AttrValue::Expr(spelling) };
    rask_ast::attr::Attr::synthetic("c_name", Some(vec![arg]))
}

/// The qualified names of every annotation in `decls`, for the two
/// reference rewrites below: an attachment is renamed only when what it names
/// is an annotation.
pub fn annotation_names(decls: &[Decl]) -> std::collections::HashSet<String> {
    decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Annotation(a) => Some(a.name.clone()),
            _ => None,
        })
        .collect()
}

/// Rename single-segment attachments by `map`.
fn rename_attachments(decls: &mut [Decl], map: &HashMap<String, String>) {
    struct Renamer<'a>(&'a HashMap<String, String>);
    impl Rewrite for Renamer<'_> {
        fn attr(&mut self, a: &mut rask_ast::attr::Attr) {
            if let [name] = a.path.as_slice() {
                if let Some(q) = self.0.get(name) {
                    a.path = vec![q.clone()];
                }
            }
        }
    }
    if !map.is_empty() {
        rewrite::rewrite_decls(decls, &mut Renamer(map));
    }
}

/// Point the program's own references at a dependency's qualified names.
///
/// `libpkg.Cat` in a type and `libpkg.greet(c)` in an expression are the only
/// two spellings modules/IM1 allows for a name the program didn't import, and
/// both name the package first — so the rewrite is by pair, `(binding, name)`,
/// rather than by name. A bare `Cat` in the program is the program's own.
///
/// `exports` is what each package binding makes available: the binding as the
/// program writes it (`libpkg`, or the alias from `import libpkg as l`) against
/// the original-to-qualified map of that package.
///
/// `@libpkg.validate(…)` is the attachment spelling of the same thing, and is
/// rewritten when `validate` is one of `annotations`.
pub fn qualify_references(
    decls: &mut [Decl],
    exports: &HashMap<String, HashMap<String, String>>,
    annotations: &std::collections::HashSet<String>,
) {
    if exports.is_empty() {
        return;
    }
    let mut r = ReferenceQualifier { exports, annotations };
    rewrite::rewrite_decls(decls, &mut r);
}

/// What an import path names, from one package: `ImportIndex`'s answer, the
/// resolver's too, as a key in `exports`.
pub struct ImportScope<'a> {
    pub exports: &'a HashMap<String, HashMap<String, String>>,
    pub index: &'a rask_resolve::ImportIndex,
    /// The importing package.
    pub importer: Option<rask_resolve::PackageId>,
    /// The qualified names a package keeps to itself, which `import pa.*`
    /// leaves out.
    pub private: &'a std::collections::HashSet<String>,
}

impl ImportScope<'_> {
    /// The package an import path names, as its key in `exports`, and how
    /// many of the path's segments that takes.
    pub fn package(&self, path: &[String]) -> Option<(String, usize)> {
        let (id, n) = self.index.package(path, self.importer)?;
        let key = self.index.key(id)?;
        self.exports.contains_key(key).then(|| (key.to_string(), n))
    }
}

/// Names brought in unqualified by `import pkg.Name` (modules/IM4, IM5).
///
/// Those *are* bare in the program, and mean the dependency's declaration.
///
/// A program that declares the name itself is modules/IM8 — two things under
/// one name in one scope — and that is reported rather than resolved: letting
/// the import win is the #1129 failure through another door, with the
/// program's own `Cat` losing and errors about fields it never wrote. Those
/// names are left alone and named in `shadowed`, for the caller to report.
pub fn unqualified_imports(
    decls: &[Decl],
    scope: &ImportScope,
) -> (HashMap<String, String>, Vec<ShadowedImport>) {
    let exports = scope.exports;
    let mut out = HashMap::new();
    let mut clashes = Vec::new();
    let declared: HashMap<String, rask_ast::Span> = decls
        .iter()
        .filter_map(|d| d.name().map(|n| (n.to_string(), d.span)))
        .collect();
    for decl in decls {
        let DeclKind::Import(import) = &decl.kind else { continue };
        let Some((pkg, len)) = scope.package(&import.path) else { continue };
        // `import pa` binds the package, not its names; `import pa.*` all of
        // them.
        if len == import.path.len() && !import.is_glob {
            continue;
        }
        let pkg = &pkg;
        let map = &exports[pkg];
        // `import pkg.*` brings in everything the package exports; the
        // grouped form `import pkg.{A, B}` parses into one declaration per
        // name, so the path's tail is the whole list either way.
        let wanted: Vec<String> = if import.is_glob {
            map.iter().filter(|(_, q)| !scope.private.contains(*q)).map(|(n, _)| n.clone()).collect()
        } else {
            import.path[len..].to_vec()
        };
        for name in wanted {
            let Some(q) = map.get(&name) else { continue };
            let local = match (&import.alias, import.is_glob) {
                (Some(alias), false) => alias.clone(),
                _ => name.clone(),
            };
            if let Some(&at) = declared.get(&local) {
                clashes.push(ShadowedImport {
                    name: local,
                    original: name,
                    package: pkg.clone(),
                    import_at: decl.span,
                    declared_at: at,
                });
                continue;
            }
            out.insert(local, q.clone());
        }
    }
    (out, clashes)
}

/// A bare `@name` that means a dependency's annotation the package never
/// imported by name. Under modules/IM1 `import liba` binds `liba`, not the
/// names in it, so `@validate` there names nothing — and an unknown attachment
/// is otherwise just data nobody reads, which made it a silent no-op.
pub struct UnimportedAnnotation {
    pub name: String,
    /// The binding that reaches it: the package, or its alias.
    pub binding: String,
    pub at: rask_ast::Span,
}

/// Every attachment in `decls` that is still a bare name nothing declares, but
/// that a visible package exports as an annotation. Run after the rewrites, so
/// an imported or package-qualified one is already gone from the list.
pub fn unimported_annotations(
    decls: &mut [Decl],
    exports: &HashMap<String, HashMap<String, String>>,
    annotations: &std::collections::HashSet<String>,
) -> Vec<UnimportedAnnotation> {
    let local: std::collections::HashSet<String> = annotation_names(decls);
    struct Finder<'a> {
        exports: &'a HashMap<String, HashMap<String, String>>,
        annotations: &'a std::collections::HashSet<String>,
        local: std::collections::HashSet<String>,
        found: Vec<UnimportedAnnotation>,
    }
    impl Rewrite for Finder<'_> {
        fn attr(&mut self, a: &mut rask_ast::attr::Attr) {
            let [name] = a.path.as_slice() else { return };
            if self.local.contains(name) || self.annotations.contains(name) {
                return;
            }
            let mut bindings: Vec<&String> = self
                .exports
                .iter()
                .filter(|(_, map)| map.get(name).is_some_and(|q| self.annotations.contains(q)))
                .map(|(binding, _)| binding)
                .collect();
            bindings.sort();
            if let Some(binding) = bindings.first() {
                self.found.push(UnimportedAnnotation {
                    name: name.clone(),
                    binding: (*binding).clone(),
                    at: a.span,
                });
            }
        }
    }
    let mut finder = Finder { exports, annotations, local, found: Vec::new() };
    rewrite::rewrite_decls(decls, &mut finder);
    finder.found
}

/// An unqualified import of a name the importing package declares itself.
pub struct ShadowedImport {
    /// The bare name both want.
    pub name: String,
    /// What the dependency calls it, which is `name` unless the import aliased.
    pub original: String,
    pub package: String,
    pub import_at: rask_ast::Span,
    pub declared_at: rask_ast::Span,
}

/// Rewrite bare names in the program that an unqualified import bound.
///
/// `import libpkg.validate` brings an annotation in the same way, and `@validate`
/// then means it.
pub fn qualify_imported_names(
    decls: &mut [Decl],
    imported: &HashMap<String, String>,
    annotations: &std::collections::HashSet<String>,
) {
    if imported.is_empty() {
        return;
    }
    qualify::rewrite_references(decls, imported);
    let imported_annotations: HashMap<String, String> = imported
        .iter()
        .filter(|(_, q)| annotations.contains(*q))
        .map(|(n, q)| (n.clone(), q.clone()))
        .collect();
    rename_attachments(decls, &imported_annotations);
}

struct ReferenceQualifier<'a> {
    exports: &'a HashMap<String, HashMap<String, String>>,
    annotations: &'a std::collections::HashSet<String>,
}

impl ReferenceQualifier<'_> {
    fn resolve(&self, pkg: &str, name: &str) -> Option<&String> {
        self.exports.get(pkg)?.get(name)
    }
}

impl Rewrite for ReferenceQualifier<'_> {
    fn attr(&mut self, a: &mut rask_ast::attr::Attr) {
        if let [pkg, name] = a.path.as_slice() {
            if let Some(q) = self.resolve(pkg, name).filter(|q| self.annotations.contains(*q)) {
                a.path = vec![q.clone()];
            }
        }
    }

    fn ty(&mut self, t: &mut TypeExpr) {
        // `libpkg.Cat`, anywhere inside a written type.
        *t = t.substitute_paths(&|path| match path {
            [pkg, name] => self.resolve(pkg, name).map(|q| vec![q.clone()]),
            _ => None,
        });
    }

    fn expr(&mut self, e: &mut Expr) {
        // `libpkg.twice(3)` parses as a method call on the package name, and
        // `libpkg.LIMIT` as a field of it. Neither is a receiver: the package
        // is a namespace, so the whole thing is one name. Rewriting it here
        // rather than leaving it to resolve is what lets everything after —
        // dispatch prefixes, codegen symbols — see a plain call.
        let replacement = match &e.kind {
            ExprKind::MethodCall { object, method, type_args, args } => {
                match package_binding(object) {
                    Some(pkg) => self.resolve(pkg, method).map(|q| ExprKind::Call {
                        func: Box::new(ident_like(e, q)),
                        args: args.clone(),
                    })
                    .filter(|_| type_args.is_none()),
                    None => None,
                }
            }
            ExprKind::Field { object, field } => match package_binding(object) {
                Some(pkg) => self.resolve(pkg, field).map(|q| ExprKind::Ident(q.clone())),
                None => None,
            },
            _ => None,
        };
        if let Some(kind) = replacement {
            e.kind = kind;
        }
        // `libpkg.Cat { … }`.
        if let ExprKind::StructLit { name, .. } = &mut e.kind {
            if let Some((pkg, member)) = name.split_once('.') {
                if let Some(q) = self.resolve(pkg, member) {
                    *name = q.clone();
                }
            }
        }
    }

    fn pattern(&mut self, p: &mut Pattern) {
        let name = match p {
            Pattern::Constructor { name, .. } | Pattern::Struct { name, .. } => name,
            // A fieldless `libpkg.Colour.Red` parses as an identifier pattern.
            Pattern::Ident(name) if name.contains('.') => name,
            Pattern::Wildcard
            | Pattern::Ident(_)
            | Pattern::Literal(_)
            | Pattern::Tuple(_)
            | Pattern::Or(_)
            | Pattern::Range { .. }
            | Pattern::TypePat { .. } => return,
        };
        // `libpkg.Colour.Red` — package, enum, variant.
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() == 3 {
            if let Some(q) = self.resolve(parts[0], parts[1]) {
                *name = format!("{}.{}", q, parts[2]);
            }
        }
    }
}

/// The package a qualified access names, when the receiver is just a name.
fn package_binding(object: &Expr) -> Option<&str> {
    match &object.kind {
        ExprKind::Ident(n) => Some(n.as_str()),
        _ => None,
    }
}

/// An `Ident` carrying the original expression's id and span, so a diagnostic
/// about the rewritten call still points at what was written.
fn ident_like(at: &Expr, name: &str) -> Expr {
    Expr { id: at.id, span: at.span, kind: ExprKind::Ident(name.to_string()) }
}

/// Write a dependency's names back the way the program spells them, in every
/// diagnostic.
///
/// `Cat_libpkg` is the compiler's name for the type; `libpkg.Cat` is the
/// program's. A message that says the first one names a type nobody wrote,
/// which is the same failure #1129 was about, one layer out — so this runs over
/// every diagnostic the pipeline produced rather than at the few places that
/// happen to build a message from a type name.
pub fn unqualify_diagnostics(
    diags: &mut [rask_diagnostics::Diagnostic],
    exports: &HashMap<String, HashMap<String, String>>,
) {
    // Each entry is (compiler name, how the program spells it, the bare name).
    let mut back: Vec<(String, String, String)> = Vec::new();
    for (pkg, map) in exports {
        for (original, q) in map {
            // A C function keeps its name (a link symbol is global), so it
            // has nothing to write back, and `rename` in a message stays
            // the word.
            if original == q {
                continue;
            }
            back.push((q.clone(), format!("{}.{}", pkg, original), original.clone()));
        }
    }
    if back.is_empty() {
        return;
    }
    // Longest first: `Cat_libpkg` and a `Cat_libpkg_extra` in the same package
    // both start the same way, and replacing the shorter one first would leave
    // `libpkg.Cat_extra`.
    back.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    let rewrite = |s: &mut String| {
        for (q, spelled, bare) in &back {
            if !s.contains(q.as_str()) {
                continue;
            }
            // Where the name sits inside a longer identifier, the compiler
            // built that identifier from it — a duplicate conformance suggests
            // `type MyDoc = …` and an opted-out encoding suggests
            // `struct DocWire { … }`, which are `MyDoc_interfacepkg` and
            // `Doc_interfacepkgWire` at this point. A dotted path can't go in the
            // middle of an identifier, so those get the bare name; anywhere
            // else gets the spelling the program uses.
            let ident_char = |c: char| c.is_alphanumeric() || c == '_';
            let mut out = String::with_capacity(s.len());
            let mut rest = s.as_str();
            while let Some(at) = rest.find(q.as_str()) {
                let before = rest[..at].chars().next_back().is_some_and(ident_char);
                let after = rest[at + q.len()..].chars().next().is_some_and(ident_char);
                out.push_str(&rest[..at]);
                out.push_str(if before || after { bare } else { spelled });
                rest = &rest[at + q.len()..];
            }
            out.push_str(rest);
            *s = out;
        }
    };
    for d in diags {
        rewrite(&mut d.message);
        for label in &mut d.labels {
            if let Some(m) = &mut label.message {
                rewrite(m);
            }
        }
        for note in &mut d.notes {
            rewrite(note);
        }
        if let Some(h) = &mut d.help {
            rewrite(&mut h.message);
            if let Some(s) = &mut h.suggestion {
                rewrite(&mut s.replacement);
            }
        }
        if let Some(f) = &mut d.fix {
            rewrite(f);
        }
        if let Some(w) = &mut d.why {
            rewrite(w);
        }
    }
}

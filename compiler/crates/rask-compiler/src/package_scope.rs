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
//! Only a dependency is qualified. A subdirectory of the program is also a
//! package by modules/PO1, and its declarations are merged the same way — but
//! it shares the program's namespace, because `func main()` in `src/main.rk`
//! is one of those and renaming it leaves the program without an entry point.
//! Giving a nested package its own scope is PO3 and a separate step.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_ast::qualify::{self, declared_name};
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
pub fn qualify_declarations(decls: &mut [Decl], pkg: &str) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for decl in decls.iter() {
        if let Some(name) = declared_name(decl) {
            map.insert(name.clone(), qualified(pkg, &name));
        }
    }
    if map.is_empty() {
        return map;
    }

    qualify::qualify_in_place(decls, &map);
    map
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
pub fn qualify_references(
    decls: &mut [Decl],
    exports: &HashMap<String, HashMap<String, String>>,
) {
    if exports.is_empty() {
        return;
    }
    let mut r = ReferenceQualifier { exports };
    rewrite::rewrite_decls(decls, &mut r);
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
    exports: &HashMap<String, HashMap<String, String>>,
) -> (HashMap<String, String>, Vec<ShadowedImport>) {
    let mut out = HashMap::new();
    let mut clashes = Vec::new();
    let declared: HashMap<String, rask_ast::Span> = decls
        .iter()
        .filter_map(|d| declared_name(d).map(|n| (n, d.span)))
        .collect();
    for decl in decls {
        let DeclKind::Import(import) = &decl.kind else { continue };
        if import.path.len() < 2 {
            continue;
        }
        let pkg = &import.path[0];
        let Some(map) = exports.get(pkg) else { continue };
        // `import pkg.*` brings in everything the package exports; the
        // grouped form `import pkg.{A, B}` parses into one declaration per
        // name, so the path's tail is the whole list either way.
        let wanted: Vec<String> = if import.is_glob {
            map.keys().cloned().collect()
        } else {
            import.path[1..].to_vec()
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
pub fn qualify_imported_names(decls: &mut [Decl], imported: &HashMap<String, String>) {
    if imported.is_empty() {
        return;
    }
    qualify::rewrite_references(decls, imported);
}

struct ReferenceQualifier<'a> {
    exports: &'a HashMap<String, HashMap<String, String>>,
}

impl ReferenceQualifier<'_> {
    fn resolve(&self, pkg: &str, name: &str) -> Option<&String> {
        self.exports.get(pkg)?.get(name)
    }
}

impl Rewrite for ReferenceQualifier<'_> {
    fn ty(&mut self, t: &mut String) {
        // `libpkg.Cat`, anywhere inside a written type. Word-level again, but
        // the word to match is two words and a dot.
        let mut out = String::with_capacity(t.len());
        let mut rest = t.as_str();
        while let Some(dot) = rest.find('.') {
            let (head, tail) = rest.split_at(dot);
            let pkg_start = head.len() - ident_suffix_len(head);
            let after = &tail[1..];
            let name_len = ident_prefix_len(after);
            let pkg = &head[pkg_start..];
            let name = &after[..name_len];
            match (pkg.is_empty() || name.is_empty()).then_some(()) {
                Some(()) => {
                    out.push_str(head);
                    out.push('.');
                    rest = after;
                }
                None => match self.resolve(pkg, name) {
                    Some(q) => {
                        out.push_str(&head[..pkg_start]);
                        out.push_str(q);
                        rest = &after[name_len..];
                    }
                    None => {
                        out.push_str(head);
                        out.push('.');
                        rest = after;
                    }
                },
            }
        }
        out.push_str(rest);
        *t = out;
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
            // `libpkg.Cat { … }` — the parser keeps the package in the name.
            ExprKind::StructLit { name, fields, spread } => match name.split_once('.') {
                Some((pkg, tail)) => self.resolve(pkg, tail).map(|q| ExprKind::StructLit {
                    name: q.clone(),
                    fields: fields.clone(),
                    spread: spread.clone(),
                }),
                None => None,
            },
            _ => None,
        };
        if let Some(kind) = replacement {
            e.kind = kind;
        }
    }

    fn pattern(&mut self, p: &mut Pattern) {
        let name = match p {
            Pattern::Constructor { name, .. } | Pattern::Struct { name, .. } => name,
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

fn ident_prefix_len(s: &str) -> usize {
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .map(char::len_utf8)
        .sum()
}

fn ident_suffix_len(s: &str) -> usize {
    s.chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .map(char::len_utf8)
        .sum()
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
            // `struct DocWire { … }`, which are `MyDoc_traitpkg` and
            // `Doc_traitpkgWire` at this point. A dotted path can't go in the
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

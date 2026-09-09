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

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind, Pattern};
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

    // Names a body binds. A local can share a spelling with one of the
    // package's own functions or consts — both are snake_case — and renaming
    // the reference would point it at a declaration instead of at the binding.
    // Collected per declaration and not per block, so a name bound anywhere in
    // a body is left alone throughout it: erring that way leaves a reference
    // that resolve reports, where erring the other way silently reads a local
    // as a function.
    //
    // Type names don't need this. A local named `Cat` is already a naming
    // violation, so the conventions keep that collision out.
    for decl in decls.iter_mut() {
        let shadowed = names_bound_in(decl);
        let mut r = Qualifier { map: &map, shadowed: &shadowed, pkg };
        rewrite::rewrite_decl(decl, &mut r);
    }

    for decl in decls.iter_mut() {
        rename_declaration(decl, &map);
    }
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
    for decl in decls.iter_mut() {
        let shadowed = names_bound_in(decl);
        let mut r = Qualifier { map: imported, shadowed: &shadowed, pkg: "" };
        rewrite::rewrite_decl(decl, &mut r);
    }
}

struct Qualifier<'a> {
    map: &'a HashMap<String, String>,
    shadowed: &'a HashSet<String>,
    pkg: &'a str,
}

impl Qualifier<'_> {
    /// A name that names a declaration here, and isn't a local.
    fn lookup(&self, name: &str) -> Option<&String> {
        if self.shadowed.contains(name) {
            return None;
        }
        self.map.get(name)
    }
}

impl Rewrite for Qualifier<'_> {
    fn ty(&mut self, t: &mut String) {
        // A type is written, not parsed, at this stage — `Vec<Cat>`, `Cat?`,
        // `i64 or Cat` — so the substitution is by word. A type string can
        // never name a local, so the shadow set doesn't apply.
        let subst: Vec<(String, String)> =
            self.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        *t = rask_ast::type_str::substitute_type_params(t, &subst);
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Ident(name) => {
                if let Some(q) = self.lookup(name) {
                    *name = q.clone();
                }
            }
            // A type name, so no shadow check.
            ExprKind::StructLit { name, .. } => {
                if let Some(q) = self.map.get(name.as_str()) {
                    *name = q.clone();
                }
            }
            // `spawn_raw { … }` and `using pool { … }` name a declaration the
            // same way a call does.
            ExprKind::BlockCall { name, .. } | ExprKind::UsingBlock { name, .. } => {
                if let Some(q) = self.lookup(name) {
                    *name = q.clone();
                }
            }
            _ => {}
        }
        let _ = self.pkg;
    }

    fn pattern(&mut self, p: &mut Pattern) {
        // `Colour.Red` in a pattern is the enum's name and the variant's; only
        // the enum is a declaration. A bare `Red` is a variant of whatever the
        // scrutinee is, and there is nothing here to rename.
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
        match name.split_once('.') {
            Some((head, tail)) => {
                if let Some(q) = self.map.get(head) {
                    *name = format!("{}.{}", q, tail);
                }
            }
            None => {
                if let Some(q) = self.map.get(name.as_str()) {
                    *name = q.clone();
                }
            }
        }
    }
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

fn declared_name(decl: &Decl) -> Option<String> {
    match &decl.kind {
        DeclKind::Fn(f) => Some(f.name.clone()),
        DeclKind::Struct(s) => Some(s.name.clone()),
        DeclKind::Enum(e) => Some(e.name.clone()),
        DeclKind::Trait(t) => Some(t.name.clone()),
        DeclKind::Const(c) => Some(c.name.clone()),
        DeclKind::TypeAlias(a) => Some(a.name.clone()),
        DeclKind::Annotation(a) => Some(a.name.clone()),
        DeclKind::Union(u) => Some(u.name.clone()),
        _ => None,
    }
}

fn rename_declaration(decl: &mut Decl, map: &HashMap<String, String>) {
    let name: &mut String = match &mut decl.kind {
        DeclKind::Fn(f) => &mut f.name,
        DeclKind::Struct(s) => &mut s.name,
        DeclKind::Enum(e) => &mut e.name,
        DeclKind::Trait(t) => &mut t.name,
        DeclKind::Const(c) => &mut c.name,
        DeclKind::TypeAlias(a) => &mut a.name,
        DeclKind::Annotation(a) => &mut a.name,
        DeclKind::Union(u) => &mut u.name,
        _ => return,
    };
    if let Some(q) = map.get(name.as_str()) {
        *name = q.clone();
    }
}

/// Every name a declaration's bodies bind, anywhere inside them.
fn names_bound_in(decl: &Decl) -> HashSet<String> {
    let mut out: HashSet<String> = HashSet::new();
    let mut params_of = |f: &rask_ast::decl::FnDecl, out: &mut HashSet<String>| {
        for p in &f.params {
            out.insert(p.name.clone());
        }
        for c in &f.context_clauses {
            if let Some(n) = &c.name {
                out.insert(n.clone());
            }
        }
    };
    match &decl.kind {
        DeclKind::Fn(f) => params_of(f, &mut out),
        DeclKind::Struct(s) => s.methods.iter().for_each(|m| params_of(m, &mut out)),
        DeclKind::Enum(e) => e.methods.iter().for_each(|m| params_of(m, &mut out)),
        DeclKind::Trait(t) => t.methods.iter().for_each(|m| params_of(m, &mut out)),
        DeclKind::Impl(i) => i.methods.iter().for_each(|m| params_of(m, &mut out)),
        _ => {}
    }
    rask_ast::visit::walk_decl(decl, &mut |e| match &e.kind {
        ExprKind::Closure { params, .. } => {
            for p in params {
                out.insert(p.name.clone());
            }
        }
        ExprKind::WithAs { bindings, .. } => {
            for b in bindings {
                out.insert(b.name.clone());
            }
        }
        ExprKind::Catch { clause, .. } => {
            out.insert(clause.binder.clone());
        }
        ExprKind::IfLet { pattern, else_binding, .. } => {
            out.extend(pattern.bound_names().into_iter().map(str::to_string));
            if let Some(b) = else_binding {
                out.insert(b.clone());
            }
        }
        ExprKind::GuardPattern { pattern, .. } | ExprKind::IsPattern { pattern, .. } => {
            out.extend(pattern.bound_names().into_iter().map(str::to_string));
        }
        ExprKind::Match { arms, .. } => {
            for arm in arms {
                out.extend(arm.pattern.bound_names().into_iter().map(str::to_string));
            }
        }
        _ => {}
    });
    bound_by_statements(decl, &mut out);
    out
}

fn bound_by_statements(decl: &Decl, out: &mut HashSet<String>) {
    fn body(stmts: &[rask_ast::stmt::Stmt], out: &mut HashSet<String>) {
        use rask_ast::stmt::{ForBinding, StmtKind, TuplePat};
        fn tuple(pats: &[TuplePat], out: &mut HashSet<String>) {
            for p in pats {
                match p {
                    TuplePat::Name(n) => {
                        out.insert(n.clone());
                    }
                    TuplePat::Wildcard => {}
                    TuplePat::Nested(inner) => tuple(inner, out),
                }
            }
        }
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Let { name, .. } | StmtKind::Mut { name, .. } => {
                    out.insert(name.clone());
                }
                StmtKind::LetTuple { patterns, .. } | StmtKind::MutTuple { patterns, .. } => {
                    tuple(patterns, out)
                }
                StmtKind::LetStruct { pattern, .. } => {
                    out.extend(pattern.bound_names().into_iter().map(str::to_string))
                }
                StmtKind::While { body: b, .. } | StmtKind::Loop { body: b, .. } => body(b, out),
                StmtKind::WhileLet { pattern, body: b, .. } => {
                    out.extend(pattern.bound_names().into_iter().map(str::to_string));
                    body(b, out);
                }
                StmtKind::For { binding, body: b, .. }
                | StmtKind::ComptimeFor { binding, body: b, .. } => {
                    match binding {
                        ForBinding::Single(n) => {
                            out.insert(n.clone());
                        }
                        ForBinding::Tuple(ns) => out.extend(ns.iter().cloned()),
                    }
                    body(b, out);
                }
                StmtKind::Comptime(b) => body(b, out),
                StmtKind::Ensure { body: b, else_handler } => {
                    body(b, out);
                    if let Some((param, handler)) = else_handler {
                        out.insert(param.clone());
                        body(handler, out);
                    }
                }
                StmtKind::Expr(_)
                | StmtKind::Assign { .. }
                | StmtKind::Return(_)
                | StmtKind::Break { .. }
                | StmtKind::Continue(_)
                | StmtKind::Discard { .. } => {}
            }
        }
    }
    match &decl.kind {
        DeclKind::Fn(f) => body(&f.body, out),
        DeclKind::Struct(s) => s.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Enum(e) => e.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Trait(t) => t.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Impl(i) => i.methods.iter().for_each(|m| body(&m.body, out)),
        DeclKind::Test(t) => body(&t.body, out),
        DeclKind::Benchmark(b) => body(&b.body, out),
        _ => {}
    }
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
    let mut back: Vec<(String, String)> = Vec::new();
    for (pkg, map) in exports {
        for (original, q) in map {
            back.push((q.clone(), format!("{}.{}", pkg, original)));
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
        for (q, spelled) in &back {
            if s.contains(q.as_str()) {
                *s = s.replace(q.as_str(), spelled);
            }
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

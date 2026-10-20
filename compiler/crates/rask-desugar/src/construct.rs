// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `Point(x: 1, y: 2)` builds a struct; `Point(x, y)` takes it apart.
//!
//! Values are written in parentheses whatever they are, so the parser can't
//! tell a construction from a call: `Point(x)` and `Circle(r)` look the same.
//! The declarations can. A struct, or a variant whose fields have names, is
//! filled by name, and a bare name is short for `name: name`. A positional
//! variant and a function are filled by position. So this pass turns every
//! call that names a struct or a named variant into the struct literal the
//! later passes already understand, and does the same for patterns.
//!
//! A name a struct and a variant share is no question in an expression: a
//! variant is built through its enum, `Shape.Pair(…)`, so bare `Pair(…)` is
//! the struct. In a pattern the enum can be left off, so bare `Pair(a, b)`
//! is whichever the matched value is. That takes its type, so this pass
//! leaves the pattern for the checker (`struct_shorthand_patterns`).

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind, FieldInit, Pattern};
use rask_ast::rewrite::{self, Rewrite};
use rask_ast::ty::TypeExpr;
use rask_ast::Span;
use std::collections::{HashMap, HashSet};

use crate::DesugarError;

const CODE: &str = "E0416";

/// What the declarations say about each name a value can be built from.
#[derive(Default)]
struct Shapes {
    /// Struct name → its field names.
    structs: HashMap<String, Vec<String>>,
    /// Enum name → variant name → its field names, `None` when positional.
    enums: HashMap<String, HashMap<String, Option<Vec<String>>>>,
    /// Variant name → field names, for variants with named fields in any
    /// enum. Empty when two enums name one differently: which one a bare use
    /// means takes the matched type, so the checker judges its fields.
    named_variants: HashMap<String, Vec<String>>,
    /// Variant names with positional fields, in any enum.
    positional_variants: HashSet<String>,
    /// `import c "…" as ns` namespaces. Their structs come from the header,
    /// read after this pass.
    c_namespaces: HashSet<String>,
}

impl Shapes {
    fn collect(decls: &[Decl], stdlib: &[Decl]) -> Shapes {
        let mut shapes = Shapes::default();
        // The stdlib first, so a program's own `Handle` replaces the stdlib's.
        for decl in stdlib.iter().chain(decls.iter()) {
            match &decl.kind {
                DeclKind::Struct(s) => {
                    let fields = s.fields.iter().map(|f| f.name.clone()).collect();
                    shapes.structs.insert(last_segment(&s.name).to_string(), fields);
                }
                DeclKind::Enum(e) => {
                    let mut variants = HashMap::new();
                    for v in &e.variants {
                        let Some(first) = v.fields.first() else { continue };
                        if is_positional_name(&first.name) {
                            variants.insert(v.name.clone(), None);
                            shapes.positional_variants.insert(v.name.clone());
                        } else {
                            let fields: Vec<String> = v.fields.iter().map(|f| f.name.clone()).collect();
                            variants.insert(v.name.clone(), Some(fields.clone()));
                            match shapes.named_variants.get(&v.name) {
                                Some(seen) if *seen != fields => {
                                    shapes.named_variants.insert(v.name.clone(), Vec::new());
                                }
                                _ => {
                                    shapes.named_variants.insert(v.name.clone(), fields);
                                }
                            }
                        }
                    }
                    shapes.enums.insert(last_segment(&e.name).to_string(), variants);
                }
                DeclKind::CImport(c) => {
                    shapes.c_namespaces.insert(c.alias.clone());
                }
                _ => {}
            }
        }
        // A program's own function owns its name the same way its own struct
        // does. `func time(x)` called as `time(1)` was built as the stdlib's
        // `time` namespace struct, which has no fields, so the call failed
        // with "takes its fields by name" (#1581).
        for decl in decls {
            let name = match &decl.kind {
                DeclKind::Fn(f) => &f.name,
                DeclKind::Extern(e) => &e.name,
                _ => continue,
            };
            let own_struct = decls.iter().any(|d| matches!(&d.kind, DeclKind::Struct(s) if last_segment(&s.name) == name));
            if !own_struct {
                shapes.structs.remove(name.as_str());
            }
        }
        shapes
    }

    /// The fields `path(…)` fills by name in an expression, or `None` when
    /// it's a call (a function, a static method, a positional variant).
    fn built_fields(&self, path: &[String], labelled: bool, method: bool) -> Option<Vec<String>> {
        let last = path.last()?;
        if let Some(variant) = self.variant_of(path) {
            return variant;
        }
        // `Response.json(…)`: a type's own function, not something built.
        if method && path.len() >= 2 && self.names_a_type(&path[path.len() - 2]) {
            return None;
        }
        if let Some(fields) = self.structs.get(last.as_str()) {
            return Some(fields.clone());
        }
        if self.named_variants.contains_key(last.as_str()) || self.positional_variants.contains(last.as_str()) {
            return self.bare_named_variant(last, labelled);
        }
        // `c.Rect(width: 2)`: a C function takes no labels, so a labelled
        // call through a C namespace builds one of its structs. The header is
        // read later, and the checker has the fields.
        match path {
            [ns, _] if labelled && self.c_namespaces.contains(ns.as_str()) => Some(Vec::new()),
            _ => None,
        }
    }

    /// The fields the pattern `path(…)`, written without labels, matches by
    /// name, or `None` when it matches by position.
    fn matched_fields(&self, path: &[String]) -> Option<Vec<String>> {
        let last = path.last()?;
        if let Some(variant) = self.variant_of(path) {
            return variant;
        }
        if self.named_variants.contains_key(last.as_str()) || self.positional_variants.contains(last.as_str()) {
            return self.bare_named_variant(last, false);
        }
        self.structs.get(last.as_str()).cloned()
    }

    /// A bare name both a struct with fields and some variant go by.
    fn struct_or_variant(&self, name: &str) -> bool {
        !name.contains('.')
            && self.structs.get(name).is_some_and(|f| !f.is_empty())
            && (self.named_variants.contains_key(name) || self.positional_variants.contains(name))
    }

    /// `Enum.Variant`: that variant's fields, if `path` names one.
    fn variant_of(&self, path: &[String]) -> Option<Option<Vec<String>>> {
        let [.., ty, variant] = path else { return None };
        self.enums.get(last_segment(ty))?.get(variant.as_str()).cloned()
    }

    /// A struct with fields or an enum. A struct with none is a namespace
    /// (`struct http { }`), and `http.Response(…)` builds what it holds.
    fn names_a_type(&self, name: &str) -> bool {
        let name = last_segment(name);
        self.structs.get(name).is_some_and(|f| !f.is_empty()) || self.enums.contains_key(name)
    }

    /// A bare variant name. Two enums can each have one, one named and one
    /// positional; then a label written says which.
    fn bare_named_variant(&self, name: &str, labelled: bool) -> Option<Vec<String>> {
        let fields = self.named_variants.get(name)?;
        (!self.positional_variants.contains(name) || labelled).then(|| fields.clone())
    }
}

fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// `_0`, `_1`: what the parser names a positional field.
fn is_positional_name(name: &str) -> bool {
    name.strip_prefix('_').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `a.b.C` as segments, with the type arguments written on it:
/// `Slot<i64>.Pair`, `Pair<i32, string>`.
fn callee_path(e: &Expr) -> Option<(Vec<String>, Vec<TypeExpr>)> {
    match &e.kind {
        ExprKind::Ident(n) => Some((n.split('.').map(str::to_string).collect(), Vec::new())),
        ExprKind::GenericName { name, type_args } => {
            Some((name.split('.').map(str::to_string).collect(), type_args.clone()))
        }
        ExprKind::Field { object, field } => {
            let (mut path, args) = callee_path(object)?;
            path.push(field.clone());
            Some((path, args))
        }
        _ => None,
    }
}

/// "`blocks: n`" — the field at the position a value was written in, as the
/// fix to suggest.
fn suggestion(fields: &[String], index: usize, value: &str) -> String {
    match fields.get(index).or(fields.first()) {
        Some(f) => format!("`{}: {}`", f, value),
        None => "`field: value`".to_string(),
    }
}

/// Turn constructions written as calls into struct literals, everywhere.
pub fn resolve_constructions(decls: &mut [Decl], stdlib: &[Decl]) -> Vec<DesugarError> {
    let shapes = Shapes::collect(decls, stdlib);
    let mut errors = Vec::new();
    for decl in decls.iter_mut() {
        let mut pass = Construct { shapes: &shapes, errors: &mut errors, at: decl.span, placed: HashSet::new() };
        rewrite::rewrite_decl(decl, &mut pass);
    }
    errors
}

struct Construct<'a> {
    shapes: &'a Shapes,
    errors: &'a mut Vec<DesugarError>,
    /// The latest expression's span: patterns carry none of their own.
    at: Span,
    /// Patterns already handled with a better location — a match arm's, an
    /// `is` test's, a destructuring `let`'s — by address, so the walk that
    /// reaches them afterwards leaves them be.
    placed: HashSet<*const Pattern>,
}

impl Construct<'_> {
    fn error(&mut self, message: String, label: &str, span: Span) {
        self.errors.push(DesugarError { message, span, code: CODE, label: label.to_string() });
    }

    /// `name(args)` as a struct literal. A value that can't be a field — a
    /// positional one, or a bare name that isn't a field — is reported with
    /// the field its position suggests.
    fn struct_literal(&mut self, name: String, type_args: Vec<TypeExpr>, args: &[CallArg], declared: &[String]) -> ExprKind {
        let mut fields = Vec::new();
        let mut spread = None;
        for (i, arg) in args.iter().enumerate() {
            match (arg.mode, &arg.name) {
                (ArgMode::Spread, _) => spread = Some(Box::new(arg.expr.clone())),
                (ArgMode::Mutate | ArgMode::Deleting, _) => self.error(
                    format!("`{}(…)` builds a value, so a field takes no `mutate` or `deleting`", name),
                    "a field's value",
                    arg.expr.span,
                ),
                (ArgMode::Default, Some(label)) => {
                    fields.push(FieldInit { name: label.clone(), value: arg.expr.clone() })
                }
                (ArgMode::Default, None) => match &arg.expr.kind {
                    // `Point(x)` is `Point(x: x)`. With no declaration to
                    // hand, the checker says whether `x` is a field.
                    ExprKind::Ident(n) if declared.is_empty() || declared.iter().any(|f| f == n) => {
                        fields.push(FieldInit { name: n.clone(), value: arg.expr.clone() })
                    }
                    ExprKind::Ident(n) => {
                        self.error(
                            format!(
                                "`{}` has no field `{}`: a bare name fills the field it names. Write {}",
                                name, n, suggestion(declared, i, n)
                            ),
                            "no field by this name",
                            arg.expr.span,
                        );
                        self.assume_positional(&mut fields, declared, i, arg);
                    }
                    _ => {
                        self.error(
                            format!("`{}(…)` takes its fields by name. Write {}", name, suggestion(declared, i, "…")),
                            "a value with no field name",
                            arg.expr.span,
                        );
                        self.assume_positional(&mut fields, declared, i, arg);
                    }
                },
            }
        }
        ExprKind::StructLit { name, type_args, fields, spread }
    }

    /// After reporting a value with no field name, carry on as if it filled
    /// the field its position suggests — the one the error names — so the
    /// checker types it there instead of reporting that field missing too.
    fn assume_positional(&self, fields: &mut Vec<FieldInit>, declared: &[String], index: usize, arg: &CallArg) {
        if let Some(field) = declared.get(index) {
            if !fields.iter().any(|f| &f.name == field) {
                fields.push(FieldInit { name: field.clone(), value: arg.expr.clone() });
            }
        }
    }
}

impl Rewrite for Construct<'_> {
    fn expr(&mut self, e: &mut Expr) {
        self.at = e.span;
        match &mut e.kind {
            ExprKind::Match { arms, .. } => {
                for arm in arms.iter_mut() {
                    let at = arm.body.span;
                    self.place(&mut arm.pattern, at);
                }
            }
            ExprKind::IsPattern { pattern, .. } => {
                let at = e.span;
                self.place(pattern, at);
            }
            _ => {}
        }
        let labelled = |args: &[CallArg]| args.iter().any(|a| a.name.is_some() || a.mode == ArgMode::Spread);
        let built = match &e.kind {
            ExprKind::Call { func, args } => callee_path(func).and_then(|(path, type_args)| {
                let fields = self.shapes.built_fields(&path, labelled(args), false)?;
                Some((path.join("."), type_args, args.clone(), fields))
            }),
            ExprKind::MethodCall { object, method, type_args: None, args } => {
                callee_path(object).and_then(|(mut path, type_args)| {
                    path.push(method.clone());
                    let fields = self.shapes.built_fields(&path, labelled(args), true)?;
                    Some((path.join("."), type_args, args.clone(), fields))
                })
            }
            _ => None,
        };
        if let Some((name, type_args, args, fields)) = built {
            e.kind = self.struct_literal(name, type_args, &args, &fields);
            return;
        }
        // A `..base` anywhere else has nothing to fill.
        let stray = match &e.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                args.iter().find(|a| a.mode == ArgMode::Spread).map(|a| a.expr.span)
            }
            _ => None,
        };
        if let Some(span) = stray {
            self.error(
                "`..base` builds a value from another one, like `Point(x: 1, ..p)`; a function has no fields for it to fill".to_string(),
                "not building a value",
                span,
            );
        }
    }

    fn body(&mut self, stmts: &mut Vec<rask_ast::stmt::Stmt>) {
        for stmt in stmts.iter_mut() {
            if let rask_ast::stmt::StmtKind::LetStruct { pattern, init, .. } = &mut stmt.kind {
                let at = init.span;
                self.place(pattern, at);
            }
        }
    }

    fn pattern(&mut self, p: &mut Pattern) {
        if self.placed.contains(&(p as *const Pattern)) {
            return;
        }
        let at = self.at;
        self.convert(p, at);
    }
}

impl Construct<'_> {
    /// Convert a whole pattern tree, reporting at `at`.
    fn place(&mut self, p: &mut Pattern, at: Span) {
        self.convert(p, at);
        self.placed.insert(p as *const Pattern);
        match p {
            Pattern::Constructor { fields, .. } | Pattern::Tuple(fields) | Pattern::Or(fields) => {
                for f in fields {
                    self.place(f, at);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, f) in fields {
                    self.place(f, at);
                }
            }
            Pattern::Wildcard
            | Pattern::Ident(_)
            | Pattern::Literal(_)
            | Pattern::Range { .. }
            | Pattern::TypePat { .. } => {}
        }
    }

    fn convert(&mut self, p: &mut Pattern, at: Span) {
        let Pattern::Constructor { name, fields } = p else { return };
        if self.shapes.struct_or_variant(name) {
            return;
        }
        let path: Vec<String> = name.split('.').map(str::to_string).collect();
        let Some(declared) = self.shapes.matched_fields(&path) else { return };
        let mut named = Vec::new();
        for (i, field) in fields.iter().enumerate() {
            let problem = match field {
                Pattern::Ident(n) if declared.is_empty() || declared.iter().any(|f| f == n) => {
                    named.push((n.clone(), Pattern::Ident(n.clone())));
                    continue;
                }
                Pattern::Ident(n) => format!(
                    "`{}` has no field `{}`: a bare name binds the field it names. Write {}",
                    name, n, suggestion(&declared, i, n)
                ),
                _ => format!("`{}(…)` matches its fields by name. Write {}", name, suggestion(&declared, i, "…")),
            };
            self.error(problem, "in this pattern", at);
            return;
        }
        *p = Pattern::Struct { name: name.clone(), fields: named, rest: false };
    }
}

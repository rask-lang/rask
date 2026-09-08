// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Generalizing an inferred parameter whose only constraint is an operator.
//!
//! `type.gradual/IN3` — "constraints from trait methods, operators, or calls
//! needing bounds produce generic" — with `func double(x) { x * 2 }` as the
//! spec's own worked example, inferred as `<T: Numeric>(x: T) -> T`.
//!
//! What existed was IN2 and nothing else. An omitted parameter type became one
//! type variable shared by the whole program, so whatever pinned it first won:
//! the `2` defaulted to `i32`, dragged `x` along, and `double(1.5)` was
//! rejected for mixing a float with an integer in a program that has no `i32`
//! in it. Then one call site was made to work (#1054) and two at different
//! types still weren't, because there was still only ever one variable.
//!
//! The signature the spec says gets inferred has always worked when written
//! out, at both types. So this writes it: the parameter gets a real type
//! parameter with the operator's bound, the return type gets written down too,
//! and each call site instantiates the pair fresh. Nothing else about
//! inference changes.
//!
//! Deliberately narrow. Every use of the parameter has to sit inside an
//! expression built only from the parameter, unsuffixed numeric literals, and
//! arithmetic or comparison operators — `x`, `x * 2`, `x * 2 > 10`. The bound
//! is the union of what those operators need, and the return type is what the
//! `return`s answer.
//!
//! Anything else pins the type, or might, and keeps IN2's concrete answer: a
//! string literal on the other side of `==` means the parameter is a `string`
//! and `T: Numeric` would be wrong; a method call says something this pass
//! can't map to a trait; a field access or an index says a specific shape.
//! `a + b` on two inferred parameters says the two are the same type and
//! nothing about which, so it stays concrete as well.
//!
//! `func count(items) { items.len() }` is the other half of #904 and is not
//! this: the table's answer is `<T>(items: Vec<T>) -> usize`, where the
//! parameter isn't the generic — its *element* is. Getting there means going
//! from "something with `.len()`" to `Vec<T>`, which is structural-to-nominal
//! inference the checker doesn't do at all.

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl, TypeParam};
use rask_ast::expr::{BinOp, Expr, ExprKind, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::NodeId;

/// Give every generalizable inferred parameter a type parameter and a bound.
pub fn generalize_inferred_params(decls: &mut [Decl]) {
    for decl in decls.iter_mut() {
        match &mut decl.kind {
            DeclKind::Fn(f) => generalize_fn(f),
            DeclKind::Struct(s) => s.methods.iter_mut().for_each(generalize_fn),
            DeclKind::Enum(e) => e.methods.iter_mut().for_each(generalize_fn),
            DeclKind::Impl(i) => i.methods.iter_mut().for_each(generalize_fn),
            _ => {}
        }
    }
}

/// What an expression built out of one inferred parameter answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The parameter's own type — the parameter itself, or arithmetic on it.
    Same,
    /// A comparison of it.
    Bool,
    /// An unsuffixed numeric literal, which is still open.
    OpenLiteral,
}

fn generalize_fn(f: &mut FnDecl) {
    // GC5: a public function's signature is written out, so there is nothing to
    // infer. A function that already declares type parameters — in the list or
    // in its name — has been given its answer too.
    if f.is_pub || !f.type_params.is_empty() || f.name.contains('<') {
        return;
    }

    let inferred: HashSet<String> = f
        .params
        .iter()
        .filter(|p| p.name != "self" && p.ty.is_empty())
        .map(|p| p.name.clone())
        .collect();
    if inferred.is_empty() {
        return;
    }

    // A closure parameter of the same name shadows ours, and this pass reads
    // names rather than resolutions — so the safe answer for a body that
    // rebinds the name is "don't".
    let shadowed = shadowed_names(&f.body);

    let mut plans: HashMap<String, Vec<String>> = HashMap::new();
    let mut returns: HashMap<String, String> = HashMap::new();
    for name in &inferred {
        if shadowed.contains(name) {
            continue;
        }
        let Some(bounds) = bounds_from_body(&f.body, name) else { continue };
        // The return type has to come out too, and written down rather than
        // left inferred: a generic function whose return type is inferred hands
        // its callers a type parameter nothing instantiates, and
        // `double(21) == 42` is then "no method `eq` on `T`". Half a signature
        // is worse than none, so a body whose `return`s aren't readable keeps
        // IN2's answer.
        if returns_a_value(&f.body) {
            match returned_shape(&f.body, name) {
                Some(shape) => {
                    returns.insert(name.clone(), shape_name(shape, name));
                }
                None => continue,
            }
        }
        plans.insert(name.clone(), bounds);
    }
    if plans.is_empty() {
        return;
    }
    // One parameter's `return` says what the function answers, so two of them
    // disagreeing about it is not something to pick between.
    if returns.values().collect::<HashSet<_>>().len() > 1 {
        return;
    }

    let mut taken = letters_already_in(f);
    let mut letters: HashMap<String, String> = HashMap::new();
    let mut added: Vec<TypeParam> = Vec::new();
    for param in f.params.iter_mut() {
        let Some(bounds) = plans.remove(&param.name) else { continue };
        let Some(letter) = free_letter(&taken) else { continue };
        taken.insert(letter.clone());
        letters.insert(param.name.clone(), letter.clone());
        param.ty = letter.clone();
        added.push(TypeParam {
            name: letter,
            is_comptime: false,
            comptime_type: None,
            bounds,
        });
    }
    if added.is_empty() {
        return;
    }

    if f.ret_ty.is_none() {
        // Keyed by parameter name and answered in terms of it, so the letter
        // goes in only now that one has been handed out.
        if let Some((name, answer)) = returns.iter().next() {
            f.ret_ty = Some(match letters.get(name) {
                Some(letter) if answer == name => letter.clone(),
                _ => answer.clone(),
            });
        }
    }
    f.type_params.extend(added);
}

/// The parameter's own name for a `Same`, so the letter can be substituted once
/// one is assigned; a real type name otherwise.
fn shape_name(shape: Shape, param: &str) -> String {
    match shape {
        Shape::Same | Shape::OpenLiteral => param.to_string(),
        Shape::Bool => "bool".to_string(),
    }
}

/// The bounds `name`'s uses require, or `None` when a use pins the type or says
/// something this pass can't name a trait for.
fn bounds_from_body(body: &[Stmt], name: &str) -> Option<Vec<String>> {
    let mut traits: Vec<String> = Vec::new();
    let mut covered: HashSet<NodeId> = HashSet::new();
    let mut ok = true;

    // Every expression that is entirely made of this parameter, open literals
    // and operators. Nested ones are classified too and add nothing new: the
    // bounds are a union, and the node set is a set.
    rask_ast::visit::walk_body(body, &mut |expr| {
        if shape_of(expr, name, &mut traits).is_none() {
            return;
        }
        if !mentions(expr, name) {
            return;
        }
        rask_ast::visit::walk_expr(expr, &mut |inner| {
            covered.insert(inner.id);
        });
    });

    if traits.is_empty() {
        return None;
    }

    // A use outside all of those might pin the type. Standing alone is the one
    // exception — `return x`, `let y = x` — and those are node ids rather than
    // shapes, because `walk_body` hands over expressions without saying where
    // they sit.
    let bare = bare_use_ids(body, name);
    rask_ast::visit::walk_body(body, &mut |expr| {
        if let ExprKind::Ident(id) = &expr.kind {
            if id == name && !covered.contains(&expr.id) && !bare.contains(&expr.id) {
                ok = false;
            }
        }
    });

    if ok {
        Some(traits)
    } else {
        None
    }
}

/// What `expr` answers when it is built only out of `name`, unsuffixed numeric
/// literals and operators. `None` for anything else — including an expression
/// that mentions a *different* name, which could be any type.
///
/// Traits the operators need are pushed onto `traits` as they are met. A `None`
/// answer can leave some behind; the caller only reads them for an expression
/// that classified.
fn shape_of(expr: &Expr, name: &str, traits: &mut Vec<String>) -> Option<Shape> {
    match &expr.kind {
        ExprKind::Ident(id) if id == name => Some(Shape::Same),
        // Still open, which is what lets `x * 2` mean `x * 2.0` for an `f64`.
        ExprKind::Int(_, None) | ExprKind::Float(_, None) => Some(Shape::OpenLiteral),
        ExprKind::Unary { op: UnaryOp::Neg, operand } => {
            match shape_of(operand, name, traits)? {
                Shape::Same => {
                    note(traits, "Numeric");
                    Some(Shape::Same)
                }
                Shape::OpenLiteral => Some(Shape::OpenLiteral),
                Shape::Bool => None,
            }
        }
        ExprKind::Binary { op, left, right } => {
            let l = shape_of(left, name, traits)?;
            let r = shape_of(right, name, traits)?;
            if l == Shape::Bool || r == Shape::Bool {
                return None;
            }
            if l == Shape::OpenLiteral && r == Shape::OpenLiteral {
                // Says nothing about the parameter either way.
                return Some(Shape::OpenLiteral);
            }
            let bound = bound_for(*op)?;
            note(traits, bound);
            match bound {
                "Numeric" => Some(Shape::Same),
                _ => Some(Shape::Bool),
            }
        }
        _ => None,
    }
}

fn note(traits: &mut Vec<String>, bound: &str) {
    if !traits.iter().any(|t| t == bound) {
        traits.push(bound.to_string());
    }
}

/// The trait an operator needs of its operands.
///
/// Only the ones whose bound is unambiguous. `&`, `|`, `<<` and friends are
/// integer-only in practice but have no trait to name, and `and`/`or` are
/// `bool`, which is a concrete type rather than a bound.
fn bound_for(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => Some("Numeric"),
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => Some("Comparable"),
        BinOp::Eq | BinOp::Ne => Some("Equal"),
        _ => None,
    }
}

/// The one answer every `return` in the body gives, or `None` when they
/// disagree or one of them isn't readable.
fn returned_shape(body: &[Stmt], name: &str) -> Option<Shape> {
    let mut answer: Option<Shape> = None;
    let mut ok = true;
    let mut ignored = Vec::new();
    walk_stmts(body, &mut |stmt| {
        let StmtKind::Return(Some(e)) = &stmt.kind else { return };
        match shape_of(e, name, &mut ignored) {
            Some(shape) => match answer {
                Some(have) if have != shape => ok = false,
                _ => answer = Some(shape),
            },
            None => ok = false,
        }
    });
    if ok {
        answer
    } else {
        None
    }
}

fn mentions(expr: &Expr, name: &str) -> bool {
    let mut found = false;
    rask_ast::visit::walk_expr(expr, &mut |inner| {
        if matches!(&inner.kind, ExprKind::Ident(id) if id == name) {
            found = true;
        }
    });
    found
}

/// The node ids at which `name` appears on its own, where standing alone is all
/// it does: returned, or the whole right-hand side of a binding.
fn bare_use_ids(body: &[Stmt], name: &str) -> HashSet<NodeId> {
    let mut ids = HashSet::new();
    let mut note_if = |e: &Expr| {
        if matches!(&e.kind, ExprKind::Ident(id) if id == name) {
            ids.insert(e.id);
        }
    };
    walk_stmts(body, &mut |stmt| match &stmt.kind {
        StmtKind::Return(Some(e)) => note_if(e),
        StmtKind::Let { init, .. } | StmtKind::Mut { init, .. } => note_if(init),
        _ => {}
    });
    ids
}

/// Does any `return` carry a value? A function with none has no return type to
/// write down, and nothing infers one for it either.
fn returns_a_value(body: &[Stmt]) -> bool {
    let mut found = false;
    walk_stmts(body, &mut |stmt| {
        if matches!(&stmt.kind, StmtKind::Return(Some(_))) {
            found = true;
        }
    });
    found
}

/// Names a closure inside the body binds, which shadow anything outer.
fn shadowed_names(body: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    rask_ast::visit::walk_body(body, &mut |expr| {
        if let ExprKind::Closure { params, .. } = &expr.kind {
            for p in params {
                names.insert(p.name.clone());
            }
        }
    });
    names
}

/// Single letters the signature already spells, so a fresh one can't collide
/// with a type parameter another parameter's type mentions (PC1).
fn letters_already_in(f: &FnDecl) -> HashSet<String> {
    let mut taken = HashSet::new();
    let mut scan = |s: &str| {
        for part in s.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if part.len() == 1 && part.chars().all(|c| c.is_ascii_uppercase()) {
                taken.insert(part.to_string());
            }
        }
    };
    for p in &f.params {
        scan(&p.ty);
    }
    if let Some(r) = &f.ret_ty {
        scan(r);
    }
    for c in &f.context_clauses {
        scan(&c.ty);
    }
    taken
}

fn free_letter(taken: &HashSet<String>) -> Option<String> {
    "TUVWXYZABCDEFGHIJKLMNOPQRS"
        .chars()
        .map(|c| c.to_string())
        .find(|l| !taken.contains(l))
}

/// Every statement in the body, nested blocks included. `walk_body` visits
/// expressions; this visits statements, which is where `return` and a binding
/// live.
fn walk_stmts(body: &[Stmt], f: &mut impl FnMut(&Stmt)) {
    for stmt in body {
        f(stmt);
        rask_ast::visit::walk_stmt(stmt, &mut |expr| {
            if let ExprKind::Block(inner) = &expr.kind {
                walk_stmts(inner, f);
            }
        });
    }
}

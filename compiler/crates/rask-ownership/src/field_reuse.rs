// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Which assignments build their new value out of the old one.
//!
//! Writing over a place releases what it held, except when the new value was
//! made from it:
//!
//! ```text
//! self.list = More(h, Heap(self.list))              // old list is the tail
//! self.bag = Bag { tag: self.bag.tag.clone(), … }   // old bag is garbage
//! ```
//!
//! Releasing the first frees what the new value points at. Lowering asks this
//! set which is which. An assignment is in it when something in its value
//! takes ownership of the place, of part of it, or of something holding it:
//! a `take` argument or receiver, a struct, tuple or array literal's element, a
//! variant's payload, `Heap(x)`, a channel send, or the value itself.
//!
//! The other half is an assignment that refills a place an earlier statement
//! in the same block moved out of — mem.parameters/PM7's consume-and-replace:
//!
//! ```text
//! let old = self.memtable          // the old memtable is `old`'s now
//! self.memtable = Memtable.new()   // nothing left in the slot to release
//! ```
//!
//! Releasing there freed what `old` holds (#1496). The
//! field read in the `let` is in the set too: lowering reads it as a move, so
//! the frame owns `old` and frees what it holds.
//!
//! It is a fact lowering needs about every body it compiles, the stdlib's
//! included, so it is worked out on its own and reports nothing. Whether a
//! body follows the ownership rules is the checker's question, asked of the
//! program on every compile and of the stdlib by its own test.

use std::collections::HashSet;

use rask_ast::decl::Decl;
use rask_ast::expr::{Expr, ExprKind, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{self, Visit};
use rask_ast::NodeId;
use rask_types::{ParamMode, TypedProgram};

use crate::OwnershipChecker;

/// The assignments in `bodies` whose value takes the place they write.
/// `signatures` are read for `take` parameters, the same as the checker reads
/// them.
pub fn field_reuses(program: &TypedProgram, bodies: &[&[Decl]], signatures: &[&[Decl]]) -> HashSet<NodeId> {
    let mut checker = OwnershipChecker::new(program);
    for decls in signatures {
        checker.collect_signatures(decls);
    }
    let mut finder = Finder { checker: &checker, found: HashSet::new() };
    for decls in bodies {
        for decl in decls.iter() {
            visit::visit_decl(decl, &mut finder);
            for body in decl_bodies(decl) {
                finder.refills_in(body);
                visit::walk_body(body, &mut |e| {
                    if let ExprKind::Block(stmts) = &e.kind {
                        finder.refills_in(stmts);
                    }
                });
            }
        }
    }
    finder.found
}

/// The statement lists a declaration holds directly.
fn decl_bodies(decl: &Decl) -> Vec<&[Stmt]> {
    use rask_ast::decl::DeclKind;
    match &decl.kind {
        DeclKind::Fn(f) => vec![&f.body[..]],
        DeclKind::Struct(s) => s.methods.iter().map(|m| &m.body[..]).collect(),
        DeclKind::Enum(e) => e.methods.iter().map(|m| &m.body[..]).collect(),
        DeclKind::Interface(t) => t.methods.iter().map(|m| &m.body[..]).collect(),
        DeclKind::Impl(i) => i.methods.iter().map(|m| &m.body[..]).collect(),
        DeclKind::Test(t) => vec![&t.body[..]],
        DeclKind::Benchmark(b) => vec![&b.body[..]],
        _ => Vec::new(),
    }
}

struct Finder<'c, 'a> {
    checker: &'c OwnershipChecker<'a>,
    found: HashSet<NodeId>,
}

impl<'c, 'a, 'e> Visit<'e> for Finder<'c, 'a> {
    fn expr(&mut self, _expr: &'e Expr) -> bool {
        true
    }

    fn stmt(&mut self, stmt: &'e Stmt) -> bool {
        if let StmtKind::Assign { target, value, .. } = &stmt.kind {
            if let Some(place) = place_path(target) {
                if self.takes(value, &place) {
                    self.found.insert(stmt.id);
                }
            }
        }
        true
    }
}

impl Finder<'_, '_> {
    /// Assignments in one statement list that refill a field an earlier `let`
    /// of the same list moved out of, and the field reads that moved them.
    /// Only the same list: a move inside a branch leaves the slot full on the
    /// other path, and that one still has to be released.
    ///
    /// Only a `let`'s move. A field handed to a `take` parameter is released
    /// at the refill as before; that convention is the callee's, and a
    /// whole-variable `b = …` after `eat(b)` relies on it.
    fn refills_in(&mut self, stmts: &[Stmt]) {
        // Each field a `let` moved out of, with the read that moved it.
        let mut emptied: Vec<(Vec<String>, rask_ast::NodeId)> = Vec::new();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Assign { target, .. } => {
                    if let Some(place) = place_path(target) {
                        if let Some(at) = emptied.iter().position(|(p, _)| *p == place) {
                            self.found.insert(stmt.id);
                            self.found.insert(emptied[at].1);
                            emptied.remove(at);
                        }
                    }
                }
                StmtKind::Let { init, .. } | StmtKind::Mut { init, .. } => {
                    if let (Some(place), ExprKind::Field { .. }) = (place_path(init), &init.kind) {
                        emptied.push((place, init.id));
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether `value`, or anything in it, takes ownership of `place`.
    fn takes(&self, value: &Expr, place: &[String]) -> bool {
        if overlaps(value, place) {
            return true;
        }
        let mut hit = false;
        visit::walk_expr(value, &mut |e| {
            if !hit && self.owned_children(e).iter().any(|c| overlaps(c, place)) {
                hit = true;
            }
        });
        hit
    }

    /// The direct parts of `e` it takes ownership of.
    fn owned_children<'e>(&self, e: &'e Expr) -> Vec<&'e Expr> {
        let c = self.checker;
        match &e.kind {
            ExprKind::StructLit { fields, spread, .. } => {
                fields.iter().map(|f| &f.value).chain(spread.as_deref()).collect()
            }
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => elems.iter().collect(),
            ExprKind::Unary { op: UnaryOp::Heap, operand } => vec![operand.as_ref()],
            ExprKind::Call { func, args } => {
                let takes = match func.name() {
                    Some("drop") => Some(vec![true]),
                    Some(name) => c.fn_take_params.get(name).cloned(),
                    None => None,
                };
                let Some(takes) = takes else { return Vec::new() };
                args.iter()
                    .enumerate()
                    .filter(|(i, _)| takes.get(*i).copied().unwrap_or(false))
                    .map(|(_, a)| &a.expr)
                    .collect()
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                if c.names_a_variant(object, method) {
                    return args.iter().map(|a| &a.expr).collect();
                }
                let modes = c.method_param_modes(object, method);
                let send = c.is_channel_send(object, method, e.span);
                let mut out: Vec<&Expr> = args
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        (send && *i == 0)
                            || matches!(modes.as_ref().and_then(|m| m.get(*i)), Some(ParamMode::Take))
                    })
                    .map(|(_, a)| &a.expr)
                    .collect();
                if c.is_take_self_method(object, method) {
                    out.push(object.as_ref());
                }
                out
            }
            _ => Vec::new(),
        }
    }
}

/// The place an expression names, as root and field names: `self.list` is
/// `["self", "list"]`. Every element of a collection is one `[]`, so `v[i]`
/// and `v[j]` count as the same place. `None` for anything else.
fn place_path(expr: &Expr) -> Option<Vec<String>> {
    match &expr.kind {
        ExprKind::Ident(name) => Some(vec![name.clone()]),
        ExprKind::Field { object, field } => {
            let mut p = place_path(object)?;
            p.push(field.clone());
            Some(p)
        }
        ExprKind::Index { object, .. } => {
            let mut p = place_path(object)?;
            p.push("[]".to_string());
            Some(p)
        }
        _ => None,
    }
}

/// `e` names `place`, something inside it, or something holding it.
fn overlaps(e: &Expr, place: &[String]) -> bool {
    let Some(path) = place_path(e) else { return false };
    let n = path.len().min(place.len());
    path[..n] == place[..n]
}

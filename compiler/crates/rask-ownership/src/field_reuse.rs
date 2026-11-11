// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Which assignments build their new value out of the old one.
//!
//! Writing over a place releases what it held, except when the new value was
//! made from it:
//!
//! ```text
//! self.list = More(h, Heap(self.list))              // old list is the tail
//! self.bag = Bag(tag: self.bag.tag.clone(), …)   // old bag is garbage
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
use rask_ast::ty::ParamMode;
use rask_check::TypedProgram;

use crate::OwnershipChecker;

/// The assignments in `bodies` whose value takes the place they write.
/// `signatures` are read for `take` parameters, the same as the checker reads
/// them.
pub fn field_reuses(program: &TypedProgram, bodies: &[&[Decl]], signatures: &[&[Decl]]) -> HashSet<NodeId> {
    let mut checker = OwnershipChecker::new(program);
    for decls in signatures {
        checker.collect_signatures(decls);
    }
    checker.infer_replaces(signatures);
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
        for (take, refill) in refill_pairs(self.checker, stmts) {
            self.found.insert(take);
            self.found.insert(refill);
        }
    }

    /// Whether `value`, or anything in it, takes ownership of `place`.
    fn takes(&self, value: &Expr, place: &[String]) -> bool {
        if self.moves(value, place) {
            return true;
        }
        let mut hit = false;
        visit::walk_expr(value, &mut |e| {
            if !hit && self.owned_children(e).iter().any(|c| self.moves(c, place)) {
                hit = true;
            }
        });
        hit
    }

    /// `e` names `place` (or a part of it, or something holding it) and
    /// moves it. A Copy part is copied: `Cursor(at: c.at, …)` takes nothing
    /// from `c`, and the old `c` still has everything to release.
    fn moves(&self, e: &Expr, place: &[String]) -> bool {
        overlaps(e, place)
            && !self.checker.node_ty(&e.id).is_some_and(|t| self.checker.is_copy(t))
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
                let modes = c.method_param_modes(e.id, object, method);
                let mut out: Vec<&Expr> = args
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| {
                        matches!(modes.as_ref().and_then(|m| m.get(*i)), Some(ParamMode::Take))
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

/// The `let x = p.f` reads in one statement list that a later `p.f = …` in
/// the same list refills, as (the read, the refill). Such a read is a move:
/// the slot is empty from the read to the refill, and `x` owns what it held.
///
/// Only when nothing between them, the refill included, can leave the list
/// early: `return`, `break`, `continue` or `try`. On that path the refill never
/// runs, so the read has to stay a view of a slot that's still full, and the
/// checker holds it to that (mem.borrowing/S5). A Copy field is copied, never
/// emptied.
pub(crate) fn refill_pairs(checker: &OwnershipChecker<'_>, stmts: &[Stmt]) -> Vec<(NodeId, NodeId)> {
    let moves = |e: &Expr| !checker.node_ty(&e.id).is_some_and(|t| checker.is_copy(t));
    let mut pairs = Vec::new();
    // Each field a `let` moved out of, with the read that moved it.
    let mut emptied: Vec<(Vec<String>, NodeId)> = Vec::new();
    for stmt in stmts {
        if may_leave_early(stmt) {
            emptied.clear();
            continue;
        }
        // `a = b` moves `b` out as well as refilling `a`: `let t = a; a = b;
        // b = t` swaps two `mutate` parameters, and `b = t` refills a slot
        // `a = b` emptied. Releasing there freed what `a` holds now (#1877).
        let moved_by_assign = match &stmt.kind {
            StmtKind::Assign { target, value, .. }
                if place_path(value).is_some() && moves(value) && place_path(value) != place_path(target) =>
            {
                place_path(value).map(|p| (p, value.id))
            }
            _ => None,
        };
        match &stmt.kind {
            StmtKind::Assign { target, .. } if place_path(target).is_some_and(|p| emptied.iter().any(|(e, _)| *e == p)) => {
                let place = place_path(target).unwrap_or_default();
                if let Some(at) = emptied.iter().position(|(p, _)| *p == place) {
                    pairs.push((emptied[at].1, stmt.id));
                    emptied.remove(at);
                }
            }
            // A whole binding moves the same way a field does: `let t = a` on
            // a `mutate a` empties the caller's slot until `a = …` refills it.
            StmtKind::Let { init, .. } | StmtKind::Mut { init, .. }
                if matches!(init.kind, ExprKind::Field { .. } | ExprKind::Ident(_))
                    && place_path(init).is_some()
                    && moves(init) =>
            {
                // A second read of the same field reads the emptied slot; the
                // latest read is the one the refill pairs with.
                let place = place_path(init).unwrap_or_default();
                emptied.retain(|(p, _)| *p != place);
                emptied.push((place, init.id));
            }
            _ => {
                // A closure called here may read the field while it's empty,
                // and which fields it reads isn't known here.
                if calls_a_closure(checker, stmt) {
                    emptied.clear();
                    continue;
                }
                // Anything else that writes the field, or the value around it,
                // breaks the pair: what the refill would put back went first.
                let written = places_written(checker, stmt);
                emptied.retain(|(p, _)| {
                    !written.iter().any(|w| {
                        let n = w.len().min(p.len());
                        w[..n] == p[..n]
                    })
                });
            }
        }
        if let Some((place, read)) = moved_by_assign {
            emptied.retain(|(p, _)| *p != place);
            emptied.push((place, read));
        }
        // A whole binding handed to a `take` parameter is moved out the same
        // way: `keep.push(a); a = [mk(10), mk(11)]` on a `mutate a` refills a
        // slot the push emptied, and releasing there freed what the vector
        // holds. Only whole bindings: a field handed to `take` keeps the
        // convention described above.
        for (place, read) in bindings_taken_by_calls(checker, stmt, &moves) {
            emptied.retain(|(p, _)| *p != place);
            emptied.push((place, read));
        }
    }
    pairs
}

/// Whole bindings `stmt` hands to a `take` parameter of a call, with the
/// argument that moved each.
fn bindings_taken_by_calls(
    checker: &OwnershipChecker<'_>,
    stmt: &Stmt,
    moves: &dyn Fn(&Expr) -> bool,
) -> Vec<(Vec<String>, NodeId)> {
    let mut out = Vec::new();
    visit::walk_stmt(stmt, &mut |e| {
        let taken: Vec<&Expr> = match &e.kind {
            ExprKind::Call { func, args } => {
                let Some(takes) = func.name().and_then(|n| checker.fn_take_params.get(n)) else { return };
                args.iter()
                    .enumerate()
                    .filter(|(i, _)| takes.get(*i).copied().unwrap_or(false))
                    .map(|(_, a)| &a.expr)
                    .collect()
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                let modes = checker.method_param_modes(e.id, object, method);
                args.iter()
                    .enumerate()
                    .filter(|(i, _)| matches!(modes.as_ref().and_then(|m| m.get(*i)), Some(ParamMode::Take)))
                    .map(|(_, a)| &a.expr)
                    .collect()
            }
            _ => return,
        };
        for arg in taken {
            if let ExprKind::Ident(name) = &arg.kind {
                if moves(arg) {
                    out.push((vec![name.clone()], arg.id));
                }
            }
        }
    });
    out
}

/// Whether `stmt` calls through a local or a parameter, a closure value, or
/// hands one to a call that can.
fn calls_a_closure(checker: &OwnershipChecker<'_>, stmt: &Stmt) -> bool {
    let mut hit = false;
    visit::walk_stmt(stmt, &mut |e| {
        // A closure value handed to a call can be called by it.
        let args = match &e.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => &args[..],
            _ => &[],
        };
        if args
            .iter()
            .any(|a| matches!(checker.node_ty(&a.expr.id), Some(rask_types::Type::Fn { .. })))
        {
            hit = true;
        }
        if let ExprKind::Call { func, .. } = &e.kind {
            if let ExprKind::Ident(_) = &func.kind {
                let local = checker
                    .program
                    .resolutions
                    .get(&func.id)
                    .and_then(|s| checker.program.symbols.get(*s))
                    .is_some_and(|sym| {
                        matches!(
                            sym.kind,
                            rask_resolve::SymbolKind::Variable { .. } | rask_resolve::SymbolKind::Parameter { .. }
                        )
                    });
                hit |= local;
            }
        }
    });
    hit
}

/// The places `stmt` may write over: assignment targets anywhere in it, and
/// each path a call in it may replace under a place it writes through
/// (`OwnershipChecker::call_writes`).
fn places_written(checker: &OwnershipChecker<'_>, stmt: &Stmt) -> Vec<Vec<String>> {
    struct Writes<'c, 'a> {
        checker: &'c OwnershipChecker<'a>,
        out: Vec<Vec<String>>,
    }
    impl<'e> Visit<'e> for Writes<'_, '_> {
        fn expr(&mut self, e: &'e Expr) -> bool {
            if let ExprKind::Take { place } = &e.kind {
                self.out.extend(self.written(place, Vec::new()));
            }
            for (place, under) in self.checker.call_writes(e) {
                self.out.extend(self.written(place, under));
            }
            true
        }
        fn stmt(&mut self, s: &'e Stmt) -> bool {
            if let StmtKind::Assign { target, .. } = &s.kind {
                self.out.extend(self.written(target, Vec::new()));
            }
            true
        }
    }
    impl Writes<'_, '_> {
        /// As a path `place_path` spells, so it compares with a `let`'s.
        fn written(&self, place: &Expr, under: Vec<String>) -> Option<Vec<String>> {
            let (root, mut p) = self.checker.written_place(place)?;
            p.insert(0, root);
            p.extend(under);
            Some(p)
        }
    }
    let mut w = Writes { checker, out: Vec::new() };
    visit::visit_stmt(stmt, &mut w);
    w.out
}

/// Whether `stmt` can leave the statement list it's in before finishing: a
/// `return` or `try` anywhere in it outside a closure, or a `break` or
/// `continue` that isn't aimed at a loop inside it. A labeled one inside a
/// loop counts, since it may name a loop further out.
fn may_leave_early(stmt: &Stmt) -> bool {
    struct Exits {
        hit: bool,
        in_loop: bool,
    }
    impl Exits {
        fn body(&mut self, body: &[Stmt]) {
            let mut inner = Exits { hit: false, in_loop: true };
            visit::visit_body(body, &mut inner);
            self.hit |= inner.hit;
        }
    }
    impl<'e> Visit<'e> for Exits {
        fn expr(&mut self, e: &'e Expr) -> bool {
            match &e.kind {
                ExprKind::Closure { .. } => false,
                ExprKind::Try { .. } => {
                    self.hit = true;
                    false
                }
                ExprKind::Loop { body, .. } => {
                    self.body(body);
                    false
                }
                _ => !self.hit,
            }
        }
        fn stmt(&mut self, s: &'e Stmt) -> bool {
            match &s.kind {
                StmtKind::Return(_) => self.hit = true,
                StmtKind::Break { label, .. } | StmtKind::Continue(label)
                    if !self.in_loop || label.is_some() =>
                {
                    self.hit = true
                }
                StmtKind::Break { .. } | StmtKind::Continue(_) => {}
                StmtKind::While { cond, body, .. } => {
                    visit::visit_expr(cond, self);
                    self.body(body);
                    return false;
                }
                StmtKind::WhileLet { expr, body, .. } => {
                    visit::visit_expr(expr, self);
                    self.body(body);
                    return false;
                }
                StmtKind::Loop { body, .. } => {
                    self.body(body);
                    return false;
                }
                StmtKind::For { iter, body, .. } | StmtKind::ComptimeFor { iter, body, .. } => {
                    visit::visit_expr(iter, self);
                    self.body(body);
                    return false;
                }
                _ => {}
            }
            !self.hit
        }
    }
    let mut v = Exits { hit: false, in_loop: false };
    visit::visit_stmt(stmt, &mut v);
    v.hit
}

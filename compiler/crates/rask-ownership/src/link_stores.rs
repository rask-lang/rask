// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Fork blocks that share a rack for reading (mem.racks/RK16).
//!
//! Following a link and reading a node write nothing, so several blocks may
//! read one rack at once. Two things do write the rack's shared state, and
//! the blocks may do neither: a write through a link or a rack (a node field,
//! an insert, a delete), and storing a link anywhere, since every stored link
//! is recorded with its rack so a delete can find it (RK15). The one place a
//! block may put a link is a `Staged` buffer, whose links aren't edges.
//!
//! A store can happen in a function the block calls, so each function gets a
//! summary: may it store a link. A loop over a list of links registers the
//! list unless no delete can happen while it runs; such a loop is "quiet",
//! and lowering walks it without registering anything.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{visit, Span};
use rask_types::Type;

use crate::replaces::Key;
use crate::{OwnershipChecker, OwnershipError, OwnershipErrorKind};

/// Per function and method: whether calling it may store a link.
pub(crate) type LinkStores = HashMap<Key, bool>;

/// How deep a closure literal called from a closure literal is followed.
const MAX_DEPTH: u32 = 8;

impl<'a> OwnershipChecker<'a> {
    /// Fill `link_stores` for every function and method in `groups`, to a
    /// fixed point: every body starts out storing nothing, and a call to one
    /// that stores makes its caller store too.
    pub(crate) fn infer_link_stores(&mut self, groups: &[&[Decl]]) {
        let mut bodies: Vec<(Key, &FnDecl)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
                match &decl.kind {
                    DeclKind::Fn(f) if !f.body_lives_elsewhere() => bodies.push((Key::Fn(f.name.clone()), f)),
                    DeclKind::Impl(i) => {
                        let ty = i.target_ty.name().unwrap_or_default();
                        for m in i.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(ty.clone(), m.name.clone()), m));
                        }
                    }
                    DeclKind::Struct(st) => {
                        for m in st.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(st.name.clone(), m.name.clone()), m));
                        }
                    }
                    DeclKind::Enum(en) => {
                        for m in en.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(en.name.clone(), m.name.clone()), m));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (key, _) in &bodies {
            self.link_stores.insert(key.clone(), false);
        }
        // A store only ever turns a summary on, so this climbs to the answer
        // in at most one round per function.
        loop {
            let mut changed = false;
            for (key, f) in &bodies {
                if self.link_stores.get(key) == Some(&true) {
                    continue;
                }
                if self.first_link_store(&f.body, 0).is_some() {
                    self.link_stores.insert(key.clone(), true);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Does this type, here, hold a link a store would record? A type
    /// parameter might, so it counts.
    fn holds_link_here(&self, ty: &Type) -> bool {
        self.program.types.holds_registered_link(ty) || self.mentions_type_param(ty)
    }

    fn holds_rack_here(&self, ty: &Type) -> bool {
        self.program.types.holds_rack(ty) || self.mentions_type_param(ty)
    }

    fn mentions_type_param(&self, ty: &Type) -> bool {
        ty.mentions_type_param(&|n| self.type_params_in_scope.iter().any(|p| p == n))
    }

    fn expr_holds_link(&self, e: &Expr) -> bool {
        self.node_ty(&e.id).is_some_and(|t| self.holds_link_here(t))
    }

    /// Can a callee handed this get at links: it holds one, or a rack it can
    /// read them out of.
    fn expr_reaches_links(&self, e: &Expr) -> bool {
        self.node_ty(&e.id).is_some_and(|t| self.holds_link_here(t) || self.holds_rack_here(t))
    }

    /// The first place in `body` that stores a link, with a phrase for it.
    pub(crate) fn first_link_store(&self, body: &[Stmt], depth: u32) -> Option<(Span, String)> {
        let mut all = Vec::new();
        Self::flatten_stmts(body, &mut all);
        let mut found: Option<(Span, String)> = None;
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| {
                if found.is_none() {
                    all.extend(Self::own_stmts(e));
                }
            });
        }
        for stmt in &all {
            if let Some(hit) = self.stmt_stores_link(stmt) {
                return Some(hit);
            }
        }
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| {
                if found.is_none() {
                    found = self.expr_stores_link(e, depth);
                }
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn stmt_stores_link(&self, stmt: &Stmt) -> Option<(Span, String)> {
        match &stmt.kind {
            // A local link is a root the compiler guards; anything else holding
            // one is storage the rack keeps a record for.
            StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => {
                let ty = self.node_ty(&init.id)?;
                (self.holds_link_here(ty) && !self.is_link_type(ty))
                    .then(|| (stmt.span, format!("`{name}` holds a link")))
            }
            StmtKind::Assign { target, value, .. } => {
                if !self.expr_holds_link(value) {
                    return None;
                }
                let bare_local = matches!(target.kind, ExprKind::Ident(_))
                    && self.node_ty(&target.id).is_some_and(|t| self.is_link_type(t));
                (!bare_local).then(|| (stmt.span, "this stores a link".to_string()))
            }
            StmtKind::For { iter, body, .. } => {
                let elem_links = self.expr_holds_link(iter);
                (elem_links && !self.loop_is_quiet(body))
                    .then(|| (iter.span, "this loop records the list it walks, since its body may delete".to_string()))
            }
            _ => None,
        }
    }

    /// A call that may store a link it's handed.
    fn expr_stores_link(&self, e: &Expr, depth: u32) -> Option<(Span, String)> {
        match &e.kind {
            ExprKind::MethodCall { object, method, args, .. } => {
                let recv = self.receiver_type_name(object);
                if recv.as_deref() == Some("Staged") {
                    return None;
                }
                let handed: Vec<&Expr> =
                    std::iter::once(object.as_ref()).chain(args.iter().map(|a| &a.expr)).collect();
                if !handed.iter().any(|a| self.expr_reaches_links(a)) {
                    return None;
                }
                let key = recv.as_ref().map(|ty| Key::Method(ty.clone(), method.clone()));
                if let Some(stores) = key.as_ref().and_then(|k| self.link_stores.get(k)) {
                    return stores.then(|| (e.span, format!("`{method}` may store a link")));
                }
                // No body to read: a stdlib method keeps what it takes.
                let qualified = recv.map(|ty| format!("{ty}_{method}")).unwrap_or_default();
                for (i, a) in args.iter().enumerate() {
                    let given = rask_stdlib::mir_metadata::argument_mode(&qualified, i + 1)
                        == Some(rask_stdlib::mir_metadata::ArgMode::Given);
                    if given && self.expr_holds_link(&a.expr) {
                        return Some((a.expr.span, format!("`{method}` keeps this link")));
                    }
                }
                None
            }
            ExprKind::Call { func, args } => {
                if !args.iter().any(|a| self.expr_reaches_links(&a.expr)) {
                    return None;
                }
                if let Some(key) = self.fn_key(func) {
                    if let Some(stores) = self.link_stores.get(&key) {
                        let name = match &key {
                            Key::Fn(n) | Key::Method(_, n) => n.clone(),
                        };
                        return stores.then(|| (e.span, format!("`{name}` may store a link")));
                    }
                    // Bodiless: what it takes, it keeps.
                    let name = match &key {
                        Key::Fn(n) | Key::Method(_, n) => n.clone(),
                    };
                    for (i, a) in args.iter().enumerate() {
                        let given = rask_stdlib::mir_metadata::argument_mode(&name, i)
                            == Some(rask_stdlib::mir_metadata::ArgMode::Given);
                        if given && self.expr_holds_link(&a.expr) {
                            return Some((a.expr.span, format!("`{name}` keeps this link")));
                        }
                    }
                    return None;
                }
                // A closure literal the body can see: what it does, the call
                // does.
                if let ExprKind::Ident(name) = &func.kind {
                    if let Some(lit) = self.closure_literals.get(name) {
                        if depth < MAX_DEPTH {
                            if let ExprKind::Closure { body, .. } = &lit.kind {
                                let stmts = std::slice::from_ref(body.as_ref());
                                return self.first_store_in_expr(stmts, depth + 1).map(|(_, why)| (e.span, why));
                            }
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// `first_link_store` over expressions rather than statements: a closure
    /// body is an expression.
    fn first_store_in_expr(&self, exprs: &[Expr], depth: u32) -> Option<(Span, String)> {
        let mut stmts: Vec<&Stmt> = Vec::new();
        let mut found = None;
        for e in exprs {
            visit::walk_expr(e, &mut |x| stmts.extend(Self::own_stmts(x)));
        }
        for st in stmts {
            if let Some(hit) = self.stmt_stores_link(st) {
                return Some(hit);
            }
        }
        for e in exprs {
            visit::walk_expr(e, &mut |x| {
                if found.is_none() {
                    found = self.expr_stores_link(x, depth);
                }
            });
        }
        found
    }

    /// The places these statements write: assignment targets, `for mutate`
    /// sources, `mutate self` and `take self` receivers, `mutate` and
    /// `deleting` arguments, and a `with` source whose binding is written.
    pub(crate) fn written_places<'e>(&self, body: &'e [Stmt]) -> Vec<&'e Expr> {
        let mut out: Vec<&'e Expr> = Vec::new();
        let mut all = Vec::new();
        Self::flatten_stmts(body, &mut all);
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| all.extend(Self::own_stmts(e)));
        }
        for st in &all {
            match &st.kind {
                StmtKind::Assign { target, .. } => out.push(target),
                StmtKind::For { mutate: true, iter, .. } => out.push(iter),
                _ => {}
            }
        }
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| match &e.kind {
                ExprKind::MethodCall { object, method, args, .. } => {
                    if self.is_mutate_self_method(object, method) || self.is_take_self_method(object, method) {
                        out.push(object);
                    }
                    for a in args {
                        if matches!(a.mode, ArgMode::Mutate | ArgMode::Deleting) {
                            out.push(&a.expr);
                        }
                    }
                }
                ExprKind::Call { args, .. } => {
                    for a in args {
                        if matches!(a.mode, ArgMode::Mutate | ArgMode::Deleting) {
                            out.push(&a.expr);
                        }
                    }
                }
                ExprKind::WithAs { bindings, body } => {
                    for b in bindings {
                        if self.body_writes_name(body, &b.name) {
                            out.push(&b.source);
                        }
                    }
                }
                _ => {}
            });
        }
        out
    }

    /// The expression at the root of a place: `p` for `p.a[i].b`.
    fn place_root(e: &Expr) -> &Expr {
        match &e.kind {
            ExprKind::Field { object, .. }
            | ExprKind::OptionalField { object, .. }
            | ExprKind::Index { object, .. } => Self::place_root(object),
            _ => e,
        }
    }

    fn root_holds(&self, place: &Expr, test: impl Fn(&Self, &Type) -> bool) -> bool {
        let root = Self::place_root(place);
        // `n.hp = 1` through a link writes the node; the place's own type is
        // the field's, so the root and every step to it are asked.
        let mut step = place;
        loop {
            if self.node_ty(&step.id).is_some_and(|t| test(self, t)) {
                return true;
            }
            if std::ptr::eq(step, root) {
                return false;
            }
            step = match &step.kind {
                ExprKind::Field { object, .. }
                | ExprKind::OptionalField { object, .. }
                | ExprKind::Index { object, .. } => object,
                _ => return false,
            };
        }
    }

    /// No delete can happen while this loop body runs: it writes nothing that
    /// holds a rack, calls no function value it can't see into, and calls no
    /// closure literal that does either. So a list of links it walks can't
    /// lose an entry, and needs neither recording nor a cursor (RK15).
    pub(crate) fn loop_is_quiet(&self, body: &[Stmt]) -> bool {
        self.quiet_at(body, 0)
    }

    fn quiet_at(&self, body: &[Stmt], depth: u32) -> bool {
        if self.written_places(body).iter().any(|p| self.root_holds(p, |s, t| s.holds_rack_here(t))) {
            return false;
        }
        let mut quiet = true;
        for stmt in body {
            visit::walk_stmt(stmt, &mut |e| {
                if !quiet {
                    return;
                }
                let ExprKind::Call { func, .. } = &e.kind else { return };
                if self.fn_key(func).is_some() {
                    return;
                }
                if !matches!(self.node_ty(&func.id), Some(Type::Fn { .. })) {
                    return;
                }
                match &func.kind {
                    ExprKind::Ident(name) if self.named_fn_values.contains(name) => {}
                    ExprKind::Ident(name) => match self.closure_literals.get(name) {
                        Some(lit) if depth < MAX_DEPTH => {
                            if let ExprKind::Closure { body: inner, .. } = &lit.kind {
                                let as_stmt = Stmt {
                                    id: inner.id,
                                    kind: StmtKind::Expr((**inner).clone()),
                                    span: inner.span,
                                };
                                if !self.quiet_at(std::slice::from_ref(&as_stmt), depth + 1) {
                                    quiet = false;
                                }
                            }
                        }
                        _ => quiet = false,
                    },
                    _ => quiet = false,
                }
            });
        }
        quiet
    }

    /// RK16: one block of a fork whose blocks share a graph. It may read
    /// anything; it may not write through a link or a rack, and may not store
    /// a link anywhere but a `Staged` buffer.
    pub(crate) fn check_shared_graph_block(&mut self, side: &Expr) {
        let ExprKind::Closure { body, .. } = &side.kind else { return };
        let as_stmt = Stmt { id: body.id, kind: StmtKind::Expr((**body).clone()), span: body.span };
        let stmts = std::slice::from_ref(&as_stmt);
        let mut hit: Option<(Span, String)> = None;
        for place in self.written_places(stmts) {
            let staged = self.node_ty(&Self::place_root(place).id).is_some_and(|t| self.is_staged_type(t));
            // Through a link (a node's field) or into a rack (an insert, a
            // delete). A local vector of links being filled is a store, which
            // the next check names.
            if !staged && self.root_holds(place, |s, t| s.is_link_type(t) || s.program.types.holds_rack(t)) {
                hit = Some((place.span, "this writes through a link or a rack".to_string()));
                break;
            }
        }
        if hit.is_none() {
            hit = self.first_link_store(stmts, 0);
        }
        if hit.is_none() {
            let gives_back = self
                .node_ty(&side.id)
                .and_then(|t| match t {
                    Type::Fn { ret, .. } => Some(self.holds_link_here(ret)),
                    _ => None,
                })
                .unwrap_or(false);
            if gives_back {
                hit = Some((body.span, "this block hands back a link".to_string()));
            }
        }
        if let Some((span, what)) = hit {
            self.errors.push(OwnershipError { kind: OwnershipErrorKind::SharedGraphWrite { what }, span });
        }
    }

    fn is_staged_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Generic { base, .. } => self.program.types.type_name(*base) == "Staged",
            Type::UnresolvedGeneric { name, .. } => name == "Staged",
            _ => false,
        }
    }

    /// Every `for` whose body can't see a delete, by the loop's source
    /// expression. Lowering walks those without registering the list.
    pub(crate) fn note_quiet_loop(&mut self, iter: &Expr, body: &[Stmt]) {
        if self.instance.is_none() && self.expr_holds_link(iter) && self.loop_is_quiet(body) {
            self.quiet_loops.insert(iter.id);
        }
    }
}

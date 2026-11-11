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

use std::collections::{BTreeSet, HashMap};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{ArgMode, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{visit, Span};
use rask_types::Type;

use crate::replaces::Key;
use crate::{OwnershipChecker, OwnershipError, OwnershipErrorKind};

/// Whether calling a function may store a link: always, or only when one of
/// its type parameters is (or holds) a link. `keep<T>(x: T)` pushing `x`
/// into a vector stores a link exactly when `T` holds one, which only the
/// call knows.
#[derive(Clone, Default, PartialEq, Debug)]
pub(crate) struct StoreSummary {
    pub(crate) always: bool,
    pub(crate) if_params: BTreeSet<String>,
    /// The owner's header parameters, in order, so a method call's receiver
    /// type arguments can be matched to them.
    pub(crate) owner_params: Vec<String>,
}

impl StoreSummary {
    /// Stores under some type arguments: what a caller that can't see them
    /// has to assume.
    fn may_store(&self) -> bool {
        self.always || !self.if_params.is_empty()
    }
}

/// Per function and method.
pub(crate) type LinkStores = HashMap<Key, StoreSummary>;

/// How deep a closure literal called from a closure literal is followed.
const MAX_DEPTH: u32 = 8;

impl<'a> OwnershipChecker<'a> {
    /// Fill `link_stores` for every function and method in `groups`, to a
    /// fixed point: every body starts out storing nothing, and a call to one
    /// that stores makes its caller store too.
    pub(crate) fn infer_link_stores(&mut self, groups: &[&[Decl]]) {
        // Each body with the type parameters its owner and its signature
        // bring into scope: a `T` might be a link, so storing one counts.
        let mut bodies: Vec<(Key, &FnDecl, Vec<String>)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
                match &decl.kind {
                    DeclKind::Fn(f) if !f.body_lives_elsewhere() => {
                        bodies.push((Key::Fn(f.name.clone()), f, Vec::new()))
                    }
                    DeclKind::Impl(i) => {
                        let ty = i.target_ty.name().unwrap_or_default();
                        let owner = rask_check::header_type_params(i, &self.program.types);
                        for m in i.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(ty.clone(), m.name.clone()), m, owner.clone()));
                        }
                    }
                    DeclKind::Struct(st) => {
                        let owner = rask_ast::type_params::struct_type_param_names(st);
                        for m in st.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(st.name.clone(), m.name.clone()), m, owner.clone()));
                        }
                    }
                    DeclKind::Enum(en) => {
                        let owner = rask_ast::type_params::enum_type_param_names(en);
                        for m in en.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Key::Method(en.name.clone(), m.name.clone()), m, owner.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }
        for (key, _, owner) in &bodies {
            self.link_stores
                .insert(key.clone(), StoreSummary { owner_params: owner.clone(), ..Default::default() });
        }
        let outer_params = std::mem::take(&mut self.type_params_in_scope);
        self.summarizing.set(true);
        // A summary only grows (a parameter joins the condition, or it turns
        // unconditional), so this reaches its fixed point.
        // Bodies that share a key (a program's method on a stdlib type and the
        // stdlib's own) are joined, or each round they'd overwrite each other
        // and the loop would never settle.
        loop {
            let mut merged: HashMap<Key, StoreSummary> = HashMap::new();
            for (key, f, owner) in &bodies {
                let mut params = owner.clone();
                params.extend(rask_ast::type_params::signature_type_param_names(f));
                self.type_params_in_scope = params;
                self.link_param_hits.borrow_mut().clear();
                let always = self.first_link_store(&f.body, 0).is_some();
                let hits = self.link_param_hits.take();
                let entry = merged
                    .entry(key.clone())
                    .or_insert_with(|| StoreSummary { owner_params: owner.clone(), ..Default::default() });
                entry.always |= always;
                entry.if_params.extend(hits);
            }
            let mut changed = false;
            for (key, mut found) in merged {
                if found.always {
                    found.if_params.clear();
                }
                // Never back down: what an earlier round found stands.
                if let Some(old) = self.link_stores.get(&key) {
                    found.always |= old.always;
                    if !found.always {
                        found.if_params.extend(old.if_params.iter().cloned());
                    } else {
                        found.if_params.clear();
                    }
                }
                if self.link_stores.get(&key) != Some(&found) {
                    self.link_stores.insert(key, found);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.summarizing.set(false);
        self.type_params_in_scope = outer_params;
    }

    /// Whether this call stores a link, by its callee's summary and the type
    /// arguments the call supplies.
    fn call_stores(&self, key: &Key, call: &Expr) -> Option<bool> {
        let summary = self.link_stores.get(key)?;
        if summary.always {
            return Some(true);
        }
        if summary.if_params.is_empty() {
            return Some(false);
        }
        let mut subst: HashMap<String, Type> = HashMap::new();
        if let Some(bindings) = self.program.call_type_args.get(&call.id) {
            for b in bindings {
                subst.insert(b.param.clone(), b.ty.clone());
            }
        }
        if let ExprKind::MethodCall { object, .. } = &call.kind {
            if let Some(Type::Generic { args, .. }) = self.node_ty(&object.id) {
                for (p, a) in summary.owner_params.iter().zip(args) {
                    if let rask_types::GenericArg::Type(t) = a {
                        subst.entry(p.clone()).or_insert_with(|| (**t).clone());
                    }
                }
            }
        }
        // A parameter the call doesn't bind is one nothing here can rule out.
        Some(summary.if_params.iter().any(|p| subst.get(p).map_or(true, |t| self.holds_link_here(t))))
    }

    /// Does this type, here, hold a link a store would record? A type
    /// parameter might. While a summary is being worked out, that "might" is
    /// noted as a condition on the parameter instead (`link_param_hits`), so
    /// each call decides it with the type it supplies.
    fn holds_link_here(&self, ty: &Type) -> bool {
        if self.program.types.holds_registered_link(ty) {
            return true;
        }
        if !self.summarizing.get() {
            return self.mentions_type_param(ty);
        }
        ty.mentions_type_param(&|n| {
            if self.type_params_in_scope.iter().any(|p| p == n) {
                self.link_param_hits.borrow_mut().insert(n.to_string());
            }
            false
        });
        false
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
    /// read them out of, or it's of a type parameter's type, which might.
    /// Asked only to skip calls that can't matter, so it notes no condition.
    fn expr_reaches_links(&self, e: &Expr) -> bool {
        self.node_ty(&e.id).is_some_and(|t| {
            self.program.types.holds_registered_link(t) || self.holds_rack_here(t) || self.mentions_type_param(t)
        })
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

    /// A function value handed to a call may be called by it, so it stores
    /// what its body stores: a closure literal's body, a declared function's
    /// summary. One this pass can't see into might store anything.
    fn fn_arg_stores_link(&self, a: &Expr, depth: u32) -> Option<(Span, String)> {
        if !matches!(self.node_ty(&a.id), Some(Type::Fn { .. })) {
            return None;
        }
        match &a.kind {
            // Walked with the rest of the body.
            ExprKind::Closure { .. } => None,
            ExprKind::Ident(name) if self.named_fn_values.contains_key(name) => None,
            _ if self.fn_key(a).is_some() => {
                let key = self.fn_key(a)?;
                self.link_stores.get(&key).is_some_and(StoreSummary::may_store)
                    .then(|| (a.span, "this function may store a link".to_string()))
            }
            ExprKind::Ident(name) if self.closure_literals.contains_key(name) => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                let ExprKind::Closure { body, .. } = &self.closure_literals[name].kind else { return None };
                self.first_store_in_expr(std::slice::from_ref(body.as_ref()), depth + 1)
                    .map(|_| (a.span, format!("`{name}` may store a link")))
            }
            _ => Some((a.span, "a function value this can't see into may store a link".to_string())),
        }
    }

    /// A call that may store a link it's handed.
    fn expr_stores_link(&self, e: &Expr, depth: u32) -> Option<(Span, String)> {
        let fn_args: Vec<&Expr> = match &e.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args.iter().map(|a| &a.expr).collect(),
            _ => Vec::new(),
        };
        if let Some(hit) = fn_args.iter().find_map(|a| self.fn_arg_stores_link(a, depth)) {
            return Some(hit);
        }
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
                if let Some(stores) = key.as_ref().and_then(|k| self.call_stores(k, e)) {
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
                    if let Some(stores) = self.call_stores(&key, e) {
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
                    if self.named_fn_values.contains_key(name) {
                        return None;
                    }
                    if let Some(lit) = self.closure_literals.get(name) {
                        if depth < MAX_DEPTH {
                            if let ExprKind::Closure { body, .. } = &lit.kind {
                                let stmts = std::slice::from_ref(body.as_ref());
                                return self.first_store_in_expr(stmts, depth + 1).map(|(_, why)| (e.span, why));
                            }
                        }
                        return None;
                    }
                }
                // A function value with no body here to read: a parameter, a
                // field. Handed links, it may keep them.
                matches!(self.node_ty(&func.id), Some(Type::Fn { .. }))
                    .then(|| (e.span, "a function value this can't see into may store a link".to_string()))
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
                // A function value handed to a call may be called by it.
                let handed: Vec<&Expr> = match &e.kind {
                    ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                        args.iter().map(|a| &a.expr).collect()
                    }
                    _ => Vec::new(),
                };
                for a in handed {
                    if !matches!(self.node_ty(&a.id), Some(Type::Fn { .. })) {
                        continue;
                    }
                    match &a.kind {
                        ExprKind::Closure { .. } => {}
                        ExprKind::Ident(name) if self.named_fn_values.contains_key(name) => {}
                        _ if self.fn_key(a).is_some() => {}
                        ExprKind::Ident(name) if self.closure_literals.contains_key(name) && depth < MAX_DEPTH => {
                            if let ExprKind::Closure { body: inner, .. } = &self.closure_literals[name].kind {
                                let as_stmt = Stmt { id: inner.id, kind: StmtKind::Expr((**inner).clone()), span: inner.span };
                                if !self.quiet_at(std::slice::from_ref(&as_stmt), depth + 1) {
                                    quiet = false;
                                }
                            }
                        }
                        _ => quiet = false,
                    }
                }
                let ExprKind::Call { func, .. } = &e.kind else { return };
                if self.fn_key(func).is_some() {
                    return;
                }
                if !matches!(self.node_ty(&func.id), Some(Type::Fn { .. })) {
                    return;
                }
                match &func.kind {
                    ExprKind::Ident(name) if self.named_fn_values.contains_key(name) => {}
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

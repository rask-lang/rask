// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Derived `eq`, `hash` and `compare`, written by the checker.
//!
//! The checker decides which types are Equal, Hashable and Comparable
//! (`auto_derive_interfaces`), so it is also the one that writes what those
//! methods do: once the field types are known, for the types that qualify and
//! no others. The bodies are ordinary declarations, checked here like a method
//! somebody typed, and handed down with the program (`TypedProgram::
//! derived_decls`). Both backends then answer `==`, `.hash()` and a map key's
//! bucket from the same code (#1391).
//!
//! The wrapper shapes have no methods: `T?`, `T or E` and tuples are
//! operator-only. So a wrapper that needs comparing through its parts' own
//! `eq` — a tuple holding a `Vec`, an optional user struct — gets a free
//! function, `__derived_eq_N`, written from its shape. A field of that type
//! calls it, a `==` on two of them is rewritten to call it
//! (`TypedProgram::wrapper_eq_calls`), and a map keyed by one hashes and
//! compares through the pair (`TypedProgram::wrapper_fns`) (#1392).

use rask_ast::decl::{Decl, DeclKind, FnDecl, ImplDecl, Param};
use rask_ast::expr::{ArgMode, BinOp, CallArg, Expr, ExprKind, MatchArm, Pattern};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::token::IntSuffix;
use rask_ast::ty::TypeExpr;
use rask_ast::{NodeId, Span};

use super::type_table::TypeOwner;
use super::{TypeChecker, TypeDef};
use crate::types::{Type, TypeId};

/// First NodeId of a compiler-written body. Its own band: the parser counts up
/// from 0, desugaring uses 10M and 20M, the stdlib 1M–3M and 40M–50M.
pub const DERIVED_ID_BASE: u32 = 30_000_000;

/// FNV-1a's offset basis and prime — what `rask_int_hash` and the string hash
/// use, so a struct's hash is built from the same mixing as its fields'.
const FNV_OFFSET: i128 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: i128 = 0x0000_0100_0000_01b3;

const SP: Span = Span::new(0, 0);

/// The `eq` and `hash` written for one wrapper type: each only when the type
/// has it (a tuple holding an `f64` is Equal and not Hashable).
#[derive(Debug, Clone)]
pub struct WrapperFns {
    pub ty: Type,
    pub eq: Option<String>,
    pub hash: Option<String>,
    /// Tuples only: `T?` and `T or E` have no order.
    pub compare: Option<String>,
}

impl TypeChecker {
    // ─── Struct and enum methods ───────────────────────────────

    /// Write the derived bodies for every type `auto_derive_interfaces` gave a
    /// derived `eq`, `hash` or `compare`.
    ///
    /// Not for a fieldless stdlib struct: `Duration`, `File`, `Random` are
    /// stand-ins for a runtime object, and an `eq` derived from no fields says
    /// yes to every pair. A program's empty struct really is always equal to
    /// another. `compare` for any struct with fields, which is where it has
    /// always been written (a `Vec<Metadata>` sorts by it).
    pub(super) fn write_derived_methods(&mut self, annotations: &[String]) {
        for idx in 0..self.types.types.len() {
            let id = TypeId(idx as u32);
            if self.derived_written.contains(&id) {
                continue;
            }
            let Some(def) = self.types.get(id).cloned() else { continue };
            if matches!(def, TypeDef::Struct { .. } | TypeDef::Enum { .. }) {
                self.derived_written.insert(id);
            }
            let program_type = !matches!(self.types.declared_by(id), TypeOwner::Stdlib);
            let derived = |methods: &[super::MethodSig], name: &str| {
                methods.iter().any(|m| m.name == name && m.derived)
            };
            let mut written = Vec::new();
            match &def {
                TypeDef::Struct { name, type_params, fields, methods, .. } => {
                    if !type_params.is_empty() || annotations.contains(name) {
                        continue;
                    }
                    let has_body = program_type || !fields.is_empty();
                    if has_body && derived(methods, "eq") {
                        let body = self.struct_eq(fields);
                        written.push(self.method("eq", true, "bool", body));
                    }
                    if has_body && derived(methods, "hash") {
                        let body = self.struct_hash(fields);
                        written.push(self.method("hash", false, "u64", body));
                    }
                    if !fields.is_empty() && derived(methods, "compare") {
                        let body = self.struct_compare(fields);
                        written.push(self.method("compare", true, "Ordering", body));
                    }
                }
                TypeDef::Enum { name, type_params, variants, methods, .. } => {
                    if !type_params.is_empty() || variants.is_empty() {
                        continue;
                    }
                    if derived(methods, "eq") {
                        let body = self.enum_eq(name, variants);
                        written.push(self.method("eq", true, "bool", body));
                    }
                    if derived(methods, "hash") {
                        let body = self.enum_hash(name, variants);
                        written.push(self.method("hash", false, "u64", body));
                    }
                }
                _ => continue,
            }
            if written.is_empty() {
                continue;
            }
            // `eq` and `hash` have a body now, and a call to one is a call:
            // `derived` is what lowering reads as "no body, compare in place".
            // `compare` keeps the flag; `<` on a struct still lowers through it.
            let names: Vec<String> = written.iter().map(|f| f.name.clone()).collect();
            if let Some(TypeDef::Struct { methods, .. } | TypeDef::Enum { methods, .. }) =
                self.types.get_mut(id)
            {
                for m in methods.iter_mut() {
                    if names.contains(&m.name) && m.name != "compare" {
                        m.derived = false;
                    }
                }
            }
            let target = self.types.type_name(id);
            let decl = Decl {
                id: self.derived_id(),
                kind: DeclKind::Impl(ImplDecl {
                    interface: None,
                    target_ty: TypeExpr::named(target.as_str()),
                    methods: written,
                    is_unsafe: false,
                    is_pub: true,
                    where_bounds: Vec::new(),
                    assoc_bindings: Vec::new(),
                    doc: None,
                }),
                span: SP,
            };
            self.pending_derived.push((decl, Some(id)));
        }
    }

    /// `return self.a == other.a && self.b == other.b`
    fn struct_eq(&mut self, fields: &[(String, Type)]) -> Vec<Stmt> {
        let compares: Vec<Expr> = fields
            .iter()
            .map(|(f, ty)| {
                let (a, b) = (self.path("self", f), self.path("other", f));
                self.eq_of(a, b, ty)
            })
            .collect();
        let all = self.all(compares);
        vec![self.ret(all)]
    }

    /// FNV over the fields' own hashes, in declaration order.
    fn struct_hash(&mut self, fields: &[(String, Type)]) -> Vec<Stmt> {
        let hashes: Vec<Expr> = fields
            .iter()
            .map(|(f, ty)| {
                let v = self.path("self", f);
                self.hash_of(v, ty)
            })
            .collect();
        let seed = self.u64_lit(FNV_OFFSET);
        let mixed = self.mix(seed, hashes);
        vec![self.ret(mixed)]
    }

    /// Lexicographic by field: `if self.a < other.a { return Ordering.Less }`,
    /// `if self.a > other.a { return Ordering.Greater }`, … `return Ordering.Equal`.
    ///
    /// `Ordering`, not a raw `-1 / 0 / 1`, because that is what `compare` is
    /// (type.operators/ORD1). Converting for the sort runtime is the sort
    /// lowering's job, at the one boundary that needs it.
    fn struct_compare(&mut self, fields: &[(String, Type)]) -> Vec<Stmt> {
        let mut body = Vec::new();
        for (f, ty) in fields {
            let (a, b) = (self.path("self", f), self.path("other", f));
            body.extend(self.compare_step(a, b, ty));
        }
        let equal = self.ordering("Equal");
        body.push(self.ret(equal));
        body
    }

    /// One lexicographic step: return the order if this pair decides it.
    ///
    /// `if a < b { return Ordering.Less }`, `if a > b { return Ordering.Greater }`.
    /// A tuple has no `<` (the wrappers are operator-only for `==`), so one is
    /// ordered through its own compare: `let c = compare(a, b)`, and `c`
    /// returned unless it is `Ordering.Equal`.
    fn compare_step(&mut self, a: Expr, b: Expr, ty: &Type) -> Vec<Stmt> {
        if let Type::Tuple(_) = ty {
            if let Some(cmp) = self.wrapper_fns(ty).compare {
                let order = self.call_wrapper(&cmp, vec![a, b]);
                let c = self.fresh_name();
                let bind = self.stmt(StmtKind::Let {
                    name: c.clone(),
                    name_span: SP,
                    ty: None,
                    init: order,
                });
                let ce = self.ident(&c);
                let equal = self.ordering("Equal");
                let decided = self.op(ce, BinOp::Eq, equal);
                let not = self.expr(ExprKind::Unary {
                    op: rask_ast::expr::UnaryOp::Not,
                    operand: Box::new(decided),
                });
                let ret_c = self.ident(&c);
                let then = self.ret_block(ret_c);
                let check = self.expr_stmt(ExprKind::If {
                    cond: Box::new(not),
                    then_branch: Box::new(then),
                    else_branch: None,
                    else_binding: None,
                });
                return vec![bind, check];
            }
        }
        let mut out = Vec::new();
        for (op, answer) in [(BinOp::Lt, "Less"), (BinOp::Gt, "Greater")] {
            let cond = self.op(a.clone(), op, b.clone());
            let ordering = self.ordering(answer);
            let then = self.ret_block(ordering);
            out.push(self.expr_stmt(ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then),
                else_branch: None,
                else_binding: None,
            }));
        }
        out
    }

    /// ```text
    /// match self {
    ///     E.A(a0, a1) => match other {
    ///         E.A(b0, b1) => return a0 == b0 && a1 == b1
    ///         _ => return false
    ///     }
    ///     E.B => match other { E.B => return true, _ => return false }
    /// }
    /// ```
    fn enum_eq(&mut self, name: &str, variants: &[(String, Vec<Type>)]) -> Vec<Stmt> {
        let single = variants.len() == 1;
        let mut arms = Vec::new();
        for (variant, tys) in variants {
            let path = format!("{name}.{variant}");
            let compares: Vec<Expr> = tys
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    let (a, b) = (self.ident(&format!("__a{i}")), self.ident(&format!("__b{i}")));
                    self.eq_of(a, b, ty)
                })
                .collect();
            let all = self.all(compares);
            let mut inner = vec![arm(variant_pattern(&path, tys.len(), "__b"), self.ret_block(all))];
            // One variant: a wildcard would never match, and the checker says so.
            if !single {
                let no = self.bool_lit(false);
                inner.push(arm(Pattern::Wildcard, self.ret_block(no)));
            }
            let other = self.ident("other");
            let body = self.expr(ExprKind::Match { scrutinee: Box::new(other), arms: inner });
            arms.push(arm(variant_pattern(&path, tys.len(), "__a"), body));
        }
        let scrutinee = self.ident("self");
        vec![self.expr_stmt(ExprKind::Match { scrutinee: Box::new(scrutinee), arms })]
    }

    /// The variant's position, then its payload's hashes.
    fn enum_hash(&mut self, name: &str, variants: &[(String, Vec<Type>)]) -> Vec<Stmt> {
        let mut arms = Vec::new();
        for (index, (variant, tys)) in variants.iter().enumerate() {
            let mut parts = vec![self.u64_lit(index as i128)];
            for (i, ty) in tys.iter().enumerate() {
                let v = self.ident(&format!("__a{i}"));
                parts.push(self.hash_of(v, ty));
            }
            let seed = self.u64_lit(FNV_OFFSET);
            let mixed = self.mix(seed, parts);
            arms.push(arm(
                variant_pattern(&format!("{name}.{variant}"), tys.len(), "__a"),
                self.ret_block(mixed),
            ));
        }
        let scrutinee = self.ident("self");
        vec![self.expr_stmt(ExprKind::Match { scrutinee: Box::new(scrutinee), arms })]
    }

    /// Check what `write_derived_methods` and `wrapper_fns` wrote, like any
    /// declaration. Checking one can ask for another wrapper's pair, so it
    /// drains until nothing is left.
    pub(super) fn check_pending_derived(&mut self) {
        while !self.pending_derived.is_empty() {
            let batch = std::mem::take(&mut self.pending_derived);
            for (decl, owner) in batch {
                let decl = match (owner, decl.kind) {
                    // A type's methods are checked against the type itself, not
                    // its name: `Span` in program scope can be an alias of
                    // something else entirely (`type alias Span = Duration`),
                    // and the stdlib's `Span` is the one being derived. So the
                    // parameters say `Self` while they're checked and get the
                    // name back afterwards, for the passes that resolve names
                    // in their own scope.
                    (Some(id), DeclKind::Impl(mut imp)) => {
                        let outer = self.current_self_type.replace(Type::Named(id));
                        for m in &imp.methods {
                            self.check_fn(m);
                        }
                        self.current_self_type = outer;
                        let named = imp.target_ty.clone();
                        for m in &mut imp.methods {
                            for p in &mut m.params {
                                if p.ty.as_ref().is_some_and(|t| t.is_name("Self")) {
                                    p.ty = Some(named.clone());
                                }
                            }
                        }
                        Decl { id: decl.id, kind: DeclKind::Impl(imp), span: decl.span }
                    }
                    (_, kind) => {
                        let decl = Decl { id: decl.id, kind, span: decl.span };
                        self.check_decl(&decl);
                        decl
                    }
                };
                self.derived_decls.push(decl);
            }
        }
    }

    /// Settle the operator `eq` calls on wrappers, now their operand types are
    /// known, and write the pairs a wrapper type argument needs.
    ///
    /// A type argument because that is where a map key is: `Map<(i64, string),
    /// V>`, and `Set<(i64, string)>`, whose map lives in a generic body this
    /// checker never sees instantiated. Native looks the pair up by the key's
    /// type (`TypedProgram::wrapper_fns`) wherever it builds the map.
    pub(super) fn settle_derived_wrappers(&mut self, decls: &[Decl]) {
        let pending = std::mem::take(&mut self.pending_wrapper_eq);
        for (call, recv, arg) in pending {
            let ty_of = |this: &Self, id: NodeId| this.node_types.get(&id).map(|t| this.ctx.apply(t));
            let (Some(a), Some(b)) = (ty_of(self, recv), ty_of(self, arg)) else { continue };
            if a != b || !Self::is_wrapper(&a) || !Self::wrapper_needs_fns(&a) {
                continue;
            }
            let Some(name) = self.wrapper_fns(&a).eq else { continue };
            let sym = self.wrapper_symbols[&name];
            let callee = self.derived_id();
            self.resolved.resolutions.insert(callee, sym);
            self.node_types.insert(callee, Type::Fn {
                params: vec![a.clone(), a.clone()],
                ret: Box::new(Type::Bool),
            });
            self.call_targets.insert(call, super::type_defs::Callee::Free(sym));
            self.wrapper_eq_calls.insert(call, (callee, name));
        }

        // The program's own expressions only: a stdlib body's types are its
        // business, and are full of `void or E` nobody keys a map with.
        let mut ids = Vec::new();
        rask_ast::visit::walk_decls(decls, &mut |e| ids.push(e.id));
        let mut args: Vec<Type> = Vec::new();
        for id in ids {
            if let Some(ty) = self.node_types.get(&id) {
                collect_wrapper_args(&self.ctx.apply(ty), &mut args);
            }
        }
        for ty in args {
            if is_concrete(&ty) {
                self.wrapper_fns(&ty);
            }
        }

        self.push_scope();
        self.check_pending_derived();
        self.pop_scope();
        self.solve_constraints();
    }

    // ─── Wrappers ──────────────────────────────────────────────

    /// A tuple, optional or result, which only `==` and the derived code reach.
    pub(super) fn is_wrapper(ty: &Type) -> bool {
        matches!(ty, Type::Tuple(_) | Type::Result { .. })
    }

    /// Whether `==` on this wrapper has to go through its parts' own `eq`:
    /// it holds something other than a scalar or a string, which the backends
    /// compare in place. Two `(i64, string)` don't need a function; two
    /// `(Vec<i64>, i64)` or `Version?` do.
    pub(super) fn wrapper_needs_fns(ty: &Type) -> bool {
        match ty {
            Type::Tuple(elems) => elems.iter().any(Self::wrapper_needs_fns),
            Type::Result { ok, err } => Self::wrapper_needs_fns(ok) || Self::wrapper_needs_fns(err),
            Type::Named(_) | Type::Generic { .. } | Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. } => true,
            _ => false,
        }
    }

    /// The `eq`/`hash` pair for a wrapper type, written the first time it is
    /// asked for.
    pub(super) fn wrapper_fns(&mut self, ty: &Type) -> WrapperFns {
        if let Some(found) = self.wrapper_fns.iter().find(|w| &w.ty == ty) {
            return found.clone();
        }
        let n = self.wrapper_fns.len();
        let fns = WrapperFns {
            ty: ty.clone(),
            eq: self.type_has_method(ty, "eq").then(|| format!("__derived_eq_{n}")),
            hash: self.type_has_method(ty, "hash").then(|| format!("__derived_hash_{n}")),
            compare: (matches!(ty, Type::Tuple(_)) && self.type_has_method(ty, "compare"))
                .then(|| format!("__derived_compare_{n}")),
        };
        // Registered before the bodies are written: one holding another
        // wrapper asks for that one's pair while this one is half built.
        self.wrapper_fns.push(fns.clone());
        let mut written = Vec::new();
        if let Some(name) = &fns.eq {
            let sym = self.wrapper_symbol(name, ty, Type::Bool);
            self.wrapper_symbols.insert(name.clone(), sym);
            written.push((name.clone(), 0, sym));
        }
        if let Some(name) = &fns.hash {
            let sym = self.wrapper_symbol(name, ty, Type::U64);
            self.wrapper_symbols.insert(name.clone(), sym);
            written.push((name.clone(), 1, sym));
        }
        if let Some(name) = &fns.compare {
            let ordering = self.ordering_type();
            let sym = self.wrapper_symbol(name, ty, ordering);
            self.wrapper_symbols.insert(name.clone(), sym);
            written.push((name.clone(), 2, sym));
        }
        for (name, which, sym) in written {
            let f = match which {
                0 => {
                    let body = self.wrapper_eq_body(ty);
                    self.free_fn(&name, ty, true, "bool", body)
                }
                1 => {
                    let body = self.wrapper_hash_body(ty);
                    self.free_fn(&name, ty, false, "u64", body)
                }
                _ => {
                    let body = self.wrapper_compare_body(ty);
                    self.free_fn(&name, ty, true, "Ordering", body)
                }
            };
            let id = self.derived_id();
            self.resolved.decl_symbols.insert(id, sym);
            self.pending_derived.push((Decl { id, kind: DeclKind::Fn(f), span: SP }, None));
        }
        fns
    }

    fn wrapper_symbol(&mut self, name: &str, ty: &Type, ret: Type) -> rask_resolve::SymbolId {
        let ret_written = self.written(&ret);
        let sym = self.resolved.symbols.insert(
            name.to_string(),
            rask_resolve::SymbolKind::Function {
                params: Vec::new(),
                ret_ty: Some(ret_written),
                is_unsafe: false,
            },
            None,
            SP,
            false,
        );
        // `hash` takes one value; `eq` and `compare` take two.
        let params = if ret == Type::U64 { vec![ty.clone()] } else { vec![ty.clone(), ty.clone()] };
        self.symbol_types.insert(sym, Type::Fn { params, ret: Box::new(ret) });
        sym
    }

    fn wrapper_eq_body(&mut self, ty: &Type) -> Vec<Stmt> {
        let (a, b) = (self.ident("a"), self.ident("b"));
        let body = self.eq_by_shape(a, b, ty);
        vec![self.ret(body)]
    }

    /// Lexicographic, element by element.
    fn wrapper_compare_body(&mut self, ty: &Type) -> Vec<Stmt> {
        let Type::Tuple(elems) = ty else { return Vec::new() };
        let mut body = Vec::new();
        for (i, t) in elems.iter().enumerate() {
            let (a, b) = (self.path("a", &i.to_string()), self.path("b", &i.to_string()));
            body.extend(self.compare_step(a, b, t));
        }
        let equal = self.ordering("Equal");
        body.push(self.ret(equal));
        body
    }

    fn wrapper_hash_body(&mut self, ty: &Type) -> Vec<Stmt> {
        let a = self.ident("a");
        let body = self.hash_by_shape(a, ty);
        vec![self.ret(body)]
    }

    /// `a == b` for two values of `ty`, as the derived code spells it: through
    /// the wrapper's function when it has one, otherwise the operator.
    fn eq_of(&mut self, a: Expr, b: Expr, ty: &Type) -> Expr {
        if Self::is_wrapper(ty) && Self::wrapper_needs_fns(ty) {
            if let Some(eq) = self.wrapper_fns(ty).eq {
                return self.call_wrapper(&eq, vec![a, b]);
            }
        }
        // `T or E` has no `==` of its own; written out by shape even when it
        // holds only scalars.
        if matches!(ty, Type::Result { err, .. } if **err != Type::None) {
            return self.eq_by_shape(a, b, ty);
        }
        self.op(a, BinOp::Eq, b)
    }

    /// The hash of a value of `ty`. A wrapper has no `.hash()`.
    fn hash_of(&mut self, v: Expr, ty: &Type) -> Expr {
        if Self::is_wrapper(ty) {
            if let Some(hash) = self.wrapper_fns(ty).hash {
                return self.call_wrapper(&hash, vec![v]);
            }
        }
        self.method_call(v, "hash", vec![])
    }

    fn eq_by_shape(&mut self, a: Expr, b: Expr, ty: &Type) -> Expr {
        match ty {
            // `if a? as x { if b? as y { x == y } else { false } } else { !b? }`
            Type::Result { ok, err } if **err == Type::None => {
                let (x, y) = (self.fresh_name(), self.fresh_name());
                let (xe, ye) = (self.ident(&x), self.ident(&y));
                let inner_eq = self.eq_of(xe, ye, ok);
                let no = self.bool_lit(false);
                let both = self.present(b.clone(), Some(y), inner_eq, no);
                let b_present = self.expr(ExprKind::IsPresent { expr: Box::new(b), binding: None });
                let b_absent = self.expr(ExprKind::Unary {
                    op: rask_ast::expr::UnaryOp::Not,
                    operand: Box::new(b_present),
                });
                self.present(a, Some(x), both, b_absent)
            }
            // `match a { T as x => match b { T as y => x == y, _ => false }, E as x => … }`
            //
            // A `void` side has nothing to bind and no pattern of its own: it
            // is the trailing `_`, and two of them are equal.
            Type::Result { ok, err } => {
                let mut arms = Vec::new();
                let mut valued = Vec::new();
                for side in [&**ok, &**err] {
                    if *side == Type::Unit {
                        continue;
                    }
                    valued.push(side.clone());
                    let (x, y) = (self.fresh_name(), self.fresh_name());
                    let written = self.written(side);
                    let (xe, ye) = (self.ident(&x), self.ident(&y));
                    let same = self.eq_of(xe, ye, side);
                    let no = self.bool_lit(false);
                    let inner = self.expr(ExprKind::Match {
                        scrutinee: Box::new(b.clone()),
                        arms: vec![
                            arm(Pattern::TypePat { ty: written.clone(), binding: Some(y) }, same),
                            arm(Pattern::Wildcard, no),
                        ],
                    });
                    arms.push(arm(Pattern::TypePat { ty: written, binding: Some(x) }, inner));
                }
                if valued.len() == 1 {
                    let other = self.written(&valued[0]);
                    let (no, yes) = (self.bool_lit(false), self.bool_lit(true));
                    let inner = self.expr(ExprKind::Match {
                        scrutinee: Box::new(b.clone()),
                        arms: vec![
                            arm(Pattern::TypePat { ty: other, binding: None }, no),
                            arm(Pattern::Wildcard, yes),
                        ],
                    });
                    arms.push(arm(Pattern::Wildcard, inner));
                }
                self.expr(ExprKind::Match { scrutinee: Box::new(a), arms })
            }
            // `a.0 == b.0 && a.1 == b.1`
            Type::Tuple(elems) => {
                let compares: Vec<Expr> = elems
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let (x, y) = (self.field(a.clone(), &i.to_string()), self.field(b.clone(), &i.to_string()));
                        self.eq_of(x, y, t)
                    })
                    .collect();
                self.all(compares)
            }
            _ => self.op(a, BinOp::Eq, b),
        }
    }

    fn hash_by_shape(&mut self, v: Expr, ty: &Type) -> Expr {
        match ty {
            // `if v? as x { mix(1, x) } else { 0 }`
            Type::Result { ok, err } if **err == Type::None => {
                let x = self.fresh_name();
                let xe = self.ident(&x);
                let inner = self.hash_of(xe, ok);
                let one = self.u64_lit(1);
                let some = self.mix(one, vec![inner]);
                let none = self.u64_lit(0);
                self.present(v, Some(x), some, none)
            }
            // `match v { T as o => mix(1, o), E as e => mix(2, e) }`, a `void`
            // side being the trailing `_` and hashing to its tag.
            Type::Result { ok, err } => {
                let mut arms = Vec::new();
                let mut unit_tag = None;
                for (tag, side) in [(1, &**ok), (2, &**err)] {
                    if *side == Type::Unit {
                        unit_tag = Some(tag);
                        continue;
                    }
                    let x = self.fresh_name();
                    let written = self.written(side);
                    let xe = self.ident(&x);
                    let inner = self.hash_of(xe, side);
                    let seed = self.u64_lit(tag);
                    let mixed = self.mix(seed, vec![inner]);
                    arms.push(arm(Pattern::TypePat { ty: written, binding: Some(x) }, mixed));
                }
                if let Some(tag) = unit_tag {
                    let lit = self.u64_lit(tag);
                    arms.push(arm(Pattern::Wildcard, lit));
                }
                self.expr(ExprKind::Match { scrutinee: Box::new(v), arms })
            }
            Type::Tuple(elems) => {
                let parts: Vec<Expr> = elems
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let x = self.field(v.clone(), &i.to_string());
                        self.hash_of(x, t)
                    })
                    .collect();
                let seed = self.u64_lit(FNV_OFFSET);
                self.mix(seed, parts)
            }
            _ => self.method_call(v, "hash", vec![]),
        }
    }

    // ─── AST shorthands ────────────────────────────────────────

    fn derived_id(&mut self) -> NodeId {
        let id = NodeId(self.next_derived_id);
        self.next_derived_id += 1;
        id
    }

    fn fresh_name(&mut self) -> String {
        self.derived_names += 1;
        format!("__d{}", self.derived_names)
    }

    fn written(&self, ty: &Type) -> TypeExpr {
        self.types.resolve_type_names(ty).to_type_expr()
    }

    fn expr(&mut self, kind: ExprKind) -> Expr {
        Expr { id: self.derived_id(), kind, span: SP }
    }

    fn stmt(&mut self, kind: StmtKind) -> Stmt {
        Stmt { id: self.derived_id(), kind, span: SP }
    }

    fn ident(&mut self, name: &str) -> Expr {
        self.expr(ExprKind::Ident(name.to_string()))
    }

    /// `base.name`
    fn path(&mut self, base: &str, name: &str) -> Expr {
        let object = self.ident(base);
        self.field(object, name)
    }

    fn field(&mut self, object: Expr, name: &str) -> Expr {
        self.expr(ExprKind::Field { object: Box::new(object), field: name.to_string() })
    }

    /// An operator in its desugared form, marked as one.
    fn op(&mut self, left: Expr, op: BinOp, right: Expr) -> Expr {
        match rask_ast::expr::binary_op_method(op) {
            Some(method) => {
                let call = self.method_call(left, method, vec![right]);
                self.operator_calls.insert(call.id);
                call
            }
            None => self.expr(ExprKind::Binary { op, left: Box::new(left), right: Box::new(right) }),
        }
    }

    fn method_call(&mut self, object: Expr, method: &str, args: Vec<Expr>) -> Expr {
        self.expr(ExprKind::MethodCall {
            object: Box::new(object),
            method: method.to_string(),
            type_args: None,
            args: args.into_iter().map(|e| CallArg { name: None, mode: ArgMode::Default, expr: e }).collect(),
        })
    }

    /// A call to one of the wrapper functions. The callee resolves to the
    /// symbol `wrapper_fns` minted for it, as a written call resolves to the
    /// declaration the resolver found.
    fn call_wrapper(&mut self, name: &str, args: Vec<Expr>) -> Expr {
        let callee = self.ident(name);
        if let Some(&sym) = self.wrapper_symbols.get(name) {
            self.resolved.resolutions.insert(callee.id, sym);
        }
        self.expr(ExprKind::Call {
            func: Box::new(callee),
            args: args.into_iter().map(|e| CallArg { name: None, mode: ArgMode::Default, expr: e }).collect(),
        })
    }

    /// `((seed ^ x0).wrapping_mul(P) ^ x1).wrapping_mul(P) …`
    fn mix(&mut self, seed: Expr, parts: Vec<Expr>) -> Expr {
        parts.into_iter().fold(seed, |h, x| {
            let xored = self.op(h, BinOp::BitXor, x);
            let prime = self.u64_lit(FNV_PRIME);
            self.method_call(xored, "wrapping_mul", vec![prime])
        })
    }

    fn all(&mut self, compares: Vec<Expr>) -> Expr {
        let mut it = compares.into_iter();
        match it.next() {
            None => self.bool_lit(true),
            Some(first) => it.fold(first, |acc, c| {
                self.expr(ExprKind::Binary { op: BinOp::And, left: Box::new(acc), right: Box::new(c) })
            }),
        }
    }

    /// `if value? as bind { then } else { otherwise }`
    fn present(&mut self, value: Expr, bind: Option<String>, then: Expr, otherwise: Expr) -> Expr {
        let cond = self.expr(ExprKind::IsPresent { expr: Box::new(value), binding: bind });
        let then = self.block_value(then);
        let otherwise = self.block_value(otherwise);
        self.expr(ExprKind::If {
            cond: Box::new(cond),
            then_branch: Box::new(then),
            else_branch: Some(Box::new(otherwise)),
            else_binding: None,
        })
    }

    fn block_value(&mut self, e: Expr) -> Expr {
        let s = self.stmt(StmtKind::Expr(e));
        self.block(vec![s])
    }

    /// `{ return e }`
    fn ret_block(&mut self, e: Expr) -> Expr {
        let r = self.ret(e);
        self.block(vec![r])
    }

    fn expr_stmt(&mut self, kind: ExprKind) -> Stmt {
        let e = self.expr(kind);
        self.stmt(StmtKind::Expr(e))
    }

    fn block(&mut self, stmts: Vec<Stmt>) -> Expr {
        self.expr(ExprKind::Block(stmts))
    }

    fn ret(&mut self, e: Expr) -> Stmt {
        self.stmt(StmtKind::Return(Some(e)))
    }

    fn u64_lit(&mut self, v: i128) -> Expr {
        self.expr(ExprKind::Int(v, Some(IntSuffix::U64)))
    }

    fn bool_lit(&mut self, b: bool) -> Expr {
        self.expr(ExprKind::Bool(b))
    }

    /// `Ordering.Less` and friends.
    fn ordering(&mut self, variant: &str) -> Expr {
        let ordering = self.ident("Ordering");
        self.field(ordering, variant)
    }

    fn method(&self, name: &str, takes_other: bool, ret: &str, body: Vec<Stmt>) -> FnDecl {
        let mut params = vec![param("self", TypeExpr::named("Self"))];
        if takes_other {
            params.push(param("other", TypeExpr::named("Self")));
        }
        fn_decl(name, params, ret, body)
    }

    fn free_fn(&self, name: &str, ty: &Type, takes_other: bool, ret: &str, body: Vec<Stmt>) -> FnDecl {
        let written = self.written(ty);
        let mut params = vec![param("a", written.clone())];
        if takes_other {
            params.push(param("b", written));
        }
        fn_decl(name, params, ret, body)
    }
}

fn param(name: &str, ty: TypeExpr) -> Param {
    Param {
        name: name.to_string(),
        name_span: SP,
        ty: Some(ty),
        is_take: false,
        is_mutate: false,
        is_deleting: false,
        default: None,
    }
}

fn fn_decl(name: &str, params: Vec<Param>, ret: &str, body: Vec<Stmt>) -> FnDecl {
    FnDecl {
        name: name.to_string(),
        type_params: Vec::new(),
        params,
        ret_ty: Some(TypeExpr::named(ret)),
        body,
        is_pub: true,
        is_private: false,
        is_comptime: false,
        is_unsafe: false,
        abi: None,
        attrs: Vec::new(),
        doc: None,
        span: SP,
        decl_start: 0,
    }
}

fn arm(pattern: Pattern, body: Expr) -> MatchArm {
    MatchArm { pattern, guard: None, body: Box::new(body) }
}

fn variant_pattern(path: &str, n: usize, prefix: &str) -> Pattern {
    if n == 0 {
        Pattern::Ident(path.to_string())
    } else {
        Pattern::Constructor {
            name: path.to_string(),
            fields: (0..n).map(|i| Pattern::Ident(format!("{prefix}{i}"))).collect(),
        }
    }
}

/// Every wrapper type standing as a type argument inside `ty`.
fn collect_wrapper_args(ty: &Type, out: &mut Vec<Type>) {
    let mut visit_arg = |t: &Type, out: &mut Vec<Type>| {
        if TypeChecker::is_wrapper(t) && !out.contains(t) {
            out.push(t.clone());
        }
        collect_wrapper_args(t, out);
    };
    match ty {
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
            for a in args {
                if let crate::types::GenericArg::Type(t) = a {
                    visit_arg(t, out);
                }
            }
        }
        Type::Tuple(elems) => elems.iter().for_each(|e| collect_wrapper_args(e, out)),
        Type::Result { ok, err } => {
            collect_wrapper_args(ok, out);
            collect_wrapper_args(err, out);
        }
        Type::Fn { params, ret } => {
            params.iter().for_each(|p| collect_wrapper_args(p, out));
            collect_wrapper_args(ret, out);
        }
        _ => {}
    }
}

/// Settled all the way down: no inference variable, and no type parameter
/// still standing for itself (`(K, V)` inside a generic body). Those are
/// written per instantiation or not at all.
fn is_concrete(ty: &Type) -> bool {
    match ty {
        Type::Var(_) | Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. } | Type::Error => false,
        Type::Generic { args, .. } => args.iter().all(|a| match a {
            crate::types::GenericArg::Type(t) => is_concrete(t),
            _ => true,
        }),
        Type::Tuple(elems) => elems.iter().all(is_concrete),
        Type::Result { ok, err } => is_concrete(ok) && is_concrete(err),
        Type::Fn { params, ret } => params.iter().all(is_concrete) && is_concrete(ret),
        Type::Array { elem, .. } => is_concrete(elem),
        _ => true,
    }
}

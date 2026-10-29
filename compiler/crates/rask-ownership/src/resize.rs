// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which parameters a function can resize (comp.advanced/EF1, EF3).
//!
//! A `with v[i]` block and a `for mutate x in v` loop both hold a place inside
//! `v`'s buffer, so nothing may change `v`'s size or replace it until they end
//! (mem.borrowing/W2, ctrl.loops/LP14). A call can do either through a
//! `mutate` parameter, so a call site needs to know, per argument, whether the
//! callee can. That is inferred, never written: EF3 says a program's effects
//! come from its bodies.
//!
//! Per parameter, not per function. The function-level effect (`rask-effects`)
//! also counts a scratch `Vec` the function builds for itself, and a helper
//! that pushes onto one of those while only writing elements of its parameter
//! must stay callable inside a `with` over that parameter.

use std::collections::{HashMap, HashSet};


use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{self, Visit};

use crate::OwnershipChecker;

/// Where a function's resize facts are filed: a free function by name, a
/// method by the type it's declared on and its name.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Fn(String),
    Method(String, String),
}

impl<'a> OwnershipChecker<'a> {
    /// Fill `fn_resizes` and `method_resizes` for every function and method in
    /// `groups`, to a fixed point over the calls between them.
    pub(crate) fn infer_resizes(&mut self, groups: &[&[Decl]]) {
        let mut bodies: Vec<(Key, &FnDecl)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
                match &decl.kind {
                    DeclKind::Fn(f) => bodies.push((Key::Fn(f.name.clone()), f)),
                    DeclKind::Impl(i) => {
                        let ty = i.target_ty.name().unwrap_or_default();
                        for m in &i.methods {
                            bodies.push((Key::Method(ty.clone(), m.name.clone()), m));
                        }
                    }
                    _ => {}
                }
            }
        }
        loop {
            let mut changed = false;
            for (key, f) in &bodies {
                // Only a parameter the callee may change can be resized by it.
                let eligible: Vec<&str> = f
                    .params
                    .iter()
                    .filter(|p| p.is_mutate || p.is_deleting)
                    .map(|p| p.name.as_str())
                    .collect();
                if eligible.is_empty() {
                    continue;
                }
                let hit = self.body_resizes(&eligible, &f.body);
                let self_hit = hit.contains("self");
                let params: Vec<bool> = f
                    .params
                    .iter()
                    .filter(|p| p.name != "self")
                    .map(|p| hit.contains(p.name.as_str()))
                    .collect();
                match key {
                    Key::Fn(name) => {
                        let all: Vec<bool> =
                            f.params.iter().map(|p| hit.contains(p.name.as_str())).collect();
                        if self.fn_resizes.get(name) != Some(&all) {
                            self.fn_resizes.insert(name.clone(), all);
                            changed = true;
                        }
                    }
                    Key::Method(ty, name) => {
                        let entry = (self_hit, params);
                        let k = (ty.clone(), name.clone());
                        if self.method_resizes.get(&k) != Some(&entry) {
                            self.method_resizes.insert(k, entry);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// The names among `params` that `body` can resize or replace.
    ///
    /// A name shadowed by a local of the same name reads as the parameter
    /// here, which can only make a call refused that could have been allowed.
    fn body_resizes(&self, params: &[&str], body: &[Stmt]) -> HashSet<String> {
        struct Walk<'c, 'a> {
            checker: &'c OwnershipChecker<'a>,
            params: &'c [&'c str],
            hit: HashSet<String>,
        }
        impl<'c, 'a, 'e> Visit<'e> for Walk<'c, 'a> {
            fn expr(&mut self, expr: &'e Expr) -> bool {
                for (place, _) in self.checker.resized_places(expr) {
                    if let Some(root) = OwnershipChecker::extract_root_and_fields(place).0 {
                        if self.params.contains(&root.as_str()) {
                            self.hit.insert(root);
                        }
                    }
                }
                true
            }
            fn stmt(&mut self, stmt: &'e Stmt) -> bool {
                if let Some(place) = self.checker.replaced_place(stmt) {
                    if let Some(root) = OwnershipChecker::extract_root_and_fields(place).0 {
                        if self.params.contains(&root.as_str()) {
                            self.hit.insert(root);
                        }
                    }
                }
                true
            }
        }
        let mut w = Walk { checker: self, params, hit: HashSet::new() };
        visit::visit_body(body, &mut w);
        w.hit
    }

    /// The places this call can resize: the receiver of a size-changing
    /// method, and each argument handed to a parameter the callee resizes,
    /// with the callee's name when it's an argument. A part of a name counts
    /// as the name (`self.items.push` resizes `self`); an element reached by
    /// indexing doesn't (`v[i].tags` is the element's own vector, not `v`'s
    /// buffer), which `extract_root_and_fields` already answers.
    pub(crate) fn resized_places<'e>(&self, expr: &'e Expr) -> Vec<(&'e Expr, Option<String>)> {
        let mut out: Vec<(&'e Expr, Option<String>)> = Vec::new();
        match &expr.kind {
            ExprKind::MethodCall { object, method, args, .. } => {
                let declared = self.program.declared_effects(expr.id).changes_size();
                let user = self
                    .receiver_type_name(object)
                    .and_then(|ty| self.method_resizes.get(&(ty, method.clone())));
                if declared {
                    out.push((object, None));
                } else if user.is_some_and(|(s, _)| *s) {
                    out.push((object, Some(method.clone())));
                }
                // Through an interface object the body can't be seen, so a
                // `mutate` parameter is taken to resize what it's handed
                // (comp.advanced: "conservative: assume both Grow and Shrink").
                if let Some(sig) = self.interface_method(object, method) {
                    if matches!(sig.self_param, rask_types::SelfParam::Mutate) {
                        out.push((object, Some(method.clone())));
                    }
                    for (i, a) in args.iter().enumerate() {
                        if matches!(sig.params.get(i), Some((_, rask_types::ParamMode::Mutate))) {
                            out.push((&a.expr, Some(method.clone())));
                        }
                    }
                }
                if let Some((_, params)) = user {
                    for (i, a) in args.iter().enumerate() {
                        if params.get(i).copied().unwrap_or(false) {
                            out.push((&a.expr, Some(method.clone())));
                        }
                    }
                }
            }
            ExprKind::Call { func, args } => {
                if let Some(name) = func.name() {
                    if let Some(params) = self.fn_resizes.get(name) {
                        for (i, a) in args.iter().enumerate() {
                            if params.get(i).copied().unwrap_or(false) {
                                out.push((&a.expr, Some(name.to_string())));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// The interface method a call on an `any Interface` value dispatches to.
    fn interface_method(&self, object: &Expr, method: &str) -> Option<rask_types::MethodSig> {
        let rask_types::Type::InterfaceObject { decl: Some(id), .. } = self.node_ty(&object.id)? else {
            return None;
        };
        match self.program.types.get(*id)? {
            rask_types::TypeDef::Interface { methods, .. } => {
                methods.iter().find(|m| m.name == method).cloned()
            }
            _ => None,
        }
    }

    /// A statement that replaces a name's collection outright: `v = …`, or
    /// `h.items = …` for a field that owns heap memory. Writing an element
    /// (`v[i] = …`) or a Copy field moves no buffer.
    pub(crate) fn replaced_place<'e>(&self, stmt: &'e Stmt) -> Option<&'e Expr> {
        let StmtKind::Assign { target, .. } = &stmt.kind else { return None };
        Self::extract_root_and_fields(target).0?;
        let ty = self.node_ty(&target.id)?;
        (!self.is_copy(ty)).then_some(target)
    }
}

/// Resize facts per function and per method, filed for call sites.
pub(crate) type FnResizes = HashMap<String, Vec<bool>>;
pub(crate) type MethodResizes = HashMap<(String, String), (bool, Vec<bool>)>;

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! type.temporary: a value that lives for the expression that makes it.
//!
//! A struct declared `@temporary` makes such values; a string slice is the
//! stdlib's. One rule covers every way of keeping one (TM2): a binding, a field
//! of a type that isn't itself temporary, or an element of a type that stores
//! its arguments. A function may return one — the call is part of the
//! expression that uses it — and a tuple or an optional holding one is itself
//! temporary rather than an error. What decides each of these is read off
//! declarations; nothing here names a stdlib type.

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};

use super::errors::{KeptBy, TypeError};
use super::type_defs::TypeDef;
use super::TypeChecker;
use crate::types::{GenericArg, Type, TypeId};

impl TypeChecker {
    /// The `@temporary` struct `ty` names, if it does.
    fn temporary_struct(&self, ty: &Type) -> Option<TypeId> {
        let id = match self.resolve_named(ty) {
            Type::Named(id) | Type::Generic { base: id, .. } => id,
            _ => return None,
        };
        matches!(self.types.get(id), Some(TypeDef::Struct { temporary: Some(_), .. })).then_some(id)
    }

    /// TM1: is a value of this type temporary? A declared temporary type, or a
    /// tuple, optional, result, array or union with one inside.
    pub(super) fn is_temporary(&self, ty: &Type) -> bool {
        if self.temporary_struct(ty).is_some() {
            return true;
        }
        match ty {
            Type::Tuple(elems) | Type::Union(elems) => elems.iter().any(|e| self.is_temporary(e)),
            Type::Result { ok, err } => self.is_temporary(ok) || self.is_temporary(err),
            Type::Array { elem, .. } => self.is_temporary(elem),
            _ => false,
        }
    }

    /// TM2: the temporary a value of this type would store, with the type
    /// that stores it: `Vec<StringSlice>` stores a `StringSlice`. A generic
    /// type stores an argument when the runtime owns storage for it (a
    /// `@handle`) or a field mentions the parameter. `Sequence<StringSlice>`
    /// stores nothing — it makes its items one call at a time — and neither
    /// does a function type.
    fn stored_temporary(&self, ty: &Type) -> Option<(Type, Type)> {
        match ty {
            Type::Generic { base, args } => {
                let held = self.held_params(*base);
                for (i, a) in args.iter().enumerate() {
                    let GenericArg::Type(t) = a else { continue };
                    if held.contains(&i) && self.is_temporary(t) {
                        return Some((ty.clone(), (**t).clone()));
                    }
                    if let Some(found) = self.stored_temporary(t) {
                        return Some(found);
                    }
                }
                None
            }
            Type::Tuple(elems) | Type::Union(elems) => elems.iter().find_map(|e| self.stored_temporary(e)),
            Type::Result { ok, err } => self.stored_temporary(ok).or_else(|| self.stored_temporary(err)),
            Type::Array { elem, .. } => self.stored_temporary(elem),
            _ => None,
        }
    }

    /// Which of a generic type's parameters its values store.
    fn held_params(&self, base: TypeId) -> Vec<usize> {
        let name = self.types.type_name(base);
        let (params, fields): (&[String], Vec<&Type>) = match self.types.get(base) {
            Some(TypeDef::Struct { type_params, fields, .. }) => {
                (type_params, fields.iter().map(|(_, t)| t).collect())
            }
            Some(TypeDef::Enum { type_params, variants, .. }) => {
                (type_params, variants.iter().flat_map(|(_, ts)| ts.iter()).collect())
            }
            _ => return Vec::new(),
        };
        if rask_stdlib::is_runtime_handle(&name) {
            return (0..params.len()).collect();
        }
        params
            .iter()
            .enumerate()
            .filter(|(_, p)| fields.iter().any(|f| f.contains(&|t| names_param(t, p))))
            .map(|(i, _)| i)
            .collect()
    }

    /// The ways a temporary type says to keep one of its values, each with the
    /// type it returns: `[("view", "StringView"), ("to_string", "string")]`.
    fn keepers(&self, ty: &Type) -> Vec<(String, String)> {
        let Some(id) = self.temporary_struct(ty) else { return Vec::new() };
        let Some(TypeDef::Struct { temporary: Some(keep), methods, .. }) = self.types.get(id) else {
            return Vec::new();
        };
        keep.iter()
            .filter_map(|k| {
                let m = methods.iter().find(|m| m.name == *k)?;
                Some((k.clone(), self.types.display_type_names(&m.ret).to_string()))
            })
            .collect()
    }

    /// The temporary at the heart of a type: the struct itself, or the one a
    /// tuple or optional carries.
    fn temporary_inside(&self, ty: &Type) -> Option<Type> {
        if self.temporary_struct(ty).is_some() {
            return Some(ty.clone());
        }
        match ty {
            Type::Tuple(elems) | Type::Union(elems) => elems.iter().find_map(|e| self.temporary_inside(e)),
            Type::Result { ok, err } => self.temporary_inside(ok).or_else(|| self.temporary_inside(err)),
            Type::Array { elem, .. } => self.temporary_inside(elem),
            _ => None,
        }
    }

    fn kept(&self, temp: &Type, holder: KeptBy, span: Span) -> TypeError {
        let temp = self.temporary_inside(temp).unwrap_or_else(|| temp.clone());
        TypeError::TemporaryKept {
            ty: self.types.display_type_names(&temp).to_string(),
            holder,
            keep: self.keepers(&temp),
            span,
        }
    }

    /// TM2 over the program: bindings, the fields of types that aren't
    /// temporary themselves, and expressions whose value stores a temporary.
    pub(super) fn check_temporaries(&mut self, decls: &[Decl], node_types: &HashMap<NodeId, Type>) {
        let mut found = Vec::new();
        for decl in decls {
            self.temporary_fields(decl, &mut found);
            let mut walk = Walk { checker: self, node_types, out: Vec::new(), reported: HashSet::new() };
            rask_ast::visit::visit_decl(decl, &mut walk);
            found.extend(walk.out);
        }
        self.errors.extend(found);
    }

    /// A field whose type is or stores a temporary, on a type that isn't
    /// temporary itself.
    fn temporary_fields(&self, decl: &Decl, out: &mut Vec<TypeError>) {
        let DeclKind::Struct(s) = &decl.kind else { return };
        let Some(id) = self.types.get_type_id(&s.name) else { return };
        let Some(TypeDef::Struct { fields, temporary: None, .. }) = self.types.get(id) else { return };
        for (name, ty) in fields {
            let temp = if self.is_temporary(ty) {
                Some(ty.clone())
            } else {
                self.stored_temporary(ty).map(|(_, t)| t)
            };
            if let Some(temp) = temp {
                let span = s.fields.iter().find(|f| f.name == *name).map(|f| f.name_span).unwrap_or(decl.span);
                out.push(self.kept(&temp, KeptBy::Field { owner: s.name.clone(), field: name.clone() }, span));
            }
        }
    }
}

/// Does this type name the parameter `p`?
fn names_param(t: &Type, p: &str) -> bool {
    matches!(t, Type::UnresolvedNamed(n) if n == p)
}

struct Walk<'a> {
    checker: &'a TypeChecker,
    node_types: &'a HashMap<NodeId, Type>,
    out: Vec<TypeError>,
    /// Expressions already reported, with everything inside them.
    reported: HashSet<NodeId>,
}

impl Walk<'_> {
    fn ty(&self, id: NodeId) -> Option<Type> {
        self.node_types.get(&id).map(|t| self.checker.resolve_named(t))
    }

    fn mark(&mut self, e: &Expr) {
        rask_ast::visit::walk_expr_pruned(e, &mut |inner| {
            self.reported.insert(inner.id);
            true
        });
    }
}

impl<'d> rask_ast::visit::Visit<'d> for Walk<'_> {
    fn stmt(&mut self, s: &'d Stmt) -> bool {
        if let StmtKind::Let { name, name_span, init, .. } | StmtKind::Mut { name, name_span, init, .. } = &s.kind {
            if let Some(t) = self.ty(init.id) {
                let holder = if self.checker.is_temporary(&t) {
                    Some((t.clone(), KeptBy::Binding(name.clone())))
                } else {
                    self.checker.stored_temporary(&t).map(|(container, temp)| {
                        (temp, KeptBy::Element(self.checker.types.display_type_names(&container).to_string()))
                    })
                };
                if let Some((temp, by)) = holder {
                    self.out.push(self.checker.kept(&temp, by, *name_span));
                    self.mark(init);
                }
            }
        }
        true
    }

    fn expr(&mut self, e: &'d Expr) -> bool {
        if self.reported.contains(&e.id) {
            return false;
        }
        if matches!(e.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) {
            if let Some(t) = self.ty(e.id) {
                if let Some((container, temp)) = self.checker.stored_temporary(&t) {
                    let by = KeptBy::Element(self.checker.types.display_type_names(&container).to_string());
                    self.out.push(self.checker.kept(&temp, by, e.span));
                    self.mark(e);
                    return false;
                }
            }
        }
        true
    }
}

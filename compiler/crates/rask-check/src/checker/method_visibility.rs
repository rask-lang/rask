// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Who may call a method (struct.modules/V1, V2, V5).
//!
//! A `private` method is for its own type's methods. A method without
//! `public` belongs to the package that declared it, and the stdlib is a
//! package of its own. A method in a conformance block has no visibility of
//! its own: it is part of the conformance.
//!
//! Checked once inference has settled every callee, against the call's own
//! position: the type whose methods it sits in, and the package its span
//! belongs to. Neither can be read off the checker's mode when a deferred
//! constraint is solved, which is why the check waits for this pass.

use rask_ast::{NodeId, Span};

use super::errors::TypeError;
use super::type_defs::Callee;
use super::type_table::TypeOwner;
use super::TypeChecker;

use rask_types::{Type, TypeDef, TypeId};

/// Who may call a method that isn't public.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MethodAccess {
    /// `private` (V5): the type's own methods only.
    Private,
    /// No `public` (V1): code in the declaring package.
    Package(TypeOwner),
}

/// A method call, with where it was written.
pub(super) struct PlacedCall {
    pub call: NodeId,
    pub span: Span,
    /// The type whose methods the call sits in, if any.
    pub inside: Option<TypeId>,
    /// The receiver is a value (`p.make()`), not a type (`Point.make()`).
    pub on_value: bool,
}

impl TypeChecker {
    /// Remember a method call for `validate_method_visibility`. A body can be
    /// inferred more than once; the first record stands.
    pub(super) fn note_method_call(&mut self, call: NodeId, span: Span, on_value: bool) {
        if self.method_calls.iter().any(|c| c.call == call) {
            return;
        }
        let inside = match self.current_self_type.as_ref().map(|t| self.resolve_named(t)) {
            Some(Type::Named(id)) | Some(Type::Generic { base: id, .. }) => Some(id),
            _ => None,
        };
        self.method_calls.push(PlacedCall { call, span, inside, on_value });
    }

    /// A method with no `self` called on a value: `p.make()` for a static
    /// `make()`. Native called it and dropped the receiver; the interpreter
    /// passed the receiver as an extra argument and failed. A static method
    /// is reached through its type, `Point.make()`.
    pub(super) fn validate_static_calls_on_values(&mut self) {
        let mut found = Vec::new();
        for placed in &self.method_calls {
            if !placed.on_value {
                continue;
            }
            let Some(Callee::Method { recv, method, .. }) = self.call_targets.get(&placed.call) else {
                continue;
            };
            let recv = self.resolve_named(&self.ctx.apply(recv));
            let (Type::Named(id) | Type::Generic { base: id, .. }) = recv else { continue };
            let sig = match self.types.get(id) {
                Some(TypeDef::Struct { methods, .. })
                | Some(TypeDef::Enum { methods, .. })
                | Some(TypeDef::NominalAlias { methods, .. }) => methods.iter().find(|m| &m.name == method),
                _ => None,
            };
            if sig.is_some_and(|m| matches!(m.self_param, rask_types::SelfParam::None)) {
                found.push(TypeError::StaticCallOnValue {
                    ty: self.types.type_name(id),
                    method: method.clone(),
                    span: placed.span,
                });
            }
        }
        self.errors.extend(found);
    }

    /// The package a span's code belongs to.
    fn owner_of_span(&self, span: Span) -> TypeOwner {
        if span.file_id >= rask_stdlib::stubs::STDLIB_FILE_ID_BASE {
            return TypeOwner::Stdlib;
        }
        match self.package_of(span) {
            Some(p) => TypeOwner::Package(p.to_string()),
            None => TypeOwner::Program,
        }
    }

    pub(super) fn validate_method_visibility(&mut self) {
        let calls = std::mem::take(&mut self.method_calls);
        for placed in &calls {
            let Some(Callee::Method { recv, method, .. }) = self.call_targets.get(&placed.call) else {
                continue;
            };
            let recv = self.resolve_named(&self.ctx.apply(recv));
            let method = method.clone();
            let Some(access) = self.method_access(&recv, &method) else { continue };
            let allowed = match &access {
                MethodAccess::Private => matches!(
                    recv,
                    Type::Named(id) | Type::Generic { base: id, .. } if placed.inside == Some(id)
                ),
                MethodAccess::Package(owner) => *owner == self.owner_of_span(placed.span),
            };
            if allowed {
                continue;
            }
            let ty = super::receiver_name(&recv, &self.types).unwrap_or_else(|| recv.to_string());
            let declared_by = match access {
                MethodAccess::Private => None,
                MethodAccess::Package(TypeOwner::Stdlib) => Some("the standard library".to_string()),
                MethodAccess::Package(TypeOwner::Package(p)) => Some(format!("package `{p}`")),
                MethodAccess::Package(TypeOwner::Program) => Some("this program".to_string()),
            };
            self.errors.push(TypeError::MethodNotVisible { ty, method, declared_by, span: placed.span });
        }
    }

    /// The restriction on calling `method` on `recv`, when it has one.
    ///
    /// A type the checker holds answers from its own record. A builtin it
    /// doesn't (`string`) has its methods in the stdlib's stub registry, and
    /// those are the stdlib's.
    fn method_access(&self, recv: &Type, method: &str) -> Option<MethodAccess> {
        if let Type::Named(id) | Type::Generic { base: id, .. } = recv {
            return self.types.method_access(*id, method).cloned();
        }
        let prefix = super::receiver_name(recv, &self.types)?;
        let stub = rask_stdlib::lookup_method(&prefix, method)?;
        (!stub.is_pub).then_some(MethodAccess::Package(TypeOwner::Stdlib))
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime handles: types whose value is one word, the address of storage the
//! runtime made, with no fields for the compiler to lay out.
//!
//! A stdlib type says so on its declaration — `@handle`, or
//! `@handle(release: "rask_vec_free")` when owning one means freeing it with
//! that runtime function. This is the one place that attribute is read; every
//! pass asks the declaration (`TypeDef::runtime_handle`), by its id.

use rask_ast::attr::Attr;

use crate::TypeDef;

/// What `@handle` on a declaration says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeHandle<'a> {
    /// What frees the storage when the owner dies. `None` when nothing does on
    /// its own: the value is closed explicitly (`@resource`), is shared by
    /// count (`Shared`, whose release depends on its strategy), or doesn't
    /// own what it points at (`Link`).
    pub release: Option<&'a str>,
    /// `@handle(niche)`: a present value is never null, so `T?` is the value
    /// with null for `none` (`Link`).
    pub niche: bool,
}

impl<'a> RuntimeHandle<'a> {
    /// The handle these attributes declare, if they carry `@handle`.
    pub fn declared(attrs: &'a [Attr]) -> Option<Self> {
        let attr = attrs.iter().find(|a| a.is("handle"))?;
        let args = || attr.args.iter().flatten();
        let release = args()
            .find(|a| a.label.as_deref() == Some("release"))
            .and_then(|a| a.value.expr())
            .and_then(|e| match &e.kind {
                rask_ast::expr::ExprKind::String(sym) => Some(sym.as_str()),
                _ => None,
            });
        let niche = args().any(|a| a.label.is_none() && a.value.ident() == Some("niche"));
        Some(RuntimeHandle { release, niche })
    }
}

impl TypeDef {
    /// The runtime handle this declaration is, if it carries `@handle`.
    pub fn runtime_handle(&self) -> Option<RuntimeHandle<'_>> {
        RuntimeHandle::declared(self.attrs())
    }
}

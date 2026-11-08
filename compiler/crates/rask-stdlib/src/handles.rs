// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime handles: types whose value is one word, the address of storage the
//! runtime made, with no fields for the compiler to lay out.
//!
//! A stdlib type says so on its declaration — `@handle`, or
//! `@handle(release: "rask_vec_free")` when owning one means freeing it with
//! that runtime function. The compiler-provided ones (mem.shared-rack-heap,
//! BX1–BX4) have no declaration to annotate and are listed here.
//!
//! Nine hand lists used to answer this, and disagreed: `Set` and `Wide` are
//! real structs yet counted as opaque in two of them, `MultitaskingRuntime`
//! and the retired `Cell` were still in others, and MIR's lacked `Handle`.

use std::collections::HashMap;
use std::sync::OnceLock;

use rask_ast::decl::DeclKind;

/// One handle type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeHandle {
    /// What frees the storage when the owner dies. `None` when nothing does on
    /// its own: the value is closed explicitly (`@resource`), is shared by
    /// count (`Shared`, whose release depends on its strategy), or doesn't
    /// own what it points at (`Link`).
    pub release: Option<&'static str>,
    /// `@handle(niche)`: a present value is never null, so `T?` is the value
    /// with null for `none` (`Link`).
    pub niche: bool,
}

/// Compiler-provided handles, which no `stdlib/*.rk` declaration describes.
const COMPILER_PROVIDED: &[(&str, RuntimeHandle)] = &[
    ("Shared", RuntimeHandle { release: None, niche: false }),
    ("Heap", RuntimeHandle { release: None, niche: false }),
    // Only ever named to make a pair of ends; the ends are the values.
    ("Channel", RuntimeHandle { release: None, niche: false }),
];

fn declared() -> &'static HashMap<String, RuntimeHandle> {
    static HANDLES: OnceLock<HashMap<String, RuntimeHandle>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        let mut out = HashMap::new();
        for decl in crate::stubs::parsed_decls() {
            let DeclKind::Struct(s) = &decl.kind else { continue };
            let Some(attr) = s.attrs.iter().find(|a| a.is("handle")) else { continue };
            let release = attr
                .args
                .iter()
                .flatten()
                .find(|a| a.label.as_deref() == Some("release"))
                .and_then(|a| a.value.expr())
                .and_then(|e| match &e.kind {
                    rask_ast::expr::ExprKind::String(sym) => Some(sym.as_str()),
                    _ => None,
                });
            let niche = attr.args.iter().flatten().any(|a| a.label.is_none() && a.value.ident() == Some("niche"));
            out.insert(s.name.clone(), RuntimeHandle { release, niche });
        }
        out
    })
}

/// The handle `name` is, if it is one. `name` is the bare type name, without
/// type arguments.
pub fn runtime_handle(name: &str) -> Option<RuntimeHandle> {
    declared().get(name).copied().or_else(|| {
        COMPILER_PROVIDED.iter().find(|(n, _)| *n == name).map(|(_, h)| *h)
    })
}

/// Is `name` a runtime handle: one word, its storage the runtime's?
pub fn is_runtime_handle(name: &str) -> bool {
    runtime_handle(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations_say_which_types_are_handles() {
        assert_eq!(runtime_handle("Vec").and_then(|h| h.release), Some("rask_vec_free"));
        assert!(is_runtime_handle("File"));
        assert!(is_runtime_handle("Link"));
        assert!(runtime_handle("Link").is_some_and(|h| h.niche));
        assert!(!runtime_handle("Vec").is_some_and(|h| h.niche));
        assert_eq!(runtime_handle("Link").and_then(|h| h.release), None);
        assert!(is_runtime_handle("Shared"));
    }

    /// Real structs, whatever their storage holds.
    #[test]
    fn a_struct_with_fields_is_not_a_handle() {
        assert!(!is_runtime_handle("Set"));
        assert!(!is_runtime_handle("Wide"));
        assert!(!is_runtime_handle("Path"));
    }
}

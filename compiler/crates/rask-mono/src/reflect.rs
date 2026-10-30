// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `reflect.variants<T>()`, out of an enum's layout.
//!
//! A variant's payload is storage, the way a struct's fields are, so its facts
//! come from the layout `reflect.fields<T>()` already reads. Native finds the
//! layout mono emitted and the interpreter computes the same one from the same
//! declaration; both hand it here, so the answer is built once.

use rask_ast::decl::field_attrs;
use rask_ast::ty::TypeExpr;
use rask_check::TypeTable;

use crate::EnumLayout;

/// One variant, as `VariantInfo` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantFact {
    pub name: String,
    /// `@rename("...")` on the variant, else its name.
    pub serial_name: String,
    pub fields: Vec<PayloadField>,
}

/// One payload field, as the `FieldInfo` in `VariantInfo.fields` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadField {
    /// What the declaration named it; a positional payload's index.
    pub name: String,
    pub type_name: String,
    /// From the start of the enum value, the same origin a struct field's
    /// offset counts from.
    pub offset: u32,
    pub size: u32,
    pub has_default: bool,
}

/// The variants in declaration order. `subst` is the instantiation's
/// arguments by parameter name, for a layout that still records `T` — mono's
/// shared one does (std.reflect/R5).
pub fn variant_facts(
    layout: &EnumLayout,
    types: &TypeTable,
    subst: &[(String, TypeExpr)],
) -> Vec<VariantFact> {
    layout
        .variants
        .iter()
        .map(|v| VariantFact {
            name: v.name.clone(),
            serial_name: field_attrs::serial_name(&v.attrs, &v.name),
            fields: v
                .fields
                .iter()
                .map(|f| PayloadField {
                    name: f.name.clone(),
                    type_name: types
                        .resolve_type_names(&f.ty)
                        .to_type_expr()
                        .substitute(&|n| subst.iter().find(|(p, _)| p == n).map(|(_, a)| a.clone()))
                        .to_string(),
                    offset: v.payload_offset + f.offset,
                    size: f.size,
                    has_default: f.has_declared_default,
                })
                .collect(),
        })
        .collect()
}

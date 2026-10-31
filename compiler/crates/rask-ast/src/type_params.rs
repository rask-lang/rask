// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which written names are a declaration's type parameters.
//!
//! Read from syntax alone, so the checker, monomorphization and ownership all
//! derive the same ordered list from the same declaration.

use crate::decl::{EnumDecl, FnDecl, StructDecl};
use crate::ty::TypeExpr;

/// PC1: single uppercase ASCII letter — always a type parameter in signatures.
pub fn is_type_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
}

/// PC1: a function's type params — explicit `<T>` declarations plus every
/// single uppercase letter in its signature types, in signature order
/// (params left to right, then return type).
///
/// Shared by the type checker and the monomorphizer so both derive the same
/// ordered list. Uses a fresh TypeTable: results match the populated table
/// because single-letter names can never be registered types (PC3), and only
/// single letters are collected.
pub fn signature_type_param_names(f: &FnDecl) -> Vec<String> {
    let mut names: Vec<String> = f.type_params.iter().map(|p| p.name.clone()).collect();
    let mut add = |n: &str| {
        if is_type_param_name(n) && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for p in &f.params {
        if let (false, Some(ty)) = (p.name == "self", &p.ty) {
            for_each_written_name(ty, &mut add);
        }
    }
    if let Some(rt) = &f.ret_ty {
        for_each_written_name(rt, &mut add);
    }
    names
}

/// Every single-segment name a written type mentions, heads included, in
/// written order: `Map<K, Vec<V>>` gives `Map`, `K`, `Vec`, `V`.
fn for_each_written_name(ty: &TypeExpr, f: &mut impl FnMut(&str)) {
    ty.walk_paths(&mut |path| {
        if let [name] = path {
            f(name);
        }
    });
}

/// PC1 for a type declaration: explicit `<T>` declarations plus every single
/// uppercase letter appearing in its field or payload types, in declaration
/// order.
///
/// `gradual-constraints` lists struct fields and enum payloads as signature
/// positions alongside function parameters, but only the function side was
/// wired up — so `struct Pair { first: T  second: U }` registered with no type
/// parameters at all, and `Pair<i64, string>` had nothing to match against.
/// SYNTAX.md's own `Pair` example didn't compile (#913).
pub fn declared_type_param_names<'a>(
    explicit: &[crate::decl::TypeParam],
    member_types: impl Iterator<Item = &'a TypeExpr>,
) -> Vec<String> {
    let mut names: Vec<String> = explicit.iter().map(|p| p.name.clone()).collect();
    let mut add = |n: &str| {
        if is_type_param_name(n) && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for ty in member_types {
        for_each_written_name(ty, &mut add);
    }
    names
}

/// PC1 for a struct: explicit `<T>` plus single letters in its field types.
pub fn struct_type_param_names(s: &StructDecl) -> Vec<String> {
    declared_type_param_names(&s.type_params, s.fields.iter().map(|f| &f.ty))
}

/// PC1 for an enum: explicit `<T>` plus single letters in its payload types.
pub fn enum_type_param_names(e: &EnumDecl) -> Vec<String> {
    declared_type_param_names(
        &e.type_params,
        e.variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)),
    )
}


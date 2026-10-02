// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The operator interfaces, and the name a conformance's method is filed under.
//!
//! `type.operator-resolution/OR2` lists twelve interfaces and OR4 lets a type carry
//! one conformance per `(Self, Rhs)` pair — so `Meters` may answer both
//! `Mul<f64>` and `Mul<Meters>`, and both blocks declare a method called `mul`.
//! One name for two bodies is a symbol collision at every layer that keys
//! methods by `{Type}_{method}`, so the applied interface's argument goes into the
//! name: `mul$f64` and `mul$Meters`.
//!
//! Three registries need that rule — the checker's method table,
//! monomorphization's, and the interpreter's — so it lives here rather than as
//! three near-identical string joins. The suffix never reaches the reader:
//! `method_display` takes it back off for diagnostics.

/// OR2: the declared operator interfaces, as `(desugared method, interface)`.
///
/// `Equal` and `Comparable` are absent on purpose — OR9 keeps comparison
/// same-type on both sides, so `eq`/`lt`/… are not resolved on the pair and
/// their conformances need no disambiguation.
use crate::ty::TypeExpr;

pub const OPERATOR_TRAITS: &[(&str, &str)] = &[
    ("add", "Add"),
    ("sub", "Sub"),
    ("mul", "Mul"),
    ("div", "Div"),
    ("rem", "Rem"),
    ("neg", "Neg"),
    ("bit_and", "BitAnd"),
    ("bit_or", "BitOr"),
    ("bit_xor", "BitXor"),
    ("bit_not", "BitNot"),
    ("shl", "Shl"),
    ("shr", "Shr"),
];

/// The operator interface a desugared method name belongs to.
pub fn operator_interface(method: &str) -> Option<&'static str> {
    OPERATOR_TRAITS
        .iter()
        .find(|(m, _)| *m == method_display(method))
        .map(|(_, t)| *t)
}

/// The method an operator interface requires.
pub fn operator_interface_method(interface_base: &str) -> Option<&'static str> {
    OPERATOR_TRAITS
        .iter()
        .find(|(_, t)| *t == interface_base)
        .map(|(m, _)| *m)
}

/// The two unary operator interfaces. They take no `Rhs`, so a type can carry only
/// one of each and the name needs no suffix.
pub fn is_unary_operator_interface(interface_base: &str) -> bool {
    matches!(interface_base, "Neg" | "BitNot")
}

/// The name a conformance's method is filed under, or `None` when the block
/// isn't an operator conformance supplying that method.
///
/// `target_ty` is the `extend` header's type, which is what `Rhs` defaults to
/// (`type.generics/GT4`): `Point implements Add` is `Add<Point>`.
pub fn conformance_method_name(
    target_ty: &TypeExpr,
    interface: Option<&TypeExpr>,
    method: &str,
) -> Option<String> {
    let interface = interface?;
    let rhs = interface.args().first().and_then(TypeExpr::name);
    filed_operator_method(&target_ty.name()?, &interface.name()?, rhs.as_deref(), method)
}

/// `conformance_method_name` on names: the receiver's, the interface's, and the
/// head of its applied `Rhs` when one is written.
pub fn filed_operator_method(
    self_base: &str,
    interface_base: &str,
    rhs: Option<&str>,
    method: &str,
) -> Option<String> {
    if operator_interface_method(interface_base) != Some(method) {
        return None;
    }
    let rhs = filed_rhs(self_base, interface_base, rhs)?;
    Some(format!("{}${}", method, rhs))
}

/// The `Rhs` an operator conformance's method is filed under: what the header
/// wrote, or the receiver when it wrote nothing or `Self`. `None` for a unary
/// interface, which has no `Rhs`.
pub fn filed_rhs(self_base: &str, interface_base: &str, rhs: Option<&str>) -> Option<String> {
    if is_unary_operator_interface(interface_base) {
        return None;
    }
    Some(match rhs {
        None | Some("Self") => self_base.to_string(),
        Some(r) => r.to_string(),
    })
}

/// The operator method a filed name stands for: `mul$f64` → `mul`.
pub fn method_display(name: &str) -> &str {
    match name.split_once('$') {
        Some((base, _)) if operator_interface_method_exists(base) => base,
        _ => name,
    }
}

fn operator_interface_method_exists(method: &str) -> bool {
    OPERATOR_TRAITS.iter().any(|(m, _)| *m == method)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> TypeExpr {
        TypeExpr::named(s)
    }

    #[test]
    fn the_applied_argument_goes_into_the_name() {
        let iface = TypeExpr::generic("Mul", vec![n("f64")]);
        assert_eq!(
            conformance_method_name(&n("Meters"), Some(&iface), "mul").as_deref(),
            Some("mul$f64")
        );
    }

    #[test]
    fn a_bare_header_means_the_receiver() {
        assert_eq!(
            conformance_method_name(&n("Point"), Some(&n("Add")), "add").as_deref(),
            Some("add$Point")
        );
    }

    #[test]
    fn a_unary_operator_keeps_its_name() {
        assert_eq!(conformance_method_name(&n("Point"), Some(&n("Neg")), "neg"), None);
    }

    #[test]
    fn a_method_the_interface_did_not_ask_for_keeps_its_name() {
        let iface = TypeExpr::generic("Mul", vec![n("f64")]);
        assert_eq!(conformance_method_name(&n("Meters"), Some(&iface), "scaled"), None);
    }

    #[test]
    fn the_suffix_comes_back_off_for_the_reader() {
        assert_eq!(method_display("mul$f64"), "mul");
        assert_eq!(method_display("mul"), "mul");
        // Not an operator method: a `$` in some other name stays put.
        assert_eq!(method_display("render$html"), "render$html");
    }
}

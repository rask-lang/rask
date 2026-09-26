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
/// (`type.generics/GT4`): `extend Point implements Add` is `Add<Point>`.
pub fn conformance_method_name(
    target_ty: &str,
    interface_ref: Option<&str>,
    method: &str,
) -> Option<String> {
    let self_base = base_name(target_ty);
    let interface_ref = interface_ref?;
    let base = base_name(interface_ref);
    if operator_interface_method(base) != Some(method) {
        return None;
    }
    if is_unary_operator_interface(base) {
        return None;
    }
    let rhs = interface_ref_arg(interface_ref).unwrap_or(self_base);
    let rhs = if rhs == "Self" { self_base } else { rhs };
    Some(format!("{}${}", method, base_name(rhs)))
}

/// The operator method a filed name stands for: `mul$f64` → `mul`.
pub fn method_display(name: &str) -> &str {
    match name.split_once('$') {
        Some((base, _)) if operator_interface_method_exists(base) => base,
        _ => name,
    }
}

/// The `Rhs` a filed operator method names: `mul$f64` → `f64`.
pub fn method_rhs(name: &str) -> Option<&str> {
    match name.split_once('$') {
        Some((base, rhs)) if operator_interface_method_exists(base) => Some(rhs),
        _ => None,
    }
}

fn operator_interface_method_exists(method: &str) -> bool {
    OPERATOR_TRAITS.iter().any(|(m, _)| *m == method)
}

/// The written type argument of an interface reference: `Mul<f64>` → `f64`.
fn interface_ref_arg(interface_ref: &str) -> Option<&str> {
    let (_, rest) = interface_ref.split_once('<')?;
    let inner = rest.trim().strip_suffix('>')?;
    let first = inner.split(',').next()?.trim();
    (!first.is_empty()).then_some(first)
}

fn base_name(s: &str) -> &str {
    s.split('<').next().unwrap_or(s).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_applied_argument_goes_into_the_name() {
        let iface = Some("Mul<f64>");
        assert_eq!(
            conformance_method_name("Meters", iface, "mul").as_deref(),
            Some("mul$f64")
        );
    }

    #[test]
    fn a_bare_header_means_the_receiver() {
        let iface = Some("Add");
        assert_eq!(
            conformance_method_name("Point", iface, "add").as_deref(),
            Some("add$Point")
        );
    }

    #[test]
    fn a_unary_operator_keeps_its_name() {
        let iface = Some("Neg");
        assert_eq!(conformance_method_name("Point", iface, "neg"), None);
    }

    #[test]
    fn a_method_the_interface_did_not_ask_for_keeps_its_name() {
        let iface = Some("Mul<f64>");
        assert_eq!(conformance_method_name("Meters", iface, "scaled"), None);
    }

    #[test]
    fn the_suffix_comes_back_off_for_the_reader() {
        assert_eq!(method_display("mul$f64"), "mul");
        assert_eq!(method_display("mul"), "mul");
        // Not an operator method: a `$` in some other name stays put.
        assert_eq!(method_display("render$html"), "render$html");
        assert_eq!(method_rhs("mul$Meters"), Some("Meters"));
        assert_eq!(method_rhs("scale"), None);
    }
}

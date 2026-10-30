// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The operator interfaces, and the name a conformance's method is filed under.
//!
//! `type.operator-resolution/OR2` lists the interfaces, and `stdlib/ops.rk`
//! declares them. This module reads that file and nothing else: every public
//! interface there is an operator interface, its one required method is the
//! name its operator desugars to, and it takes a right-hand side exactly when
//! it declares an `Rhs` parameter. A Rust copy of the list used to sit in
//! `rask-ast` beside the file, and nothing checked the two agreed.
//!
//! OR4 lets a type carry one conformance per `(Self, Rhs)` pair, so `Meters`
//! may answer both `Mul<f64>` and `Mul<Meters>`, and both blocks declare a
//! method called `mul`. One name for two bodies is a symbol collision at every
//! layer that keys methods by `{Type}_{method}`, so the applied interface's
//! argument goes into the name: `mul$f64` and `mul$Meters`. The checker's
//! method table, monomorphization's and the interpreter's all need that rule,
//! so it lives here. The suffix never reaches the reader: `method_display`
//! takes it back off for diagnostics.
//!
//! `Equal` and `Comparable` aren't operator interfaces: OR9 keeps comparison
//! same-type on both sides, so `eq`/`lt`/… are not resolved on the pair and
//! their conformances need no disambiguation.

use rask_ast::decl::DeclKind;
use rask_ast::ty::TypeExpr;
use std::sync::OnceLock;

/// The file that declares them.
const OPS_FILE: &str = "ops.rk";

struct OperatorInterface {
    name: &'static str,
    method: &'static str,
    has_rhs: bool,
    /// The method takes nothing but `self`.
    unary: bool,
}

fn table() -> &'static [OperatorInterface] {
    static TABLE: OnceLock<Vec<OperatorInterface>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let decls = crate::stubs::parsed_file(OPS_FILE)
            .unwrap_or_else(|| panic!("stdlib/{OPS_FILE} didn't parse; the operator interfaces live there"));
        let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
        decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Interface(i) if i.is_pub => Some(i),
                _ => None,
            })
            .map(|i| {
                let [method] = i.methods.as_slice() else {
                    panic!("stdlib/{OPS_FILE}: operator interface `{}` must require exactly one method", i.name)
                };
                OperatorInterface {
                    name: leak(&i.name),
                    method: leak(&method.name),
                    has_rhs: i.type_params.iter().any(|p| p.name == "Rhs"),
                    unary: method.params.iter().all(|p| p.name == "self"),
                }
            })
            .collect()
    })
}

/// The operator interface a desugared method name belongs to.
pub fn operator_interface(method: &str) -> Option<&'static str> {
    let method = method_display(method);
    table().iter().find(|o| o.method == method).map(|o| o.name)
}

/// The method an operator interface requires.
pub fn operator_interface_method(interface_base: &str) -> Option<&'static str> {
    table().iter().find(|o| o.name == interface_base).map(|o| o.method)
}

/// The operator interfaces with no operand besides `self` (`Neg`, `BitNot`).
pub fn is_unary_operator_interface(interface_base: &str) -> bool {
    table().iter().any(|o| o.name == interface_base && o.unary)
}

/// Whether an operator interface takes an `Rhs`. The unary ones don't, and
/// neither does `Slice`: `x[i..j]` passes the range's ends, but the range isn't
/// a second operand whose type picks a conformance. A type carries one of each
/// of these, so the name needs no suffix.
pub fn has_rhs(interface_base: &str) -> bool {
    table().iter().any(|o| o.name == interface_base && o.has_rhs)
}

/// The name a conformance's method is filed under, or `None` when the block
/// isn't an operator conformance supplying that method.
///
/// `target_ty` is the `extend` header's type, which is what `Rhs` defaults to
/// (`type.generics/GT4`): `Point implements Add` is `Add<Point>`.
///
/// `interface_name` is the interface's own name as the checker identified it
/// (`TypedProgram::conformance_interfaces`), never the header's text:
/// `ops.Mul<f64>` and `o.Mul<f64>` under `import ops as o` are both `Mul`.
pub fn conformance_method_name(
    target_ty: &TypeExpr,
    interface: &TypeExpr,
    interface_name: &str,
    method: &str,
) -> Option<String> {
    let rhs = interface.args().first().and_then(TypeExpr::name);
    filed_operator_method(&target_ty.name()?, interface_name, rhs.as_deref(), method)
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
/// wrote, or the receiver when it wrote nothing or `Self`. `None` for an
/// interface with no `Rhs`.
pub fn filed_rhs(self_base: &str, interface_base: &str, rhs: Option<&str>) -> Option<String> {
    if !has_rhs(interface_base) {
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
        Some((base, _)) if table().iter().any(|o| o.method == base) => base,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::expr::{binary_op_method, unary_op_method, BinOp, UnaryOp};

    fn n(s: &str) -> TypeExpr {
        TypeExpr::named(s)
    }

    #[test]
    fn the_applied_argument_goes_into_the_name() {
        let iface = TypeExpr::generic("Mul", vec![n("f64")]);
        assert_eq!(
            conformance_method_name(&n("Meters"), &iface, "Mul", "mul").as_deref(),
            Some("mul$f64")
        );
    }

    #[test]
    fn a_bare_header_means_the_receiver() {
        assert_eq!(
            conformance_method_name(&n("Point"), &n("Add"), "Add", "add").as_deref(),
            Some("add$Point")
        );
    }

    #[test]
    fn a_unary_operator_keeps_its_name() {
        assert_eq!(conformance_method_name(&n("Point"), &n("Neg"), "Neg", "neg"), None);
        assert!(is_unary_operator_interface("Neg"));
        assert!(is_unary_operator_interface("BitNot"));
        assert!(!is_unary_operator_interface("Add"));
        assert!(!is_unary_operator_interface("Slice"));
    }

    #[test]
    fn slice_takes_no_rhs() {
        assert!(!has_rhs("Slice"));
        assert_eq!(operator_interface(rask_ast::operators::SLICE_METHOD), Some("Slice"));
    }

    #[test]
    fn a_method_the_interface_did_not_ask_for_keeps_its_name() {
        let iface = TypeExpr::generic("Mul", vec![n("f64")]);
        assert_eq!(conformance_method_name(&n("Meters"), &iface, "Mul", "scaled"), None);
    }

    #[test]
    fn a_qualified_header_files_under_the_interface_it_names() {
        let iface = TypeExpr::Named { path: vec!["o".into(), "Mul".into()], args: vec![n("f64")] };
        assert_eq!(
            conformance_method_name(&n("Meters"), &iface, "Mul", "mul").as_deref(),
            Some("mul$f64")
        );
    }

    #[test]
    fn the_suffix_comes_back_off_for_the_reader() {
        assert_eq!(method_display("mul$f64"), "mul");
        assert_eq!(method_display("mul"), "mul");
        // Not an operator method: a `$` in some other name stays put.
        assert_eq!(method_display("render$html"), "render$html");
    }

    /// Desugaring picks the method names before the stdlib is parsed, so it
    /// can't read them from here. What it can't do is name a method `ops.rk`
    /// doesn't declare: every operator that becomes a call outside `Equal` and
    /// `Comparable` must land on an operator interface.
    #[test]
    fn every_desugared_operator_has_an_interface() {
        let comparison = |op: BinOp| op.is_comparison();
        let mut missing = Vec::new();
        for op in BinOp::ALL.into_iter().filter(|op| !comparison(*op)) {
            if let Some(m) = binary_op_method(op) {
                if operator_interface(m).is_none() {
                    missing.push(m);
                }
            }
        }
        for op in [UnaryOp::Neg, UnaryOp::BitNot] {
            if let Some(m) = unary_op_method(op) {
                if operator_interface(m).is_none() {
                    missing.push(m);
                }
            }
        }
        if operator_interface(rask_ast::operators::SLICE_METHOD).is_none() {
            missing.push(rask_ast::operators::SLICE_METHOD);
        }
        assert!(missing.is_empty(), "desugared to, but stdlib/ops.rk declares no interface for: {missing:?}");
    }
}

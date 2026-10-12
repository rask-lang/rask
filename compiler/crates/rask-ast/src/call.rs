// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which parameter each argument of a call fills (SYNTAX.md, named arguments).
//!
//! Unlabeled arguments fill the parameters in order from the first. A label
//! fills the parameter it names, wherever it's written. A parameter nothing
//! fills takes its default. Once a call names one argument it names the rest:
//! an unlabeled argument after a label has no position left to mean.
//!
//! The desugarer places calls to free functions and the checker places calls
//! it resolves through a receiver; both go through `place_args`.
//!
//! Arguments run in the order written (type.operators/EO1). When placing
//! moves one argument that runs code ahead of another, the checker binds
//! those arguments first, in written order (`arg_labels::plan_hoist`).

/// How the call fills one parameter, in parameter order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSlot {
    /// The argument written at this index.
    Written(usize),
    /// The parameter's declared default.
    Default,
}

/// Why a call's arguments don't fit its parameters. Indexes are into the
/// written arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    /// A label no parameter has.
    UnknownLabel { arg: usize },
    /// A second argument for a parameter something already fills.
    Repeated { arg: usize, param: usize },
    /// An unlabeled argument after a labeled one.
    PositionalAfterLabel { arg: usize },
    /// More arguments than parameters.
    TooMany { arg: usize },
    /// A parameter with no argument and no default.
    Missing { param: usize },
}

/// One parameter: its name, and whether it has a default.
pub struct ParamSpec<'a> {
    pub name: &'a str,
    pub has_default: bool,
}

/// Place each written argument (its label, if any) on a parameter.
pub fn place_args(params: &[ParamSpec], labels: &[Option<&str>]) -> Result<Vec<ArgSlot>, PlaceError> {
    let mut slots: Vec<Option<usize>> = vec![None; params.len()];
    let mut labeled = false;
    for (arg, label) in labels.iter().enumerate() {
        let param = match label {
            Some(label) => {
                labeled = true;
                params
                    .iter()
                    .position(|p| p.name == *label)
                    .ok_or(PlaceError::UnknownLabel { arg })?
            }
            None if labeled => return Err(PlaceError::PositionalAfterLabel { arg }),
            None if arg < params.len() => arg,
            None => return Err(PlaceError::TooMany { arg }),
        };
        if slots[param].is_some() {
            return Err(PlaceError::Repeated { arg, param });
        }
        slots[param] = Some(arg);
    }
    slots
        .iter()
        .zip(params)
        .enumerate()
        .map(|(param, (slot, spec))| match slot {
            Some(arg) => Ok(ArgSlot::Written(*arg)),
            None if spec.has_default => Ok(ArgSlot::Default),
            None => Err(PlaceError::Missing { param }),
        })
        .collect()
}

/// Whether evaluating `e` can do anything another argument could see: call
/// something, assign, branch. A name, a literal, a field read and a closure
/// (made, not called) can't.
pub fn runs_code(e: &crate::expr::Expr) -> bool {
    use crate::expr::ExprKind;
    match &e.kind {
        ExprKind::Ident(_)
        | ExprKind::GenericName { .. }
        | ExprKind::Int(..)
        | ExprKind::Float(..)
        | ExprKind::String(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::None
        | ExprKind::Closure { .. } => false,
        ExprKind::Field { object, .. } => runs_code(object),
        ExprKind::Array(items) | ExprKind::Tuple(items) => items.iter().any(runs_code),
        _ => true,
    }
}

/// Whether placing changed nothing: every argument written, in order.
pub fn is_identity(slots: &[ArgSlot]) -> bool {
    slots.iter().enumerate().all(|(i, s)| *s == ArgSlot::Written(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(spec: &[(&'static str, bool)]) -> Vec<ParamSpec<'static>> {
        spec.iter().map(|(name, has_default)| ParamSpec { name, has_default: *has_default }).collect()
    }

    #[test]
    fn labels_place_by_name_in_any_order() {
        let p = params(&[("a", false), ("b", false)]);
        assert_eq!(
            place_args(&p, &[Some("b"), Some("a")]),
            Ok(vec![ArgSlot::Written(1), ArgSlot::Written(0)])
        );
    }

    #[test]
    fn positional_then_labels_and_defaults() {
        let p = params(&[("a", false), ("b", true), ("c", false)]);
        assert_eq!(
            place_args(&p, &[None, Some("c")]),
            Ok(vec![ArgSlot::Written(0), ArgSlot::Default, ArgSlot::Written(1)])
        );
    }

    #[test]
    fn positional_after_a_label_is_an_error() {
        let p = params(&[("a", false), ("b", false)]);
        assert_eq!(place_args(&p, &[Some("a"), None]), Err(PlaceError::PositionalAfterLabel { arg: 1 }));
    }

    #[test]
    fn a_label_for_a_filled_parameter_is_an_error() {
        let p = params(&[("a", false), ("b", false)]);
        assert_eq!(place_args(&p, &[None, Some("a")]), Err(PlaceError::Repeated { arg: 1, param: 0 }));
    }

    #[test]
    fn unknown_missing_and_too_many() {
        let p = params(&[("a", false)]);
        assert_eq!(place_args(&p, &[Some("z")]), Err(PlaceError::UnknownLabel { arg: 0 }));
        assert_eq!(place_args(&p, &[]), Err(PlaceError::Missing { param: 0 }));
        assert_eq!(place_args(&p, &[None, None]), Err(PlaceError::TooMany { arg: 1 }));
    }
}

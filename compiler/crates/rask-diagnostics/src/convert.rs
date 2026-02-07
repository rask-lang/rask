// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Conversions from compiler error types to `Diagnostic`.
//!
//! Both the CLI and LSP use these conversions. The `ToDiagnostic` trait
//! is implemented for every compiler error type.

use crate::{Diagnostic, ToDiagnostic};
use rask_ast::Span;

// ============================================================================
// Lex Errors
// ============================================================================

impl ToDiagnostic for rask_lexer::LexError {
    fn to_diagnostic(&self) -> Diagnostic {
        let mut diag = Diagnostic::error(&self.message)
            .with_code("E0001")
            .with_primary(self.span, "unexpected character");

        if let Some(ref hint) = self.hint {
            diag = diag.with_help(hint.as_str());
        }

        diag
    }
}

// ============================================================================
// Parse Errors
// ============================================================================

impl ToDiagnostic for rask_parser::ParseError {
    fn to_diagnostic(&self) -> Diagnostic {
        let mut diag = Diagnostic::error(&self.message)
            .with_code("E0100")
            .with_primary(self.span, "here");

        if let Some(ref hint) = self.hint {
            diag = diag.with_help(hint.as_str());
        }

        diag
    }
}

// ============================================================================
// Resolve Errors
// ============================================================================

impl ToDiagnostic for rask_resolve::ResolveError {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_resolve::ResolveErrorKind::*;

        match &self.kind {
            UndefinedSymbol { name } => Diagnostic::error(format!("undefined symbol: `{}`", name))
                .with_code("E0200")
                .with_primary(self.span, "not found in this scope"),

            DuplicateDefinition { name, previous } => {
                Diagnostic::error(format!("duplicate definition: `{}`", name))
                    .with_code("E0201")
                    .with_primary(self.span, "redefined here")
                    .with_secondary(*previous, "previously defined here")
            }

            InvalidBreak { label } => {
                let msg = match label {
                    Some(l) => format!("break with label `{}` outside of loop", l),
                    None => "break outside of loop".to_string(),
                };
                Diagnostic::error(msg)
                    .with_code("E0204")
                    .with_primary(self.span, "cannot break here")
            }

            InvalidContinue { label } => {
                let msg = match label {
                    Some(l) => format!("continue with label `{}` outside of loop", l),
                    None => "continue outside of loop".to_string(),
                };
                Diagnostic::error(msg)
                    .with_code("E0205")
                    .with_primary(self.span, "cannot continue here")
            }

            InvalidReturn => Diagnostic::error("return outside of function")
                .with_code("E0206")
                .with_primary(self.span, "cannot return here"),

            UnknownPackage { path } => {
                let path_str = if path.is_empty() {
                    "<empty>".to_string()
                } else {
                    path.join(".")
                };
                Diagnostic::error(format!("unknown package: `{}`", path_str))
                    .with_code("E0207")
                    .with_primary(self.span, "package not found")
            }

            NotVisible { name } => {
                Diagnostic::error(format!("`{}` is not public", name))
                    .with_code("E0203")
                    .with_primary(self.span, "not visible from this scope")
                    .with_help("mark the item as `public` to make it accessible")
            }

            ShadowsImport { name } => {
                Diagnostic::error(format!("`{}` shadows an imported name", name))
                    .with_code("E0208")
                    .with_primary(self.span, "conflicts with import")
                    .with_help("use a different name or alias the import")
            }

            CircularDependency { path } => {
                Diagnostic::error(format!(
                    "circular import: {}",
                    path.join(" -> ")
                ))
                .with_code("E0202")
                .with_primary(self.span, "cycle detected here")
            }

            ShadowsBuiltin { name } => {
                Diagnostic::error(format!("`{}` shadows a built-in", name))
                    .with_code("E0209")
                    .with_primary(self.span, "cannot redefine built-in")
                    .with_help("built-in types and functions cannot be redefined; use a different name")
            }
        }
    }
}

// ============================================================================
// Type Errors
// ============================================================================

impl ToDiagnostic for rask_types::TypeError {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_types::TypeError::*;

        match self {
            Mismatch {
                expected,
                found,
                span,
            } => Diagnostic::error("mismatched types")
                .with_code("E0308")
                .with_primary(
                    *span,
                    format!("expected `{}`, found `{}`", expected, found),
                ),

            Undefined(name) => Diagnostic::error(format!("undefined type: `{}`", name))
                .with_code("E0309")
                .with_primary(Span::new(0, 0), "type not found"),

            ArityMismatch {
                expected,
                found,
                span,
            } => Diagnostic::error(format!(
                "expected {} argument{}, found {}",
                expected,
                if *expected == 1 { "" } else { "s" },
                found
            ))
            .with_code("E0310")
            .with_primary(*span, format!("takes {} argument{}", expected, if *expected == 1 { "" } else { "s" })),

            NotCallable { ty, span } => {
                Diagnostic::error(format!("type `{}` is not callable", ty))
                    .with_code("E0311")
                    .with_primary(*span, "not a function")
            }

            NoSuchField { ty, field, span } => {
                Diagnostic::error(format!(
                    "no field `{}` on type `{}`",
                    field, ty
                ))
                .with_code("E0312")
                .with_primary(*span, "unknown field")
            }

            NoSuchMethod { ty, method, span } => {
                Diagnostic::error(format!(
                    "no method `{}` found for type `{}`",
                    method, ty
                ))
                .with_code("E0313")
                .with_primary(*span, "method not found")
            }

            InfiniteType { span, .. } => {
                Diagnostic::error("infinite type detected")
                    .with_code("E0314")
                    .with_primary(*span, "type references itself infinitely")
                    .with_note("a type variable would create an infinite type")
            }

            CannotInfer { span } => Diagnostic::error("cannot infer type")
                .with_code("E0315")
                .with_primary(*span, "type annotation needed")
                .with_help("add an explicit type annotation"),

            InvalidTypeString(s) => {
                Diagnostic::error(format!("invalid type: `{}`", s))
                    .with_code("E0309")
                    .with_primary(Span::new(0, 0), "invalid type expression")
            }

            TryInNonPropagatingContext { return_ty, span } => {
                Diagnostic::error(format!(
                    "`try` requires function returning Result or Option, found `{}`",
                    return_ty
                ))
                .with_code("E0316")
                .with_primary(*span, "try used here")
                .with_help("change the function return type to `T or E` to use `try`")
            }

            TryOutsideFunction { span } => {
                Diagnostic::error("`try` can only be used within a function")
                    .with_code("E0317")
                    .with_primary(*span, "not inside a function")
            }

            MissingReturn {
                function_name,
                expected_type,
                span,
            } => Diagnostic::error(format!(
                "missing return statement in `{}`",
                function_name
            ))
            .with_code("E0318")
            .with_primary(*span, "function ends without returning")
            .with_help(format!(
                "add `return` statement with a value of type `{}`",
                expected_type
            )),

            GenericError(msg, span) => Diagnostic::error(format!("generic argument error: {}", msg))
                .with_code("E0319")
                .with_primary(*span, "invalid generic argument"),
        }
    }
}

// ============================================================================
// Trait Errors
// ============================================================================

impl ToDiagnostic for rask_types::TraitError {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_types::TraitError::*;

        match self {
            NotSatisfied {
                ty,
                trait_name,
                span,
            } => Diagnostic::error(format!(
                "type `{}` does not satisfy trait `{}`",
                ty, trait_name
            ))
            .with_code("E0700")
            .with_primary(*span, format!("trait `{}` not implemented", trait_name)),

            MissingMethod {
                ty,
                trait_name,
                method,
                span,
            } => Diagnostic::error(format!(
                "missing method `{}` required by trait `{}`",
                method, trait_name
            ))
            .with_code("E0701")
            .with_primary(*span, format!("method `{}` missing", method))
            .with_help(format!(
                "implement `{}` for type `{}` to satisfy trait `{}`",
                method, ty, trait_name
            )),

            SignatureMismatch {
                method,
                expected,
                found,
                span,
                ..
            } => Diagnostic::error(format!("method `{}` has wrong signature", method))
                .with_code("E0702")
                .with_primary(*span, format!("expected `{}`, found `{}`", expected, found)),

            UnknownTrait(name) => Diagnostic::error(format!("unknown trait: `{}`", name))
                .with_code("E0703")
                .with_primary(Span::new(0, 0), "trait not found"),

            ConflictingMethods {
                method,
                trait1,
                trait2,
            } => Diagnostic::error(format!(
                "conflicting method `{}` from traits `{}` and `{}`",
                method, trait1, trait2
            ))
            .with_code("E0704")
            .with_primary(Span::new(0, 0), "conflicting definitions"),
        }
    }
}

// ============================================================================
// Ownership Errors
// ============================================================================

impl ToDiagnostic for rask_ownership::OwnershipError {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_ownership::OwnershipErrorKind::*;

        match &self.kind {
            UseAfterMove { name, moved_at } => {
                Diagnostic::error(format!("use of moved value: `{}`", name))
                    .with_code("E0800")
                    .with_primary(self.span, "value used here after move")
                    .with_secondary(*moved_at, "value moved here")
                    .with_note(format!(
                        "`{}` was moved because ownership was transferred",
                        name
                    ))
                    .with_help("consider cloning the value or using a `read` borrow instead")
            }

            BorrowConflict {
                name,
                requested,
                existing,
                existing_span,
            } => Diagnostic::error(format!(
                "cannot {} `{}` while it is being {}",
                requested, name, existing
            ))
            .with_code("E0801")
            .with_primary(self.span, format!("{} access here", requested))
            .with_secondary(*existing_span, format!("{} access here", existing))
            .with_help(match (
                format!("{}", requested).as_str(),
                format!("{}", existing).as_str(),
            ) {
                ("written to", "read") => {
                    "wait until the read borrow ends, or pass ownership with `own`"
                }
                _ => "restructure the code to avoid conflicting access",
            }),

            MutateWhileBorrowed { name, borrow_span } => {
                Diagnostic::error(format!(
                    "`{}` cannot be changed while it's being read",
                    name
                ))
                .with_code("E0802")
                .with_primary(self.span, "mutation occurs here")
                .with_secondary(*borrow_span, "borrow is active here")
                .with_help("wait until the borrow ends before mutating")
            }

            InstantBorrowEscapes { source_type } => {
                Diagnostic::error(format!(
                    "cannot store reference from `{}`",
                    source_type
                ))
                .with_code("E0803")
                .with_primary(self.span, "reference would escape")
                .with_help("use the value inline or copy it out")
            }

            BorrowEscapes { name } => {
                Diagnostic::error(format!(
                    "`{}` would become invalid after this point",
                    name
                ))
                .with_code("E0804")
                .with_primary(self.span, "borrow would escape scope")
                .with_help("ensure the value lives long enough, or clone it")
            }

            ResourceNotConsumed { name } => {
                Diagnostic::error(format!(
                    "resource `{}` must be consumed before scope exit",
                    name
                ))
                .with_code("E0805")
                .with_primary(self.span, "resource goes out of scope here")
                .with_help(format!(
                    "call `.close()` on `{}` or use `ensure` for cleanup",
                    name
                ))
            }
        }
    }
}

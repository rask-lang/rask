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
            diag = diag
                .with_help(hint.as_str())
                .with_fix(hint.as_str())
                .with_why("the lexer expected a valid token at this position");
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
            diag = diag
                .with_help(hint.as_str())
                .with_fix(hint.as_str())
                .with_why("the parser expected valid syntax at this position");
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
                .with_primary(self.span, "not found in this scope")
                .with_help("check spelling or add an import")
                .with_fix("check spelling or add an import")
                .with_why("all symbols must be defined before use — Rask requires explicit imports"),

            DuplicateDefinition { name, previous } => {
                Diagnostic::error(format!("duplicate definition: `{}`", name))
                    .with_code("E0201")
                    .with_primary(self.span, "redefined here")
                    .with_secondary(*previous, "previously defined here")
                    .with_help("rename one of the definitions")
                    .with_fix("rename one of the definitions")
                    .with_why("each name can only be defined once in a scope")
            }

            InvalidBreak { label } => {
                let msg = match label {
                    Some(l) => format!("break with label `{}` outside of loop", l),
                    None => "break outside of loop".to_string(),
                };
                Diagnostic::error(msg)
                    .with_code("E0204")
                    .with_primary(self.span, "cannot break here")
                    .with_help("break can only be used inside `loop`, `while`, or `for`")
                    .with_fix("move this `break` inside a `loop`, `while`, or `for` block")
                    .with_why("`break` can only exit loop constructs")
            }

            InvalidContinue { label } => {
                let msg = match label {
                    Some(l) => format!("continue with label `{}` outside of loop", l),
                    None => "continue outside of loop".to_string(),
                };
                Diagnostic::error(msg)
                    .with_code("E0205")
                    .with_primary(self.span, "cannot continue here")
                    .with_help("continue can only be used inside `loop`, `while`, or `for`")
                    .with_fix("move this `continue` inside a `loop`, `while`, or `for` block")
                    .with_why("`continue` can only skip to the next loop iteration")
            }

            InvalidReturn => Diagnostic::error("return outside of function")
                .with_code("E0206")
                .with_primary(self.span, "cannot return here")
                .with_help("return can only be used inside a function body")
                .with_fix("move this `return` inside a function body")
                .with_why("`return` exits the enclosing function — it has no meaning at the top level"),

            UnknownPackage { path } => {
                let path_str = if path.is_empty() {
                    "<empty>".to_string()
                } else {
                    path.join(".")
                };
                Diagnostic::error(format!("unknown package: `{}`", path_str))
                    .with_code("E0207")
                    .with_primary(self.span, "package not found")
                    .with_help("check the package name or add it as a dependency")
                    .with_fix("check the package name or add it as a dependency")
                    .with_why("imported packages must exist in the project or be declared as dependencies")
            }

            NotVisible { name } => {
                Diagnostic::error(format!("`{}` is not public", name))
                    .with_code("E0203")
                    .with_primary(self.span, "not visible from this scope")
                    .with_help("mark the item as `public` to make it accessible")
                    .with_fix(format!("mark `{}` as `public`, or access it from the defining module", name))
                    .with_why("items are private by default — only `public` items are accessible from other modules")
            }

            ShadowsImport { name } => {
                Diagnostic::error(format!("`{}` shadows an imported name", name))
                    .with_code("E0208")
                    .with_primary(self.span, "conflicts with import")
                    .with_help("use a different name or alias the import")
                    .with_fix("use a different name or alias the import with `import pkg.Name as Alias`")
                    .with_why("shadowing imports makes code ambiguous — Rask disallows it for clarity")
            }

            CircularDependency { path } => {
                Diagnostic::error(format!(
                    "circular import: {}",
                    path.join(" -> ")
                ))
                .with_code("E0202")
                .with_primary(self.span, "cycle detected here")
                .with_help("break the cycle by restructuring imports or extracting shared types")
                .with_fix("extract shared types into a separate module to break the cycle")
                .with_why("circular imports create unresolvable dependencies — restructure into a DAG")
            }

            ShadowsBuiltin { name } => {
                Diagnostic::error(format!("`{}` shadows a built-in", name))
                    .with_code("E0209")
                    .with_primary(self.span, "cannot redefine built-in")
                    .with_help("use a different name")
                    .with_fix("use a different name")
                    .with_why("built-in types and functions are reserved — redefining them would break language semantics")
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
                )
                .with_help(format!("change this to type `{}`", expected))
                .with_fix(format!("change this to type `{}`", expected))
                .with_why("Rask is statically typed — every expression must match its expected type"),

            Undefined(name) => Diagnostic::error(format!("undefined type: `{}`", name))
                .with_code("E0309")
                .with_primary(Span::new(0, 0), "type not found")
                .with_help("check spelling or add an import for this type")
                .with_fix("check spelling or add an import for this type")
                .with_why("all types must be defined or imported before use"),

            ArityMismatch {
                expected,
                found,
                span,
            } => {
                let fix_msg = if *found > *expected {
                    "remove the extra arguments".to_string()
                } else {
                    format!("add the missing argument{}", if expected - found == 1 { "" } else { "s" })
                };
                Diagnostic::error(format!(
                    "expected {} argument{}, found {}",
                    expected,
                    if *expected == 1 { "" } else { "s" },
                    found
                ))
                .with_code("E0310")
                .with_primary(*span, format!("takes {} argument{}", expected, if *expected == 1 { "" } else { "s" }))
                .with_help(fix_msg.clone())
                .with_fix(fix_msg)
                .with_why("function calls must provide exactly the number of arguments the function declares")
            }

            NotCallable { ty, span } => {
                Diagnostic::error(format!("type `{}` is not callable", ty))
                    .with_code("E0311")
                    .with_primary(*span, "not a function")
                    .with_help("only functions and closures can be called with `()`")
                    .with_fix("only functions and closures can be called with `()`")
                    .with_why("the call operator `()` requires a callable type")
            }

            NoSuchField { ty, field, span } => {
                Diagnostic::error(format!(
                    "no field `{}` on type `{}`",
                    field, ty
                ))
                .with_code("E0312")
                .with_primary(*span, "unknown field")
                .with_help("check the struct definition for available fields")
                .with_fix("check the struct definition for available fields")
                .with_why("struct field access is checked at compile time — only declared fields exist")
            }

            NoSuchMethod { ty, method, span } => {
                Diagnostic::error(format!(
                    "no method `{}` found for type `{}`",
                    method, ty
                ))
                .with_code("E0313")
                .with_primary(*span, "method not found")
                .with_help(format!("check available methods on `{}`", ty))
                .with_fix(format!("check available methods on `{}`", ty))
                .with_why("method calls are resolved at compile time against the type's extend blocks")
            }

            InfiniteType { span, .. } => {
                Diagnostic::error("infinite type detected")
                    .with_code("E0314")
                    .with_primary(*span, "type references itself infinitely")
                    .with_help("break the cycle with an explicit type annotation")
                    .with_fix("break the cycle with an explicit type annotation or use `Owned<T>` for indirection")
                    .with_why("a type cannot contain itself without indirection")
            }

            CannotInfer { span } => Diagnostic::error("cannot infer type")
                .with_code("E0315")
                .with_primary(*span, "type annotation needed")
                .with_help("add an explicit type annotation")
                .with_fix("add an explicit type annotation")
                .with_why("the compiler needs enough context to determine every type — ambiguous cases need annotations"),

            InvalidTypeString(s) => {
                Diagnostic::error(format!("invalid type: `{}`", s))
                    .with_code("E0309")
                    .with_primary(Span::new(0, 0), "invalid type expression")
                    .with_help("expected a type like `i32`, `string`, or a struct name")
                    .with_fix("use a type like `i32`, `string`, or a struct name")
                    .with_why("type expressions must be valid type names or parameterized types")
            }

            TryInNonPropagatingContext { return_ty, span } => {
                Diagnostic::error(format!(
                    "`try` requires function returning Result or Option, found `{}`",
                    return_ty
                ))
                .with_code("E0316")
                .with_primary(*span, "try used here")
                .with_help("change the function return type to `T or E` to use `try`")
                .with_fix("change the function return type to `T or E`")
                .with_why("`try` propagates errors upward — the enclosing function must declare an error type in its return")
            }

            TryOutsideFunction { span } => {
                Diagnostic::error("`try` can only be used within a function")
                    .with_code("E0317")
                    .with_primary(*span, "not inside a function")
                    .with_help("move this into a function body")
                    .with_fix("move this `try` expression inside a function body")
                    .with_why("`try` needs a function to propagate errors to")
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
            ))
            .with_fix(format!("add `return` statement with a value of type `{}`", expected_type))
            .with_why("all code paths in a non-void function must produce a value via explicit `return`"),

            GenericError(msg, span) => Diagnostic::error(format!("generic argument error: {}", msg))
                .with_code("E0319")
                .with_primary(*span, "invalid generic argument")
                .with_help("check the generic parameter count and types")
                .with_fix("check the generic parameter count and types")
                .with_why("generic arguments must match the declaration's type parameter constraints"),

            AliasingViolation { var, borrow_span, access_span } => {
                Diagnostic::error(format!("cannot mutate `{}` while borrowed", var))
                    .with_code("E0320")
                    .with_primary(*access_span, format!("cannot mutate `{}` here", var))
                    .with_secondary(*borrow_span, format!("`{}` is borrowed here", var))
                    .with_help("restructure the code to avoid mutating while borrowed, or clone the value")
                    .with_fix("restructure the code to avoid mutating while borrowed, or clone the value")
                    .with_why("while a value is borrowed, it cannot be mutated — this prevents data races and iterator invalidation")
            }

            MutateReadOnlyParam { name, span } => {
                Diagnostic::error(format!("cannot mutate parameter `{}`", name))
                    .with_code("E0321")
                    .with_primary(*span, format!("`{}` is read-only (default)", name))
                    .with_help("add `mutate` before the parameter to allow mutation".to_string())
                    .with_fix("add `mutate` keyword to the parameter declaration")
                    .with_why("parameters are read-only by default — add `mutate` to indicate the function modifies this value")
            }

            VolatileViewStored { source_var, view_var, source_span, store_span } => {
                Diagnostic::error(format!("cannot hold view from growable source `{}`", source_var))
                    .with_code("E0322")
                    .with_primary(*source_span, format!("`{}` can grow or shrink — view is instant", source_var))
                    .with_secondary(*store_span, format!("`{}` tries to hold this view across a statement boundary", view_var))
                    .with_help("copy the value out, or use a closure for multi-statement access")
                    .with_fix(format!("use {}.clone() or {}.modify(key, |e| {{ ... }})", source_var, source_var))
                    .with_why("Vec, Pool, and Map can grow or shrink, which would invalidate any persistent view — views are released at the semicolon")
            }

            MutateBorrowedSource { source_var, view_var, borrow_span, mutate_span } => {
                Diagnostic::error(format!("cannot mutate `{}` while viewed by `{}`", source_var, view_var))
                    .with_code("E0323")
                    .with_primary(*mutate_span, format!("cannot mutate `{}` here", source_var))
                    .with_secondary(*borrow_span, format!("view `{}` created here — active until block ends", view_var))
                    .with_help("finish using the view before mutating, or work with a copy")
                    .with_fix(format!("use {}.clone() to create an independent copy", view_var))
                    .with_why("mutating a source can invalidate views into it")
            }
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
            .with_primary(*span, format!("trait `{}` not implemented", trait_name))
            .with_help(format!("add `extend {} : {} {{ ... }}`", ty, trait_name))
            .with_fix(format!("add `extend {} : {} {{ ... }}`", ty, trait_name))
            .with_why("trait bounds require the type to provide all methods declared by the trait"),

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
                "add `func {}(...)` in `extend {} : {}`",
                method, ty, trait_name
            ))
            .with_fix(format!("add `func {}(...)` in `extend {} : {}`", method, ty, trait_name))
            .with_why("trait implementations must provide all required methods"),

            SignatureMismatch {
                method,
                expected,
                found,
                span,
                ..
            } => Diagnostic::error(format!("method `{}` has wrong signature", method))
                .with_code("E0702")
                .with_primary(*span, format!("expected `{}`, found `{}`", expected, found))
                .with_help(format!("change `{}` signature to match the trait", method))
                .with_fix(format!("change `{}` signature to match the trait", method))
                .with_why("trait method signatures are contracts — implementations must match exactly"),

            UnknownTrait(name) => Diagnostic::error(format!("unknown trait: `{}`", name))
                .with_code("E0703")
                .with_primary(Span::new(0, 0), "trait not found")
                .with_help("check spelling or add an import for this trait")
                .with_fix("check spelling or add an import for this trait")
                .with_why("traits must be defined or imported before use in bounds"),

            ConflictingMethods {
                method,
                trait1,
                trait2,
            } => Diagnostic::error(format!(
                "conflicting method `{}` from traits `{}` and `{}`",
                method, trait1, trait2
            ))
            .with_code("E0704")
            .with_primary(Span::new(0, 0), "conflicting definitions")
            .with_help(format!("rename or disambiguate `{}` in one of the trait implementations", method))
            .with_fix(format!("disambiguate `{}` in one of the trait implementations", method))
            .with_why("when two traits provide the same method name, the compiler can't determine which to call"),
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
                    .with_fix("clone the value before the transfer, or use a `read` borrow instead")
                    .with_why("`own` transfers ownership — the caller can no longer access the value")
            }

            BorrowConflict {
                name,
                requested,
                existing,
                existing_span,
            } => {
                let fix_msg = match (
                    format!("{}", requested).as_str(),
                    format!("{}", existing).as_str(),
                ) {
                    ("written to", "read") => {
                        "wait until the read borrow ends, or pass ownership with `own`"
                    }
                    _ => "restructure the code to avoid conflicting access",
                };
                Diagnostic::error(format!(
                    "cannot {} `{}` while it is being {}",
                    requested, name, existing
                ))
                .with_code("E0801")
                .with_primary(self.span, format!("{} access here", requested))
                .with_secondary(*existing_span, format!("{} access here", existing))
                .with_help(fix_msg)
                .with_fix(fix_msg)
                .with_why("concurrent read and write access to the same value would be a data race")
            }

            MutateWhileBorrowed { name, borrow_span } => {
                Diagnostic::error(format!(
                    "`{}` cannot be changed while it's being read",
                    name
                ))
                .with_code("E0802")
                .with_primary(self.span, "mutation occurs here")
                .with_secondary(*borrow_span, "borrow is active here")
                .with_help("wait until the borrow ends before mutating")
                .with_fix("wait until the borrow ends before mutating")
                .with_why("mutation during an active borrow could invalidate the borrow's view of the data")
            }

            InstantBorrowEscapes { source_type } => {
                Diagnostic::error(format!(
                    "cannot store reference from `{}`",
                    source_type
                ))
                .with_code("E0803")
                .with_primary(self.span, "reference would escape")
                .with_help("use the value inline or copy it out")
                .with_fix("use the value inline or copy it out")
                .with_why("collection element references are expression-scoped — they can't outlive the access expression")
            }

            BorrowEscapes { name } => {
                Diagnostic::error(format!(
                    "`{}` would become invalid after this point",
                    name
                ))
                .with_code("E0804")
                .with_primary(self.span, "borrow would escape scope")
                .with_help("ensure the value lives long enough, or clone it")
                .with_fix("ensure the value lives long enough, or clone it")
                .with_why("references cannot outlive their source — Rask prevents dangling references by construction")
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
                .with_fix(format!("call `.close()` on `{}` or use `ensure` for cleanup", name))
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }
        }
    }
}

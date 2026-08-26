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
            .with_code(self.code)
            .with_primary(self.span, self.label);

        if let Some(ref hint) = self.hint {
            diag = diag
                .with_help(hint.as_str())
                .with_fix(hint.as_str())
                .with_why(self.why);
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
                .with_why(self.why.as_deref()
                    .unwrap_or("the parser expected valid syntax at this position"));
        }

        diag
    }
}

// ============================================================================
// Resolve Errors
// ============================================================================

/// Names the language used to have, and what took their place. Keeping the list
/// here rather than in the resolver means the resolver doesn't have to know
/// anything about renames — it just fails to find the name, as it should.
fn retired_name(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "Cell" => Some((
            "Shared.new(…)` / `Shared<T>",
            "`Cell`, `Shared` and `Mutex` were one concept — one value, several \
             accessors, a scoped view — differing only in what synchronization they \
             took. That is a strategy, not three types, so it moved into \
             `Shared<T, S>`, and `Cell` is the `Local` strategy \
             [analysis.storage-consolidation]",
        )),
        "Owned" => Some((
            "Heap<T>",
            "`Owned` named single ownership, which every Rask value already has, so \
             it distinguished nothing. What differs is the indirection: the value \
             lives on the heap instead of inline [mem.heap]",
        )),
        _ => None,
    }
}

impl ToDiagnostic for rask_resolve::ResolveError {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_resolve::ResolveErrorKind::*;

        match &self.kind {
            // A name that used to exist gets told what replaced it. "Not found
            // in this scope" is true and useless for a rename someone is
            // meeting for the first time.
            UndefinedSymbol { name } if retired_name(name).is_some() => {
                let (replacement, why) = retired_name(name).unwrap();
                Diagnostic::error(format!("`{}` is not a type any more", name))
                    .with_code("E0200")
                    .with_primary(self.span, format!("`{}` was removed", name))
                    .with_fix(format!("write `{}`", replacement))
                    .with_why(why)
            }

            UndefinedSymbol { name } => Diagnostic::error(format!("undefined symbol: `{}`", name))
                .with_code("E0200")
                .with_primary(self.span, "not found in this scope")
                .with_help("check spelling or add an import")
                .with_fix("check spelling or add an import")
                .with_why("all symbols must be defined before use — Rask requires explicit imports"),

            // structure.modules/IM1: `pkg.Name` follows from `import pkg`. The
            // stdlib's own source is resolved alongside the program and declares
            // each module as a plain type, so the name was in scope whether or
            // not it was imported — native compiled and ran `math.sin(x)` with
            // no import while the interpreter, which binds a module only when it
            // sees the import, died at runtime (#723).
            ModuleNotImported { name } => Diagnostic::error(format!("`{}` is used but never imported", name))
                .with_code("E0210")
                .with_primary(self.span, format!("`{}` needs an import to be in scope", name))
                .with_fix(format!("import {}", name))
                .with_why("a module's name comes from its import — without one there's nothing bringing `{}` into scope [structure.modules/IM1]".replace("{}", name)),

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

            UnknownBreakTarget { name, labels } => {
                let mut d = Diagnostic::error(format!(
                    "`{name}` is neither a variable to break with nor a label to break to"
                ))
                .with_code("E0210")
                .with_primary(self.span, "not a variable, and no loop here carries this label");
                d = match labels.split_first() {
                    Some((nearest, _)) => d
                        .with_help(format!(
                            "the enclosing loops are labelled {}",
                            labels
                                .iter()
                                .map(|l| format!("`{l}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .with_fix(format!("break {nearest}")),
                    None => d
                        .with_help("no enclosing loop is labelled — this reads as a break value")
                        .with_fix(format!("declare `{name}`, or drop it and write `break`")),
                };
                d.with_why(
                    "`break x` breaks out with the value `x` unless `x` labels an enclosing loop",
                )
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

            ConflictingExtern { name, previous, current, previous_span: _ } => {
                Diagnostic::error(format!(
                    "`{}` is already declared with a different signature", name
                ))
                .with_code("E0213")
                .with_primary(self.span, format!("declared here as `{}`", current))
                // The earlier declaration is usually in the stdlib, and a span
                // from another file renders as a blank line against this one —
                // so name the signature rather than pointing at it.
                .with_note(format!("already declared as `{}`", previous))
                .with_help(format!(
                    "match the existing declaration, or drop this one — `{}` is already in scope",
                    name
                ))
                .with_fix(previous.clone())
                .with_why(
                    "an `extern` name is a single symbol in the linked program, so two \
                     signatures for it can't both be right. The stdlib declares some C \
                     functions itself — std.fs declares `strlen` — and a second declaration \
                     used to replace it silently, which left the stdlib's own calls checked \
                     against the wrong signature",
                )
            }

            ShadowsBuiltin { name } => {
                Diagnostic::error(format!("`{}` shadows a built-in", name))
                    .with_code("E0209")
                    .with_primary(self.span, "cannot redefine built-in")
                    .with_help("use a different name")
                    .with_fix("use a different name")
                    .with_why("built-in types and functions are reserved — redefining them would break language semantics")
            }

            NoSuchStdlibExport { module, symbol, suggestion } => {
                let d = Diagnostic::error(format!(
                    "`{}` has no `{}` to import", module, symbol
                ))
                .with_code("E0212")
                .with_primary(self.span, "not part of this module")
                .with_why(
                    "a selective import names one symbol out of a module; importing a name \
                     the module doesn't have used to be accepted here and only failed later, \
                     at code generation",
                );
                match suggestion {
                    Some(name) => d
                        .with_help(format!("did you mean `import {}.{}`?", module, name))
                        .with_fix(format!("import {}.{}", module, name)),
                    None => d
                        .with_help(format!("import the whole module with `import {}`", module))
                        .with_fix(format!("import {}", module)),
                }
            }

            CHeaderNotFound { header, detail } => {
                Diagnostic::error(format!("C header not found: `{}`", header))
                    .with_code("E0210")
                    .with_primary(self.span, detail.as_str())
                    .with_help("check the header path or install the library's development package")
                    .with_fix("verify the header path and include directories")
                    .with_why("import c requires the header file to exist in system or project include paths")
            }

            CParseError { header, detail } => {
                Diagnostic::error(format!("failed to parse C header: `{}`", header))
                    .with_code("E0211")
                    .with_primary(self.span, detail.as_str())
                    .with_help("check the header for C++ or non-standard extensions")
                    .with_fix("use explicit `extern \"C\"` bindings for problematic declarations")
                    .with_why("the built-in C parser handles standard C headers — use explicit bindings for edge cases")
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
            } => {
                let diag = Diagnostic::error("mismatched types")
                    .with_code("E0308")
                    .with_primary(
                        *span,
                        format!("expected `{}`, found `{}`", expected, found),
                    )
                    .with_why("Rask is statically typed — every expression must match its expected type");

                // An optional is `Result { ok: T, err: None }` underneath, so the
                // Result branch below catches it too unless it's split off
                // first. It used to say "wrap with `try` to propagate the
                // error" for a `T?` — there is no error, and `try` is not the
                // fix a reader wants here: it only works at all inside a
                // function that itself returns an optional, and it throws the
                // absent case away at the call (#939).
                if found.is_option() {
                    if let Some(inner) = found.as_option() {
                        if *inner == *expected {
                            return diag
                                .with_fix(format!(
                                    "say what an absent value should do:\n\
                                     x ?? default      // supply one\n\
                                     x!                // assert it's there, panic if not\n\
                                     if x? as v {{ … }}  // handle both"
                                ))
                                .with_help(format!(
                                    "this is a `{}` — it may hold nothing, and a `{}` can't",
                                    found, expected
                                ));
                        }
                    }
                }

                // Suggest `try` when found is Result<T, E> and expected is T
                if let rask_types::Type::Result { ok, .. } = found {
                    if **ok == *expected {
                        return diag
                            .with_fix("wrap with `try` to propagate the error")
                            .with_help(format!("this expression returns `{}` — use `try` to unwrap the ok value or propagate the error", found));
                    }
                }

                diag
                    .with_fix(format!("change this to type `{}`", expected))
                    .with_help(format!("change this to type `{}`", expected))
            }

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

            // SEQ31: `collect` was removed, so the bare "no method" reads as if
            // it were a typo. Name the replacement instead.
            NoSuchMethod { ty, method, span } if method == "collect" => {
                Diagnostic::error(format!("no method `collect` on `{}`", ty))
                    .with_code("E0313")
                    .with_primary(*span, "materializing terminals name what they build")
                    .with_fix(
                        "`to_vec()` for a Vec, `to_map()` for a Map of pairs, `join(sep)` for a string"
                            .to_string(),
                    )
                    .with_why(
                        "`collect` didn't say what it produced, so it needed an annotation to \
                         mean anything — the named terminals say it at the call [type.sequence/SEQ31]"
                            .to_string(),
                    )
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

            UnimplementedStdlibMethod { ty, method, span } => {
                Diagnostic::error(format!(
                    "`{}.{}` is declared but not implemented yet",
                    ty, method
                ))
                .with_code("E0353")
                .with_primary(*span, "no implementation behind this signature")
                .with_help(format!(
                    "the signature exists so the API can be referenced, but \
                     neither backend implements `{}.{}` — pick another method \
                     or implement it in stdlib/",
                    ty, method
                ))
                .with_note(
                    "stdlib signatures are marked `@unimplemented` when nothing \
                     backs them, so this is caught at the call instead of \
                     surfacing later as a missing function during codegen"
                )
            }
            NotDisplayable { ty, interpolated, span } => {
                let site = if *interpolated { "this placeholder" } else { "this call" };
                let mut diag = Diagnostic::error(format!(
                    "`{}` does not implement `Displayable`",
                    ty
                ))
                .with_code("E0826")
                .with_primary(*span, format!("`{}` has no `to_string()`", ty))
                .with_why(format!(
                    "{} renders this value through `Displayable`. Structs and enums opt in, \
                     so the compiler never invents output that looks intentional but isn't \
                     [std.fmt/D3, D4]",
                    if *interpolated { "`{}`" } else { "`print`" },
                ));
                // The two cases have genuinely different fixes.
                if ty.ends_with('?') || ty.contains(" or ") {
                    diag = diag
                        .with_help(format!(
                            "{} holds a `{}`, which may not have a value to show",
                            site, ty
                        ))
                        .with_fix(
                            "supply the missing case — `{value ?? \"none\"}` — or narrow \
                             first with `if value? as v { … }`",
                        );
                } else {
                    diag = diag.with_fix(format!(
                        "give it one: `extend {} with Displayable {{ func to_string(self) -> string {{ … }} }}`",
                        ty
                    ))
                    .with_note(format!(
                        "an error type only needs `message()` — `extend {} {{ func message(self) -> string }}` \
                         bridges to Displayable on its own [std.fmt/D5]",
                        ty
                    ));
                }
                diag
            }
            UnboundedTypeParamMethod { param, method, bounds, span } => {
                let bound_list = if bounds.is_empty() {
                    format!("`{}` has no bounds", param)
                } else {
                    format!("bounds on `{}` are {}", param, bounds.join(" + "))
                };
                Diagnostic::error(format!(
                    "no method `{}` provided by the bounds on `{}`",
                    method, param
                ))
                .with_code("E0313")
                .with_primary(*span, "method not found")
                .with_help(format!(
                    "add a trait bound that declares `{}`, e.g. `where {}: SomeTrait`",
                    method, param
                ))
                .with_fix(format!("where {}: /* trait declaring `{}` */", param, method))
                .with_why(format!(
                    "a type parameter only has the methods its bounds bring into scope ({})",
                    bound_list
                ))
            }

            InfiniteType { span, .. } => {
                Diagnostic::error("infinite type detected")
                    .with_code("E0314")
                    .with_primary(*span, "type references itself infinitely")
                    .with_help("break the cycle with an explicit type annotation")
                    .with_fix("break the cycle with an explicit type annotation or use `Heap<T>` for indirection")
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

            UnknownTypeName { name, suggestion, span } => {
                let diag = Diagnostic::error(format!("unknown type `{}`", name))
                    .with_code("E0356")
                    .with_primary(*span, "not a declared type")
                    .with_why("only single uppercase letters (`T`, `U`) are type parameters — any longer name must be a declared type [type.gradual/PC2]");
                if let Some(s) = suggestion {
                    diag.with_fix(format!("did you mean `{}`?", s))
                        .with_help(format!("did you mean `{}`?", s))
                } else {
                    diag.with_help(format!(
                        "declare the type, or declare a type parameter: `func f<{}>(...)`",
                        name
                    ))
                    .with_fix(format!("declare `{}` or add it as a type parameter", name))
                }
            }

            UnresolvedType { name, hint, span } => {
                let shown = hint.as_deref().unwrap_or("SomeType");
                Diagnostic::error(format!("couldn't work out the type of `{}`", name))
                    .with_code("E0361")
                    .with_primary(*span, "type is still open here")
                    .with_fix(format!("annotate it: `let {}: {} = …`", name, shown))
                    .with_help(format!(
                        "nothing in scope pins this down, so there's no type to compile against.                          Writing it out settles it: `let {}: {} = …`.                          If you think it should have been inferable, that's a compiler bug worth reporting.",
                        name, shown
                    ))
                    .with_why("every value needs a known type before it can be compiled — guessing one would silently pick the wrong size for a float, a string, or a struct")
            }

            SingleLetterTypeName { name, kind, span } => {
                Diagnostic::error(format!(
                    "single-letter type name `{}` is reserved for type parameters",
                    name
                ))
                .with_code("E0357")
                .with_primary(*span, format!("{} named with a single letter", kind))
                .with_help(format!("rename `{}` to a descriptive name", name))
                .with_fix(format!("rename `{}` to a descriptive name", name))
                .with_why("single uppercase letters in signatures always mean type parameters — a concrete type with that name would be unusable [type.gradual/PC3]")
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

            TryErrorMismatch { inner_err, outer_err, span } => {
                Diagnostic::error(format!(
                    "error type mismatch: `try` propagates `{}`, but function returns `_ or {}`",
                    inner_err, outer_err
                ))
                .with_code("E0355")
                .with_primary(*span, format!("propagates `{}`", inner_err))
                .with_fix(format!(
                    "name the wrap here — `expr catch e => return {}.SomeVariant(e)` — or give `{}` a variant taking a single `{}`, which `try` then fills in on its own",
                    outer_err, outer_err, inner_err
                ))
                .with_why("try propagates errors to the enclosing function — the error types must be compatible [error-types/ER9]")
            }

            AmbiguousErrorWrap { inner_err, outer_err, variants, span } => {
                Diagnostic::error(format!(
                    "`try` can't tell which variant of `{}` should wrap `{}`",
                    outer_err, inner_err
                ))
                .with_code("E0359")
                .with_primary(*span, format!("propagates `{}`", inner_err))
                .with_note(format!(
                    "{} each take a single `{}`",
                    variants.iter().map(|v| format!("`{}`", v)).collect::<Vec<_>>().join(" and "),
                    inner_err,
                ))
                .with_fix(format!(
                    "name the one you want: `expr catch e => return {}.{}(e)`",
                    outer_err,
                    variants.first().map(String::as_str).unwrap_or("Variant"),
                ))
                .with_why("`try` only wraps on its own when exactly one variant of the boundary enum takes the error [error-types/ER31a]")
            }

            TryAbsenceIntoResult { return_ty, span } => {
                Diagnostic::error("`try` here would propagate `none`, and this function has no absent branch")
                    .with_code("E0360")
                    .with_primary(*span, "`none` has nowhere to go")
                    .with_note(format!("this function returns `{}`", return_ty))
                    .with_fix("name the error instead: `x ?? return <error>`")
                    .with_why("bare `try` sends the operand's other branch out unchanged, so it has to fit the return — an absence doesn't fit an error branch [type.errors/ER47]")
            }

            TryErrorIntoOptional { return_ty, span } => {
                Diagnostic::error("`try` here would propagate an error, and this function only returns absence")
                    .with_code("E0361")
                    .with_primary(*span, "the error has nowhere to go")
                    .with_note(format!("this function returns `{}`", return_ty))
                    .with_fix("drop the error where it happens: `r catch _ => return none`")
                    .with_why("bare `try` sends the operand's other branch out unchanged, so it has to fit the return [type.errors/ER47]")
            }

            TryOnFlatShape { found, span } => {
                Diagnostic::error(format!(
                    "`try` on `{}` has two ways to leave — the error and the absence",
                    found
                ))
                .with_code("E0362")
                .with_primary(*span, "which branch should leave?")
                .with_fix("say both: `try f() ?? return none` — `try` sends the error up, `??` handles the absence here")
                .with_why("a value that can fail and can be absent needs each outcome spelled out; `try` alone would guess [type.errors/ER47, ER16b]")
            }

            CatchOnOptional { found, span } => {
                Diagnostic::error(format!("`catch` on `{}` — an absence carries no error to bind", found))
                    .with_code("E0363")
                    .with_primary(*span, "nothing to catch")
                    .with_fix("use `??` for the absent case: `x ?? <value>`")
                    .with_why("`catch` names or drops an error; `none` isn't one [type.errors/ER14]")
            }

            PresenceTestOnResult { found, span } => {
                Diagnostic::error(format!("`?` on `{}` — `?` asks whether a value is there, and this can fail", found))
                    .with_code("E0368")
                    .with_primary(*span, "this is a result, not an optional")
                    .with_fix("test the error with `r is <ErrorType> as e`, or handle it with `r catch e => …`")
                    .with_why("presence and failure are different questions: a miss carries nothing, a failure carries an error you shouldn't step over [type.errors/ER12]")
            }

            CoalesceOnResult { found, span } => {
                Diagnostic::error(format!("`??` on `{}` — `?` marks absence, and this can fail", found))
                    .with_code("E0364")
                    .with_primary(*span, "this is a result, not an optional")
                    .with_fix("use `catch _ => <value>`, which says an error is being dropped")
                    .with_why("the fallbacks are split by shape on purpose: a miss carries nothing, a failure carries something you shouldn't silently lose [type.errors/ER12]")
            }

            CoalesceOnNonOptional { found, from_index, value_span, default_span, .. } => {
                // Two labels, because the operator is not the thing that's
                // wrong. Blaming the whole expression put the caret on the
                // binding and left the reader to work out which half of it
                // couldn't be missing (#662): the left operand's type is the
                // reason, and the fallback is the part to delete.
                let d = Diagnostic::error(format!(
                    "`??` on `{}` — there's no absent branch to fall back to", found
                ))
                .with_code("E0831")
                .with_primary(*value_span, format!("this is a `{}`, and it's always there", found))
                .with_secondary(*default_span, "so the fallback can never run");
                let d = if *from_index {
                    // The reason this case gets its own advice: indexing is the
                    // one operation that *looks* like it might not find
                    // anything and still hands back a plain `T`.
                    d.with_fix("ask for the optional instead of indexing:\n    m[k] ?? fallback   →   m.get(k) ?? fallback")
                } else {
                    d.with_fix("drop the `??` and its right side:\n    x ?? fallback   →   x")
                };
                d.with_why("`??` supplies the other branch of a `T?`. A value that is always present has no other branch, so there is nothing for the right side to be [type.optionals/OPT3, OPT11]")
            }

            ForceUnwrapOnNonOptional { found, span } => {
                Diagnostic::error(format!(
                    "`!` on `{}` — there's no payload to force out", found
                ))
                .with_code("E0832")
                .with_primary(*span, format!("this is a `{}`, and it's always there", found))
                .with_fix("drop the `!`:\n    x!   →   x")
                .with_why("`x!` takes the payload out of a `T?` and panics when there isn't one. A value that is always present has no wrapper to take it out of [type.optionals/OPT13]")
            }

            NotOnOptional { found, span } => {
                Diagnostic::error(format!("`!` on `{}` — negation doesn't reach through an optional", found))
                    .with_code("E0830")
                    .with_primary(*span, "this is an optional, not a bool")
                    .with_fix("test for absence with `x is none`, or narrow first and negate the payload: `if x? as v { !v }`")
                    .with_why("`T?` doesn't coerce to `T`; lifting `!` through the wrapper would be ambiguous — on a `bool?`, `!x` could mean negate the payload or test for absence, and `x!` already means force-unwrap [type.optionals/OPT5, OPT15]")
            }

            NarrowingNeedsPolicy { from, to, span } => {
                Diagnostic::error(format!(
                    "`{}` doesn't fit in `{}` — some values would be lost",
                    from, to
                ))
                    .with_code("E0370")
                    .with_primary(*span, format!("this is a `{}`", from))
                    .with_fix(format!(
                        "say which values to lose: `x.to<{}>()!` asserts it fits, \
                         `x.wrap<{}>()` keeps the low bits, `x.clamp<{}>()` pins to the range",
                        to, to, to
                    ))
                    .with_why("widening is implicit because it can't fail; this can, so the policy is written at the site rather than guessed [type.primitives/CV1a, CV2]")
            }

            IntFloatArithmetic { op, left, right, span } => {
                let (int_ty, float_ty) = if matches!(left, rask_types::Type::F32 | rask_types::Type::F64) {
                    (right, left)
                } else {
                    (left, right)
                };
                Diagnostic::error(format!(
                    "`{}` between `{}` and `{}` — one is an integer, the other a float",
                    op, left, right
                ))
                    .with_code("E0371")
                    .with_primary(*span, format!("`{}` on the left, `{}` on the right", left, right))
                    .with_fix(format!(
                        "bring the `{int_ty}` over and say what happens when it doesn't land exactly: `x.round<{float_ty}>()` is the usual one, and `x as {float_ty}` is only for the widths where it can't lose anything"
                    ))
                    .with_why(
                        "int→float is never implicit — a wide integer doesn't survive the trip (past 2^53 an `i64` loses its low bits in an `f64`), so the conversion has to be written. Native took the left operand's type and dropped the other side, which is why this was a wrong answer rather than an error [type.primitives/CV1a]"
                            .to_string(),
                    )
            }

            MixedSignednessArithmetic { op, left, right, span } => {
                Diagnostic::error(format!(
                    "`{}` between `{}` and `{}` — one is signed, the other isn't",
                    op, left, right
                ))
                    .with_code("E0371")
                    .with_primary(*span, format!("`{}` on the left, `{}` on the right", left, right))
                    .with_fix(format!(
                        "bring the `{}` over to `{}`, and say what happens when it doesn't fit: \
                         `x.clamp<{}>()` pins to the range, `x.wrap<{}>()` keeps the low bits, \
                         `x.to<{}>()` hands back a `{} or ConvertError`",
                        right, left, left, left, left, left
                    ))
                    .with_why(format!(
                        "there is no result type that holds both — `{}` can't hold every `{}` \
                         and `{}` can't hold every `{}`, so widening one silently would be a \
                         guess. Comparison is the exception: `a < b` has an answer by value even \
                         across signedness, and arithmetic doesn't [type.operators/ORD4]",
                        left, right, right, left
                    ))
            }

            NoAutoWrapOutsideReturn { value, target, span } => {
                Diagnostic::error(format!(
                    "a `{}` doesn't become a `{}` here — auto-wrap only fires at `return`",
                    value, target
                ))
                    .with_code("E0828")
                    .with_primary(*span, format!("this is a `{}`, and nothing wraps it", value))
                    .with_fix(format!(
                        "get the value from something that already returns `{}` — a call, or a \
                         small `func` whose `return` does the wrapping",
                        target
                    ))
                    .with_why("at a `return` the branch is obvious from the signature; at an assignment it isn't, so the choice between the success and error branch is written rather than inferred. Optionals are exempt — a `T?` widens anywhere, because `none` is the only other branch [type.errors/ER11]")
            }

            TakeOnNonOptional { found, span } => {
                Diagnostic::error(format!("`take` needs an optional slot, found `{}`", found))
                    .with_code("E0365")
                    .with_primary(*span, "not a `T?` place")
                    .with_fix("make the slot optional, or read it without emptying it:\n    pending: Request   →   pending: Request?\n    take conn.pending  →   conn.pending")
                    .with_why("`take` leaves `none` behind, so the place has to have an absent branch to leave [type.optionals/OPT32]")
            }

            TakeOnImmutablePlace { name, span } => {
                Diagnostic::error(format!("`take` would empty `{}`, which is a `let` binding", name))
                    .with_code("E0366")
                    .with_primary(*span, "not writable")
                    .with_fix(format!("declare it `mut {} = …`", name))
                    .with_why("`take` writes `none` back into the slot — that's a mutation [type.optionals/OPT32]")
            }

            WrapperMethodCut { method, receiver, fix, span } => {
                Diagnostic::error(format!("no method `{}` on `{}`", method, receiver))
                    .with_code("E0367")
                    .with_primary(*span, "the wrapper shapes have no methods")
                    .with_fix(fix.clone())
                    .with_why("`T?` and `T or E` are operator-only: one spelling per job, and the right side is lazy by construction [std.api/SD4]")
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

            FrozenContextWrite { op, elem, span } => {
                Diagnostic::error(format!("cannot {} in a frozen `Pool<{}>` context", op, elem))
                    .with_code("E0325")
                    .with_primary(*span, format!("this {} needs a mutable pool context", op))
                    .with_help(format!("drop `frozen` from the `using Pool<{}>` clause", elem))
                    .with_fix(format!("using Pool<{}>", elem))
                    .with_why("a `using frozen Pool<T>` context is read-only (mem.pools/PF5) — it allows reads through handles but no writes, inserts, removes, or clears")
            }

            NonOptionalLink { span } => {
                Diagnostic::error("a required `Link<T>` edge is not supported yet")
                    .with_code("E0327")
                    .with_primary(*span, "this edge is required, so delete has no `none` to set it to")
                    .with_help("write the field as `Link<T>?` for now")
                    .with_fix("add `?` — `target: Link<Entity>?`")
                    .with_why("a required edge needs two things this prototype doesn't have: a batch to build it in (a cycle needs one side written before its target exists) and a declared delete policy — cascade or restrict — for when its target dies, since there is no `none` to fall back to. An optional edge needs neither. Inside a container (`Vec<Link<T>>`, `Map<K, Link<T>>`) a bare link is fine either way: delete drops the entry rather than nulling it")
            }

            LocalSharedSent { name, span } => {
                Diagnostic::error("this `Shared` is task-local and cannot be sent")
                    .with_code("E0346")
                    .with_primary(*span, format!("`{}` uses the `Local` strategy", name))
                    .with_fix(
                        "drop the `.local` — `Shared.new(…)` locks, and `Shared.mutex(…)` \
                         locks more cheaply when writes dominate"
                    )
                    .with_why(
                        "`Local` takes no lock at all, so two tasks touching it would \
                         race. It is the opt-out, not the default, and this error is \
                         what makes it safe to reach for [conc.sync/SH7]",
                    )
            }

            SharedStrategyMismatch { found, expected, span } => {
                Diagnostic::error(format!(
                    "this box uses the `{}` strategy, but `{}` is expected here",
                    found, expected
                ))
                    .with_code("E0381")
                    .with_primary(*span, format!("`Shared<_, {}>` here", found))
                    .with_fix(format!(
                        "build it with `Shared.{}(…)`, or write the type as \
                         `Shared<_, {}>` if `{}` is what you meant",
                        match expected.as_str() {
                            "Mutex" => "mutex",
                            "Local" => "local",
                            _ => "new",
                        },
                        found, found
                    ))
                    .with_help(
                        "code that works under any strategy says so: \
                         `func serve<S>(c: Shared<Config, S>)` [conc.sync/SH4]",
                    )
                    .with_why(
                        "the strategy picks which lock the accessors take, so the two \
                         have to agree. A `Local` box read through the read-write-lock \
                         entry points blocks forever — it has no lock for them to take \
                         [conc.sync/SH2]",
                    )
            }

            RetiredBoxType { name, replacement, span } => {
                Diagnostic::error(format!(
                    "`{}` is not a type any more — it's a strategy on `Shared`", name
                ))
                    .with_code("E0380")
                    .with_primary(*span, format!("`{}` used as a type here", name))
                    .with_fix(format!("write `{}`", replacement))
                    .with_why(
                        "`Cell`, `Shared` and `Mutex` were one concept — one value, \
                         several accessors, a scoped view — differing only in what \
                         synchronization they took. That is a strategy, not three \
                         types, so it moved into `Shared<T, S>` \
                         [analysis.storage-consolidation]",
                    )
            }

            MutateConst { name, span } => {
                Diagnostic::error(format!("cannot mutate `{}` — declared `let`", name))
                    .with_code("E0322")
                    .with_primary(*span, format!("`{}` is a let binding — immutable", name))
                    .with_help(format!("change `let {}` to `mut {}` to allow mutation", name, name))
                    .with_fix(format!("replace `let {}` with `mut {}`", name, name))
                    .with_why("`let` bindings forbid rebinding and mutation. Use `mut` when you need to modify the value or call mutating methods.")
            }

            MutateBoundName { name, from, span } => {
                use rask_types::BoundFrom;
                let d = Diagnostic::error(format!(
                    "cannot mutate `{}` — it's a binding, not a slot",
                    name
                ))
                    .with_code("E0372");
                match from {
                    BoundFrom::Payload => d
                        .with_primary(*span, format!(
                            "`{}` is the value the test proved was there, read out of the original",
                            name
                        ))
                        .with_help("write through the original, or build a new value and put it back")
                        .with_fix(format!(
                            "read what you need inside the block and assign back outside it — \
                             `mut copy = {}.field` … `original = …` — or give the type a method \
                             that takes `mutate self` and call it on the original",
                            name
                        ))
                        .with_why("`as v` names the payload a test proved present, and there is no `let` here to make `mut`. The payload is read out of the scrutinee, so a write to `v` would land on the copy and be lost — the compiler rejects it instead of dropping it silently [type.optionals/OPT19]"),
                    BoundFrom::Element => d
                        .with_primary(*span, format!(
                            "`{}` is a read-only element of the collection being walked",
                            name
                        ))
                        .with_help("add `mutate` to the loop to write through the element")
                        .with_fix(format!("for mutate {} in … {{ … }}", name))
                        .with_why("a plain `for` yields elements read-only; `for mutate x in xs` is the mode whose writes reach the collection. Without it the two backends disagreed about what the write meant — the interpreter wrote through and native dropped it [std.iteration/I1, I4]"),
                }
            }

            MutateWithBinding { name, span } => {
                Diagnostic::error(format!("cannot mutate `{}` — bound from a shared read lock", name))
                    .with_code("E0360")
                    .with_primary(*span, format!("`{}` comes from `.read()` — concurrent readers may hold the lock", name))
                    .with_help("use `.write()` for exclusive access if you need to mutate".to_string())
                    .with_fix(format!("with shared.write() as {} {{ … }}", name))
                    .with_why("a shared read lock permits other readers at the same time (conc.sync/R1) — writing back through it would race them")
            }

            StringIsImmutable { method, span } => {
                // `push` on a builder takes a string, `push_char` takes a char —
                // point at whichever matches what they were reaching for.
                let builder_call = match method.as_str() {
                    "push_char" | "push_byte" => "b.push_char(c)",
                    _ => "b.push(s)",
                };
                Diagnostic::error(format!("`string` has no `{}` — strings are immutable", method))
                    .with_code("E0331")
                    .with_primary(*span, "a string can't be modified in place".to_string())
                    .with_help("build the text in a StringBuilder, then call .build() for the finished string")
                    .with_fix(format!("mut b = StringBuilder.new()  …  {}  …  let s = b.build()", builder_call))
                    .with_why("a string is an immutable 16-byte value and every copy shares one buffer, so an in-place write would change copies you never touched. A StringBuilder owns its buffer alone, and build() hands it over without copying [std.strings/S7]")
            }
            StringNewRemoved { span } => {
                Diagnostic::error("`string.new()` doesn't exist — an empty string is `\"\"`")
                    .with_code("E0331")
                    .with_primary(*span, "no such constructor".to_string())
                    .with_help("if this was the start of a string you meant to append to, use a StringBuilder — `string` can't be mutated")
                    .with_fix("let s = \"\"".to_string())
                    .with_why("one spelling per operation [std.api/SD5] — `\"\"` is already the empty string, and `string.new()` only ever existed to open a sequence of pushes that `string` doesn't support [std.strings/S7]")
            }
            StringSliceStored { source_var, slice_expr, yields_sequence, view_var, slice_span, store_span } => {
                // Only an exact reprint of the user's expression gets quoted;
                // otherwise say "this slice" and point at it with the span.
                // A near-miss quote reads as their own code and sends them
                // looking for a line they never wrote (#694).
                let subject = match slice_expr {
                    Some(expr) => format!("`{}`", expr),
                    None if *yields_sequence => "this split".to_string(),
                    None => "this slice".to_string(),
                };
                let d = Diagnostic::error(format!(
                    "{} gives {} into `{}`, not {}",
                    subject,
                    if *yields_sequence { "views" } else { "a view" },
                    source_var,
                    if *yields_sequence { "new strings" } else { "a new string" }
                ))
                    .with_code("E0324")
                    .with_primary(*slice_span, format!(
                        "{} can't outlive the statement",
                        if *yields_sequence { "these views" } else { "this view" }
                    ))
                    .with_secondary(*store_span, format!("`{}` would hold {} past the end of this line", view_var, if *yields_sequence { "them" } else { "it" }))
                    .with_why("a view borrows the source's buffer instead of copying it — keeping one past the statement would leave it pointing at freed bytes");
                // Two fixes, and they differ in cost, so both get named:
                // `.view()` is zero-copy and keeps the source buffer alive,
                // `.to_string()` copies out and releases it (std.strings/V2,
                // the spec's FIX 1 and FIX 2).
                if *yields_sequence {
                    let d = d.with_help("collect views with .view(), or copy each piece out with .to_string() as you go");
                    match slice_expr {
                        Some(expr) => d.with_fix(format!(
                            "for piece in {} {{ {}.push(piece.view()) }}  — .to_string() instead of .view() to copy the bytes out",
                            expr, view_var
                        )),
                        None => d.with_fix(format!(
                            "loop over the pieces and push `piece.view()` into `{}` — or `piece.to_string()` to copy the bytes out",
                            view_var
                        )),
                    }
                } else {
                    let d = d.with_help("add .view() to store it zero-copy, or .to_string() to copy the bytes out");
                    match slice_expr {
                        Some(expr) => d.with_fix(format!(
                            "let {}: StringView = {}.view()  — or {}.to_string() for an independent copy",
                            view_var, expr, expr
                        )),
                        None => d.with_fix(format!(
                            "add `.view()` to the slice to store it zero-copy, or `.to_string()` to copy the bytes out of `{}`",
                            source_var
                        )),
                    }
                }
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

            WithGuardEscapes { name, type_name, span } => {
                Diagnostic::error(format!("the `with` guard `{}` can't leave its block", name))
                    .with_code("E0829")
                    .with_primary(*span, format!("`{}` (a `{}`) is only valid while this block holds access", name, type_name))
                    .with_help("copy a field out, or add a method that returns an owned value")
                    .with_fix(format!("with … as {} {{ {}.some_field }}", name, name))
                    .with_why("`with` hands out access to the box's payload for the block's duration, not a value of its own — returning the guard itself would leave a view into memory the lock no longer protects once the block ends")
            }

            TornLockUpdate { binding, box_name, first_field, second_field, first_span, second_span } => {
                Diagnostic::warning("multi-field update under a lock without staged()".to_string())
                    .with_code("W0907")
                    .with_primary(*first_span, format!("`{}` written first", first_field))
                    .with_secondary(
                        *second_span,
                        format!("`{}` second — a panic between these leaves other tasks a half-done update", second_field),
                    )
                    .with_help(format!(
                        "stage the update: `with {}.staged() as {} {{ … }}` commits as one move on a clean exit and discards on a panic",
                        box_name, binding,
                    ))
                    .with_fix(format!("with {}.staged() as {} {{ … }}", box_name, binding))
                    .with_why("Rask has no lock poisoning — a panic mid-update releases the lock and the next task reads whatever was written (ctrl.panic/LK1–LK4). `staged()` makes the update atomic against that by construction. Add `@allow(torn_lock_update)` to the enclosing function if partial state is harmless here [tool.warnings/W9]")
            }

            StagedOutsideWith { name, span } => {
                Diagnostic::error("`staged()` only works as the source of a `with` block")
                    .with_code("E0846")
                    .with_primary(*span, "there is no block here for the commit to happen at")
                    .with_help(format!("write `with {}.staged() as v {{ … }}`; for a single field, `{}.write().field` takes the lock for the expression", name, name))
                    .with_fix(format!("with {}.staged() as v {{ … }}", name))
                    .with_why("staged access works on a copy and commits it as one move when the block exits. `read`/`write` also have an expression-scoped form, where the lock is held for the chain (mem.borrowing/E5) — staged has none, because there would be nowhere to put the commit [conc.sync/ST1]")
            }

            StagedOnLocal { name, span } => {
                Diagnostic::error("`staged()` has nothing to protect under `Local`")
                    .with_code("E0845")
                    .with_primary(*span, format!("`{}` is a `Shared<T, Local>` — one task, no unwind boundary", name))
                    .with_help("use `.write()` here; reach for `staged()` under `Readers` or `Mutex`, where another task could read a torn update")
                    .with_fix(format!("with {}.write() as …", name))
                    .with_why("staged access exists to make a multi-field update atomic against a panic that other tasks would observe. Under `Local` there is no other task to observe it, so the clone buys nothing and costs a copy [conc.sync/ST3a]")
            }

            BareSharedWith { name, binding, span } => {
                Diagnostic::error(format!("`with {} as {}` doesn't say which lock", name, binding))
                    .with_code("E0839")
                    .with_primary(*span, "a `Shared` is read by many or written by one — this could be either")
                    .with_help("name the lock: `.read()` for concurrent readers, `.write()` for exclusive access")
                    .with_fix(format!("with {}.read() as {} {{ … }}", name, binding))
                    .with_why("the two locks behave differently — a read binding permits other readers and never writes back, a write binding blocks them and does — so which one you get is written rather than inferred [conc.sync/R4]")
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

            NoAllocViolation { reason, function_name, span } => {
                Diagnostic::error(format!("heap allocation in @no_alloc function `{}`", function_name))
                    .with_code("E0324")
                    .with_primary(*span, reason.clone())
                    .with_help("use stack-allocated alternatives or pre-allocated buffers")
                    .with_fix("remove the allocation or move it outside the @no_alloc function")
                    .with_why("@no_alloc functions run in real-time contexts where heap allocation causes unpredictable latency")
            }

            GuardElseMustDiverge { found, span } => {
                Diagnostic::error("guard pattern 'else' block must diverge")
                    .with_code("E0325")
                    .with_primary(*span, format!("'else' block has type `{}`, but must diverge", found))
                    .with_help("use 'return', 'break', 'continue', or 'panic' to ensure the block never completes normally")
                    .with_fix("add a 'return' statement at the end of the 'else' block")
                    .with_why("guard patterns bind variables in the outer scope — the 'else' path must exit to ensure the binding is always valid")
            }

            MissingMutateAnnotation { param_name, param_index: _, span } => {
                Diagnostic::error(format!("parameter `{}` requires `mutate` annotation at call site", param_name))
                    .with_code("E0326")
                    .with_primary(*span, format!("add `mutate` before this argument"))
                    .with_help(format!("call with `mutate {}`", param_name))
                    .with_fix(format!("add `mutate` annotation"))
                    .with_why("mutable parameters require explicit annotation at call site for clarity")
            }

            MissingOwnAnnotation { param_name, param_index: _, span } => {
                Diagnostic::error(format!("parameter `{}` requires `own` annotation at call site", param_name))
                    .with_code("E0327")
                    .with_primary(*span, format!("add `own` before this argument"))
                    .with_help(format!("call with `own {}`", param_name))
                    .with_fix(format!("add `own` annotation"))
                    .with_why("ownership transfer requires explicit annotation at call site for clarity")
            }

            UnexpectedAnnotation { annotation, param_name, param_index: _, span } => {
                Diagnostic::error(format!("unexpected `{}` annotation for parameter `{}`", annotation, param_name))
                    .with_code("E0328")
                    .with_primary(*span, format!("remove this annotation"))
                    .with_help(format!("remove `{}` annotation — parameter does not expect it", annotation))
                    .with_fix("remove the annotation")
                    .with_why("annotations must match parameter declarations")
            }

            MissingDeletingMarker { callee, arg, param_name, span } => {
                Diagnostic::error(format!(
                    "`{}` can delete from `{}` — say `deleting`, not `mutate`",
                    callee, arg
                ))
                    .with_code("E0330")
                    .with_primary(*span, format!("passed to the `deleting {}` parameter", param_name))
                    .with_fix(format!("{}(deleting {}, …)", callee, arg))
                    .with_why("PM5: the marker follows the signature. A `deleting` parameter is a `mutate` parameter that may also delete nodes the caller never named, and those are different contracts — writing `mutate` for both would print them the same. Your links into that rack are revoked at this call, which is worth seeing here rather than discovering at the next read [mem.parameters/PM4, PM5, analysis.fourth-option]")
            }

            MissingMutateMarker { callee, arg, param_name, span } => {
                Diagnostic::error(format!(
                    "`{}` mutates `{}` — mark it at the call site",
                    callee, arg
                ))
                    .with_code("E0373")
                    .with_primary(*span, format!("passed to the `mutate {}` parameter", param_name))
                    .with_fix(format!("{}(mutate {}, …)", callee, arg))
                    .with_why("the compiler backstops a misread *move* — using a value after it's moved is an error — but nothing backstops a misread mutation: both readings are legal code, so the one that can't be caught gets written down. The marker follows the signature, not the argument's size, so a Copy argument writes it too. A method receiver is exempt — `player.take_damage(10)` operates on the receiver by construction [mem.parameters/PM4, PM5]")
            }

            TryOnNonResult { found, span } => {
                Diagnostic::error(format!("`try` requires a Result type, found `{}`", found))
                    .with_code("E0329")
                    .with_primary(*span, "not a Result type")
            }

            NotIterable { found, span } => {
                let ty = found.to_string();
                let mut diag = Diagnostic::error(format!("`{}` can't be iterated", ty))
                    .with_code("E0827")
                    .with_primary(*span, "`for` needs a collection here")
                    .with_why(
                        "`for` walks a Vec, Map, Pool, array, slice, range or iterator \
                         chain. A single value has no elements to visit \
                         [ctrl.control-flow]",
                    );
                // Each of these is a specific slip with a specific answer, and
                // naming the answer beats restating the rule.
                let is_number = matches!(
                    ty.as_str(),
                    "i8" | "i16" | "i32" | "i64" | "i128"
                        | "u8" | "u16" | "u32" | "u64" | "u128"
                );
                diag = match ty.as_str() {
                    // A string is a sequence, just not of itself.
                    "string" => diag.with_fix("ask for the elements: `for c in s.chars()`"),
                    // A count in the iterator position wants the range it counts.
                    _ if is_number => diag.with_fix("count with a range: `for i in 0..n`"),
                    _ => diag.with_fix(format!(
                        "iterate a collection — a `Vec<{}>`, or a field of `{}` that holds one",
                        ty, ty
                    )),
                };
                diag
            }

            UnsafeRequired { operation, span } => {
                Diagnostic::error(format!("{} requires an `unsafe` block", operation))
                    .with_code("E0330")
                    .with_primary(*span, "unsafe operation outside unsafe block")
            }

            TraitObjectSelfReturn { trait_name, method, span } => {
                Diagnostic::error(format!("method `{}` returns Self — cannot be called through `any {}`", method, trait_name))
                    .with_code("E0332")
                    .with_primary(*span, "Self-returning method")
                    .with_help("Self-returning methods are incompatible with trait objects because the concrete type is erased (TR2)")
            }

            TraitObjectGenericMethod { trait_name, method, span } => {
                Diagnostic::error(format!("generic method `{}` — cannot be called through `any {}`", method, trait_name))
                    .with_code("E0819")
                    .with_primary(*span, "generic method")
                    .with_help("generic methods can't be dispatched dynamically: each instantiation needs its own code, but a trait object erases the concrete type. Call it on the concrete type instead (TR3)")
            }

            // Encode/Decode are shape markers, not method sets — you can't write
            // them out, so "add the required methods" is the wrong advice. A type
            // qualifies when every field does (std.encoding/E12–E17); the fix is
            // to change the field, which means naming it.
            ExcludedFieldNeedsDefault { ty, field, span } => {
                Diagnostic::error(format!("`{}` cannot be decoded", ty))
                    .with_code("E0377")
                    .with_primary(*span, format!(
                        "field `{}` is left out of the wire form and has nothing to fill it",
                        field
                    ))
                    .with_fix(format!(
                        "give `{}` a declared default (`{}: T = value`), or an `@default(expr)` \
                         override if the default should only apply to decoding",
                        field, field
                    ))
                    .with_why("a decode has to build the whole struct, and a `private` or `@no_serialize` field never appears in the input — so its value comes from its default or from nowhere. Encoding is unaffected: it never needs a value for a field it omits [std.encoding/E13a, type.structs/FD1, FD6]")
            }

            NotSerializable { ty, trait_name, verb, field, field_ty, span } => {
                let label = match (field, field_ty) {
                    (Some(f), Some(fty)) => {
                        format!("field `{}` has type `{}`, which can't be {}", f, fty, verb)
                    }
                    (Some(f), None) => format!("field `{}` can't be {}", f, verb),
                    _ => format!("`{}` has a field that can't be {}", ty, verb),
                };
                // The list of what qualifies belongs in `fix`: the formatter
                // drops `help` whenever fix/why are set, and someone reading
                // this is exactly the person who needs the list.
                let target = match field {
                    Some(f) => format!("`{}`", f),
                    None => "the offending field".to_string(),
                };
                Diagnostic::error(format!("`{}` cannot be {}", ty, verb))
                    .with_code("E0333")
                    .with_primary(*span, label)
                    .with_fix(format!(
                        "mark {} with `@no_serialize`, or give it a serializable type — bool, char, \
                         the integer and float types, string, `T?`, tuples, `Vec<T>`, \
                         `Map<string, T>`, or a struct or enum of those",
                        target
                    ))
                    .with_why(format!("`{}` isn't implemented by hand — a type has it when its fields do, all the way down (std.encoding/E12)", trait_name))
            }

            TraitNotSatisfied { ty, trait_name, context, span } => {
                use rask_types::TraitBoundContext as Ctx;
                let d = Diagnostic::error(format!("`{}` does not implement `{}`", ty, trait_name))
                    .with_code("E0333")
                    .with_primary(*span, match context {
                        Ctx::NumericBound => format!("`{}` is not one of the types `{}` covers", ty, trait_name),
                        _ => format!("`{}` is missing methods `{}` requires", ty, trait_name),
                    });
                match context {
                    // A numeric trait is a set of primitive types, not a list
                    // of methods — there is nothing to implement.
                    Ctx::NumericBound => d
                        .with_fix(format!(
                            "pass one of the types `{}` covers:\n    Integer  → i8 i16 i32 i64 i128 u8 u16 u32 u64 u128\n    Float    → f32 f64\n    Numeric  → either",
                            trait_name
                        ))
                        .with_why("the numeric traits are membership, not conformance: their contents are constants like MIN, MAX and BITS, and a type is a member because of what it is [type.primitives/NT1-NT3]"),
                    Ctx::GenericBound => d
                        .with_fix(format!(
                            "pass a type that implements `{0}`, or declare the conformance:\n    extend {1} with {0} {{ … }}",
                            trait_name, ty
                        ))
                        .with_why("a type parameter's bound is a promise the body relies on, so it's checked against the type argument at the call [type.generics/G1]"),
                    Ctx::ConformanceHeader => d
                        .with_fix(format!(
                            "add the missing methods to the block, or drop `{}` from its header:\n    extend {} with {} {{ … }}",
                            trait_name, ty, trait_name
                        ))
                        .with_why("the header is the claim and the block is the evidence — a conformance is only declared once the methods are there [type.generics/G1]"),
                    Ctx::TraitObjectCast => d
                        .with_fix(format!(
                            "implement the trait before boxing:\n    extend {} with {} {{ … }}",
                            ty, trait_name
                        ))
                        .with_why("`as any Trait` builds a vtable from the concrete type's methods, so every method the trait declares has to be there [type.generics/TR1]"),
                }
            }

            NoSuchTrait { trait_name, known, span } => {
                let d = Diagnostic::error(format!("no trait named `{}`", trait_name))
                    .with_code("E0833")
                    .with_primary(*span, "this name isn't a trait");
                let refs: Vec<&str> = known.iter().map(|s| s.as_str()).collect();
                match crate::suggestions::did_you_mean(trait_name, refs) {
                    Some(hint) => d.with_fix(hint),
                    None => d.with_fix(format!(
                        "declare it, or drop the bound:\n    trait {} {{ … }}",
                        trait_name
                    )),
                }
                .with_why("a bound has to name a trait that exists — nothing can satisfy one that doesn't, so every call site would fail [type.generics/G1]")
            }

            PublicDuckTrait { name, span } => {
                Diagnostic::error(format!("`duck trait {}` cannot be public", name))
                    .with_code("E0824")
                    .with_primary(*span, "shape-matching can't cross a package boundary")
                    .with_fix(format!("drop `duck` to harden it — `public trait {}`, then declare conformance with `extend Type with {} {{}}` on each matching type. Or drop `public` to keep it a package-internal sketch", name, name))
                    .with_why("a duck trait matches by shape, so an external type could start or stop satisfying it without either author changing a line they'd notice — a break semver can't describe. Duck traits stay package-internal (DT1)")
            }

            StringAddForbidden { span } => {
                Diagnostic::error("the `+` operator cannot be used on strings")
                    .with_code("E0335")
                    .with_primary(*span, "strings don't support `+`")
                    .with_help("use `string.concat(a, b)` or interpolation `\"{a}{b}\"`")
                    .with_fix("replace `a + b` with `string.concat(a, b)` or `\"{a}{b}\"`")
                    .with_why("string concatenation allocates — Rask requires the allocation to be visible through the method name or interpolation syntax")
            }

            NominalMismatch { expected, found, nominal_name, span } => {
                let expected_is_nominal = format!("{}", expected) == *nominal_name;
                let (label, fix, why) = if expected_is_nominal {
                    // Expected nominal, found raw: wrap with constructor
                    (
                        format!("expected `{}`, found `{}`", expected, found),
                        format!("wrap with `{}(...)` to construct the nominal type", nominal_name),
                        format!("`{}` is a distinct type — raw `{}` values don't convert implicitly [type.aliases/T9]", nominal_name, found),
                    )
                } else {
                    // Expected raw, found nominal: unwrap with .value
                    (
                        format!("expected `{}`, found `{}`", expected, found),
                        format!("use `.value` to extract the underlying `{}` from `{}`", expected, nominal_name),
                        format!("`{}` is a distinct type — it doesn't convert to `{}` implicitly [type.aliases/T9]", nominal_name, expected),
                    )
                };
                Diagnostic::error("nominal type mismatch")
                    .with_code("E0340")
                    .with_primary(*span, label)
                    .with_fix(fix)
                    .with_help(format!("`type {} = ...` creates a distinct type — use `{}(value)` to wrap, `.value` to unwrap", nominal_name, nominal_name))
                    .with_why(why)
            }

            PublicInferredError { function_name, span } => {
                Diagnostic::error(format!(
                    "public function `{}` must declare error types explicitly",
                    function_name
                ))
                .with_code("E0335")
                .with_primary(*span, "replace `_` with explicit error types")
                .with_why("public functions are API contracts — callers need to see error types (ER21)")
                .with_help("use the \"Make error type explicit\" quick action to fill in the inferred union")
            }

            NonExhaustiveMatch { missing, span } => {
                let missing_str = missing.join(", ");
                Diagnostic::error(format!("non-exhaustive match: missing {}", missing_str))
                    .with_code("E0340")
                    .with_primary(*span, format!("missing variants: {}", missing_str))
                    .with_help("add the missing variants or a wildcard `_` arm")
                    .with_fix("add the missing variants or a wildcard `_` arm")
                    .with_why("match expressions must cover all possible values")
            }

            UndefinedName { name, span } => {
                Diagnostic::error(format!("undefined name `{}`", name))
                    .with_code("E0341")
                    .with_primary(*span, format!("`{}` is not defined", name))
                    .with_help("check spelling or add an import for this name")
                    .with_fix("check spelling or add an import")
                    .with_why("all names must be defined or imported before use")
            }

            UnknownContext { name, span } => {
                Diagnostic::error(format!("unknown context `{}` in `using` block", name))
                    .with_code("E0342")
                    .with_primary(*span, "not a recognized context")
                    .with_help("valid contexts are: `Multitasking`, `ThreadPool`")
                    .with_fix("replace with a valid context name")
                    .with_why("`using` blocks require a known runtime context to initialize")
            }

            SignatureRuntimeContext { ctx, span } => {
                Diagnostic::error(format!("`using {}` cannot appear on a function signature", ctx))
                    .with_code("E0351")
                    .with_primary(*span, "remove this clause from the signature")
                    .with_help(format!(
                        "`using {}` is a block, not a signature annotation — \
                         wrap the call site in `using {} {{ ... }}` instead",
                        ctx, ctx
                    ))
                    .with_why("`using Multitasking` installs a process-global runtime slot; \
                               functions don't declare it, they just use it [conc.async/CC1]")
            }

            EntryPointContext { entry, alias, ty, span } => {
                let binding = alias.clone().unwrap_or_else(|| "ctx".to_string());
                // `Pool<Player>` → `Pool`, so the suggestion names the constructor
                // the way it's actually written.
                let head = ty.split('<').next().unwrap_or(ty).trim();
                Diagnostic::error(format!("`{}` cannot declare a `using` context", entry))
                    .with_code("E0831")
                    .with_primary(*span, "nothing can supply this")
                    .with_fix(format!(
                        "drop the clause and own it here — `mut {}: {} = {}.new()` — \
                         then call the functions that declare `using {}`; they resolve \
                         it out of `{}`'s scope",
                        binding, ty, head, ty, entry
                    ))
                    .with_why(format!(
                        "a `using` clause is a hidden parameter the caller fills in, and \
                         `{}` has no caller — the parameter would be left holding whatever \
                         the stack came up with [mem.context/CC11]",
                        entry
                    ))
            }

            SpawnOutsideBlock { span } => {
                Diagnostic::error("`spawn` must be inside a `using Multitasking { ... }` block")
                    .with_code("E0352")
                    .with_primary(*span, "`spawn` used here without a runtime")
                    .with_help("wrap this code in `using Multitasking { ... }`")
                    .with_why("spawn() requires an active runtime slot installed by `using Multitasking { }` [conc.async/CC1]")
            }

            CyclicTypeAlias { cycle, span } => {
                Diagnostic::error(format!("cyclic type alias: {}", cycle))
                    .with_code("E0343")
                    .with_primary(*span, "cycle detected here")
                    .with_help("break the cycle by removing one of the aliases")
                    .with_fix("break the cycle by removing one of the aliases")
                    .with_why("type aliases cannot form cycles — each alias must eventually resolve to a concrete type (T6)")
            }

            PrivateFieldAccess { ty, field, span } => {
                Diagnostic::error(format!("field `{}` on `{}` is private", field, ty))
                    .with_code("E0344")
                    .with_primary(*span, "private field")
                    .with_help("private fields can only be accessed inside extend blocks for this type")
                    .with_fix("use a public method to access this field")
                    .with_why("private fields restrict access to the type's own extend blocks (V5)")
            }

            TypeCalledAsFunction { name, kind, fields, span } => {
                let is_enum = kind.ends_with("enum");
                let fix = if is_enum {
                    format!("name a variant: `{}.Variant`", name)
                } else if fields.is_empty() {
                    format!("write the literal: `{} {{}}`", name)
                } else {
                    let list: Vec<String> = fields.iter()
                        .map(|f| format!("{}: …", f))
                        .collect();
                    format!("write the literal: `{} {{ {} }}`", name, list.join(", "))
                };
                let why = if is_enum {
                    "an enum value is one of its variants — there's no whole-enum constructor. \
                     `Name(value)` builds a nominal type declared with `type Name = …` (T7)"
                } else {
                    "`Name(value)` builds a nominal type declared with `type Name = …` (T7). \
                     Structs are built field by field — there are no tuple structs (S1)"
                };
                Diagnostic::error(format!("`{}` is {}, so calling it doesn't construct one", name, kind))
                    .with_code("E0345")
                    .with_primary(*span, format!("`{}` used as a function", name))
                    .with_help(fix.clone())
                    .with_fix(fix)
                    .with_why(why)
            }

            MissingFields { ty, fields, span } => {
                let (label, list) = if fields.len() == 1 {
                    ("missing field".to_string(), format!("`{}`", fields[0]))
                } else {
                    let quoted: Vec<String> = fields.iter().map(|f| format!("`{f}`")).collect();
                    ("missing fields".to_string(), quoted.join(", "))
                };
                Diagnostic::error(format!("{label} in `{ty}` initializer: {list}"))
                    .with_code("E0822")
                    .with_primary(*span, format!("{label}: {list}"))
                    .with_help(format!("provide a value for {list}, or give the field a default with `= <value>`"))
                    .with_fix("give every field a value — construction never zero-initializes")
                    .with_why("a field with no default must be provided; the compiler names it rather than silently zeroing (FD4)")
            }

            PublicMissingAnnotation { function_name, params, missing_return, span } => {
                let mut msg = format!("public function `{}` requires explicit type annotations", function_name);
                if !params.is_empty() {
                    msg.push_str(&format!(" — unannotated parameters: {}", params.join(", ")));
                }
                let mut diag = Diagnostic::error(msg)
                    .with_code("E0334")
                    .with_primary(*span, "public function with missing annotations")
                    .with_why("public functions are API contracts — callers need to see the full signature (GC5)");
                if !params.is_empty() {
                    diag = diag.with_help(format!("add type annotations to: {}", params.join(", ")));
                }
                if *missing_return {
                    diag = diag.with_help("add an explicit return type with `->`");
                }
                diag
            }

            DiscardCopyType { name, ty, span } => {
                Diagnostic::warning(format!(
                    "`discard {}` on Copy type `{}` has no effect",
                    name, ty
                ))
                .with_code("W0301")
                .with_primary(*span, "Copy types are trivially cleaned up")
                .with_help(format!("remove `discard {}` — Copy types don't need explicit cleanup", name))
                .with_why("Copy types (primitives, small values) are cleaned up automatically — `discard` is only meaningful for heap-allocated or move-only types")
            }

            DiscardResourceType { name, ty, span } => {
                Diagnostic::error(format!(
                    "cannot `discard` resource `{}` of type `{}`",
                    name, ty
                ))
                .with_code("E0335")
                .with_primary(*span, "resource types must be consumed properly")
                .with_help(format!("call `.close()` or another consuming method on `{}`", name))
                .with_fix(format!("replace `discard {}` with `{}.close()`", name, name))
                .with_why("resource types must be consumed exactly once — `discard` would silently leak the resource")
            }

            UseAfterDiscard { name, discarded_at, span } => {
                Diagnostic::error(format!("use of discarded value: `{}`", name))
                    .with_code("E0336")
                    .with_primary(*span, "value used here after discard")
                    .with_secondary(*discarded_at, "value discarded here")
                    .with_help("remove the `discard` or restructure so the value isn't needed after this point")
                    .with_why("`discard` explicitly drops a value and invalidates its binding — using it afterwards is an error")
            }

            ZeroStep { span } => {
                Diagnostic::error("zero step".to_string())
                    .with_code("E0337")
                    .with_primary(*span, "step must be non-zero")
                    .with_help("use a positive step for ascending ranges, or a negative step for descending ranges")
                    .with_why("a zero step would loop forever without making progress [ctrl.ranges/SP3]")
            }

            StepDirectionMismatch { range_span, step_span, range_direction, step_direction } => {
                Diagnostic::warning("step direction mismatch — range will be empty".to_string())
                    .with_code("W0302")
                    .with_primary(*step_span, format!("{} step on {} range", step_direction, range_direction))
                    .with_secondary(*range_span, format!("range is {}", range_direction))
                    .with_help(format!(
                        "use a {} step for a {} range, or swap start and end",
                        if *step_direction == "positive" { "negative" } else { "positive" },
                        range_direction
                    ))
                    .with_why("a positive step on a descending range (or negative step on ascending range) produces zero iterations [ctrl.ranges/SP1-SP2]")
            }

            MessageCoverageMissing { variant, enum_name, span } => {
                Diagnostic::error(format!(
                    "@message variant `{}` on `{}` has no message template and cannot auto-delegate",
                    variant, enum_name
                ))
                    .with_code("E0338")
                    .with_primary(*span, "variant needs @message(\"...\") annotation")
                    .with_help("add @message(\"template\") to this variant, or change the payload to an error type")
                    .with_why("every variant in a @message enum must have an explicit template or a single Error payload that auto-delegates [type.errors/ER26]")
            }

            BareSyncAccess { ty, method, span } => {
                Diagnostic::error(format!(
                    "standalone `.{}()` on `{}` must be chained with field access",
                    method, ty,
                ))
                    .with_code("E0339")
                    .with_primary(*span, format!("`.{}()` without field chain", method))
                    .with_help(format!("use `{}.{}().field` for inline access, or `with {}.{}() as v {{ }}` for multi-statement access", ty.to_lowercase(), method, ty.to_lowercase(), method))
                    .with_why(format!("sync inline access is expression-scoped — the lock is held only for the chain [mem.borrowing/E5]"))
            }
            MixedDiscriminants { enum_name, span } => {
                Diagnostic::error(format!("enum `{}` mixes explicit and auto-indexed discriminants", enum_name))
                    .with_code("E0340")
                    .with_primary(*span, "if any variant has `= N`, all must")
                    .with_why("mixed discriminants make variant ordering ambiguous [type.enums/E16]")
            }
            DiscriminantWithPayload { enum_name, variant, span } => {
                Diagnostic::error(format!("variant `{}` on `{}` has fields and an explicit discriminant", variant, enum_name))
                    .with_code("E0341")
                    .with_primary(*span, format!("variant `{}` cannot have both", variant))
                    .with_why("enums with explicit discriminants are integer-backed and cannot carry payloads [type.enums/E17]")
            }
            DuplicateDiscriminant { enum_name, value, first, second, span } => {
                Diagnostic::error(format!("duplicate discriminant value {} in `{}`", value, enum_name))
                    .with_code("E0342")
                    .with_primary(*span, format!("both `{}` and `{}` have value {}", first, second, value))
                    .with_why("each variant must have a unique discriminant value [type.enums/E15]")
            }
            TagOnUnnamedPayload { enum_name, variant, tag, span } => {
                Diagnostic::error(format!("`@tag(\"{}\")` needs a named payload, and `{}.{}` has an unnamed one", tag, enum_name, variant))
                    .with_code("E0841")
                    .with_primary(*span, format!("`{}` carries one unnamed value", variant))
                    .with_fix(format!("{} {{ value: … }}   // name the field, and the tag sits beside it", variant))
                    .with_why(format!("internal tagging writes the tag as a field inside the payload's own object, so the payload needs field names for `{}` to sit beside. An unnamed value has no key to pair it with, and the compiler will not invent one. Dropping `@tag` also works: external tagging (std.encoding/E23) puts an unnamed payload directly under the variant name [std.encoding/E24]", tag))
            }
            TagCollidesWithField { enum_name, variant, tag, span } => {
                Diagnostic::error(format!("`@tag(\"{}\")` collides with a field on `{}.{}`", tag, enum_name, variant))
                    .with_code("E0842")
                    .with_primary(*span, format!("`{}` already has a field named `{}`", variant, tag))
                    .with_fix(format!("rename the field to something other than `{}`, or point `@tag` at a name no variant uses", tag))
                    .with_why(format!("the tag and the field would both be written as `\"{}\"` in the same object, so the encoded JSON would carry that key twice. Duplicate keys are not valid JSON, and a decoder reading it back keeps only one — losing either the variant or the field [std.encoding/E24]", tag))
            }
            ResultNotDisjoint { ty, span } => {
                Diagnostic::error(format!("`T or E` needs distinct types — both sides are `{}`", ty))
                    .with_code("E0343")
                    .with_primary(*span, "T and E must differ")
                    .with_help("newtype one side (e.g. `type MyError = ...`) or pick a different error type")
                    .with_why("type-based branch disambiguation only works when T and E are distinct [type.errors/ER3]")
            }
            ResultNotDisjointAtInstantiation { callee, param, arg, other, span } => {
                Diagnostic::error(format!("`{}` may not be `{}` here", param, arg))
                    .with_code("E0358")
                    .with_primary(*span, format!("{} = {}", param, arg))
                    .with_help(format!(
                        "newtype one side, e.g. `type Cached{arg} = {arg} with (…)`, and pass that instead"
                    ))
                    .with_why(format!(
                        "`{callee}` returns `{param} or {other}`; the compiler picks the branch from the value's \
                         type, so with {param} = {arg} both branches would be `{arg}` and the caller could not \
                         tell them apart [type.errors/ER3a]"
                    ))
            }
            ErrorTraitMissing { ty, span } => {
                Diagnostic::error(format!("error type `{}` does not implement `Error`", ty))
                    .with_code("E0344")
                    .with_primary(*span, format!("`{}` needs a `message` method", ty))
                    .with_help(format!("add `extend {ty} {{ func message(self) -> string {{ ... }} }}`"))
                    .with_why("every error type must provide `func message(self) -> string`; primitives don't qualify — newtype them [type.errors/ER4]")
            }
            DuplicateSumVariant { ty, variant, span } => {
                Diagnostic::error(format!("duplicate variant `{}` in sum type `{}`", variant, ty))
                    .with_code("E0354")
                    .with_primary(*span, format!("`{}` appears more than once", variant))
                    .with_help("flatten the union or rename one branch")
                    .with_why("a sum type cannot contain the same payload variant twice — the compiler picks the branch from the value's type, and a `(T or E) or E` value fits both [type.unions/U5]")
            }
            TryInEnsure { region, span } => {
                Diagnostic::error(format!("`try` can\'t be used {}", region))
                    .with_code("E0844")
                    .with_primary(*span, "there is no caller to propagate an error to from here")
                    .with_help("drop the `try` and handle the error where it happens: `ensure f.close() else |e| { log(e.message()) }`")
                    .with_fix("remove `try`")
                    .with_why("cleanup runs at scope exit, after the function has already decided what it returns — an error raised there has nowhere to go, so ensure ignores it by default and `else |e|` is how you see it [ctrl.ensure/ER3-ER4]")
            }
            ElseBindingNotResult { name, span } => {
                Diagnostic::error(format!("`else as {}` requires a Result condition", name))
                    .with_code("E0345")
                    .with_primary(*span, "the `if` condition has no error to bind")
                    .with_help("use `else as e` only when the condition is `if r?` on a `T or E`")
                    .with_why("`else as e` binds the error branch of a Result — Option absence has no payload [type.errors/ER22]")
            }
            TypePatternNotResult { ty_name, found, span } => {
                // Two shapes of the same mistake: a scrutinee that has no
                // branches at all, and one whose branches don't include this
                // type. The second reads badly under the first's wording.
                if matches!(found, rask_types::Type::Result { .. }) {
                    Diagnostic::error(format!(
                        "`{}` is not a branch of `{}` — this test can never be true",
                        ty_name, found
                    ))
                    .with_code("E0346")
                    .with_primary(*span, "not one of its branches")
                    .with_fix("name one of the branches, or drop the test")
                    .with_why("`is` dispatches on the branches the scrutinee actually has [type.errors/ER23]")
                } else {
                    Diagnostic::error(format!("`is {}` needs a two-branch scrutinee", ty_name))
                        .with_code("E0346")
                        .with_primary(*span, format!("found `{}`", found))
                        .with_fix("test a `T or E` or a `T?` — a plain value has no branch to pick")
                        .with_why("`is Type as name` dispatches on one branch of a two-branch value [type.errors/ER23]")
                }
            }
            TypePatternNotInUnion { ty_name, union, span } => {
                Diagnostic::error(format!("`{}` is not a component of `{}`", ty_name, union))
                    .with_code("E0347")
                    .with_primary(*span, format!("not in `{}`", union))
                    .with_help("the type in a type pattern must appear in the Result's error union")
                    .with_why("type dispatch can only match types that the Result is declared to contain [type.errors/ER23]")
            }
            BadFieldAnnotation { attr, field, problem, fix, span } => {
                Diagnostic::error(format!("`@{}` on field `{}`: {}", attr, field, problem))
                    .with_code("E0376")
                    .with_primary(*span, format!("`@{}` here", attr))
                    .with_fix(fix.clone())
                    .with_why("a serialization annotation the compiler can't act on is worse than one it rejects — the wire format would differ from what the source says [std.encoding/E19, E21]")
            }

            BadAnnotation { name, problem, fix, why, span } => {
                Diagnostic::error(format!("annotation `{}`: {}", name, problem))
                    .with_code("E0843")
                    .with_primary(*span, format!("`{}` here", name))
                    .with_fix(fix.clone())
                    .with_why(*why)
            }

            LegacyWrapperConstructor { name, span } => {
                let (what, fix) = match name.as_str() {
                    "Some" => (
                        "Option has no `Some` constructor — bare values auto-wrap at return/assignment",
                        "drop the wrapper: `return value` instead of `return Some(value)`",
                    ),
                    "Ok" => (
                        "Result has no `Ok` constructor — bare T values auto-wrap at return",
                        "drop the wrapper: `return value` instead of `return Ok(value)`",
                    ),
                    "Err" => (
                        "Result has no `Err` constructor — return the error value directly",
                        "drop the wrapper: `return MyError.Variant` instead of `return Err(MyError.Variant)`",
                    ),
                    _ => ("legacy wrapper constructor", "remove the wrapper"),
                };
                Diagnostic::error(format!("`{}(...)` is no longer a valid constructor", name))
                    .with_code("E0348")
                    .with_primary(*span, format!("`{}` is not callable", name))
                    .with_help(fix)
                    .with_why(format!("{} [type.optionals/OPT2, type.errors/ER2]", what))
            }
            MatchOnOption { span } => {
                Diagnostic::error("match on an Option is not supported")
                    .with_code("E0349")
                    .with_primary(*span, "Option is not a user enum")
                    .with_help("use the ?-operator family: `if x? { ... } else { ... }`, `x?.field ?? default`, or `if x == none { return }`")
                    .with_why("Option has two states — the operator family covers both more concisely [type.optionals/NO_MATCH]")
            }
            LegacyWrapperPattern { name, with_binding, span } => {
                let fix = match name.as_str() {
                    "Some" if *with_binding => "use the operator form: `if x? as v { ... }`, or `let v = x ?? return none` in guard position",
                    "Some" => "use the operator form: `if x? { ... }`, `x?`, or `x != none`",
                    "None" => "use `x == none` for the absent check",
                    "Ok" if *with_binding => "use the operator form: `if r? as v { ... }`, or a type pattern: `r is <T> as v else { ... }`",
                    "Ok" => "use `r?` for the present check",
                    "Err" if *with_binding => "use a type pattern: `if r is <ErrType> as e { ... }`, `match r { ... <ErrType> as e => ... }`",
                    "Err" => "use `r is <ErrType>` with the function's actual error type",
                    _ => "use the operator form or a type pattern",
                };
                Diagnostic::error(format!("`{}` is not a pattern name", name))
                    .with_code("E0350")
                    .with_primary(*span, format!("`{}` is not a pattern", name))
                    .with_help(fix)
                    .with_why("Option/Result have no Some/None/Ok/Err constructors or patterns — operators and type patterns cover all cases [type.optionals/OPT2, type.errors/ER2]")
            }
            InvalidCast { src_ty: source, dst_ty: target, target_name, class, span } => {
                use rask_types::InvalidCastClass as C;
                let n = target_name;
                let (label, why, fix) = match class {
                    C::Narrowing => (
                        format!("cannot narrow `{}` to `{}` with `as`", source, target),
                        "`as` only permits lossless widening — narrowing may lose data [type.primitives/CV2]",
                        Some(format!("x.to<{n}>()!   // asserts it fits\n  x.wrap<{n}>()   // wraps\n  x.clamp<{n}>()   // clamps", n = n)),
                    ),
                    C::SignReinterpret => (
                        format!("cannot reinterpret sign converting `{}` to `{}` with `as`", source, target),
                        "`as` only permits lossless widening — a negative value has no unsigned representation [type.primitives/CV3]",
                        Some(format!("x.to<{n}>()!   // asserts it fits\n  x.wrap<{n}>()   // bit-preserving\n  x.clamp<{n}>()   // clamps", n = n)),
                    ),
                    C::FloatToInt => (
                        format!("cannot convert float `{}` to integer `{}` with `as`", source, target),
                        "float-to-int loses the fraction and can overflow — `as` doesn't allow it [type.primitives/CV4]",
                        Some(format!("x.round<{n}>()!   // nearest\n  x.floor<{n}>()!   // toward -inf\n  x.ceil<{n}>()!   // toward +inf\n  x.to<{n}>()!   // only if there's no fraction", n = n)),
                    ),
                    C::FloatNarrowing => (
                        format!("cannot narrow `{}` to `{}` with `as`", source, target),
                        "`as` only permits lossless widening — narrowing a float loses precision [type.primitives/CV4]",
                        None,
                    ),
                    C::IntToChar => (
                        format!("cannot convert `{}` to `char` with `as`", source),
                        "not every integer is a valid Unicode scalar value [type.primitives/CH5]",
                        Some("char.from_u32(n)   // returns char?".to_string()),
                    ),
                    C::Bool => (
                        "no conversion between `bool` and numeric types with `as`".to_string(),
                        "`bool` is not a number — there is no implicit int↔bool conversion [type.primitives/BL3]",
                        Some("n != 0   // int → bool\n  if b { 1 } else { 0 }   // bool → int".to_string()),
                    ),
                    C::Other => (
                        format!("cannot convert `{}` to `{}` with `as`", source, target),
                        "`as` only permits lossless widening [type.primitives/CV1]",
                        None,
                    ),
                };
                let mut diag = Diagnostic::error(label.clone())
                    .with_code("E0817")
                    .with_primary(*span, label)
                    .with_why(why);
                if let Some(fix) = fix {
                    diag = diag.with_fix(fix.clone()).with_help(format!("use an explicit conversion:\n  {}", fix));
                }
                diag
            }
            AsCastNotConvertible { src_ty, target_name, span } => {
                Diagnostic::error(format!(
                    "`as {}` reinterprets the bits — it doesn't convert",
                    target_name,
                ))
                    .with_code("E0838")
                    .with_primary(*span, format!("this is a `{}`", src_ty))
                    .with_fix(format!(
                        "to give a value a type, annotate the binding:\n                           let x: {t} = …\n\
                         to reinterpret on purpose, say so:\n                           unsafe {{ … as {t} }}",
                        t = target_name,
                    ))
                    .with_why(
                        "`as` converts between numbers and boxes a trait object \
                         (`as any Trait`); to any other target it is a bit \
                         reinterpretation, which is unsafe [type.primitives/CV1–CV4, \
                         mem.unsafe]",
                    )
            }
            InvalidConvert { message, span } => {
                Diagnostic::error(message.clone())
                    .with_code("E0818")
                    .with_primary(*span, "invalid conversion form")
                    .with_why("each conversion form names its data-loss behavior; the source and target kinds must match it [type.primitives/CV5–CV10]")
            }
            IntLiteralOutOfRange { literal, ty, min, max, span } => {
                let label = format!("`{}` doesn't fit in `{}`", literal, ty);
                Diagnostic::error(format!("integer literal `{}` is out of range for `{}`", literal, ty))
                    .with_code("E0825")
                    .with_primary(*span, label)
                    .with_why(format!(
                        "`{}` holds {} through {} — a literal outside that range would have to \
                         wrap, and nothing here wraps silently",
                        ty, min, max,
                    ))
                    .with_help(format!(
                        "use a type that holds it, or convert at the use site:\n  \
                         x.wrap<{ty}>()   // wraps\n  x.clamp<{ty}>()   // clamps",
                        ty = ty,
                    ))
            }
            ToMapNeedsPairs { elem, span } => {
                Diagnostic::error(format!(
                    "`to_map` needs a sequence of pairs, got a sequence of `{}`",
                    elem
                ))
                .with_code("E0830")
                .with_primary(*span, "each item must be a (K, V) tuple")
                .with_fix(
                    "produce the pairs first — `.map(|u| (u.id, u))` — then `to_map()`".to_string(),
                )
                .with_why(
                    "a Map needs a key per value, and `to_map` reads the key out of the first tuple slot rather than inventing one [type.sequence/SEQ29]"
                        .to_string(),
                )
            }

            UnhashableMapKey { key, fix, span } => {
                use rask_types::MapKeyFix;
                let d = Diagnostic::error(format!("`{}` can't be a Map key", key))
                    .with_code("E0834")
                    .with_primary(*span, format!("`{}` is not Hashable", key));
                match fix {
                    MapKeyFix::Float => {
                        let bits = if *key == rask_types::Type::F32 { 32 } else { 64 };
                        d.with_fix(format!(
                            "key on the bits — `map.insert(x.to_bits(), v)` with a `u{bits}` key — or on a rounded integer if that is what the key means"
                        ))
                        .with_why(
                            "a Map key has to hash equal whenever it compares equal, and `NaN != NaN` breaks that — a NaN key can never be looked up again, and `-0.0` and `0.0` compare equal while their bits differ [type.generics/HA4]"
                                .to_string(),
                        )
                    }
                    MapKeyFix::NominalClause => d
                        .with_fix(format!(
                            "list it where the type is declared: `type {key} = … with (Equal, Hashable)`"
                        ))
                        .with_why(
                            "a nominal newtype inherits exactly the traits its `with (…)` clause names — it deliberately doesn't pick up the wrapped type's, so a Map key has to be asked for [type.aliases/T11, type.generics/HA1]"
                                .to_string(),
                        ),
                    MapKeyFix::ExtendBlock => d
                        .with_fix(format!(
                            "extend {key} with Equal {{ func eq(self, other: {key}) -> bool {{ … }} }}\n  extend {key} with Hashable {{ func hash(self) -> u64 {{ … }} }}"
                        ))
                        .with_why(
                            "a Map key has to hash equal whenever it compares equal. Auto-derive covers primitives and aggregates whose every field is itself Hashable; anything else says so with a declared conformance [type.generics/HA1, G1]"
                                .to_string(),
                        ),
                }
            }

            LinearInContainer { container, elem, span } => {
                let rule = if container == "Map" { "RC3" } else { "RC1" };
                let label = format!("`{}` cannot hold linear value `{}`", container, elem);
                Diagnostic::error(label.clone())
                    .with_code("E0820")
                    .with_primary(*span, label)
                    .with_why(format!(
                        "`{}` drop can't consume its elements, but `{}` is linear — it must be \
                         consumed exactly once, so it can't be silently dropped [mem.resource-types/{}]",
                        container, elem, rule,
                    ))
                    .with_help(
                        "store linear values in a `Pool<T>` (explicit removal, RC2) or an \
                         optional `T?` (match and consume, RC4) — not a Vec or Map"
                            .to_string(),
                    )
            }
            IndexTypeMismatch { container, found, kind, span } => {
                use rask_types::IndexErrorKind as K;
                let (label, why, fix) = match kind {
                    K::ExpectedInteger => (
                        format!("cannot index `{}` with `{}`", container, found),
                        "position-indexed containers take an integer index — any integer width, range-checked as a value [std.collections/V1]".to_string(),
                        None,
                    ),
                    K::ExpectedKey(key) => (
                        format!("cannot index `{}` with `{}`", container, found),
                        format!("a map is indexed by its key type `{}` [std.collections/K1]", key),
                        None,
                    ),
                    K::ExpectedHandle(handle) => (
                        format!("cannot index `{}` with `{}`", container, found),
                        format!("a pool is keyed by its handle, not a position — index it with `{}` [mem.pools/PL4]", handle),
                        Some(format!("let h = pool.insert(value)   // h: {}", handle)),
                    ),
                    K::NotSliceable => (
                        format!("cannot slice `{}` with a range", container),
                        "range indexing produces a slice — only Vec, arrays, slices, and strings support it [std.collections/V1]".to_string(),
                        None,
                    ),
                };
                let mut diag = Diagnostic::error(label.clone())
                    .with_code("E0819")
                    .with_primary(*span, label)
                    .with_why(why);
                if let Some(fix) = fix {
                    diag = diag.with_help(fix);
                }
                diag
            }
            FixedArrayGrowth { method, array, span } => {
                let (elem, len) = match array {
                    rask_types::Type::Array { elem, len } => (elem.to_string(), *len),
                    other => (other.to_string(), 0),
                };
                let label = format!("`{}` doesn't exist on a fixed array", method);
                Diagnostic::error(label.clone())
                    .with_code("E0843")
                    .with_primary(
                        *span,
                        format!("`[{}; {}]` always holds {} element{}",
                            elem, len, len, if len == 1 { "" } else { "s" }),
                    )
                    .with_fix(format!(
                        "let it grow: `mut a: Vec<{}> = […]` — the same literal builds one",
                        elem
                    ))
                    .with_why(
                        "a fixed array's length is part of its type, and its storage is exactly that wide — there is nowhere for another element to go [std.collections/V1]"
                            .to_string(),
                    )
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

/// Using a `Link<T>` after its node was deleted.
///
/// The move checker is what proves this, but nothing moved: `delete` freed the
/// node, so every name for it is dead. Saying "moved" here would be wrong, and
/// the generic advice — "add `.clone()`" — would hand back a second dead pointer.
fn link_deleted_diagnostic(
    name: &str,
    deleted_at: rask_ast::Span,
    use_span: rask_ast::Span,
    maybe: bool,
) -> Diagnostic {
    let headline = if maybe {
        format!("`{}` may name a deleted node — possible use after free", name)
    } else {
        format!("`{}` names a deleted node — this is a use after free", name)
    };
    let primary = if maybe {
        format!("`{}` is dead on at least one path reaching here", name)
    } else {
        format!("`{}` points at freed memory from here on", name)
    };
    Diagnostic::error(headline)
        .with_code("E0328")
        .with_primary(use_span, primary)
        .with_secondary(deleted_at, format!("the node `{}` names was deleted here", name))
        .with_help("read what you need before the delete, or keep the reference in a field so the rack can null it")
        .with_fix("move the reads above the delete, or rack the link in a `Link<T>?` field")
        .with_why("a `Link<T>` is a pointer to a node, and `delete` frees the node — so every name for it dies at once. A field can survive, because the rack nulls it and the `?` makes you check; a local can't be reached by the rack, so the compiler proves here that you never follow one")
}

impl ToDiagnostic for rask_ownership::OwnershipError {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_ownership::OwnershipErrorKind::*;

        match &self.kind {
            UseAfterMove { name, moved_at, reason }
                if matches!(reason, rask_ownership::MoveReason::LinkDeleted) =>
            {
                link_deleted_diagnostic(name, *moved_at, self.span, false)
            }

            UseAfterMaybeMove { name, moved_at, reason }
                if matches!(reason, rask_ownership::MoveReason::LinkDeleted) =>
            {
                link_deleted_diagnostic(name, *moved_at, self.span, true)
            }

            SmallInstantiationTooBig { type_name, base_name, size, offending_field } => {
                let label = match offending_field {
                    Some((field, field_size, field_ty)) => format!(
                        "{} bytes at `{}` — `{}` is a `{}` there, {} of them",
                        size, type_name, field, field_ty, field_size
                    ),
                    None => format!("{} bytes at `{}`, and the limit is 16", size, type_name),
                };
                Diagnostic::error(format!(
                    "`@small` type `{}` outgrew the copy threshold at `{}`",
                    base_name, type_name
                ))
                .with_code("E0375")
                .with_primary(self.span, label)
                .with_fix(format!(
                    "don't instantiate `{}` with a type that big — or drop `@small`, \
                     since the fence can't hold for every type argument",
                    base_name
                ))
                .with_why("`@small` is read at the definition but it's a promise about every instantiation. The same source text is 16 bytes at one type argument and 32 at another, and only the second one breaks the promise — so the check runs per instantiation, like any other generic bound [mem.value/SM3, type.generics/G2]")
            }

            SmallTypeTooBig { type_name, size, offending_field } => {
                let label = match offending_field {
                    Some((field, field_size)) => format!(
                        "{} bytes — `{}` is the {}-byte field that took it over 16",
                        size, field, field_size
                    ),
                    None => format!("{} bytes, and the limit is 16", size),
                };
                let fix = match offending_field {
                    Some((field, _)) => format!(
                        "shrink or move out `{}` — or drop `@small` and let the \
                         move errors at the call sites stand",
                        field
                    ),
                    None => "shrink the type — or drop `@small` and let the move \
                             errors at the call sites stand"
                        .to_string(),
                };
                Diagnostic::error(format!(
                    "`@small` type `{}` outgrew the copy threshold",
                    type_name
                ))
                .with_code("E0374")
                .with_primary(self.span, label)
                .with_fix(fix)
                .with_why("`@small` asserts one thing: the type stays within the 16-byte copy threshold (mem.value/SM1). It buys the *location* of the error — without it, growing past 16 bytes flips every assignment from copy to move and those errors land wherever the type is used, with only the move note pointing back at a field nobody was looking at [mem.value/SM2, VS1, VS6]")
            }

            MutateParamLeftEmpty { name, consumed_at, declared_at, maybe } => {
                let label = if *maybe {
                    format!("`{}` is given away on some paths here and not replaced", name)
                } else {
                    format!("`{}` is given away here and not replaced", name)
                };
                let fix = if *maybe {
                    format!("put a value back on every path — or move the `{}` out of the branch", name)
                } else {
                    format!("assign a replacement before returning: `{} = …`", name)
                };
                Diagnostic::error(format!("gave `{}` away and didn't put anything back", name))
                    .with_code("E0836")
                    .with_primary(*consumed_at, label)
                    .with_secondary(*declared_at, format!("`{}` is declared `mutate`", name))
                    .with_fix(fix)
                    .with_why("a `mutate` parameter is exclusive access, not ownership: the caller keeps the value and reads it after the call. Taking it out and writing a replacement back is what the mode is for — leaving the slot empty hands them a hole [mem.parameters/PM2, PM6]".to_string())
            }

            ConsumeBorrowedParam { name, declared_at, is_mutate, sink } => {
                let how = if *is_mutate { "`mutate` parameter" } else { "borrowed parameter" };
                let label = match sink {
                    Some(s) => format!("`{}` takes ownership, and `{}` isn't yours to give", s, name),
                    None => format!("this takes ownership, and `{}` isn't yours to give", name),
                };
                let mutate_note = if *is_mutate {
                    " `mutate` is exclusive access — you may write through it, not give it away."
                } else {
                    ""
                };
                Diagnostic::error(format!("cannot give away `{}` — it's borrowed, not owned", name))
                    .with_code("E0835")
                    .with_primary(self.span, label)
                    .with_secondary(*declared_at, format!("`{}` is declared as a {}", name, how))
                    .with_fix(format!("take it: `take {}: …` in the signature — then the caller can see it goes", name))
                    .with_why(format!(
                        "the caller keeps a parameter it didn't mark `take` and goes on using it, so consuming it here would leave them holding something that's gone. For a `@resource` that's a second close of a real handle.{} [mem.parameters/PM1, mem.linear/L1]",
                        mutate_note
                    ))
            }

            UseAfterMove { name, moved_at, reason } => {
                use rask_ownership::MoveReason;
                let (note, help) = match reason {
                    MoveReason::SizeExceedsThreshold { type_name, size } => (
                        format!(
                            "`{}` is {} bytes (copy threshold is 16) — assignment moves instead of copying",
                            type_name, size
                        ),
                        format!(
                            "add `{}.clone()` if you need an independent copy",
                            name
                        ),
                    ),
                    MoveReason::OwnsHeapMemory { type_name } => (
                        format!(
                            "`{}` owns heap memory — assignment moves instead of copying",
                            type_name
                        ),
                        format!(
                            "add `{}.clone()` if you need an independent copy",
                            name
                        ),
                    ),
                    MoveReason::Unique { type_name } => (
                        format!("`{}` is @unique — implicit copy is disabled", type_name),
                        format!(
                            "add `{}.clone()` if the type supports it",
                            name
                        ),
                    ),
                    MoveReason::Resource { type_name } => (
                        format!("`{}` is @resource — must be consumed exactly once", type_name),
                        "restructure so the resource is only used once".to_string(),
                    ),
                    MoveReason::LinkDeleted => (
                        // Unreachable — the guarded arm above handles it.
                        format!("`{}` names a deleted node", name),
                        "read the node before deleting it".to_string(),
                    ),
                    MoveReason::LinkMoved => (
                        format!(
                            "a `Link<T>` moves like any other name for a node — `{}` handed its node over rather than copying it",
                            name
                        ),
                        "read through the new name, or keep the edge in a `Link<T>?` field where the rack maintains it".to_string(),
                    ),
                    MoveReason::Owned => (
                        format!(
                            "`{}` is a Heap box — it was consumed there, and its \
                             memory went with it",
                            name
                        ),
                        "consume it once. If two owners are really needed, clone the \
                         value into a second box"
                            .to_string(),
                    ),
                    MoveReason::Unknown => (
                        format!("`{}` was moved — assignment transfers ownership", name),
                        format!(
                            "add `{}.clone()` if you need a separate copy",
                            name
                        ),
                    ),
                };
                Diagnostic::error(format!("use of moved value: `{}`", name))
                    .with_code("E0800")
                    .with_primary(self.span, "value used here after move")
                    .with_secondary(*moved_at, "value moved here")
                    .with_note(note)
                    .with_help(help)
            }

            UseAfterMaybeMove { name, moved_at, reason } => {
                use rask_ownership::MoveReason;
                let note = match reason {
                    MoveReason::Resource { type_name } => format!(
                        "`{}` is @resource — moved on one branch but not the other",
                        type_name
                    ),
                    MoveReason::Owned => format!(
                        "`{}` is a Heap box — consumed on one branch but not the other, \
                         and after the branches join the compiler has to assume it went",
                        name
                    ),
                    _ => format!(
                        "`{}` is moved on one branch but not the other — after the branches \
                         join the compiler must assume it was moved",
                        name
                    ),
                };
                Diagnostic::error(format!("use of maybe-moved value: `{}`", name))
                    .with_code("E0813")
                    .with_primary(self.span, "value used here, but it may have been moved")
                    .with_secondary(*moved_at, "value moved here on one path")
                    .with_note(note)
                    .with_help(format!(
                        "move `{}` on every path, or restructure so the use happens inside the \
                         branch that still owns it",
                        name
                    ))
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

            MoveFromBorrowedParam { name } => {
                Diagnostic::error(format!(
                    "cannot move `{}` — parameter is borrowed, not owned",
                    name
                ))
                .with_code("E0806")
                .with_primary(self.span, "move occurs here")
                .with_fix(format!("use `take {}` in the parameter list to transfer ownership", name))
                .with_why("borrowed parameters can only be read — the caller retains ownership")
            }

            ResourceAlreadyConsumed { name, consumed_at } => {
                Diagnostic::error(format!("resource `{}` already consumed", name))
                    .with_code("E0807")
                    .with_primary(self.span, "second use here")
                    .with_secondary(*consumed_at, "resource consumed here")
                    .with_why("resources must be consumed exactly once")
            }

            MutateWhileBorrowed { name, borrow_span } => {
                Diagnostic::error(format!(
                    "`{}` cannot be changed while it's being read",
                    name
                ))
                .with_code("E0802")
                .with_primary(self.span, "mutation occurs here")
                .with_secondary(*borrow_span, "borrow is active here")
                .with_help(format!("restructure so the borrow ends before mutating, or use `{}.clone()` to work on an independent copy", name))
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

            MutateParamNotReplaced { name, ty, consumed_at } => {
                let ctor = if ty.is_empty() {
                    "a new one".to_string()
                } else {
                    format!("`{}.new(…)`", ty)
                };
                Diagnostic::error(format!(
                    "`{}` was consumed and never put back",
                    name
                ))
                .with_code("E0807")
                .with_primary(self.span, format!("`{}` is still empty when this returns", name))
                .with_secondary(*consumed_at, format!("`{}` was consumed here", name))
                .with_help(format!(
                    "assign {} to `{}` before returning, or declare it `take {}` and return the replacement",
                    ctor, name, name
                ))
                .with_fix(format!("{} = {}", name, ctor))
                .with_why("`mutate` lends the value and takes it back — the caller keeps using the same binding afterwards. Consuming it is allowed, because consume-and-replace is a real pattern, but the slot has to hold something again by the time control leaves [mem.parameters/PM3]")
            }

            ConsumedBorrowedParam { name, ty } => {
                let decl = if ty.is_empty() {
                    format!("take {}", name)
                } else {
                    format!("take {}: {}", name, ty)
                };
                Diagnostic::error(format!(
                    "`{}` is borrowed from the caller — it can't be given away here",
                    name
                ))
                .with_code("E0806")
                .with_primary(self.span, format!("`{}` is consumed here", name))
                .with_help(format!(
                    "declare it `{}` if this function should own it, or read what you need instead of passing it on",
                    decl
                ))
                .with_fix(decl)
                .with_why("a parameter without `take` is a borrow: the caller keeps the value and goes on using it. Consuming it here would consume it twice — for a `@resource` that means the cleanup runs twice, which mem.linear/L1 exists to make impossible at compile time [mem.parameters/PM3, mem.linear/L1]")
            }

            LinkOutlivesRack { link, rack, via } => {
                let (primary, fix) = match via {
                    rask_ownership::LinkEscape::Return => (
                        format!(
                            "`{}` lives in `{}`, and `{}` dies when this function returns",
                            link, rack, rack
                        ),
                        format!(
                            "return the node's data instead, or take the rack as a parameter so it outlives the call: `func …(mutate {}: Rack<…>) -> Link<…>`",
                            rack
                        ),
                    ),
                    rask_ownership::LinkEscape::Assignment { target } => (
                        format!(
                            "`{}` outlives `{}`, and the node `{}` points at dies with it",
                            target, rack, link
                        ),
                        format!(
                            "move `{}` out to where `{}` lives, or copy the fields you need out of the node before the scope ends",
                            rack, target
                        ),
                    ),
                };
                Diagnostic::error(format!("`{}` would outlive the rack it points into", link))
                    .with_code("E0379")
                    .with_primary(self.span, primary)
                    .with_fix(fix)
                    .with_why("a `Link<T>` is a pointer to a node, and the nodes live in the rack — so when the rack goes out of scope the node goes with it and the link dangles. Nothing else catches this: no `delete` happened, so the use-after-delete rule never looks, and a link is Copy so it escapes the scope that produced it. A link into a rack the *caller* owns is fine — that rack outlives the call")
            }
            NodeWriteNeedsWritableRack { link, rack } => {
                // `with_help` isn't rendered on this path — `fix` and `why` are —
                // so the actionable line goes in `with_fix`.
                let d = match rack {
                    Some(s) => Diagnostic::error(format!(
                        "cannot write this node — `{}` is only readable here",
                        s
                    ))
                    .with_code("E0378")
                    .with_primary(
                        self.span,
                        format!("writes a node in `{}` through `{}`", s, link),
                    )
                    .with_fix(format!(
                        "make the rack writable: `mut {}` if it's a local, `mutate {}: Rack<…>` if it's a parameter",
                        s, s
                    )),
                    None => Diagnostic::error(format!(
                        "cannot write this node — `{}` is a view",
                        link
                    ))
                    .with_code("E0378")
                    .with_primary(
                        self.span,
                        format!("`{}` was lent for reading, not for writing", link),
                    )
                    .with_fix(format!(
                        "say the function writes it: `mutate {}: Link<…>` in the signature — the caller then marks it `mutate {}` at the call",
                        link, link
                    )),
                };
                d.with_why("writing a node is a permission, and permission travels with the link: `n: Link<T>` is a view — it reads the node and everything reachable from it — and `mutate n: Link<T>` is the writable one. It's the same borrow-versus-mutate distinction every other type has, so no second link type is needed and no rack has to be threaded through. A view stays a view when you follow an edge, and can't be passed on as `mutate`; a writer's permission does travel along edges, because an edge only connects nodes in one rack")
            }
            UndeclaredDelete { param, operation } => {
                Diagnostic::error(format!(
                    "this can delete nodes the caller never named — declare `deleting {}`",
                    param
                ))
                .with_code("E0329")
                .with_primary(self.span, format!("{} chooses which nodes die", operation))
                .with_help(format!(
                    "declare it: `deleting {}: Rack<…>` — or delete only links the caller handed over, as `take` parameters",
                    param
                ))
                .with_fix(format!("deleting {}", param))
                .with_why("`delete(take link)` is safe for the caller because the link is consumed at the call site — they can see the name die. A delete that chooses its own victim can't be seen from outside, so the caller's links are revoked at the call instead, and `deleting` is what tells them to expect it. `mutate` doesn't imply it: inserting and writing can't invalidate a link, deleting can")
            }

            ResourceNotConsumedOpaque { name, where_ } => {
                Diagnostic::error(format!(
                    "resource `{}` must be consumed before scope exit",
                    name
                ))
                .with_code("E0805")
                .with_primary(self.span, "resource goes out of scope here")
                .with_help(format!(
                    "consume the resource inside `{}`, then `discard {}` — or hold it \
                     somewhere the compiler can name, like a plain field",
                    name, name
                ))
                .with_fix(format!(
                    "take the resource out of `{}` and consume it",
                    name
                ))
                .with_note(format!(
                    "the resource sits in {} — there is no field path to it, so the whole of `{}` is owed rather than one field",
                    where_, name
                ))
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }

            ResourceNotConsumed { name } => {
                Diagnostic::error(format!(
                    "resource `{}` must be consumed before scope exit",
                    name
                ))
                .with_code("E0805")
                .with_primary(self.span, "resource goes out of scope here")
                .with_help(format!(
                    "call a consuming method (e.g. `.close()`) on `{}`, or use `ensure` for cleanup",
                    name
                ))
                .with_fix(format!("call a consuming method (e.g. `.close()`) on `{}`, or use `ensure` for cleanup", name))
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }

            ResourceDiscardedAsStatement { type_name } => {
                Diagnostic::error(format!(
                    "value of resource type `{}` is dropped without being consumed",
                    type_name
                ))
                .with_code("E0840")
                .with_primary(self.span, "produced here and immediately dropped")
                .with_help("bind it to a name and call a consuming method on it, e.g. `let h = ...; h.join()`")
                .with_fix("bind the value to a name so it can be consumed")
                .with_why("resource types must be explicitly consumed — this prevents resource leaks")
            }

            OwnedNotConsumed { name } => {
                Diagnostic::error(format!(
                    "`{}` was allocated with `Heap(…)` and never dropped",
                    name
                ))
                .with_code("E0837")
                .with_primary(self.span, "the value goes out of scope here, still owned")
                .with_help(format!(
                    "consume it exactly once: `drop({})`, hand it to a `take` parameter, \
                     store it in a field, or return it",
                    name
                ))
                .with_fix(format!("drop({})", name))
                .with_why(
                    "a Heap value has one owner and must be consumed exactly once \
                     (mem.linear/L1) — nothing else frees it",
                )
            }

            ResourceNotConsumedInClosure { name, context } => {
                Diagnostic::error(format!(
                    "resource `{}` captured by {} is not consumed on all code paths",
                    name, context
                ))
                .with_code("E0810")
                .with_primary(self.span, format!("{} body ends without consuming `{}`", context, name))
                .with_help(format!(
                    "consume `{}` on every code path, or use `ensure` inside the {} body",
                    name, context
                ))
                .with_fix(format!("consume `{}` (e.g. `ensure {{ {}.close() }}`) at the top of the {} body", name, name, context))
                .with_why("resource types must be consumed exactly once — a closure/spawn that captures a resource takes ownership and must consume it")
            }

            EnsureMaybeConsumed { name, ensure_at, consumed_at } => {
                Diagnostic::error(format!(
                    "consumption of `{}` depends on which path ran",
                    name
                ))
                .with_code("E0821")
                .with_primary(*ensure_at, "cleanup scheduled here")
                .with_secondary(*consumed_at, format!("`{}` consumed only on this branch", name))
                .with_help(format!(
                    "exit inside the consuming branch (`return` right after consuming `{}`), \
                     or consume `{}` on every path",
                    name, name
                ))
                .with_fix(format!("move the exit inside the branch that consumes `{}`", name))
                .with_why("which cleanups run is decided at compile time (ctrl.ensure/C3) — a value \
                           consumed on some paths but not others has no definite answer at scope exit (C4)")
            }

            FrozenContextMutation { context_ty, operation } => {
                Diagnostic::error(format!(
                    "cannot {} in frozen context `{}`",
                    operation, context_ty
                ))
                .with_code("E0807")
                .with_primary(self.span, format!("{} not allowed in frozen context", operation))
                .with_help("remove `frozen` from the context clause, or remove the mutation")
                .with_why("frozen contexts guarantee no structural mutations — this enables safe iteration without generation checks")
            }

            WithBlockStructuralMutation { collection, operation, binding_span } => {
                Diagnostic::error(format!(
                    "cannot {} `{}` inside `with` block",
                    operation, collection
                ))
                .with_code("E0808")
                .with_primary(self.span, format!("{} not allowed inside with block", operation))
                .with_secondary(*binding_span, "element borrowed here")
                .with_help("move the structural mutation outside the with block")
                .with_fix("move the structural mutation outside the with block")
                .with_why(format!(
                    "{} can reallocate, invalidating the borrowed element. \
                     Pool handles survive reallocation — use Pool if you need insert/remove inside with",
                    collection
                ))
            }

            WithBlockBoundHandleRemoved { handle, collection: _, binding_span } => {
                Diagnostic::error(format!(
                    "cannot remove `{}` inside `with` block — it's the bound element",
                    handle
                ))
                .with_code("E0809")
                .with_primary(self.span, "removing the element you're borrowing")
                .with_secondary(*binding_span, "element borrowed here")
                .with_help("move the removal outside the with block")
                .with_fix("move the removal outside the with block")
                .with_why("removing the bound element frees its memory — the binding would dangle")
            }

            WithBlockClear { collection, binding_span } => {
                Diagnostic::error(format!(
                    "cannot clear `{}` inside `with` block",
                    collection
                ))
                .with_code("E0810")
                .with_primary(self.span, "clear invalidates all elements")
                .with_secondary(*binding_span, "element borrowed here")
                .with_help("move the clear outside the with block")
                .with_fix("move the clear outside the with block")
                .with_why("clearing the collection frees all elements — the binding would dangle")
            }

            UseAfterDiscard { name, discarded_at } => {
                Diagnostic::error(format!("use of discarded value: `{}`", name))
                    .with_code("E0811")
                    .with_primary(self.span, "value used here after discard")
                    .with_secondary(*discarded_at, "value discarded here")
                    .with_help("remove the `discard` or restructure so the value isn't needed after this point")
                    .with_why("`discard` explicitly drops a value and invalidates its binding — using it afterwards is an error")
            }

            DiscardResource { name } => {
                Diagnostic::error(format!(
                    "cannot discard resource `{}` — use its consuming method",
                    name
                ))
                .with_code("E0812")
                .with_primary(self.span, "resource types cannot be discarded")
                .with_help(format!("call `.close()` or another consuming method on `{}`", name))
                .with_fix(format!("replace `discard {}` with `{}.close()`", name, name))
                .with_why("resource types must be consumed properly — `discard` would silently leak the resource")
            }

            ScopeLimitedClosureEscapes { name } => {
                Diagnostic::error(format!(
                    "closure `{}` captures scoped borrow and cannot escape",
                    name
                ))
                .with_code("E0813")
                .with_primary(self.span, "closure would outlive its captured borrow")
                .with_fix("prefix the closure with `own` to move captures instead of borrowing them")
                .with_why("closures that capture block-scoped borrows are limited to that block's lifetime — returning or storing them would create a dangling reference (SL2)")
            }

            ForMutateStructuralMutation { collection, operation, loop_span } => {
                Diagnostic::error(format!(
                    "cannot {} `{}` during `for mutate` — invalidates iteration",
                    operation, collection
                ))
                .with_code("E0814")
                .with_primary(self.span, format!("{} not allowed during mutable iteration", operation))
                .with_secondary(*loop_span, format!("`{}` is being iterated here", collection))
                .with_help("collect changes and apply them after the loop")
                .with_why("structural mutations (insert, remove, push, clear) invalidate the iterator — elements may shift or be reallocated")
            }

            ForMutateTakeItem { item, collection, loop_span } => {
                Diagnostic::error(format!(
                    "cannot pass `{}` to `take` parameter — borrowed from `{}`",
                    item, collection
                ))
                .with_code("E0815")
                .with_primary(self.span, "would move element out of collection")
                .with_secondary(*loop_span, format!("`{}` is borrowed from `{}` during iteration", item, collection))
                .with_help(format!("clone `{}` before passing, or restructure to avoid taking ownership", item))
                .with_why("for-mutate borrows elements in place — taking ownership would leave a hole in the collection")
            }

            LinearWildcardDiscard { position, type_name } => {
                use rask_ownership::LinearDiscardPosition;
                let (label, help) = match position {
                    LinearDiscardPosition::Scrutinee => (
                        format!("`_` would silently drop linear value of type `{}`", type_name),
                        "name the value with `name` (or `Type as name`) and consume it on every arm".to_string(),
                    ),
                    LinearDiscardPosition::Field { constructor, field, index } => {
                        let where_ = match (field, index) {
                            (Some(f), _) => format!("field `{}` of `{}`", f, constructor),
                            (None, Some(i)) => format!("field {} of `{}`", i, constructor),
                            (None, None) => format!("a field of `{}`", constructor),
                        };
                        (
                            format!("`_` discards linear `{}` payload in {}", type_name, where_),
                            "bind the linear field by name and consume it (e.g. `try value.close()`)".to_string(),
                        )
                    }
                };
                Diagnostic::error(format!(
                    "wildcard would silently drop linear value of type `{}`",
                    type_name
                ))
                .with_code("E0816")
                .with_primary(self.span, label)
                .with_help(help)
                .with_why("linear values must be consumed exactly once — a `_` here would leak the resource [ER42/ER43]")
            }
        }
    }
}

// ============================================================================
// Runtime Errors
// ============================================================================

impl ToDiagnostic for rask_interp::RuntimeDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic {
        use rask_interp::RuntimeError;

        match &self.error {
            RuntimeError::DivisionByZero => {
                Diagnostic::error("division by zero")
                    .with_code("R0001")
                    .with_primary(self.span, "divisor is zero")
                    .with_help("add a check before dividing: `if divisor != 0 { ... }`")
                    .with_fix("check divisor != 0 before division")
                    .with_why("division by zero is undefined")
            }

            RuntimeError::IntegerOverflow(msg) => {
                Diagnostic::error(msg)
                    .with_code("R0010")
                    .with_primary(self.span, "arithmetic overflowed here")
                    .with_why("default arithmetic panics on overflow in all builds (type.overflow/OV1)")
                    .with_help("use `Wrapping<T>` from `num` for intentional wrapping, or widen the type")
            }

            RuntimeError::IndexOutOfBounds { index, len } => {
                Diagnostic::error(format!("index {} out of bounds", index))
                    .with_code("R0002")
                    .with_primary(self.span, format!("length is {}", len))
                    .with_help("check the index is within bounds before accessing")
                    .with_fix("add a bounds check: `if index < collection.len() { ... }`")
                    .with_why("accessing an out-of-bounds index is unsafe")
            }

            RuntimeError::UndefinedVariable(name) => {
                Diagnostic::error(format!("undefined variable `{}`", name))
                    .with_code("R0003")
                    .with_primary(self.span, "not found in scope")
                    .with_help("check the variable name or ensure it's declared before use")
            }

            RuntimeError::UndefinedFunction(name) => {
                Diagnostic::error(format!("undefined function `{}`", name))
                    .with_code("R0004")
                    .with_primary(self.span, "not found in scope")
                    .with_help(
                        "check the spelling, import the module that declares it, \
                         or — if it is a stdlib function — this backend may not implement it yet",
                    )
            }

            RuntimeError::TypeError(msg) => {
                Diagnostic::error(msg)
                    .with_code("R0005")
                    .with_primary(self.span, "type error occurred here")
            }

            RuntimeError::ArityMismatch { expected, got } => {
                Diagnostic::error(format!(
                    "expected {} argument{}, got {}",
                    expected,
                    if *expected == 1 { "" } else { "s" },
                    got
                ))
                .with_code("R0006")
                .with_primary(self.span, "wrong number of arguments")
                .with_help(format!("function expects {} argument{}", expected, if *expected == 1 { "" } else { "s" }))
            }

            RuntimeError::NoSuchMethod { ty, method } => {
                Diagnostic::error(format!("no method `{}` on type `{}`", method, ty))
                    .with_code("R0007")
                    .with_primary(self.span, format!("method not found on `{}`", ty))
                    .with_help("check the method name or the type's available methods")
            }

            RuntimeError::NoSuchField { ty, field } => {
                Diagnostic::error(format!("no field `{}` on type `{}`", field, ty))
                    .with_code("R0008")
                    .with_primary(self.span, format!("field not found on `{}`", ty))
                    .with_help("check the field name or the type's available fields")
            }

            RuntimeError::ResourceClosed { resource_type, operation } => {
                Diagnostic::error(format!("resource is closed; cannot {} a closed {}", operation, resource_type))
                    .with_code("R0009")
                    .with_primary(self.span, "resource already closed")
                    .with_help("check if the resource is still open before using it")
                    .with_why("using a closed resource is invalid")
            }

            RuntimeError::Panic(msg) => {
                Diagnostic::error(format!("panic: {}", msg))
                    .with_code("R0010")
                    .with_primary(self.span, "panic occurred here")
            }

            RuntimeError::MainReturnedError(msg) => {
                Diagnostic::error(format!("main returned an error: {}", msg))
                    .with_code("R0022")
                    .with_primary(self.span, "error returned from here")
                    .with_help("the process exits with status 1")
                    .with_why("an error out of main is a failed run, not a successful one [struct.targets/EX4]")
            }

            RuntimeError::NoMatchingArm => {
                Diagnostic::error("no matching arm in match")
                    .with_code("R0011")
                    .with_primary(self.span, "no arm matches")
                    .with_help("add a wildcard `_` arm to handle all cases")
                    .with_fix("add `_ => { ... }` at the end of the match")
                    .with_why("match expressions must be exhaustive")
            }

            RuntimeError::MultipleEntryPoints => {
                Diagnostic::error("multiple @entry functions found")
                    .with_code("R0012")
                    .with_primary(self.span, "multiple entry points")
                    .with_help("only one `func main()` or `@entry` function allowed per program")
                    .with_why("programs need a single, unambiguous entry point")
            }

            RuntimeError::NoEntryPoint => {
                Diagnostic::error("no entry point found")
                    .with_code("R0013")
                    .with_primary(self.span, "no entry point")
                    .with_help("add `func main()` or mark a function with `@entry`")
                    .with_why("programs need an entry point to start execution")
            }

            RuntimeError::RecursionTooDeep { function, depth } => {
                Diagnostic::error(format!(
                    "recursion too deep: {} nested calls, and the interpreter is out \
                     of stacks to continue on",
                    depth
                ))
                .with_code("R0023")
                .with_primary(self.span, format!("`{}` called at the limit", function))
                .with_fix(
                    "check the base case if this was meant to terminate; otherwise \
                     rewrite it as a loop, or run it natively with `rask run`, which \
                     has no such limit",
                )
                .with_why(
                    "the interpreter spends one host stack frame per Rask call and those \
                     frames are large, so it moves onto a fresh stack every few hundred \
                     calls rather than overflowing. That chain is capped — around a \
                     gigabyte of live stack — so a recursion that never terminates stops \
                     here with a message instead of taking the machine down",
                )
            }

            RuntimeError::AssertionFailed(msg) => {
                Diagnostic::error(format!("assertion failed: {}", msg))
                    .with_code("R0014")
                    .with_primary(self.span, "assertion failed here")
                    .with_why("assertion detected a violated invariant")
            }

            RuntimeError::CheckFailed(msg) => {
                Diagnostic::error(format!("check failed: {}", msg))
                    .with_code("R0015")
                    .with_primary(self.span, "check failed here")
                    .with_why("check detected a test failure")
            }

            RuntimeError::ForcedAbsent => {
                Diagnostic::error("! on a value that was absent")
                    .with_code("R0016")
                    .with_primary(self.span, "there was no value here to take")
                    .with_help("`??` substitutes a value, `x is T as v` tests for one first")
                    .with_fix("replace `x!` with `x ?? default`")
                    .with_why("`!` takes the payload of a `T?` and panics when the value is absent [type.optionals/OPT13]")
            }

            RuntimeError::ForcedError => {
                Diagnostic::error("! on a value that was an error")
                    .with_code("R0016")
                    .with_primary(self.span, "this call returned its error branch")
                    .with_help("`try` propagates the error, `catch e =>` handles it here")
                    .with_fix("replace `r!` with `try r`")
                    .with_why("`!` takes the ok payload of a `T or E` and panics on the error branch [type.errors/ER15]")
            }

            RuntimeError::Generic(msg) => {
                Diagnostic::error(msg)
                    .with_code("R0017")
                    .with_primary(self.span, "error occurred here")
            }


            // Control flow and special cases - no diagnostic
            RuntimeError::Exit(_)
            | RuntimeError::Return(_)
            | RuntimeError::Break(..)
            | RuntimeError::Continue(_)
            | RuntimeError::TryError(_)
            | RuntimeError::TestSkipped(_)
            | RuntimeError::TestExpectFail => {
                // These are not actual errors, just control flow
                Diagnostic::error(format!("{}", self.error))
                    .with_primary(self.span, "control flow")
            }
        }
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! How the interpreter's runtime errors read to the user.

use rask_diagnostics::{Diagnostic, ToDiagnostic};

impl ToDiagnostic for crate::RuntimeDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic {
        use crate::RuntimeError;

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
                    .with_code("R0018")
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

            // The detail says whether it wants framing. A comparison reads as
            // "assertion failed: 1 == 2 (…)"; a hand-written message stands on
            // its own, which is what native prints. Prefixing both gave the one
            // form the words twice and the other a prefix native never uses
            // (#1098).
            RuntimeError::AssertionFailed(detail) => {
                Diagnostic::error(detail.framed("assertion failed"))
                    .with_code("R0014")
                    .with_primary(self.span, "assertion failed here")
                    .with_why("assertion detected a violated invariant")
            }

            RuntimeError::CheckFailed(detail) => {
                Diagnostic::error(detail.framed("check failed"))
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

            RuntimeError::ForcedError(msg) => {
                Diagnostic::error(format!("! on a value that was an error: {}", msg))
                    .with_code("R0019")
                    .with_primary(self.span, format!("`!` found the error `{}` here", msg))
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

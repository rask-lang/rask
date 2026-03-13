// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Stdlib dispatch for the MIR interpreter.
//!
//! `PureStdlib` handles comptime (no I/O). `RealStdlib` (future) handles scripting.

use crate::{MiriError, MiriValue};

/// Dispatch trait for stdlib function calls.
///
/// The MIR interpreter calls this when it encounters a `Call` to a function
/// that isn't in its function table (i.e., not user-defined MIR).
pub trait StdlibProvider {
    /// Try to handle a stdlib call. Returns:
    /// - `Ok(Some(value))` — call handled, return value
    /// - `Ok(None)` — call handled, no return value (void)
    /// - `Err(...)` — call failed or not recognized
    fn call(&mut self, name: &str, args: &[MiriValue]) -> Result<Option<MiriValue>, MiriError>;
}

/// Comptime stdlib: pure computation only, no I/O.
pub struct PureStdlib;

impl StdlibProvider for PureStdlib {
    fn call(&mut self, name: &str, args: &[MiriValue]) -> Result<Option<MiriValue>, MiriError> {
        match name {
            // String methods
            "string_len" | "str_len" => {
                let s = args.first().ok_or_else(|| MiriError::UnsupportedOperation(
                    format!("{name}: missing argument"),
                ))?;
                match s {
                    MiriValue::String(s) => Ok(Some(MiriValue::I64(s.len() as i64))),
                    _ => Err(MiriError::type_mismatch("string", s)),
                }
            }
            "string_is_empty" | "str_is_empty" => {
                let s = args.first().ok_or_else(|| MiriError::UnsupportedOperation(
                    format!("{name}: missing argument"),
                ))?;
                match s {
                    MiriValue::String(s) => Ok(Some(MiriValue::Bool(s.is_empty()))),
                    _ => Err(MiriError::type_mismatch("string", s)),
                }
            }
            "string_contains" => {
                if args.len() < 2 {
                    return Err(MiriError::UnsupportedOperation(
                        format!("{name}: expected 2 arguments"),
                    ));
                }
                match (&args[0], &args[1]) {
                    (MiriValue::String(haystack), MiriValue::String(needle)) => {
                        Ok(Some(MiriValue::Bool(haystack.contains(needle.as_str()))))
                    }
                    _ => Err(MiriError::UnsupportedOperation(
                        format!("{name}: expected string arguments"),
                    )),
                }
            }
            "string_concat" | "string_add" => {
                if args.len() < 2 {
                    return Err(MiriError::UnsupportedOperation(
                        format!("{name}: expected 2 arguments"),
                    ));
                }
                match (&args[0], &args[1]) {
                    (MiriValue::String(a), MiriValue::String(b)) => {
                        Ok(Some(MiriValue::String(format!("{a}{b}"))))
                    }
                    _ => Err(MiriError::UnsupportedOperation(
                        format!("{name}: expected string arguments"),
                    )),
                }
            }

            // Printing (comptime diagnostic — future @comptime_print)
            "print" | "println" => {
                Err(MiriError::UnsupportedOperation(
                    "I/O is not available at compile time".to_string(),
                ))
            }

            _ => {
                Err(MiriError::UnsupportedOperation(
                    format!("function '{name}' is not available at compile time"),
                ))
            }
        }
    }
}

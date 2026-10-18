// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Emitting an object file through `rask_compiler::emit`, with its MIR
//! diagnostics printed.

use rask_compiler::emit::{BackEnd, BuildMode, Entry};
use rask_diagnostics::formatter::DiagnosticFormatter;

/// Compile to `obj_path`, printing the MIR analyses' diagnostics. On failure
/// the errors that aren't diagnostics come back for the caller to report.
pub fn emit_object(
    input: &BackEnd,
    entry: &Entry,
    target: Option<&str>,
    build_mode: BuildMode,
    obj_path: &str,
) -> Result<(), Vec<String>> {
    let emitted = rask_compiler::emit::emit(input, entry, target, build_mode, obj_path);
    if !emitted.diagnostics.is_empty() {
        let fmt = DiagnosticFormatter::new(input.source_text.unwrap_or(""))
            .with_file_name(input.source_file.unwrap_or("<unknown>"));
        for d in &emitted.diagnostics {
            eprintln!("{}", fmt.format(d));
        }
    }
    if emitted.succeeded() {
        return Ok(());
    }
    if emitted.errors.is_empty() {
        return Err(vec!["aborting due to MIR analysis errors".to_string()]);
    }
    Err(emitted.errors)
}

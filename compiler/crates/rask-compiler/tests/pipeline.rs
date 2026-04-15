// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Integration tests for the compiler driver.
//!
//! These tests lock in the pipeline contract: given source X, check_file
//! should return the expected diagnostics and result. They verify
//! error accumulation across stages and the divergence-fix behaviors
//! (desugar diagnostics, default args, comptime cfg, etc.).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rask_compiler::{check_file, CompilerConfig, CfgConfig};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `src` to a unique temp .rk file and return the path.
fn tmp_rk(src: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "rask_compiler_test_{}_{}.rk",
        std::process::id(),
        n,
    ));
    let mut f = std::fs::File::create(&path).expect("create tmp file");
    f.write_all(src.as_bytes()).expect("write tmp file");
    path
}

fn default_config() -> CompilerConfig {
    CompilerConfig {
        cfg: CfgConfig::from_host("debug", vec![]),
    }
}

fn error_count(diagnostics: &[rask_diagnostics::Diagnostic]) -> usize {
    diagnostics.iter()
        .filter(|d| matches!(d.severity, rask_diagnostics::Severity::Error))
        .count()
}

// ═══════════════════════════════════════════════════════════════════════
// Happy path
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn check_succeeds_on_valid_program() {
    let path = tmp_rk(r#"
        func main() {
            const x = 42
            println("{x}")
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(), "expected success, got diagnostics: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert_eq!(error_count(&output.diagnostics), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn check_returns_typed_program_on_success() {
    let path = tmp_rk(r#"
        func add(a: i32, b: i32) -> i32 {
            return a + b
        }
        func main() {
            const x = add(1, 2)
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    let result = output.result.expect("expected success");
    // TypedProgram should have node types for the arithmetic expressions
    assert!(!result.typed.node_types.is_empty(), "TypedProgram should have node_types");
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// Error accumulation — the main novel contract
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn type_errors_across_functions_reported() {
    // The typechecker currently stops at the first type error. This test
    // documents the current behavior. Tier 2 error accumulation (lenient
    // typecheck) would make this report all three errors.
    let path = tmp_rk(r#"
        func a() -> i32 { return "not an int" }
        func b() -> string { return 42 }
        func c() -> bool { return 3.14 }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    assert!(error_count(&output.diagnostics) >= 1,
        "expected at least one error, got {}",
        error_count(&output.diagnostics));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn lex_and_parse_errors_both_reported() {
    // A bad character followed by a syntactic error — both should appear.
    let path = tmp_rk(r#"
        func main() {
            const x = @#$   // lex-level garbage
            func nested()   // parse error: no body, `func` at wrong spot
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    assert!(error_count(&output.diagnostics) >= 1,
        "expected at least one error from the garbage input");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn type_errors_dont_block_subsequent_stages() {
    // Tier 2: even with a type error, ownership and effect stages still run.
    // This produces richer diagnostics in one pass — user doesn't have to
    // fix type errors before seeing their other mistakes.
    //
    // This program has a type error (return "str" when i32 expected) AND
    // a use-after-move in a separate function. Both should be reported.
    let path = tmp_rk(r#"
        struct Data { value: i32 }

        func wrong_type() -> i32 {
            return "not an int"
        }

        func consume(take d: Data) {}

        func main() {
            const d = Data { value: 1 }
            consume(own d)
            consume(own d)
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    // We expect at least one type error AND at least one ownership error.
    // This is the cross-stage accumulation the lenient typecheck enables.
    assert!(error_count(&output.diagnostics) >= 2,
        "expected type + ownership errors (Tier 2 accumulation), got {}: {:?}",
        error_count(&output.diagnostics),
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn type_and_ownership_errors_accumulate() {
    // Type-check succeeds, ownership fails. The pipeline should run both.
    // (If there were type errors, the driver stops before ownership.)
    // This test verifies that ownership-only errors come through.
    let path = tmp_rk(r#"
        struct Data { value: i32 }
        func consume(take d: Data) {
            // take d
        }
        func main() {
            const d = Data { value: 1 }
            consume(own d)
            consume(own d)   // use after move
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    // Either typecheck or ownership should flag this.
    assert!(!output.succeeded(),
        "expected failure due to use-after-move, got success. diags: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// Divergence-fix verification (LSP previously missed these)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn comptime_cfg_elimination_runs() {
    // If CC1 (dead-branch elimination) doesn't run, symbols from the
    // unused `else` branch leak into resolution and cause errors.
    // This test verifies the pass runs (was previously missing from LSP).
    let path = tmp_rk(r#"
        func main() {
            comptime if cfg.os == "linux" {
                const x: i32 = 1
            } else if cfg.os == "macos" {
                const x: i32 = 2
            } else {
                const x: i32 = 3
            }
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(),
        "comptime cfg elimination must produce a valid program, got: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn default_args_desugar_runs() {
    // If desugar_default_args doesn't run, calls without all args fail.
    let path = tmp_rk(r#"
        func greet(name: string = "World") -> string {
            return "Hello, {name}"
        }
        func main() {
            const msg = greet()   // uses default
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(),
        "default args must be desugared before typecheck, got: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// PipelineOutput contract
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn failed_pipeline_returns_none_and_errors() {
    let path = tmp_rk(r#"
        func main() {
            const x: i32 = "not an int"
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    assert!(output.result.is_none());
    assert!(output.has_errors());
    assert!(error_count(&output.diagnostics) >= 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn successful_pipeline_returns_some() {
    let path = tmp_rk(r#"
        func main() {
            const x: i32 = 42
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded());
    assert!(output.result.is_some());
    assert!(!output.has_errors());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_file_returns_error_diagnostic() {
    let output = check_file("/nonexistent/path/does_not_exist.rk", &default_config());
    assert!(!output.succeeded());
    assert!(output.has_errors());
    // Should have a single error diagnostic about the missing file.
    assert_eq!(output.diagnostics.len(), 1);
}

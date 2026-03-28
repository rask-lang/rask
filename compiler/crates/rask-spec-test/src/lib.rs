// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Literate testing for Rask spec files.
//!
//! Extracts annotated code blocks from markdown spec files and runs them
//! through the compiler to verify they behave as documented.
//!
//! # Annotation Format
//!
//! Add HTML comments before code blocks to mark them as tests.
//!
//! ## Available annotations:
//!
//! - `<!-- test: compile -->` - Must compile without errors
//! - `<!-- test: compile-fail -->` - Must fail to compile
//! - `<!-- test: parse -->` - Must parse (skip type checking)
//! - `<!-- test: parse-fail -->` - Must fail to parse
//! - `<!-- test: skip -->` - Don't test this block
//! - `<!-- test: run | expected -->` - Run via interpreter + native, verify output
//! - `<!-- test: run-interp | expected -->` - Run via interpreter only (codegen escape hatch)
//! - (no annotation) - Skipped by default

pub mod deps;
pub mod extract;
pub mod runner;

pub use deps::{check_staleness, extract_deps, SpecDeps, StalenessWarning};
pub use extract::{extract_tests, has_rk_tests, Expectation, SpecTest};
pub use runner::{run_test, run_test_with_config, run_rk_test_file, NativeResult, RkTestResult, RunConfig, TestResult, TestSummary};

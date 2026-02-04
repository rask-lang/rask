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
//! - `<!-- test: run\n...\n-->` - Run and verify output matches
//! - (no annotation) - Skipped by default

pub mod extract;
pub mod runner;

pub use extract::{extract_tests, Expectation, SpecTest};
pub use runner::{run_test, TestResult, TestSummary};

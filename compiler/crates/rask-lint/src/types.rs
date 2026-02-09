// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Output types for `rask lint`.

use serde::Serialize;

/// Complete lint report for a file.
#[derive(Debug, Serialize)]
pub struct LintReport {
    pub version: u32,
    pub file: String,
    pub success: bool,
    pub diagnostics: Vec<LintDiagnostic>,
    pub error_count: usize,
    pub warning_count: usize,
}

/// A single lint finding.
#[derive(Debug, Serialize)]
pub struct LintDiagnostic {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    pub location: LintLocation,
    pub fix: String,
}

/// Source location.
#[derive(Debug, Serialize)]
pub struct LintLocation {
    pub line: usize,
    pub column: usize,
    pub source_line: String,
}

/// Severity level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Options for lint.
pub struct LintOpts {
    /// Include rules matching these patterns (e.g., "naming/*")
    pub rules: Vec<String>,
    /// Exclude rules matching these patterns
    pub excludes: Vec<String>,
}

impl Default for LintOpts {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            excludes: Vec::new(),
        }
    }
}

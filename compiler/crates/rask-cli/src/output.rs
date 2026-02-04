//! CLI output formatting with colors and styling.
//!
//! Respects NO_COLOR and FORCE_COLOR environment variables.
//! Colors are automatically disabled when output is piped.

use colored::{ColoredString, Colorize};

/// Initialize color support based on environment.
/// Call once at startup.
pub fn init() {
    // colored crate handles NO_COLOR automatically,
    // but we add explicit FORCE_COLOR support
    if std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    } else if std::env::var("FORCE_COLOR").is_ok() {
        colored::control::set_override(true);
    }
}

// === Error Output ===

pub fn error_label() -> ColoredString {
    "error".red().bold()
}

pub fn hint_label() -> ColoredString {
    "hint".cyan()
}

pub fn hint_text(msg: &str) -> ColoredString {
    msg.dimmed()
}

pub fn error_arrow() -> ColoredString {
    "-->".blue()
}

pub fn line_number(n: usize) -> ColoredString {
    format!("{:3}", n).blue().bold()
}

pub fn pipe() -> ColoredString {
    "|".blue()
}

pub fn caret() -> ColoredString {
    "^".red().bold()
}

pub fn hint_equals() -> ColoredString {
    "=".cyan()
}

// === Success Output ===

pub fn banner_ok(phase: &str) -> String {
    format!(
        "{} {} {}",
        "===".dimmed(),
        format!("{} OK", phase).green().bold(),
        "===".dimmed()
    )
}

pub fn banner_fail(phase: &str, count: usize) -> String {
    let msg = if count == 1 {
        format!("{} FAILED: 1 error", phase)
    } else {
        format!("{} FAILED: {} errors", phase, count)
    };
    format!("{} {} {}", "===".dimmed(), msg.red().bold(), "===".dimmed())
}

// === Status Output ===

pub fn status_pass() -> ColoredString {
    "✓".green()
}

pub fn status_fail() -> ColoredString {
    "✗".red()
}

// === Help Output ===

pub fn title(name: &str) -> ColoredString {
    name.bold()
}

pub fn version(v: &str) -> ColoredString {
    v.dimmed()
}

pub fn section_header(header: &str) -> ColoredString {
    header.yellow().bold()
}

pub fn command(name: &str) -> ColoredString {
    name.green()
}

pub fn arg(name: &str) -> ColoredString {
    name.cyan()
}

// === Decorations ===

pub fn separator(width: usize) -> ColoredString {
    "─".repeat(width).dimmed()
}

pub fn file_path(path: &str) -> ColoredString {
    path.underline()
}

// === Test Summary ===

pub fn passed_count(n: usize) -> ColoredString {
    format!("{} passed", n).green()
}

pub fn failed_count(n: usize) -> ColoredString {
    if n > 0 {
        format!("{} failed", n).red()
    } else {
        format!("{} failed", n).normal()
    }
}

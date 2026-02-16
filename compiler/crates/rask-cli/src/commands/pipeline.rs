// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package detection and shared pipeline helpers.
//!
//! Detects whether a .rk file is part of a multi-file package and sets up
//! the package registry for cross-package resolution.

use std::path::Path;
use std::process;

use rask_ast::decl::Decl;
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use rask_resolve::{PackageId, PackageRegistry};

use crate::{output, show_diagnostics, Format};

/// A discovered package context for multi-file compilation.
pub struct PackageContext {
    pub registry: PackageRegistry,
    pub root_id: PackageId,
    /// All declarations from the root package (all files combined).
    pub all_decls: Vec<Decl>,
}

/// Result of the frontend pipeline (lex → parse → desugar → resolve → typecheck → ownership).
pub struct FrontendResult {
    pub decls: Vec<Decl>,
    pub typed: rask_types::TypedProgram,
    /// Source text for diagnostics (available in single-file mode).
    pub source: Option<String>,
    /// External package names (for interpreter package namespace registration).
    pub package_names: Vec<String>,
}

/// Run the frontend pipeline on a .rk file.
/// Automatically detects whether the file is part of a multi-file package.
pub fn run_frontend(path: &str, format: Format) -> FrontendResult {
    if let Some(mut pkg_ctx) = detect_package(path) {
        return run_frontend_package(&mut pkg_ctx, path, format);
    }
    run_frontend_single(path, format)
}

/// Frontend pipeline for a single .rk file (existing behavior).
fn run_frontend_single(path: &str, format: Format) -> FrontendResult {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), output::file_path(path), e);
            process::exit(1);
        }
    };

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();
    if !lex_result.is_ok() {
        let diags: Vec<Diagnostic> = lex_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "lex", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Lex", lex_result.errors.len()));
        }
        process::exit(1);
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();
    if !parse_result.is_ok() {
        let diags: Vec<Diagnostic> = parse_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "parse", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Parse", parse_result.errors.len()));
        }
        process::exit(1);
    }

    rask_desugar::desugar(&mut parse_result.decls);

    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "resolve", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            }
            process::exit(1);
        }
    };

    let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(t) => t,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_diagnostics(&diags, &source, path, "typecheck", format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            }
            process::exit(1);
        }
    };

    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_diagnostics(&diags, &source, path, "ownership", format);
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        }
        process::exit(1);
    }

    FrontendResult {
        decls: parse_result.decls,
        typed,
        source: Some(source),
        package_names: vec![],
    }
}

/// Frontend pipeline for a multi-file package.
fn run_frontend_package(pkg_ctx: &mut PackageContext, path: &str, format: Format) -> FrontendResult {
    rask_desugar::desugar(&mut pkg_ctx.all_decls);

    // Resolver handles external packages via collect_package_exports —
    // only pass root package decls here to avoid double registration.
    let resolved = match rask_resolve::resolve_package(
        &pkg_ctx.all_decls,
        &pkg_ctx.registry,
        pkg_ctx.root_id,
    ) {
        Ok(r) => r,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            for d in &diags {
                eprintln!("{}: {}", output::error_label(), d.message);
                if let Some(fix) = &d.fix {
                    eprintln!("  fix: {}", fix);
                }
            }
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            }
            process::exit(1);
        }
    };

    // Merge external package declarations so the typechecker and ownership
    // checker can see struct/enum/fn definitions from dependencies.
    let mut package_names = Vec::new();
    for pkg in pkg_ctx.registry.packages() {
        if pkg.id != pkg_ctx.root_id {
            package_names.push(pkg.name.clone());
            for decl in pkg.all_decls() {
                pkg_ctx.all_decls.push(decl.clone());
            }
        }
    }

    let typed = match rask_types::typecheck(resolved, &pkg_ctx.all_decls) {
        Ok(t) => t,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            for d in &diags {
                eprintln!("{}: {}", output::error_label(), d.message);
            }
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            }
            process::exit(1);
        }
    };

    let ownership_result = rask_ownership::check_ownership(&typed, &pkg_ctx.all_decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        for d in &diags {
            eprintln!("{}: {}", output::error_label(), d.message);
        }
        if format == Format::Human {
            eprintln!("\n{}", output::banner_fail("Ownership", ownership_result.errors.len()));
        }
        process::exit(1);
    }

    let _ = path; // used for context only, not for reading in package mode

    FrontendResult {
        decls: std::mem::take(&mut pkg_ctx.all_decls),
        typed,
        source: None,
        package_names,
    }
}

/// Detect whether a .rk file belongs to a multi-file package.
///
/// Walks up from the file's directory looking for `build.rk`, stopping at
/// `.git` or filesystem root. If found, that directory is the project root
/// with full package context (deps, manifest, build scripts).
/// No `build.rk` means single-file mode.
pub fn detect_package(file_path: &str) -> Option<PackageContext> {
    let path = Path::new(file_path);
    let file_dir = path.parent()?;

    // Walk up looking for build.rk, stop at .git or root
    if let Some(project_root) = find_project_root(file_dir) {
        return discover_package(&project_root);
    }

    // No build.rk found — single-file mode
    None
}

/// Walk up from `start_dir` looking for the closest `build.rk`.
/// Stops at `.git` boundary or filesystem root.
fn find_project_root(start_dir: &Path) -> Option<std::path::PathBuf> {
    let mut dir = start_dir.canonicalize().unwrap_or_else(|_| start_dir.to_path_buf());

    loop {
        if dir.join("build.rk").is_file() {
            return Some(dir);
        }

        // Stop at repo boundary
        if dir.join(".git").exists() {
            return None;
        }

        // Move to parent
        match dir.parent() {
            Some(parent) if parent != dir => {
                dir = parent.to_path_buf();
            }
            _ => return None, // Reached filesystem root
        }
    }
}

/// Public helper for finding the project root from a file path.
/// Used by cmd_compile for output directory logic.
pub fn find_project_root_from(file_path: &str) -> Option<std::path::PathBuf> {
    let path = Path::new(file_path);
    let file_dir = path.parent()?;
    find_project_root(file_dir)
}

/// Run PackageRegistry::discover on a directory and build a PackageContext.
fn discover_package(root: &Path) -> Option<PackageContext> {
    let mut registry = PackageRegistry::new();
    let root_id = registry.discover(root).ok()?;

    let all_decls: Vec<Decl> = registry.get(root_id)?
        .all_decls()
        .cloned()
        .collect();

    // Skip if the package has no declarations
    if all_decls.is_empty() {
        return None;
    }

    Some(PackageContext {
        registry,
        root_id,
        all_decls,
    })
}

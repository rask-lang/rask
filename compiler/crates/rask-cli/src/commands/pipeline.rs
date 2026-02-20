// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package detection and shared pipeline helpers.
//!
//! Detects whether a .rk file is part of a multi-file package and sets up
//! the package registry for cross-package resolution.

use std::path::Path;
use std::process;

use rask_ast::decl::{Decl, DeclKind};
use rask_diagnostics::{Diagnostic, ToDiagnostic};
use rask_resolve::{PackageId, PackageRegistry};

use rask_diagnostics::formatter::DiagnosticFormatter;

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
    /// Per-file source text for multi-file diagnostics (path, source).
    pub source_files: Vec<(std::path::PathBuf, String)>,
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

    // Inject compilable stdlib functions (those with non-empty bodies)
    let stdlib_decls = rask_stdlib::StubRegistry::compilable_decls();
    if !stdlib_decls.is_empty() {
        parse_result.decls.extend(stdlib_decls);
    }

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
        source_files: vec![],
    }
}

/// Frontend pipeline for a multi-file package.
fn run_frontend_package(pkg_ctx: &mut PackageContext, path: &str, format: Format) -> FrontendResult {
    // Collect per-file source text early so error paths can use them.
    let source_files: Vec<(std::path::PathBuf, String)> = pkg_ctx.registry.packages()
        .iter()
        .flat_map(|pkg| pkg.files.iter())
        .map(|f| (f.path.clone(), f.source.clone()))
        .collect();

    rask_desugar::desugar(&mut pkg_ctx.all_decls);

    // Inject compilable stdlib functions (those with non-empty bodies)
    let stdlib_decls = rask_stdlib::StubRegistry::compilable_decls();
    if !stdlib_decls.is_empty() {
        pkg_ctx.all_decls.extend(stdlib_decls);
    }

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
            show_multifile_diagnostics(&diags, &source_files, format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Resolve", errors.len()));
            }
            process::exit(1);
        }
    };

    // Merge public external package declarations with `{pkg}$` name prefix
    // so the typechecker/ownership checker can see definitions from deps
    // without colliding with root package names.
    let mut package_names = Vec::new();

    // Collect unqualified imports from the root package before we start
    // mutating all_decls. Maps (package_name, symbol_name) pairs.
    let unqualified_imports: Vec<(String, String)> = pkg_ctx.all_decls.iter()
        .filter_map(|d| {
            if let DeclKind::Import(imp) = &d.kind {
                // `import pkg.Symbol` => path = ["pkg", "Symbol"]
                if imp.path.len() == 2 {
                    return Some((imp.path[0].clone(), imp.path[1].clone()));
                }
                // `import pkg.*` => glob import
                if imp.is_glob && imp.path.len() == 1 {
                    return Some((imp.path[0].clone(), "*".to_string()));
                }
            }
            None
        })
        .collect();

    for pkg in pkg_ctx.registry.packages() {
        if pkg.id != pkg_ctx.root_id {
            package_names.push(pkg.name.clone());
            for decl in pkg.all_decls() {
                let is_pub = match &decl.kind {
                    DeclKind::Fn(f) => f.is_pub,
                    DeclKind::Struct(s) => s.is_pub,
                    DeclKind::Enum(e) => e.is_pub,
                    DeclKind::Trait(t) => t.is_pub,
                    DeclKind::Const(c) => c.is_pub,
                    DeclKind::Impl(_) => true,
                    _ => false,
                };
                if !is_pub {
                    continue;
                }

                // Add prefixed version (used by qualified access: lib.greet)
                let prefixed = prefix_decl(&decl, &pkg.name);
                pkg_ctx.all_decls.push(prefixed);

                // For unqualified imports (`import lib.Point`), also add
                // an unprefixed copy so the typechecker sees the bare name.
                let decl_name = match &decl.kind {
                    DeclKind::Fn(f) => Some(f.name.as_str()),
                    DeclKind::Struct(s) => Some(s.name.as_str()),
                    DeclKind::Enum(e) => Some(e.name.as_str()),
                    DeclKind::Trait(t) => Some(t.name.as_str()),
                    DeclKind::Const(c) => Some(c.name.as_str()),
                    _ => None,
                };
                if let Some(name) = decl_name {
                    let needs_unprefixed = unqualified_imports.iter().any(|(p, s)| {
                        p == &pkg.name && (s == name || s == "*")
                    });
                    if needs_unprefixed {
                        pkg_ctx.all_decls.push(decl.clone());
                    }
                }
                // Impl blocks always need an unprefixed copy too — methods
                // register under the type name, and the type's unprefixed
                // copy needs its methods.
                if matches!(&decl.kind, DeclKind::Impl(_)) {
                    pkg_ctx.all_decls.push(decl.clone());
                }
            }
        }
    }

    let typed = match rask_types::typecheck(resolved, &pkg_ctx.all_decls) {
        Ok(t) => t,
        Err(errors) => {
            let diags: Vec<Diagnostic> = errors.iter().map(|e| e.to_diagnostic()).collect();
            show_multifile_diagnostics(&diags, &source_files, format);
            if format == Format::Human {
                eprintln!("\n{}", output::banner_fail("Typecheck", errors.len()));
            }
            process::exit(1);
        }
    };

    let ownership_result = rask_ownership::check_ownership(&typed, &pkg_ctx.all_decls);
    if !ownership_result.is_ok() {
        let diags: Vec<Diagnostic> = ownership_result.errors.iter().map(|e| e.to_diagnostic()).collect();
        show_multifile_diagnostics(&diags, &source_files, format);
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
        source_files,
    }
}

/// Show diagnostics for multi-file packages.
///
/// Matches each diagnostic to a source file by checking which file's
/// source text can contain the diagnostic's primary span. Falls back
/// to message-only display when ambiguous.
fn show_multifile_diagnostics(
    diagnostics: &[Diagnostic],
    source_files: &[(std::path::PathBuf, String)],
    format: Format,
) {
    for d in diagnostics {
        // Find the primary label span to match against files.
        let primary_span = d.labels.iter()
            .find(|l| l.style == rask_diagnostics::LabelStyle::Primary)
            .map(|l| l.span.end);

        let matched = primary_span.and_then(|end| {
            // Without file IDs in spans, match by checking which file
            // can contain this byte offset. Only use the match when
            // exactly one file qualifies to avoid showing the wrong file.
            let candidates: Vec<_> = source_files.iter()
                .filter(|(_, src)| end <= src.len() && !src.is_empty())
                .collect();
            if candidates.len() == 1 { Some(candidates[0]) } else { None }
        });

        match (format, matched) {
            (Format::Human, Some((path, source))) => {
                let file_name = path.to_string_lossy();
                let fmt = DiagnosticFormatter::new(source).with_file_name(&file_name);
                eprintln!("{}", fmt.format(d));
            }
            _ => {
                // Fallback: message-only display.
                eprintln!("{}: {}", output::error_label(), d.message);
                if let Some(fix) = &d.fix {
                    eprintln!("  fix: {}", fix);
                }
            }
        }
    }
}

/// Clone a declaration with its name prefixed by `{pkg_name}$`.
/// The `$` separator is not a valid Rask identifier character, so user code
/// cannot collide with prefixed names.
fn prefix_decl(decl: &Decl, pkg_name: &str) -> Decl {
    let mut d = decl.clone();
    match &mut d.kind {
        DeclKind::Fn(f) => f.name = format!("{}${}", pkg_name, f.name),
        DeclKind::Struct(s) => s.name = format!("{}${}", pkg_name, s.name),
        DeclKind::Enum(e) => e.name = format!("{}${}", pkg_name, e.name),
        DeclKind::Trait(t) => t.name = format!("{}${}", pkg_name, t.name),
        DeclKind::Const(c) => c.name = format!("{}${}", pkg_name, c.name),
        DeclKind::Impl(i) => i.target_ty = format!("{}${}", pkg_name, i.target_ty),
        _ => {}
    }
    d
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

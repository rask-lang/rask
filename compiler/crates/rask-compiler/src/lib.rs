// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Compiler driver — single source of truth for the compilation pipeline.
//!
//! Every CLI command, LSP analysis, and test should go through this crate
//! instead of calling rask-lexer/parser/resolve/types/ownership directly.
//! This eliminates pipeline duplication and the divergence bugs it causes.
//!
//! # Error accumulation
//!
//! The pipeline accumulates errors across stages rather than bailing at the
//! first failure:
//!
//! - **Lex errors** don't stop parsing (parser handles partial tokens).
//! - **Desugar errors** don't stop resolution.
//! - **Type errors** are collected via `typecheck_with_stdlib_lenient`, which
//!   returns a partial TypedProgram. Ownership + effect stages still run on
//!   that partial program so users see type errors, ownership errors, and
//!   effect warnings in a single pipeline pass.
//! - **Resolve errors** are currently blocking (no partial ResolvedProgram).
//!   Lenient resolve is future work.
//!
//! # Known divergence
//!
//! `rask build` (in rask-cli's `build.rs`) does NOT yet use this driver.
//! Converting it exposed a pre-existing stdlib dispatch issue (Option/Result
//! being registered both as resolver builtins and as stdlib enum decls)
//! that requires separate work in rask-resolve or rask-stdlib. Until then,
//! `build.rs` keeps its own inline pipeline with filtered stdlib decls.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rask_ast::decl::{Decl, DeclKind};
use rask_diagnostics::{Diagnostic, Severity, ToDiagnostic};

// Public because `rask test` and `rask bench` assemble the back half of the
// pipeline themselves rather than going through `finalize_compile`, and they
// have to run this pass too — a derived `compare` that only `rask run`
// generates is a method that exists or doesn't depending on the subcommand.
pub mod derive;
mod comptime_eval;

// Re-export key types so callers don't need direct deps on pipeline crates.
pub use rask_comptime::CfgConfig;
pub use rask_effects::{EffectMap, EffectWarning};
pub use rask_effects::frozen::FrozenDiagnostic;
pub use rask_mir::ComptimeGlobalMeta;
pub use rask_mono::MonoProgram;
pub use rask_resolve::{PackageId, PackageRegistry};
pub use rask_types::TypedProgram;

// ============================================================================
// Core types
// ============================================================================

/// Compiler configuration. Callers build this; the driver uses it.
pub struct CompilerConfig {
    pub cfg: CfgConfig,
}

/// A discovered package context for multi-file compilation.
pub struct PackageContext {
    pub registry: PackageRegistry,
    pub root_id: PackageId,
    /// All declarations from the root package (all files combined).
    pub all_decls: Vec<Decl>,
}

impl PackageContext {
    /// See [`dependency_annotations`].
    pub fn dependency_annotations(&self) -> Vec<(String, Decl)> {
        dependency_annotations(&self.registry, self.root_id)
    }
}

/// Public annotation declarations from every package other than `root_id`,
/// each paired with the name of the package that declares it.
///
/// Desugar fills an annotation's declared defaults into the attachment text, and
/// it runs before name resolution — so a dependency's declarations can't be
/// looked up later and have to be handed in. Without them an attachment of an
/// imported annotation lost every defaulted field (type.annotations/AN3).
///
/// The package name travels with the declaration because the name alone isn't
/// enough: two dependencies may both declare `validate`, and filling from
/// whichever came last is silently the wrong value. Desugar matches these
/// against the file's own imports.
pub fn dependency_annotations(
    registry: &PackageRegistry,
    root_id: PackageId,
) -> Vec<(String, Decl)> {
    registry
        .packages()
        .iter()
        .filter(|p| p.id != root_id)
        .flat_map(|p| {
            p.all_decls()
                .filter(|d| matches!(&d.kind, DeclKind::Annotation(a) if a.is_pub))
                .map(move |d| (p.name.clone(), d.clone()))
        })
        .collect()
}

/// Result of the frontend pipeline (through ownership + effects).
pub struct CheckResult {
    pub typed: TypedProgram,
    pub decls: Vec<Decl>,
    pub package_names: Vec<String>,
    pub source_files: Vec<(PathBuf, String)>,
    pub effects: EffectMap,
    pub effect_warnings: Vec<EffectWarning>,
    pub frozen_diagnostics: Vec<FrozenDiagnostic>,
}

/// Result of the full compilation pipeline (through monomorphization).
pub struct CompileResult {
    pub typed: TypedProgram,
    pub mono: MonoProgram,
    pub decls: Vec<Decl>,
    pub comptime_globals: HashMap<String, ComptimeGlobalMeta>,
    pub package_modules: HashSet<String>,
}

/// Output of any pipeline operation.
///
/// Always contains ALL diagnostics from every stage that ran, regardless
/// of whether the pipeline succeeded. This means callers see resolve errors,
/// type errors, and ownership errors in one shot — not one category at a time.
pub struct PipelineOutput<T> {
    /// The result, if the pipeline completed without blocking errors.
    pub result: Option<T>,
    /// All diagnostics (errors + warnings) from every stage that ran.
    pub diagnostics: Vec<Diagnostic>,
    /// Source files for diagnostic display. Available even when the
    /// pipeline fails — needed to map errors to the correct file.
    pub source_files: Vec<(PathBuf, String)>,
}

impl<T> PipelineOutput<T> {
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| matches!(d.severity, Severity::Error))
    }

    pub fn succeeded(&self) -> bool {
        self.result.is_some()
    }

    fn fail(diagnostics: Vec<Diagnostic>) -> Self {
        Self { result: None, diagnostics, source_files: Vec::new() }
    }

    fn fail_with_sources(diagnostics: Vec<Diagnostic>, source_files: Vec<(PathBuf, String)>) -> Self {
        Self { result: None, diagnostics, source_files }
    }

    fn ok(value: T, diagnostics: Vec<Diagnostic>) -> Self {
        Self { result: Some(value), diagnostics, source_files: Vec::new() }
    }

    fn ok_with_sources(value: T, diagnostics: Vec<Diagnostic>, source_files: Vec<(PathBuf, String)>) -> Self {
        Self { result: Some(value), diagnostics, source_files }
    }
}

// ============================================================================
// Package detection (moved from pipeline.rs — single implementation)
// ============================================================================

/// Detect whether a .rk file belongs to a multi-file package.
///
/// Walks up from the file's directory looking for `build.rk`, stopping at
/// `.git` or filesystem root. Returns a `PackageContext` with all parsed
/// declarations if found.
pub fn detect_package(file_path: &str) -> Option<PackageContext> {
    let path = Path::new(file_path);
    let file_dir = path.parent()?;
    let file_dir = if file_dir.as_os_str().is_empty() {
        std::env::current_dir().ok()?
    } else {
        file_dir.to_path_buf()
    };

    let project_root = find_project_root(&file_dir)?;
    discover_package(&project_root)
}

/// Find the project root from a file path (public for output directory logic).
pub fn find_project_root_from(file_path: &str) -> Option<PathBuf> {
    let path = Path::new(file_path);
    let file_dir = path.parent()?;
    let file_dir = if file_dir.as_os_str().is_empty() {
        std::env::current_dir().ok()?
    } else {
        file_dir.to_path_buf()
    };
    find_project_root(&file_dir)
}

fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.canonicalize().unwrap_or_else(|_| start_dir.to_path_buf());
    loop {
        if dir.join("build.rk").is_file() {
            return Some(dir);
        }
        if dir.join(".git").exists() {
            return None;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return None,
        }
    }
}

fn discover_package(root: &Path) -> Option<PackageContext> {
    let mut registry = PackageRegistry::new();
    let root_id = registry.discover(root).ok()?;
    let all_decls: Vec<Decl> = registry.get(root_id)?.all_decls().cloned().collect();
    if all_decls.is_empty() {
        return None;
    }
    Some(PackageContext { registry, root_id, all_decls })
}

// ============================================================================
// check — frontend pipeline with error accumulation
// ============================================================================

/// Check a .rk file: lex → parse → desugar → resolve → typecheck → ownership → effects.
///
/// Auto-detects package context. Accumulates errors from all stages that run,
/// so callers see everything at once instead of one error category at a time.
pub fn check_file(path: &str, config: &CompilerConfig) -> PipelineOutput<CheckResult> {
    if let Some(mut pkg_ctx) = detect_package(path) {
        return check_package(&mut pkg_ctx, config);
    }
    check_single(path, config)
}

/// The files a single-file command compiles as one unit.
///
/// `foo_test.rk` beside `foo.rk` is a companion test file (std.testing/T3): the
/// two are the same module, so the tests see its private members (T4). Compiled
/// alone the companion sees nothing at all — every name in it is `E0200
/// undefined symbol`, which is what the convention promised not to happen.
///
/// Only for loose files. Inside a package the whole package already compiles
/// together, and `foo.rk` on its own is one file as it always was.
fn companion_group(path: &Path) -> Vec<PathBuf> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return vec![path.to_path_buf()];
    };
    let Some(base) = stem.strip_suffix("_test") else {
        return vec![path.to_path_buf()];
    };
    let module = path.with_file_name(format!("{base}.rk"));
    if module.is_file() {
        // The module first, so its diagnostics come before the test file's.
        vec![module, path.to_path_buf()]
    } else {
        vec![path.to_path_buf()]
    }
}

/// Check one .rk file, plus its `_test.rk` companion if it has one.
fn check_single(path: &str, config: &CompilerConfig) -> PipelineOutput<CheckResult> {
    check_sources(&companion_group(Path::new(path)), config)
}

/// Check a set of .rk files as one compilation unit (no package context).
///
/// Each file gets its own `file_id` so diagnostics render against the right
/// source, and node ids chain across them so combining the declarations can't
/// produce two nodes with the same id — the same rules `rask-resolve`'s package
/// loader follows, for the same reasons.
fn check_sources(paths: &[PathBuf], config: &CompilerConfig) -> PipelineOutput<CheckResult> {
    let mut diags = Vec::new();
    let mut source_files: Vec<(PathBuf, String)> = Vec::new();
    let mut decls: Vec<Decl> = Vec::new();
    let mut next_id: u32 = 0;

    for (idx, path) in paths.iter().enumerate() {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                let d = Diagnostic::error(format!("reading {}: {}", path.display(), e));
                return PipelineOutput::fail(vec![d]);
            }
        };
        let file_id = idx as u16;

        // --- Lex ---
        let mut lexer = rask_lexer::Lexer::new_with_file_id(&source, file_id);
        let lex_result = lexer.tokenize();
        for e in &lex_result.errors {
            diags.push(e.to_diagnostic());
        }

        // --- Parse (continue even with lex errors — parser handles partial tokens) ---
        let mut parser =
            rask_parser::Parser::new_with_file_id(lex_result.tokens, next_id, file_id);
        let parse_result = parser.parse();
        next_id = parser.next_node_id();
        for e in &parse_result.errors {
            diags.push(e.to_diagnostic());
        }
        source_files.push((path.clone(), source));
        if !parse_result.is_ok() {
            return PipelineOutput::fail_with_sources(diags, source_files);
        }
        decls.extend(parse_result.decls);
    }

    let mut parse_result = rask_parser::ParseResult { decls, errors: Vec::new() };

    // --- Comptime cfg elimination (CC1) ---
    rask_comptime::eliminate_comptime_if(&mut parse_result.decls, &config.cfg);

    // --- Desugar (accumulate errors, continue) ---
    let desugar_errors = rask_desugar::desugar_with_diagnostics(&mut parse_result.decls);
    for e in &desugar_errors {
        diags.push(
            Diagnostic::error(e.message.clone())
                .with_code("E0338")
                .with_primary(e.span, "variant needs @message(\"...\") annotation"),
        );
    }

    // --- Resolve (blocking — need ResolvedProgram) ---
    // Resolved alongside the program: the stdlib's own bodies are compiled
    // into every program, so their names have to bind for anything downstream
    // to know what a call inside them refers to (#425).
    let stdlib_bodies = rask_stdlib::StubRegistry::compilable_decls();
    let resolved = match rask_resolve::resolve_with_stdlib_and_cfg(
        &parse_result.decls,
        &stdlib_bodies,
        config.cfg.to_cfg_values(),
    ) {
        Ok(r) => r,
        Err(errors) => {
            for e in &errors {
                diags.push(e.to_diagnostic());
            }
            return PipelineOutput::fail_with_sources(diags, source_files);
        }
    };

    // --- Typecheck (lenient — always returns TypedProgram + errors, so
    //     ownership/effects can still run and show accumulated diagnostics) ---
    let stdlib_decls = rask_stdlib::StubRegistry::typecheck_decls();
    let (typed, type_errors) =
        rask_types::typecheck_with_stdlib_lenient(resolved, &parse_result.decls, &stdlib_decls);
    for e in &type_errors {
        diags.push(e.to_diagnostic());
    }

    // --- Ownership (non-blocking — accumulate and continue) ---
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    for e in &ownership_result.errors {
        diags.push(e.to_diagnostic());
    }

    // --- Effects (non-blocking metadata) ---
    let (effects, effect_warnings) = rask_effects::infer_effects(&parse_result.decls);
    for w in &effect_warnings {
        diags.push(effect_warning_to_diagnostic(w));
    }

    // --- Frozen context enforcement ---
    let frozen_diagnostics = rask_effects::frozen::check(&parse_result.decls, &effects);
    for d in &frozen_diagnostics {
        diags.push(frozen_to_diagnostic(d));
    }

    // --- Cleanup order (mem.resource-types/EO1) ---
    for w in rask_effects::ensure_order::check(&parse_result.decls) {
        diags.push(ensure_order_to_diagnostic(&w));
    }

    let package_names = collect_builtin_imports(&parse_result.decls);

    // --- Comptime folds (CT1) and comptime tests (T11) ---
    if !diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        diags.extend(comptime_diagnostics_for(&parse_result.decls, &typed, &config.cfg));
    }

    if diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        return PipelineOutput::fail_with_sources(diags, source_files);
    }

    PipelineOutput::ok_with_sources(
        CheckResult {
            typed,
            decls: parse_result.decls,
            package_names,
            source_files: source_files.clone(),
            effects,
            effect_warnings,
            frozen_diagnostics,
        },
        diags,
        source_files,
    )
}


/// CT1: a comptime initializer that overflows or divides by zero is a compile
/// error, so `rask check` has to run the fold to answer "does this compile".
///
/// It didn't, and the two paths that go through it disagreed with the one that
/// doesn't: `rask check` said OK to a program `rask run` refused, and the
/// interpreter reported the same overflow at *runtime* under its own code
/// (R0017) instead of as the compile error it is (#325).
///
/// Monomorphization is what `evaluate_comptime_globals` needs and nothing else
/// here does, so it's built and thrown away. A program with no comptime const
/// pays for it and gets nothing; that's the price of check and run agreeing.
fn comptime_diagnostics_for(
    decls: &[Decl],
    typed: &rask_types::TypedProgram,
    cfg: &CfgConfig,
) -> Vec<Diagnostic> {
    // T11 tests need neither typecheck output nor monomorphization — the AST
    // interpreter runs them straight off the decls.
    let mut diags = evaluate_comptime_tests(decls, Some(cfg));

    if !decls.iter().any(|d| matches!(&d.kind, DeclKind::Const(c) if is_comptime_init(&c.init, decls))) {
        return diags;
    }
    let Ok(mono) = rask_mono::monomorphize(typed, decls) else {
        // Monomorphization has its own diagnostics on the compile path; check
        // stays quiet about them rather than reporting them twice.
        return diags;
    };
    diags.extend(evaluate_comptime_globals(decls, typed, &mono, Some(cfg)).1);
    diags
}

/// Check a multi-file package.
pub fn check_package(
    pkg_ctx: &mut PackageContext,
    config: &CompilerConfig,
) -> PipelineOutput<CheckResult> {
    let mut diags = Vec::new();

    let source_files: Vec<(PathBuf, String)> = pkg_ctx.registry
        .get(pkg_ctx.root_id)
        .map(|pkg| pkg.files.iter().map(|f| (f.path.clone(), f.source.clone())).collect())
        .unwrap_or_default();

    // --- Comptime cfg elimination (CC1) ---
    rask_comptime::eliminate_comptime_if(&mut pkg_ctx.all_decls, &config.cfg);

    // --- Desugar ---
    // A dependency's public annotations come along: defaults are filled into
    // attachment text here, before name resolution, so they can't be looked up
    // later (type.annotations/AN3).
    let dep_annotations = pkg_ctx.dependency_annotations();
    let desugar_errors =
        rask_desugar::desugar_package(&mut pkg_ctx.all_decls, &dep_annotations);
    for e in &desugar_errors {
        diags.push(
            Diagnostic::error(e.message.clone())
                .with_code("E0338")
                .with_primary(e.span, "variant needs @message(\"...\") annotation"),
        );
    }

    // --- Merge external package declarations ---
    let mut package_names = Vec::new();
    let unqualified_imports = collect_unqualified_imports(&pkg_ctx.all_decls);

    for pkg in pkg_ctx.registry.packages() {
        if pkg.id == pkg_ctx.root_id {
            continue;
        }
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

            pkg_ctx.all_decls.push(prefix_decl(&decl, &pkg.name));

            let decl_name = match &decl.kind {
                DeclKind::Fn(f) => Some(f.name.as_str()),
                DeclKind::Struct(s) => Some(s.name.as_str()),
                DeclKind::Enum(e) => Some(e.name.as_str()),
                DeclKind::Trait(t) => Some(t.name.as_str()),
                DeclKind::Const(c) => Some(c.name.as_str()),
                _ => None,
            };
            if let Some(name) = decl_name {
                let needs_unprefixed = unqualified_imports
                    .iter()
                    .any(|(p, s)| p == &pkg.name && (s == name || s == "*"));
                if needs_unprefixed {
                    pkg_ctx.all_decls.push(decl.clone());
                }
            }
            if matches!(&decl.kind, DeclKind::Impl(_)) {
                pkg_ctx.all_decls.push(decl.clone());
            }
        }
    }

    // --- Resolve ---
    // With the stdlib's own bodies, same as the single-file path. The type
    // check below always includes them, so leaving them out here meant every
    // name inside them arrived unresolved: a package — any package, even an
    // empty one — reported 161 "undefined name" errors for the stdlib's
    // internals (`fopen`, `rask_alloc`, …), pinned to spans in the user's file
    // (#203).
    let stdlib_bodies = rask_stdlib::StubRegistry::compilable_decls();
    let resolved = match rask_resolve::resolve_package_with_stdlib_and_cfg(
        &pkg_ctx.all_decls,
        &pkg_ctx.registry,
        pkg_ctx.root_id,
        &stdlib_bodies,
        config.cfg.to_cfg_values(),
    ) {
        Ok(r) => r,
        Err(errors) => {
            for e in &errors {
                diags.push(e.to_diagnostic());
            }
            return PipelineOutput::fail(diags);
        }
    };

    // --- Typecheck (lenient — always returns TypedProgram + errors) ---
    let stdlib_decls = rask_stdlib::StubRegistry::typecheck_decls();
    let (typed, type_errors) =
        rask_types::typecheck_with_stdlib_lenient(resolved, &pkg_ctx.all_decls, &stdlib_decls);
    for e in &type_errors {
        diags.push(e.to_diagnostic());
    }

    // --- Ownership (non-blocking) ---
    let ownership_result = rask_ownership::check_ownership(&typed, &pkg_ctx.all_decls);
    for e in &ownership_result.errors {
        diags.push(e.to_diagnostic());
    }

    // --- Effects ---
    let (effects, effect_warnings) = rask_effects::infer_effects(&pkg_ctx.all_decls);
    for w in &effect_warnings {
        diags.push(effect_warning_to_diagnostic(w));
    }

    let frozen_diagnostics = rask_effects::frozen::check(&pkg_ctx.all_decls, &effects);
    for d in &frozen_diagnostics {
        diags.push(frozen_to_diagnostic(d));
    }

    // --- Cleanup order (mem.resource-types/EO1) ---
    for w in rask_effects::ensure_order::check(&pkg_ctx.all_decls) {
        diags.push(ensure_order_to_diagnostic(&w));
    }

    // --- Comptime folds (CT1) and comptime tests (T11) ---
    if !diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        diags.extend(comptime_diagnostics_for(&pkg_ctx.all_decls, &typed, &config.cfg));
    }

    if diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        return PipelineOutput::fail_with_sources(diags, source_files);
    }

    PipelineOutput::ok_with_sources(
        CheckResult {
            typed,
            decls: std::mem::take(&mut pkg_ctx.all_decls),
            package_names,
            source_files: source_files.clone(),
            effects,
            effect_warnings,
            frozen_diagnostics,
        },
        diags,
        source_files,
    )
}

// ============================================================================
// compile — full pipeline through monomorphization
// ============================================================================

/// Compile a .rk file: check + hidden_params + derive + stdlib + monomorphize.
///
/// Returns everything codegen needs. Does NOT emit object files.
pub fn compile_file(
    path: &str,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
) -> PipelineOutput<CompileResult> {
    compile_file_with(path, dep_decls, config, |_, _| {})
}

/// `compile_file`, with a chance to rewrite the declarations first.
///
/// `transform` runs after the frontend and the derive/stdlib/dependency merge,
/// and before monomorphization — the one point where the decl list is complete
/// and nothing has been laid out yet. That's where `rask test` swaps `main` for
/// a test runner and `rask bench` for a benchmark runner.
///
/// Those two used to open-code the whole frontend to get that one edit in, so a
/// fix to resolve, typecheck, ownership or mono had to be made twice — and one
/// of the copies used the plain typecheck, which left every stdlib body untyped
/// and only showed up as 17 failures in `rask test examples/validation` (#330,
/// #697).
pub fn compile_file_with(
    path: &str,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
) -> PipelineOutput<CompileResult> {
    if let Some(mut pkg_ctx) = detect_package(path) {
        return compile_package_with(&mut pkg_ctx, dep_decls, config, transform);
    }
    compile_single(path, dep_decls, config, transform)
}

fn compile_single(
    path: &str,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
) -> PipelineOutput<CompileResult> {
    let check_output = check_single(path, config);
    finalize_compile(check_output, dep_decls, HashSet::new(), config, transform)
}

pub fn compile_package(
    pkg_ctx: &mut PackageContext,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
) -> PipelineOutput<CompileResult> {
    compile_package_with(pkg_ctx, dep_decls, config, |_, _| {})
}

/// `compile_package`, with the same decl hook as `compile_file_with`.
pub fn compile_package_with(
    pkg_ctx: &mut PackageContext,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
) -> PipelineOutput<CompileResult> {
    // Collect package_modules from the registry before check consumes pkg_ctx.
    let mut package_modules = HashSet::new();
    for pkg in pkg_ctx.registry.packages() {
        if pkg.id != pkg_ctx.root_id {
            package_modules.insert(pkg.name.clone());
        }
    }
    // Also include builtin stdlib modules referenced by imports.
    for decl in &pkg_ctx.all_decls {
        if let DeclKind::Import(import) = &decl.kind {
            if let Some(first) = import.path.first() {
                if rask_resolve::is_builtin_module(first) {
                    package_modules.insert(first.clone());
                }
            }
        }
    }

    let check_output = check_package(pkg_ctx, config);
    finalize_compile(check_output, dep_decls, package_modules, config, transform)
}

/// Fill in the parameter types `type.gradual` let the author leave out.
///
/// `func greet(name) { … }` parses with an empty type string, and every pass
/// after the checker reads that string. Empty means `void` to all of them, so
/// the body's `"Hi, {name}"` interpolated an address instead of the string
/// (#905). The checker already solved it; this copies the answer in, so the
/// declaration says what the function actually takes.
fn write_back_inferred_params(decls: &mut [Decl], typed: &TypedProgram) {
    fn fill(f: &mut rask_ast::decl::FnDecl, typed: &TypedProgram) {
        let Some(solved) = typed.inferred_fn_params.get(&f.name) else {
            return;
        };
        for p in f.params.iter_mut().filter(|p| p.ty.is_empty()) {
            if let Some((_, ty)) = solved.iter().find(|(n, _)| *n == p.name) {
                p.ty = format!("{}", ty);
            }
        }
    }
    for decl in decls.iter_mut() {
        match &mut decl.kind {
            DeclKind::Fn(f) => fill(f, typed),
            DeclKind::Struct(s) => s.methods.iter_mut().for_each(|m| fill(m, typed)),
            DeclKind::Enum(e) => e.methods.iter_mut().for_each(|m| fill(m, typed)),
            _ => {}
        }
    }
}

/// Shared post-check compilation: hidden params, derive, stdlib, mono, comptime.
fn finalize_compile(
    check_output: PipelineOutput<CheckResult>,
    dep_decls: Vec<Decl>,
    package_modules: HashSet<String>,
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
) -> PipelineOutput<CompileResult> {
    let mut diags = check_output.diagnostics;
    let pkg_source_files = check_output.source_files;
    let mut check = match check_output.result {
        Some(c) => c,
        None => return PipelineOutput::fail_with_sources(diags, pkg_source_files),
    };

    // --- Write inferred parameter types back into the declarations ---
    // Everything after this point reads a parameter's type off its declaration
    // string, and an omitted one is empty, which reads as `void`. The checker
    // solved it; put the answer where the rest of the pipeline looks (#905).
    write_back_inferred_params(&mut check.decls, &check.typed);

    // --- Hidden parameter desugaring ---
    // CC8 ambiguity surfaces here as a pipeline diagnostic; a hard error stops
    // the build before monomorphization, like any other pass.
    let hp_diags = rask_mir::hidden_params::desugar_hidden_params_with_types(
        &mut check.decls,
        Some(&check.typed),
    );
    if !hp_diags.is_empty() {
        diags.extend(hp_diags);
        return PipelineOutput::fail_with_sources(diags, pkg_source_files);
    }

    // --- Derive synthetic method bodies (compare, etc.) ---
    derive::generate_derived_methods(&mut check.decls, &check.typed);

    // --- Inject compiled stdlib functions + struct defs ---
    let stdlib_fn_decls = rask_stdlib::StubRegistry::compilable_decls();
    let stdlib_struct_defs = rask_stdlib::StubRegistry::compilable_struct_defs();
    check.decls.extend(stdlib_fn_decls);
    check.decls.extend(stdlib_struct_defs);

    // --- Merge dependency declarations ---
    if !dep_decls.is_empty() {
        let mut dep_decls_desugared = dep_decls;
        // A dependency's own attachments are filled from its own declarations —
        // that's the same compilation unit, so nothing extra is needed here.
        rask_desugar::desugar(&mut dep_decls_desugared);
        check.decls.extend(dep_decls_desugared);
    }

    // --- Caller's decl rewrite (test/bench runners) ---
    transform(&mut check.decls, &check.typed);

    // --- Monomorphize ---
    let mono = if package_modules.is_empty() {
        rask_mono::monomorphize(&check.typed, &check.decls)
    } else {
        rask_mono::monomorphize_with_packages(&check.typed, &check.decls, package_modules.clone())
    };
    let mono = match mono {
        Ok(m) => m,
        Err(e) => {
            diags.push(mono_diagnostic(e));
            return PipelineOutput::fail_with_sources(diags, pkg_source_files);
        }
    };

    // --- Evaluate comptime globals (single source of truth) ---
    // Hard errors (overflow, divide-by-zero) become pipeline diagnostics and
    // fail the build like any other pass — no separate handling downstream.
    let (comptime_globals, mut ct_diags) =
        evaluate_comptime_globals(&check.decls, &check.typed, &mono, Some(&config.cfg));
    ct_diags.extend(evaluate_comptime_tests(&check.decls, Some(&config.cfg)));
    if !ct_diags.is_empty() {
        diags.extend(ct_diags);
        return PipelineOutput::fail_with_sources(diags, pkg_source_files);
    }

    PipelineOutput::ok_with_sources(
        CompileResult {
            typed: check.typed,
            mono,
            decls: check.decls,
            comptime_globals,
            package_modules,
        },
        diags,
        pkg_source_files,
    )
}

// ============================================================================
// Comptime global evaluation
// ============================================================================

// The comptime-global evaluator lives in `comptime_eval` — the single source
// of truth used by both the pipeline (below) and the CLI's test/bench paths.
pub use crate::comptime_eval::{evaluate_comptime_globals, evaluate_comptime_tests};

pub(crate) fn is_comptime_init(init: &rask_ast::expr::Expr, decls: &[Decl]) -> bool {
    use rask_ast::expr::ExprKind;

    matches!(&init.kind, ExprKind::Comptime { .. })
        || matches!(&init.kind, ExprKind::Call { func, .. }
            if matches!(&func.kind, ExprKind::Ident(name)
                if decls.iter().any(|d| matches!(&d.kind,
                    DeclKind::Fn(f) if f.name == *name && f.is_comptime))))
}

// ============================================================================
// Helpers
// ============================================================================

fn collect_builtin_imports(decls: &[Decl]) -> Vec<String> {
    let mut names = Vec::new();
    for decl in decls {
        if let DeclKind::Import(import) = &decl.kind {
            if let Some(first) = import.path.first() {
                if rask_resolve::is_builtin_module(first)
                    && !names.contains(first)
                {
                    names.push(first.clone());
                }
            }
        }
    }
    names
}

fn collect_unqualified_imports(decls: &[Decl]) -> Vec<(String, String)> {
    decls.iter()
        .filter_map(|d| {
            if let DeclKind::Import(imp) = &d.kind {
                if imp.path.len() == 2 {
                    return Some((imp.path[0].clone(), imp.path[1].clone()));
                }
                if imp.is_glob && imp.path.len() == 1 {
                    return Some((imp.path[0].clone(), "*".to_string()));
                }
            }
            None
        })
        .collect()
}

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

fn mono_diagnostic(e: rask_mono::MonomorphizeError) -> Diagnostic {
    use rask_mono::MonomorphizeError as ME;
    match &e {
        ME::AmbiguousMethod { type_name, method, span, .. } => Diagnostic::error(e.to_string())
            .with_code("E0823")
            .with_primary(*span, format!("no `{}.{}` to call here", type_name, method))
            .with_help(format!("rename one of the two `{}` types", type_name)),
        _ => Diagnostic::error(e.to_string()),
    }
}

fn effect_warning_to_diagnostic(w: &EffectWarning) -> Diagnostic {
    let mut diag = if w.is_error {
        Diagnostic::error(&w.message)
    } else {
        Diagnostic::warning(&w.message)
    };
    diag = diag.with_code(w.code).with_primary(w.span, &w.label);
    if let Some(fix) = &w.fix {
        diag = diag.with_fix(fix);
    }
    if let Some(why) = &w.why {
        diag = diag.with_why(why);
    }
    diag
}

/// EO1: `ensure` runs LIFO, so a dependency registered *after* its dependent is
/// torn down first — the dependent's cleanup then calls into something that's
/// already gone. The FIX shows the two lines reordered rather than describing
/// the rule, because "swap these" is the whole of it.
fn ensure_order_to_diagnostic(w: &rask_effects::ensure_order::EnsureOrderWarning) -> Diagnostic {
    Diagnostic::warning(format!(
        "`{}` is cleaned up before `{}`, which needs it",
        w.dependency, w.dependent
    ))
    .with_code("W0908")
    .with_primary(
        w.span,
        format!("registered last, so this runs first and `{}` is gone", w.dependency),
    )
    .with_secondary(
        w.dependent_span,
        format!("`{}` still needs `{}` when this runs", w.dependent, w.dependency),
    )
    .with_fix(w.fixed_order.clone())
    .with_why("`ensure` bodies run LIFO — the last one registered runs first. A resource derived from another has to be cleaned up first, which means its `ensure` comes second. Registered the other way round, the cleanup calls into a dependency that's already torn down; across an FFI boundary that's undefined behaviour the language otherwise makes impossible [mem.resource-types/EO1]")
}

fn frozen_to_diagnostic(d: &FrozenDiagnostic) -> Diagnostic {
    let diag = if d.is_error {
        Diagnostic::error(&d.message)
    } else {
        Diagnostic::warning(&d.message)
    };
    diag.with_code(d.code).with_primary(d.span, "")
}

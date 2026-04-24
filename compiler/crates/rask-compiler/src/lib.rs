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

mod derive;

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

/// Check a single .rk file (no package context).
fn check_single(path: &str, config: &CompilerConfig) -> PipelineOutput<CheckResult> {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            let d = Diagnostic::error(format!("reading {}: {}", path, e));
            return PipelineOutput::fail(vec![d]);
        }
    };

    let mut diags = Vec::new();

    // --- Lex ---
    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();
    for e in &lex_result.errors {
        diags.push(e.to_diagnostic());
    }

    // --- Parse (continue even with lex errors — parser handles partial tokens) ---
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let mut parse_result = parser.parse();
    for e in &parse_result.errors {
        diags.push(e.to_diagnostic());
    }
    if !parse_result.is_ok() {
        return PipelineOutput::fail(diags);
    }

    // --- Comptime cfg elimination (CC1) ---
    rask_comptime::eliminate_comptime_if(&mut parse_result.decls, &config.cfg);

    // --- Desugar (accumulate errors, continue) ---
    let desugar_errors = rask_desugar::desugar_with_diagnostics(&mut parse_result.decls);
    rask_desugar::desugar_default_args(&mut parse_result.decls);
    for e in &desugar_errors {
        diags.push(
            Diagnostic::error(e.message.clone())
                .with_code("E0338")
                .with_primary(e.span, "variant needs @message(\"...\") annotation"),
        );
    }

    // --- Resolve (blocking — need ResolvedProgram) ---
    let resolved = match rask_resolve::resolve_with_cfg(
        &parse_result.decls,
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

    let package_names = collect_builtin_imports(&parse_result.decls);

    if diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        return PipelineOutput::fail(diags);
    }

    PipelineOutput::ok(
        CheckResult {
            typed,
            decls: parse_result.decls,
            package_names,
            source_files: vec![(PathBuf::from(path), source)],
            effects,
            effect_warnings,
            frozen_diagnostics,
        },
        diags,
    )
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
    let desugar_errors = rask_desugar::desugar_with_diagnostics(&mut pkg_ctx.all_decls);
    rask_desugar::desugar_default_args(&mut pkg_ctx.all_decls);
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
    let resolved = match rask_resolve::resolve_package_with_cfg(
        &pkg_ctx.all_decls,
        &pkg_ctx.registry,
        pkg_ctx.root_id,
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
    if let Some(mut pkg_ctx) = detect_package(path) {
        return compile_package(&mut pkg_ctx, dep_decls, config);
    }
    compile_single(path, dep_decls, config)
}

fn compile_single(
    path: &str,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
) -> PipelineOutput<CompileResult> {
    let check_output = check_single(path, config);
    finalize_compile(check_output, dep_decls, HashSet::new(), config)
}

pub fn compile_package(
    pkg_ctx: &mut PackageContext,
    dep_decls: Vec<Decl>,
    config: &CompilerConfig,
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
                if rask_resolve::BUILTIN_MODULE_NAMES.contains(&first.as_str()) {
                    package_modules.insert(first.clone());
                }
            }
        }
    }

    let check_output = check_package(pkg_ctx, config);
    finalize_compile(check_output, dep_decls, package_modules, config)
}

/// Shared post-check compilation: hidden params, derive, stdlib, mono, comptime.
fn finalize_compile(
    check_output: PipelineOutput<CheckResult>,
    dep_decls: Vec<Decl>,
    package_modules: HashSet<String>,
    config: &CompilerConfig,
) -> PipelineOutput<CompileResult> {
    let mut diags = check_output.diagnostics;
    let pkg_source_files = check_output.source_files;
    let mut check = match check_output.result {
        Some(c) => c,
        None => return PipelineOutput::fail_with_sources(diags, pkg_source_files),
    };

    // --- Hidden parameter desugaring ---
    rask_mir::hidden_params::desugar_hidden_params_with_types(
        &mut check.decls,
        Some(&check.typed.node_types),
    );

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
        rask_desugar::desugar(&mut dep_decls_desugared);
        check.decls.extend(dep_decls_desugared);
    }

    // --- Monomorphize ---
    let mono = if package_modules.is_empty() {
        rask_mono::monomorphize(&check.typed, &check.decls)
    } else {
        rask_mono::monomorphize_with_packages(&check.typed, &check.decls, package_modules.clone())
    };
    let mono = match mono {
        Ok(m) => m,
        Err(e) => {
            diags.push(Diagnostic::error(format!("monomorphization failed: {:?}", e)));
            return PipelineOutput::fail_with_sources(diags, pkg_source_files);
        }
    };

    // --- Evaluate comptime globals ---
    let comptime_globals = evaluate_comptime_globals(&check.decls, Some(&config.cfg));

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

/// Evaluate comptime const declarations via the AST interpreter.
///
/// For MIR-based fast-path evaluation, callers can use `MirEvalContext`
/// in rask-cli's codegen module (which falls back to this on failure).
pub fn evaluate_comptime_globals(
    decls: &[Decl],
    cfg: Option<&CfgConfig>,
) -> HashMap<String, ComptimeGlobalMeta> {
    use rask_ast::decl::DeclKind;
    use rask_ast::stmt::StmtKind;

    let mut comptime_interp = rask_comptime::ComptimeInterpreter::new();
    if let Some(c) = cfg {
        comptime_interp.inject_cfg(c);
    }
    comptime_interp.register_functions(decls);

    let mut globals = HashMap::new();
    let mut comptime_consts: Vec<(String, &rask_ast::expr::Expr)> = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Const(c) => {
                if is_comptime_init(&c.init, decls) {
                    comptime_consts.push((c.name.clone(), &c.init));
                }
            }
            DeclKind::Fn(f) => {
                for stmt in &f.body {
                    if let StmtKind::Const { name, init, .. } = &stmt.kind {
                        if is_comptime_init(init, decls) {
                            comptime_consts.push((name.clone(), init));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for (name, init) in comptime_consts {
        comptime_interp.reset_branch_count();
        match comptime_interp.eval_expr(init) {
            Ok(val) => {
                let type_prefix = val.type_prefix().to_string();
                let elem_count = val.elem_count();
                if let Some(bytes) = val.serialize() {
                    globals.insert(name, ComptimeGlobalMeta { bytes, elem_count, type_prefix });
                }
            }
            Err(_) => {} // comptime eval failures are non-fatal
        }
    }

    globals
}

fn is_comptime_init(init: &rask_ast::expr::Expr, decls: &[Decl]) -> bool {
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
                if rask_resolve::BUILTIN_MODULE_NAMES.contains(&first.as_str())
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

fn effect_warning_to_diagnostic(w: &EffectWarning) -> Diagnostic {
    let diag = if w.is_error {
        Diagnostic::error(&w.message)
    } else {
        Diagnostic::warning(&w.message)
    };
    let label = if w.is_error {
        format!("`{}` reaches spawn here", w.callee_name)
    } else {
        format!("`{}` has IO effect", w.callee_name)
    };
    diag.with_code(w.code).with_primary(w.span, label)
}

fn frozen_to_diagnostic(d: &FrozenDiagnostic) -> Diagnostic {
    let diag = if d.is_error {
        Diagnostic::error(&d.message)
    } else {
        Diagnostic::warning(&d.message)
    };
    diag.with_code(d.code).with_primary(d.span, "")
}

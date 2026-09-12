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
use rask_ast::Span;
use rask_diagnostics::{Diagnostic, Severity, ToDiagnostic};

// Public because `rask test` and `rask bench` assemble the back half of the
// pipeline themselves rather than going through `finalize_compile`, and they
// have to run this pass too — a derived `compare` that only `rask run`
// generates is a method that exists or doesn't depending on the subcommand.
pub mod package_scope;
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

impl CompilerConfig {
    /// Tell the frontend what machine it is compiling for.
    ///
    /// `usize` is pointer-sized, and nothing that builds a `Type::usize()` has
    /// a config in hand, so the width is a process-wide fact set from here —
    /// once, at the top of every entry point, so it can never describe a
    /// different machine from the `cfg` values in the same config.
    fn declare_target(&self) {
        rask_ast::primitives::set_pointer_bits(self.cfg.pointer_bits());
    }
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
    /// Each dependency's declarations, original name against the qualified one
    /// the rest of the pipeline uses. Kept so a diagnostic raised after the
    /// check can still say `libpkg.Cat` rather than `Cat_libpkg`.
    pub qualified_names: HashMap<String, HashMap<String, String>>,
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

/// Was this span parsed out of `stdlib/*.rk` rather than the user's program?
///
/// Re-exported so callers that render diagnostics don't each need rask-mono.
pub use rask_mono::is_stdlib_span;

/// The declarations the interpreter runs: the program's, plus the stdlib modules
/// that are written in Rask.
///
/// Native compiles `stdlib/*.rk` including its bodies (`compilable_decls`); the
/// interpreter used to ignore that source entirely and run hand-written Rust
/// from `rask-interp/src/stdlib/` instead. So a module written in Rask still had
/// two implementations, one per backend, and they disagreed — `Path.parent()`
/// answered `none` natively (#688) while the interpreter got it right, and the
/// rest of the Path family segfaulted. Handing the same source to both backends
/// is what makes "written in Rask" mean one implementation.
///
/// The stdlib goes first and the program second, because registration is
/// last-writer-wins and the program has to be the last writer. A program may
/// reuse a stdlib type's name (rask#258) — `struct JsonError` over stdlib's
/// `enum JsonError` — and with the program first, the stdlib's `message` body
/// overwrote the user's and ran `match self` against a struct.
pub fn program_decls(decls: &[Decl]) -> Vec<Decl> {
    let mut all = rask_stdlib::StubRegistry::compilable_decls();
    all.extend(decls.to_vec());
    all
}

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
fn check_sources(paths: &[PathBuf], config: &CompilerConfig) -> PipelineOutput<CheckResult> {
    let mut loaded: Vec<(PathBuf, String)> = Vec::new();
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(s) => loaded.push((path.clone(), s)),
            Err(e) => {
                let d = Diagnostic::error(format!("reading {}: {}", path.display(), e));
                return PipelineOutput::fail(vec![d]);
            }
        }
    }
    check_loaded(&loaded, config)
}

/// Check source that is already in memory, as one compilation unit.
///
/// The browser playground has no filesystem, so it hands the editor's buffer
/// straight here. Everything after reading the file is the same work `rask
/// check` does — including compiling `stdlib/*.rk` alongside the program, which
/// the playground used to skip: without it `numbers.map(…).to_vec()` came back
/// as "no method `to_vec` found for type `Sequence<i32>`", because every
/// stdlib module written in Rask was simply absent (#1177).
pub fn check_source(name: &str, source: &str, config: &CompilerConfig) -> PipelineOutput<CheckResult> {
    check_loaded(&[(PathBuf::from(name), source.to_string())], config)
}

/// The compilation unit itself, once every file has been read.
///
/// Each file gets its own `file_id` so diagnostics render against the right
/// source, and node ids chain across them so combining the declarations can't
/// produce two nodes with the same id — the same rules `rask-resolve`'s package
/// loader follows, for the same reasons.
fn check_loaded(
    files: &[(PathBuf, String)],
    config: &CompilerConfig,
) -> PipelineOutput<CheckResult> {
    config.declare_target();

    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut source_files: Vec<(PathBuf, String)> = Vec::new();
    let mut decls: Vec<Decl> = Vec::new();
    let mut next_id: u32 = 0;

    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();

    for (idx, (path, source)) in files.iter().enumerate() {
        let source = source.clone();
        let path = path.clone();
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
    // `import c "x.h"` looks beside the file that imports it, so resolution has
    // to know where each file came from (#1096). `file_id` is the index above.
    let source_dirs: HashMap<u16, PathBuf> = paths
        .iter()
        .enumerate()
        .filter_map(|(idx, p)| Some((idx as u16, p.parent()?.to_path_buf())))
        .collect();
    let resolved = match rask_resolve::resolve_with_stdlib_cfg_and_dirs(
        &parse_result.decls,
        &stdlib_bodies,
        config.cfg.to_cfg_values(),
        source_dirs,
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

    // --- CT60: a `comptime func` keeps its promise where it is written ---
    for e in rask_effects::comptime_purity::check(&parse_result.decls, &effects) {
        diags.push(comptime_purity_to_diagnostic(&e));
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

    drop_stdlib_cascade(&mut diags);
    if diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        return PipelineOutput::fail_with_sources(diags, source_files);
    }

    PipelineOutput::ok_with_sources(
        CheckResult {
            typed,
            decls: parse_result.decls,
            package_names,
            qualified_names: HashMap::new(),
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
fn has_comptime_let(body: &[rask_ast::stmt::Stmt], decls: &[Decl]) -> bool {
    body.iter().any(|st| match &st.kind {
        rask_ast::stmt::StmtKind::Let { init, .. } => is_comptime_init(init, decls),
        _ => false,
    })
}

/// Identity of a diagnostic for de-duplication: its code and message. Two
/// passes reporting the same unfoldable const produce byte-identical text.
fn diag_key(d: &Diagnostic) -> (Option<String>, String) {
    (d.code.as_ref().map(|c| c.0.clone()), d.message.clone())
}

fn comptime_diagnostics_for(
    decls: &[Decl],
    typed: &rask_types::TypedProgram,
    cfg: &CfgConfig,
) -> Vec<Diagnostic> {
    // T11 tests need neither typecheck output nor monomorphization — the AST
    // interpreter runs them straight off the decls.
    let mut diags = evaluate_comptime_tests(decls, Some(cfg));

    let any_comptime_init = decls.iter().any(|d| match &d.kind {
        DeclKind::Const(c) => is_comptime_init(&c.init, decls),
        // A function-local `let x = comptime { … }` folds the same way, and
        // check has to see it too or its warning would only appear on the
        // compile path — which is one backend reporting and the other not.
        DeclKind::Fn(f) => has_comptime_let(&f.body, decls),
        DeclKind::Test(t) => has_comptime_let(&t.body, decls),
        _ => false,
    });
    // A `value.(comptime { … })` naming a field is neither a const nor a let,
    // and the block still has to finish for the program to compile (CT53). A
    // program whose only comptime code was one of these skipped the whole stage
    // and type-checked clean, then failed at the end of a build (#1090).
    let any_field_name_block = {
        let mut found = false;
        rask_ast::visit::walk_decls(decls, &mut |e| {
            if let rask_ast::expr::ExprKind::DynamicField { field_expr, .. } = &e.kind {
                found |= matches!(field_expr.kind, rask_ast::expr::ExprKind::Comptime { .. });
            }
        });
        found
    };
    if !any_comptime_init && !any_field_name_block {
        return diags;
    }
    // `monomorphize_for_analysis`, not `monomorphize`: a file of `test` blocks
    // has no `main`, and the plain one calls that a fatal error — so check said
    // nothing at all about the comptime consts in every test file we have,
    // including ones that would fail to compile.
    let Ok(mono) = rask_mono::monomorphize_for_analysis(typed, decls) else {
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
    config.declare_target();

    let mut exports: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut out = check_package_scoped(pkg_ctx, config, &mut exports);
    // Every diagnostic, including the ones raised before the check got far
    // enough to produce a result — those are the ones most likely to name a
    // dependency's type.
    package_scope::unqualify_diagnostics(&mut out.diagnostics, &exports);
    out
}

fn check_package_scoped(
    pkg_ctx: &mut PackageContext,
    config: &CompilerConfig,
    exports: &mut HashMap<String, HashMap<String, String>>,
) -> PipelineOutput<CheckResult> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    // Every package's files, placed at the slot its spans name. The list used
    // to hold only the root's, so a diagnostic about a dependency's
    // declaration was rendered against whichever of the consumer's files sat
    // in that slot — `Colour is a built-in type` pointed at a line of main.rk
    // that doesn't exist (#1126).
    let mut source_files: Vec<(PathBuf, String)> = Vec::new();
    for pkg in pkg_ctx.registry.packages() {
        for f in &pkg.files {
            let slot = f.file_id as usize;
            if source_files.len() <= slot {
                source_files.resize(slot + 1, (PathBuf::new(), String::new()));
            }
            source_files[slot] = (f.path.clone(), f.source.clone());
        }
    }

    // --- Comptime cfg elimination (CC1) ---
    rask_comptime::eliminate_comptime_if(&mut pkg_ctx.all_decls, &config.cfg);

    // --- Merge external package declarations ---
    //
    // Before desugaring, not after. A dependency's bodies are ordinary Rask and
    // need the same rewrites the root's do — merged afterwards, `Dog { age: 7 }`
    // in a library reached the checker as a call and came back "`Dog` is a
    // struct, so calling it doesn't construct one", in a file the consumer
    // never wrote (#1112).
    let mut package_names = Vec::new();

    // Names another package declared but did not make public, and where. A
    // package's own declarations are all merged now (#1100), so nothing stops
    // the program naming a dependency's internals — checked after resolve,
    // where the use sites are. Keyed by the name as the dependency's author
    // wrote it, which is the name a program would try.
    let mut private_elsewhere: HashMap<String, (rask_resolve::PackageId, String, Span)> =
        HashMap::new();

    // A dependency's declarations carry where they came from: `Cat` in
    // `libpkg` becomes `libpkg_Cat`, and every reference to it inside the
    // package with it. Two `Cat`s can then both exist, which is what
    // modules/RE2 says they are — a type's identity is (origin package, origin
    // name) and not the name alone (#1129).
    //
    // Every declaration is merged, not just the public ones. A package's own
    // bodies call its private helpers by their bare names, so leaving those
    // out means the package can't be resolved at all — and they reached MIR
    // anyway, through a separate list merged *after* resolve. A subdirectory is
    // a package (modules/PO1), so this is what made the ordinary `src/` layout
    // fail: `func main()` in `src/main.rk` is a private declaration by this
    // rule (#1100). Those keep sharing the program's namespace — renaming that
    // `main` leaves the program without an entry point.
    let mut merged: Vec<(rask_resolve::PackageId, Vec<Decl>)> = Vec::new();
    for pkg in pkg_ctx.registry.packages() {
        if pkg.id == pkg_ctx.root_id {
            continue;
        }
        package_names.push(pkg.name.clone());
        let mut decls: Vec<Decl> = pkg.all_decls().map(|d| d.clone()).collect();
        for decl in &decls {
            if !is_public_decl(decl) {
                if let Some(name) = declared_name(decl) {
                    private_elsewhere.insert(name, (pkg.id, pkg.name.clone(), decl.span));
                }
            }
        }
        if pkg.is_external {
            let map = package_scope::qualify_declarations(&mut decls, &pkg.name);
            exports.insert(pkg.name.clone(), map);
        }
        merged.push((pkg.id, decls));
    }

    // Now that every dependency's names are settled, point each package's
    // references at them — the program's `libpkg.Cat` and `libpkg.greet(c)`,
    // and a dependency's references to its own dependencies. Aliases come from
    // the importing package's own `import … as` lines, so this is per package
    // rather than program-wide.
    if !exports.is_empty() {
        let root_visible = visible_packages(&pkg_ctx.all_decls, exports);
        package_scope::qualify_references(&mut pkg_ctx.all_decls, &root_visible);
        let (imported, shadowed) =
            package_scope::unqualified_imports(&pkg_ctx.all_decls, &root_visible);
        package_scope::qualify_imported_names(&mut pkg_ctx.all_decls, &imported);
        diags.extend(shadowed.iter().map(shadowed_import_diagnostic));
        for (_, decls) in merged.iter_mut() {
            let visible = visible_packages(decls, exports);
            package_scope::qualify_references(decls, &visible);
            let (imported, shadowed) = package_scope::unqualified_imports(decls, &visible);
            package_scope::qualify_imported_names(decls, &imported);
            diags.extend(shadowed.iter().map(shadowed_import_diagnostic));
        }
    }
    for (_, decls) in merged {
        pkg_ctx.all_decls.extend(decls);
    }
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return PipelineOutput::fail_with_sources(diags, source_files);
    }

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
                // A dependency's private helper is no longer in the program's
                // namespace at all, so resolve reports it as a name that
                // doesn't exist. It does exist — the package just kept it —
                // and saying so is the difference between "check the spelling"
                // and "this isn't part of that library's surface".
                match private_declaration_named(e, &private_elsewhere) {
                    Some(d) => diags.push(d),
                    None => diags.push(e.to_diagnostic()),
                }
            }
            return PipelineOutput::fail_with_sources(diags, source_files);
        }
    };

    // --- A dependency's internals are not the program's to name ---
    //
    // Merging every declaration is what lets a package call its own private
    // helpers (#1100); it also puts those helpers in the program's namespace,
    // where nothing else would object. The use sites are here, so the check is
    // here: a resolution that reaches a name another package kept to itself,
    // from a file that isn't that package's.
    if !private_elsewhere.is_empty() {
        let mut file_owner: HashMap<u16, rask_resolve::PackageId> = HashMap::new();
        for pkg in pkg_ctx.registry.packages() {
            for f in &pkg.files {
                file_owner.insert(f.file_id, pkg.id);
            }
        }
        let mut where_used: HashMap<rask_ast::NodeId, Span> = HashMap::new();
        rask_ast::visit::walk_decls(&pkg_ctx.all_decls, &mut |e| {
            where_used.insert(e.id, e.span);
        });

        for (node, sym_id) in &resolved.resolutions {
            let Some(sym) = resolved.symbols.get(*sym_id) else { continue };
            let base = sym.name.split('<').next().unwrap_or(&sym.name);
            let Some((owner, owner_name, decl_span)) = private_elsewhere.get(base) else {
                continue;
            };
            let Some(use_span) = where_used.get(node) else { continue };
            if file_owner.get(&use_span.file_id) == Some(owner) {
                continue;
            }
            diags.push(
                Diagnostic::error(format!("`{}` is private to `{}`", base, owner_name))
                    .with_code("E0877")
                    .with_primary(*use_span, format!("`{}` can't be named from here", base))
                    .with_secondary(*decl_span, format!("declared here, without `public`"))
                    .with_help(format!(
                        "mark it `public func {}` (or `public struct`, …) in `{}` if it \
                         is meant to be part of that package's API",
                        base, owner_name
                    )),
            );
        }
        if diags.iter().any(|d| d.severity == Severity::Error) {
            return PipelineOutput::fail_with_sources(diags, source_files);
        }
    }

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

    // --- CT60: a `comptime func` keeps its promise where it is written ---
    for e in rask_effects::comptime_purity::check(&pkg_ctx.all_decls, &effects) {
        diags.push(comptime_purity_to_diagnostic(&e));
    }

    // --- Cleanup order (mem.resource-types/EO1) ---
    for w in rask_effects::ensure_order::check(&pkg_ctx.all_decls) {
        diags.push(ensure_order_to_diagnostic(&w));
    }

    // --- Comptime folds (CT1) and comptime tests (T11) ---
    if !diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        diags.extend(comptime_diagnostics_for(&pkg_ctx.all_decls, &typed, &config.cfg));
    }

    drop_stdlib_cascade(&mut diags);
    if diags.iter().any(|d| matches!(d.severity, Severity::Error)) {
        return PipelineOutput::fail_with_sources(diags, source_files);
    }

    PipelineOutput::ok_with_sources(
        CheckResult {
            typed,
            decls: std::mem::take(&mut pkg_ctx.all_decls),
            package_names,
            qualified_names: exports.clone(),
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
    config: &CompilerConfig,
) -> PipelineOutput<CompileResult> {
    compile_file_with(path, config, |_, _| {})
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
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
) -> PipelineOutput<CompileResult> {
    if let Some(mut pkg_ctx) = detect_package(path) {
        return compile_package_with(&mut pkg_ctx, config, transform);
    }
    compile_single(path, config, transform)
}

fn compile_single(
    path: &str,
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
) -> PipelineOutput<CompileResult> {
    let check_output = check_single(path, config);
    finalize_compile(check_output, HashSet::new(), config, transform)
}

pub fn compile_package(
    pkg_ctx: &mut PackageContext,
    config: &CompilerConfig,
) -> PipelineOutput<CompileResult> {
    compile_package_with(pkg_ctx, config, |_, _| {})
}

/// `compile_package`, with the same decl hook as `compile_file_with`.
pub fn compile_package_with(
    pkg_ctx: &mut PackageContext,
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
    finalize_compile(check_output, package_modules, config, transform)
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
    // Where the check's diagnostics were rewritten already; from here on the
    // back half of the pipeline adds its own, and mono in particular names
    // types.
    let qualified = check.qualified_names.clone();
    let mut out = finalize_compile_inner(check, package_modules, config, transform, diags, pkg_source_files);
    package_scope::unqualify_diagnostics(&mut out.diagnostics, &qualified);
    return out;
}

fn finalize_compile_inner(
    mut check: CheckResult,
    package_modules: HashSet<String>,
    config: &CompilerConfig,
    transform: impl FnOnce(&mut Vec<Decl>, &TypedProgram),
    mut diags: Vec<Diagnostic>,
    pkg_source_files: Vec<(PathBuf, String)>,
) -> PipelineOutput<CompileResult> {

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

    // A second copy of every dependency declaration used to be merged here,
    // after the check. It existed because `check_package` merged only the
    // *public* ones, so the private helpers had to reach MIR some other way —
    // and they arrived having never been in a resolve scope, which is why a
    // package with a subdirectory couldn't call its own helpers (#1100).
    // `check_package` merges all of them now, before resolve, so this list was
    // the same declarations a second time.

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
    // Hard errors (a panic, the branch quota, overflow, divide-by-zero) become
    // pipeline diagnostics and fail the build like any other pass. Warnings —
    // a const the evaluator couldn't fold, so it runs at startup — ride along.
    //
    // Comptime *tests* are not run here: `check_sources` / `check_package`
    // already ran them, and every path into this function comes through one of
    // those. Running them again was free only in the sense that a comptime test
    // has no side effects — it still interpreted every body twice on `rask run`
    // and `rask build`.
    let (comptime_globals, ct_diags) =
        evaluate_comptime_globals(&check.decls, &check.typed, &mono, Some(&config.cfg));
    // Check folded these same consts a moment ago, so anything it already
    // reported is in `diags` and would print twice. Both passes fold by
    // design — check has to answer "does this compile" without building the
    // program — but check gives up when there's no entry point to monomorphize
    // from, and a test-only file has none. So drop the repeats rather than the
    // warnings, or a `rask test` on such a file would say nothing at all.
    let ct_failed = ct_diags.iter().any(|d| matches!(d.severity, Severity::Error));
    let seen: Vec<(Option<String>, String)> = diags.iter().map(diag_key).collect();
    diags.extend(ct_diags.into_iter().filter(|d| !seen.contains(&diag_key(d))));
    if ct_failed {
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

/// Does this declaration say `public`?
///
/// `extend` blocks carry their own visibility through the methods in them, so
/// they are treated as public here and the methods are checked by the type
/// their receiver names.
fn is_public_decl(decl: &Decl) -> bool {
    match &decl.kind {
        DeclKind::Fn(f) => f.is_pub,
        DeclKind::Struct(s) => s.is_pub,
        DeclKind::Enum(e) => e.is_pub,
        DeclKind::Trait(t) => t.is_pub,
        DeclKind::Const(c) => c.is_pub,
        DeclKind::TypeAlias(a) => a.is_pub,
        DeclKind::Annotation(a) => a.is_pub,
        DeclKind::Impl(_) => true,
        _ => true,
    }
}

/// The name a declaration puts in scope, if it puts one there.
///
/// `extend` blocks have none — they attach to a type that is named elsewhere —
/// and neither do imports, exports or the package block itself.
/// E0877 for a resolve failure that is really a dependency's own declaration.
fn private_declaration_named(
    e: &rask_resolve::ResolveError,
    private_elsewhere: &HashMap<String, (rask_resolve::PackageId, String, Span)>,
) -> Option<Diagnostic> {
    let name = match &e.kind {
        rask_resolve::ResolveErrorKind::UndefinedSymbol { name } => name,
        _ => return None,
    };
    let base = name.split('<').next().unwrap_or(name);
    let (_, owner_name, decl_span) = private_elsewhere.get(base)?;
    Some(
        Diagnostic::error(format!("`{}` is private to `{}`", base, owner_name))
            .with_code("E0877")
            .with_primary(e.span, format!("`{}` can't be named from here", base))
            .with_secondary(*decl_span, "declared here, without `public`")
            .with_help(format!(
                "`{}` keeps this one to itself — mark it `public` there if it \
                 belongs in the API, or reach for something that is",
                owner_name
            )),
    )
}

/// modules/IM8: an unqualified import and a local declaration want one name.
fn shadowed_import_diagnostic(s: &package_scope::ShadowedImport) -> Diagnostic {
    let qualified = format!("{}.{}", s.package, s.original);
    // `libpkg` + `Cat` → `LibpkgCat`, so the alias reads as the type it is.
    let mut alias = String::new();
    let mut head = s.package.chars();
    if let Some(c) = head.next() {
        alias.extend(c.to_uppercase());
        alias.push_str(head.as_str());
    }
    alias.push_str(&s.original);
    Diagnostic::error(format!(
        "importing `{}` collides with the `{}` declared here",
        qualified, s.name
    ))
    .with_code("E0876")
    .with_primary(s.import_at, format!("this brings `{}` in as `{}`", qualified, s.name))
    .with_secondary(s.declared_at, format!("and `{}` is declared here", s.name))
    .with_fix(format!("import {}.{} as {}", s.package, s.original, alias))
    .with_help(format!(
        "or drop the unqualified import and write `{}` where you need it — \
         depending on `{}` is fine either way, the two are different types \
         (modules/RE2)",
        qualified, s.package
    ))
}

/// What each package binding in these declarations makes available.
///
/// `import libpkg` binds `libpkg`; `import libpkg as l` binds `l`. A package's
/// own name is always visible to itself, so a library referring to its own
/// declarations by `mylib.Name` works too.
fn visible_packages(
    decls: &[Decl],
    exports: &HashMap<String, HashMap<String, String>>,
) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    for decl in decls {
        let DeclKind::Import(import) = &decl.kind else { continue };
        let Some(pkg) = import.path.first() else { continue };
        let Some(map) = exports.get(pkg) else { continue };
        let binding = match (&import.alias, import.path.len()) {
            // `import pkg.Name as N` renames the member, not the package.
            (Some(alias), 1) => alias.clone(),
            _ => pkg.clone(),
        };
        out.insert(binding, map.clone());
    }
    out
}

fn declared_name(decl: &Decl) -> Option<String> {
    match &decl.kind {
        DeclKind::Fn(f) => Some(f.name.clone()),
        DeclKind::Struct(s) => Some(s.name.clone()),
        DeclKind::Enum(e) => Some(e.name.clone()),
        DeclKind::Trait(t) => Some(t.name.clone()),
        DeclKind::Const(c) => Some(c.name.clone()),
        DeclKind::TypeAlias(a) => Some(a.name.clone()),
        DeclKind::Annotation(a) => Some(a.name.clone()),
        DeclKind::Union(u) => Some(u.name.clone()),
        _ => None,
    }
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

/// Drop diagnostics that land in stdlib source, when the user's own code has
/// errors of its own.
///
/// A program that declares `struct T` is rejected by PC3 (E0357) — single
/// uppercase letters are type parameters, so a concrete type with that name is
/// unusable. That is the right error. What followed it was six more, in
/// `stdlib/num.rk`, about `Wrapping<T>` having no `wrapping_add`: the user's
/// `T` had taken the place of that declaration's own type parameter. Nothing
/// there is actionable — the file isn't theirs, and it compiles fine on its own
/// — so it is noise on top of the one error that matters.
///
/// Only when there is a user error to keep. A program whose *only* errors are
/// in the stdlib has found a real stdlib bug, and hiding it would leave whoever
/// is working on the stdlib with a failure and no message.
fn drop_stdlib_cascade(diags: &mut Vec<Diagnostic>) {
    let user_error = diags.iter().any(|d| {
        matches!(d.severity, Severity::Error)
            && d.primary_span().is_some_and(|s| !rask_mono::is_stdlib_span(s))
    });
    if !user_error {
        return;
    }
    diags.retain(|d| {
        !matches!(d.severity, Severity::Error)
            || d.primary_span().is_none_or(|s| !rask_mono::is_stdlib_span(s))
    });
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

/// CT60: the promise `comptime func` makes, checked at the definition.
fn comptime_purity_to_diagnostic(e: &rask_effects::comptime_purity::ComptimePurityError) -> Diagnostic {
    let via = match &e.via {
        Some(call) => format!(" — `{}` does", call),
        None => String::new(),
    };
    let diag = Diagnostic::error(format!(
        "`comptime func {}` reaches {} at compile time{}",
        e.func, e.effect, via
    ))
    .with_code("E0875")
    .with_primary(e.span, format!("{} isn't available while compiling", e.effect))
    .with_fix(format!(
        "drop `comptime` from `{}` and let its callers decide, or move the \
         {} out and pass the result in",
        e.func, e.effect
    ))
    .with_why(
        "`comptime func` asserts at the definition what CT6 otherwise checks at \
         each call: that the body stays inside the compile-time subset, \
         transitively. Without the check the keyword bought nothing — the \
         failure surfaced later and elsewhere, as the evaluator not finding a \
         function it had never registered [ctrl.comptime/CT7, CT60]"
            .to_string(),
    );
    if e.via.is_some() && e.span != e.decl_span {
        diag.with_secondary(e.decl_span, "declared `comptime` here")
    } else {
        diag
    }
}

fn frozen_to_diagnostic(d: &FrozenDiagnostic) -> Diagnostic {
    let diag = if d.is_error {
        Diagnostic::error(&d.message)
    } else {
        Diagnostic::warning(&d.message)
    };
    diag.with_code(d.code).with_primary(d.span, "")
}

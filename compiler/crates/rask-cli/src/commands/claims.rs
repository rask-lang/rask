// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Claims (type.claims): a package's `claims.rk`, checked by the build.
//!
//! `claims.rk` compiles as a package of its own that imports the one it
//! checks, which is what limits it to that package's `public` items. Beside
//! it goes one generated case function per claim, which calls the claim with
//! one `claims.input(...)` per parameter: the parameter's type says what to
//! build, and lowering builds it through the shape `json.decode` would read.
//! The result links under sim. One process runs every claim; a
//! counterexample is cut down by re-running it on smaller choices before it's
//! reported.

use std::path::{Path, PathBuf};
use std::process;

use colored::Colorize;
use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::Span;

use crate::output;
use crate::Format;

/// Default per-input step budget (type.claims/D2).
const DEFAULT_BUDGET: i64 = 10_000_000;

/// What the claims of a package came to.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimsOutcome {
    /// The package has no `claims.rk`.
    NoClaims,
    /// Every claim held, or ended without a verdict that fails the build.
    Held,
    /// A claim is false, or the claims didn't compile.
    Failed,
}

/// Whether the claims are run, or only compiled (type.claims/R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimsMode {
    /// `rask check`: no code is generated, so the claims are type-checked.
    Check,
    /// Every command that builds: compiled, run and reported.
    Run,
}

/// How this invocation runs claims, from its flags. Set once by `main`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaimsOptions {
    /// `--claims=strict` (type.claims/V6): only "checked all" passes.
    pub strict: bool,
    /// `--claim-seeds N`: sampled claims run on N more seeds after the
    /// build's own, looking further without changing what a build decides.
    pub extra_seeds: u64,
}

static OPTIONS: std::sync::OnceLock<ClaimsOptions> = std::sync::OnceLock::new();

pub fn set_options(opts: ClaimsOptions) {
    let _ = OPTIONS.set(opts);
}

fn options() -> ClaimsOptions {
    OPTIONS.get().copied().unwrap_or_default()
}

/// Check the claims of the package at `path`, if it has any.
pub fn check_package_claims(path: &str, format: Format, mode: ClaimsMode) -> ClaimsOutcome {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let sources = match claims_sources(&root) {
        Ok(sources) => sources,
        Err(e) => {
            eprintln!("{}: {e}", output::error_label());
            return ClaimsOutcome::Failed;
        }
    };
    if sources.is_empty() {
        return ClaimsOutcome::NoClaims;
    }
    let mut plan = match plan_claims(&sources) {
        Ok(plan) => plan,
        Err(diags) => {
            for d in &diags {
                crate::show_diagnostic_multi(d, &sources);
            }
            return ClaimsOutcome::Failed;
        }
    };
    let Some(compiled) = compile_claims(path, &sources, &mut plan) else {
        return ClaimsOutcome::Failed;
    };
    if plan.claims.is_empty() || mode == ClaimsMode::Check {
        return ClaimsOutcome::Held;
    }
    let runs = claim_runs(&plan);
    let input = rask_compiler::emit::BackEnd::of(&compiled.result, None, None);
    let entry = rask_compiler::emit::Entry::Claims(&runs);
    let lowered = match rask_compiler::emit::lower(&input, &entry) {
        Ok(lowered) => lowered,
        Err(errors) => {
            for e in &errors {
                eprintln!("{}: compile claims: {}", output::error_label(), e);
            }
            return ClaimsOutcome::Failed;
        }
    };

    // A claim nothing it reaches changed in keeps its verdict (D4).
    let keys: Vec<u64> = plan.claims.iter().enumerate()
        .map(|(i, c)| verdict_key(c, &rask_compiler::emit::reached_from(&lowered, &input, &case_fn(i))))
        .collect();
    let cache_path = root.join("build").join(".cache").join("claims");
    let mut cache = VerdictCache::load(&cache_path);
    let cached: Vec<Option<Record>> = plan.claims.iter().zip(&keys)
        .map(|(c, k)| cache.get(&c.name, *k))
        .collect();
    let to_run: Vec<usize> = (0..plan.claims.len()).filter(|i| cached[*i].is_none()).collect();
    let opts = options();

    let bin = if to_run.is_empty() && opts.extra_seeds == 0 {
        None
    } else {
        match link_claims(&input, &entry, lowered, &compiled.link_opts) {
            Some(bin) => Some(bin),
            None => return ClaimsOutcome::Failed,
        }
    };
    let ran = match (&bin, to_run.is_empty()) {
        (Some(bin), false) => {
            let only = to_run.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            run_bin(bin, &[("RASK_CLAIM_ONLY", only)])
        }
        _ => (Vec::new(), String::new(), process::ExitStatus::default()),
    };
    let mut records = ran.0;
    for (i, c) in cached.into_iter().enumerate() {
        if let Some(r) = c {
            debug_assert_eq!(r.claim, plan.claims[i].name);
            records.push(r);
        }
    }
    let ok = run_claims(bin.as_deref(), (records, ran.1, ran.2), &plan, &compiled, &root, format);

    // Only a claim that held is kept: a false one is what's being worked on,
    // and it runs and shrinks again until it holds.
    for (i, claim) in plan.claims.iter().enumerate() {
        if to_run.contains(&i) {
            if let Some(r) = ok.held.iter().find(|r| r.claim == claim.name) {
                cache.put(&claim.name, keys[i], r);
            }
        }
    }
    cache.save(&cache_path, &plan);

    if let Some(bin) = bin {
        // Same switch as a test binary's: a claims process that dies takes the
        // evidence with it.
        if std::env::var_os("RASK_KEEP_TEST_BIN").is_some() {
            eprintln!("{}: claims binary kept at {}", output::warning_label(), bin.display());
        } else {
            let _ = std::fs::remove_file(&bin);
        }
    }
    if ok.all_held { ClaimsOutcome::Held } else { ClaimsOutcome::Failed }
}

// ─── Cached verdicts (type.claims/D4) ───────────────────────

/// What a claim's verdict depends on: the claim itself, its seed and budget,
/// the MIR of every function it reaches, the runtime it links against, and
/// the compiler that made the rest. A comment edit elsewhere in the package
/// leaves it alone; a change to anything the claim calls doesn't.
fn verdict_key(claim: &ClaimPlan, reached: &std::collections::BTreeMap<&str, &rask_mir::MirFunction>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    claim.name.hash(&mut h);
    claim.seed.hash(&mut h);
    claim.budget.hash(&mut h);
    for f in reached.values() {
        f.to_string().hash(&mut h);
    }
    runtime_fingerprint().hash(&mut h);
    super::cache::compiler_fingerprint().hash(&mut h);
    h.finish()
}

/// The runtime's sources, by content: the linker compiles them per build, so
/// an edit there changes what a claim runs without changing the compiler.
fn runtime_fingerprint() -> u64 {
    use std::hash::{Hash, Hasher};
    static PRINT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *PRINT.get_or_init(|| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Ok(dir) = super::link::find_runtime_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "c" || x == "h"))
                .collect();
            files.sort();
            for f in files {
                f.file_name().hash(&mut h);
                std::fs::read(&f).unwrap_or_default().hash(&mut h);
            }
        }
        h.finish()
    })
}

/// `build/.cache/claims`: one line per claim that held, `name<TAB>key<TAB>record`.
struct VerdictCache {
    entries: Vec<(String, u64, String)>,
}

impl VerdictCache {
    fn load(path: &Path) -> VerdictCache {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let entries = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\t');
                let name = parts.next()?.to_string();
                let key = u64::from_str_radix(parts.next()?, 16).ok()?;
                Some((name, key, parts.next()?.to_string()))
            })
            .collect();
        VerdictCache { entries }
    }

    fn get(&self, name: &str, key: u64) -> Option<Record> {
        let (_, _, raw) = self.entries.iter().find(|(n, k, _)| n == name && *k == key)?;
        Record::parse(raw)
    }

    fn put(&mut self, name: &str, key: u64, r: &Record) {
        self.entries.retain(|(n, _, _)| n != name);
        self.entries.push((name.to_string(), key, r.raw.clone()));
    }

    /// Keeps the claims the plan still has, so a deleted claim's line goes.
    fn save(&self, path: &Path, plan: &Plan) {
        let text: String = self
            .entries
            .iter()
            .filter(|(n, _, _)| plan.claims.iter().any(|c| &c.name == n))
            .map(|(n, k, raw)| format!("{n}\t{k:016x}\t{raw}\n"))
            .collect();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }
}

// ─── Planning: the case functions ───────────────────────────

/// One claim to run: a claim as declared, or one instantiation of a
/// generic claim (G1).
#[derive(Clone)]
struct ClaimPlan {
    /// How it's reported: `antisymmetric<ledger.Entry>` for an instantiation.
    name: String,
    /// What its case function calls, type arguments included.
    callee: String,
    /// Which source declares it, by index into the sources.
    file: usize,
    /// Within that source; the file id is whatever the compile gives the file.
    span: Span,
    params: Vec<(String, Span)>,
    seed: u64,
    budget: i64,
}

/// A claim with type parameters, before it's instantiated.
struct GenericClaim {
    claim: ClaimPlan,
    type_params: usize,
    /// For a stdlib law, the interface it's the law of: it's instantiated
    /// only with the types that declare their own conformance to it.
    law_of: Option<String>,
}

struct Plan {
    /// Each claims source's path, in source order.
    files: Vec<PathBuf>,
    claims: Vec<ClaimPlan>,
    generics: Vec<GenericClaim>,
    /// The generated source: the end-of-body marker and a case function per claim.
    runner: String,
    /// For each line of `runner`, the claim and parameter it builds an input
    /// for, so an error there can point at the parameter instead.
    input_lines: Vec<Option<(usize, usize)>>,
    /// For each line of `runner`, the claim whose case function it's in.
    case_lines: Vec<Option<usize>>,
}

impl Plan {
    /// The case functions and the `main` that calls them.
    fn write_runner(&mut self) {
        let mut runner = format!(
            "import claims\n\nfunc {}() {{\n    claims.reached_end()\n}}\n\n",
            rask_desugar::CLAIM_END_MARKER
        );
        let mut input_lines: Vec<Option<(usize, usize)>> = vec![None; runner.lines().count()];
        let mut case_lines: Vec<Option<usize>> = vec![None; runner.lines().count()];
        for (i, c) in self.claims.iter().enumerate() {
            runner.push_str(&format!("func {}() {{\n    {}(\n", case_fn(i), c.callee));
            input_lines.extend([None, None]);
            case_lines.extend([Some(i), Some(i)]);
            for (k, (name, _)) in c.params.iter().enumerate() {
                let sep = if k + 1 == c.params.len() { "" } else { "," };
                runner.push_str(&format!("        claims.input(\"{name}\"){sep}\n"));
                input_lines.push(Some((i, k)));
                case_lines.push(Some(i));
            }
            runner.push_str("    )\n}\n\n");
            input_lines.extend([None, None, None]);
            case_lines.extend([Some(i), Some(i), None]);
        }
        // Monomorphization keeps what `main` reaches; the claim runner replaces it.
        runner.push_str("func main() {\n");
        for i in 0..self.claims.len() {
            runner.push_str(&format!("    {}()\n", case_fn(i)));
        }
        runner.push_str("}\n");
        if std::env::var_os("RASK_CLAIMS_DUMP").is_some() {
            eprintln!("{runner}");
        }
        self.runner = runner;
        self.input_lines = input_lines;
        self.case_lines = case_lines;
    }

    /// Every generic claim at every combination of `types`, after the
    /// claims as declared. A law takes only the types that declare their own
    /// conformance to its interface (`declared`: type, interface).
    fn instantiate(&mut self, types: &[String], declared: &[(String, String)]) {
        for g in &self.generics {
            let law_types: Vec<String>;
            let types = match &g.law_of {
                Some(iface) => {
                    law_types = declared.iter().filter(|(_, i)| i == iface).map(|(t, _)| t.clone()).collect();
                    &law_types[..]
                }
                None => types,
            };
            let mut combos: Vec<Vec<&String>> = vec![Vec::new()];
            for _ in 0..g.type_params {
                combos = combos
                    .into_iter()
                    .flat_map(|c| types.iter().map(move |t| {
                        let mut c = c.clone();
                        c.push(t);
                        c
                    }))
                    .collect();
            }
            for args in combos {
                let args = args.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ");
                let mut c = g.claim.clone();
                c.name = format!("{}<{args}>", g.claim.name);
                c.callee = c.name.clone();
                c.seed = fnv1a(&format!("{}{}", g.claim.seed, args));
                self.claims.push(c);
            }
        }
        self.write_runner();
    }

    /// The claim a line of the runner belongs to.
    fn claim_at(&self, offset: usize) -> Option<usize> {
        let line = self.runner.get(..offset).map_or(0, |t| t.matches('\n').count());
        self.case_lines.get(line).copied().flatten()
    }
}

fn case_fn(i: usize) -> String {
    format!("__claim_case_{i}")
}

/// The stdlib's interface laws, a claims source every package is held to.
const LAWS_FILE: &str = "<stdlib laws>";
const LAWS: &str = include_str!("laws.rk");

/// The conformances the package at `root` writes out itself, as (type,
/// interface): `Score implements Comparable` is `("Score", "Comparable")`.
/// Only a public type's; the laws see the package from outside.
fn declared_conformances(root: &Path) -> Vec<(String, String)> {
    let mut public = std::collections::HashSet::new();
    let mut found = Vec::new();
    for (id, path) in rask_resolve::package::package_source_files(root).iter().enumerate() {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let tokens = rask_lexer::Lexer::new_with_file_id(&src, id as u16).tokenize().tokens;
        for d in rask_parser::Parser::new_with_file_id(tokens, 0, id as u16).parse().decls {
            match &d.kind {
                DeclKind::Struct(s) if s.is_pub && s.type_params.is_empty() => {
                    public.insert(s.name.clone());
                }
                DeclKind::Enum(e) if e.is_pub && e.type_params.is_empty() => {
                    public.insert(e.name.clone());
                }
                DeclKind::Impl(i) => {
                    if let (Some(iface), Some(ty)) = (i.interface.as_ref().and_then(|t| t.name()), i.target_ty.name()) {
                        found.push((ty, iface));
                    }
                }
                _ => {}
            }
        }
    }
    found.retain(|(t, _)| public.contains(t));
    found.sort();
    found.dedup();
    found
}

/// The claims sources of the package at `root`: its own `claims.rk`, then
/// each `[claims]` entry of its manifest, a file or every `.rk` file under a
/// directory (type.claims/W1, W2), then the stdlib's laws when the package
/// declares a conformance they're about. They compile together as one
/// package.
fn claims_sources(root: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut sources = shared_sources(root)?;
    let laws = ["Equal", "Comparable", "Hashable", "Cloneable"];
    if declared_conformances(root).iter().any(|(_, i)| laws.contains(&i.as_str())) {
        sources.push((PathBuf::from(LAWS_FILE), LAWS.to_string()));
    }
    Ok(sources)
}

fn shared_sources(root: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut sources = Vec::new();
    let own = root.join(rask_resolve::package::CLAIMS_FILE);
    if let Ok(text) = std::fs::read_to_string(&own) {
        sources.push((own, text));
    }
    let manifest = root.join("rask.toml");
    if !manifest.is_file() {
        return Ok(sources);
    }
    let decl = rask_manifest::load(&manifest)?;
    for c in &decl.claims {
        let dir = match (&c.path, &c.git) {
            (Some(p), _) => root.join(p),
            (None, Some(_)) => super::claims_git::checkout(root, c)?,
            (None, None) => continue,
        };
        let files = if dir.is_dir() {
            crate::collect_rk_files(&dir).into_iter().map(PathBuf::from).collect()
        } else if dir.is_file() {
            vec![dir.clone()]
        } else {
            return Err(format!("[claims] {}: `{}` doesn't exist", c.name, dir.display()));
        };
        if files.is_empty() {
            return Err(format!("[claims] {}: `{}` holds no `.rk` files", c.name, dir.display()));
        }
        for f in files {
            let text = std::fs::read_to_string(&f).map_err(|e| format!("[claims] {}: reading `{}`: {e}", c.name, f.display()))?;
            sources.push((f, text));
        }
    }
    Ok(sources)
}

/// The claims the sources declare and the code that calls them. A file that
/// doesn't parse plans nothing: the compile reports why.
fn plan_claims(sources: &[(PathBuf, String)]) -> Result<Plan, Vec<rask_diagnostics::Diagnostic>> {
    let mut claims = Vec::new();
    let mut generics = Vec::new();
    let mut errors = Vec::new();
    for (file, (_, src)) in sources.iter().enumerate() {
        let mut lexer = rask_lexer::Lexer::new_with_file_id(src, file as u16);
        let tokens = lexer.tokenize().tokens;
        let decls = rask_parser::Parser::new_with_file_id(tokens, 0, file as u16).parse().decls;
        for f in decls.iter().filter_map(claim_fn) {
            if let Some(p) = f.params.iter().find(|p| p.is_mutate) {
                errors.push(
                    rask_diagnostics::Diagnostic::error(format!("claim `{}` takes `{}` as `mutate`", f.name, p.name))
                        .with_code("E0952")
                        .with_primary(p.name_span, "a claim's input can't be `mutate`")
                        .with_help("take it as a plain input, and make a `mut` copy inside the claim if it needs changing"),
                );
                continue;
            }
            let text = src.get(f.span.start..f.span.end).unwrap_or(&f.name);
            let claim = ClaimPlan {
                name: f.name.clone(),
                callee: f.name.clone(),
                file,
                span: f.span,
                params: f.params.iter().map(|p| (p.name.clone(), p.name_span)).collect(),
                seed: fnv1a(text),
                budget: claim_budget(f).unwrap_or(DEFAULT_BUDGET),
            };
            if f.type_params.is_empty() {
                claims.push(claim);
            } else {
                let law_of = (sources[file].0 == Path::new(LAWS_FILE))
                    .then(|| f.type_params.first()?.bounds.first()?.ty.name())
                    .flatten();
                generics.push(GenericClaim { claim, type_params: f.type_params.len(), law_of });
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let files = sources.iter().map(|(p, _)| p.clone()).collect();
    let mut plan = Plan {
        files,
        claims,
        generics,
        runner: String::new(),
        input_lines: Vec::new(),
        case_lines: Vec::new(),
    };
    plan.write_runner();
    Ok(plan)
}

fn claim_fn(d: &Decl) -> Option<&FnDecl> {
    match &d.kind {
        DeclKind::Fn(f) if f.attrs.iter().any(|a| a.is("claim")) => Some(f),
        _ => None,
    }
}

fn claim_budget(f: &FnDecl) -> Option<i64> {
    let attr = f.attrs.iter().find(|a| a.is("claim_budget"))?;
    let arg = attr.args.as_ref()?.first()?;
    match arg.value.expr().map(|e| &e.kind) {
        Some(rask_ast::expr::ExprKind::Int(n, _)) => i64::try_from(*n).ok(),
        _ => None,
    }
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ─── Compiling ──────────────────────────────────────────────

/// The claims package's file of generated code.
const RUNNER_FILE: &str = "<claims runner>";

struct Compiled {
    result: rask_compiler::CompileResult,
    link_opts: super::link::LinkOptions,
    /// Every source in the compile, by file id, for rendering diagnostics.
    sources: Vec<(PathBuf, String)>,
    /// The id the compile gave each claims source, in source order.
    claims_files: Vec<u16>,
}

/// The package at `path`, prepared to build, with the root package opened up
/// as a dependency: seen from the claims, its names are its own and only
/// what it made `public` is reachable.
fn prepare(path: &str) -> Option<(super::build::PreparedBuild, String)> {
    let opts = super::build::BuildOptions {
        profile: "debug".to_string(),
        verbose: false,
        target: None,
        no_cache: false,
        force: false,
        no_default_features: false,
    };
    let mut prepared = super::build::prepare_build(path, opts);
    if prepared.dep_errors > 0 {
        eprintln!("{}", output::banner_fail("Build", prepared.dep_errors));
        return None;
    }
    let root_id = prepared.root_id;
    let root_name = prepared.registry.get(root_id)?.name.clone();
    if let Some(root_pkg) = prepared.registry.get_mut(root_id) {
        root_pkg.is_external = true;
    }
    Some((prepared, root_name))
}

/// The claims package added to `prepared`: the claims sources and the runner.
struct ClaimsPackage {
    ctx: rask_compiler::PackageContext,
    config: rask_compiler::CompilerConfig,
    link_opts: super::link::LinkOptions,
    /// Every source in the compile, by file id.
    sources: Vec<(PathBuf, String)>,
    claims_files: Vec<u16>,
    runner_file: u16,
}

fn add_claims_package(
    prepared: super::build::PreparedBuild,
    root_name: &str,
    claim_sources: &[(PathBuf, String)],
    runner: &str,
) -> Option<ClaimsPackage> {
    let mut prepared = prepared;
    let root_id = prepared.root_id;
    let claims_id = match prepared.registry.add_source_package(
        &format!("{root_name}_claims"),
        claim_sources
            .iter()
            .cloned()
            .chain([(PathBuf::from(RUNNER_FILE), runner.to_string())])
            .collect(),
        vec![root_id],
        false,
    ) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            return None;
        }
    };

    let mut sources: Vec<(PathBuf, String)> = Vec::new();
    let mut claims_files = vec![0u16; claim_sources.len()];
    let mut runner_file = u16::MAX;
    for pkg in prepared.registry.packages() {
        for f in &pkg.files {
            let slot = f.file_id as usize;
            if sources.len() <= slot {
                sources.resize(slot + 1, (PathBuf::new(), String::new()));
            }
            sources[slot] = (f.path.clone(), f.source.clone());
            if pkg.id == claims_id {
                if let Some(i) = claim_sources.iter().position(|(p, _)| *p == f.path) {
                    claims_files[i] = f.file_id;
                }
                if f.path == Path::new(RUNNER_FILE) {
                    runner_file = f.file_id;
                }
            }
        }
    }
    let all_decls: Vec<Decl> = prepared.registry.get(claims_id)?.all_decls().cloned().collect();
    let cfg = rask_comptime::CfgConfig::from_host("debug", prepared.resolved_feature_names.clone());
    Some(ClaimsPackage {
        ctx: rask_compiler::PackageContext { registry: prepared.registry, root_id: claims_id, all_decls },
        config: crate::compiler_config(cfg).with_claims(),
        link_opts: prepared.link_opts,
        sources,
        claims_files,
        runner_file,
    })
}

/// The package's public types a generic claim may be instantiated with:
/// structs and enums without type parameters of their own, spelled the way
/// a claims source names them.
fn public_types(prepared: &super::build::PreparedBuild, root_name: &str) -> Vec<String> {
    let mut out: Vec<String> = prepared
        .registry
        .all_decls(prepared.root_id)
        .into_iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Struct(s) if s.is_pub && s.type_params.is_empty() => Some(&s.name),
            DeclKind::Enum(e) if e.is_pub && e.type_params.is_empty() => Some(&e.name),
            _ => None,
        })
        .map(|n| format!("{root_name}.{n}"))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Instantiate every generic claim with every public type (G1, G2), and
/// keep the instantiations the checker accepts: the type conforms to the
/// claim's bounds and a claim can be given one (IN1). The checker decides
/// both, the same way it would for a call written by hand.
fn instantiate_generics(path: &str, claim_sources: &[(PathBuf, String)], plan: &mut Plan) -> Option<()> {
    let (prepared, root_name) = prepare(path)?;
    let types = public_types(&prepared, &root_name);
    let conformances: Vec<(String, String)> = declared_conformances(&prepared.root)
        .into_iter()
        .map(|(t, i)| (format!("{root_name}.{t}"), i))
        .collect();
    let declared = plan.claims.len();
    plan.instantiate(&types, &conformances);
    let mut pkg = add_claims_package(prepared, &root_name, claim_sources, &plan.runner)?;
    let output = rask_compiler::check_package(&mut pkg.ctx, &pkg.config);

    let mut rejected = std::collections::HashSet::new();
    let mut failed = false;
    for d in output.diagnostics.iter().filter(|d| d.severity == rask_diagnostics::Severity::Error) {
        let at = d
            .labels
            .iter()
            .find(|l| l.span.file_id == pkg.runner_file)
            .and_then(|l| plan.claim_at(l.span.start));
        match at {
            Some(i) if i >= declared => {
                rejected.insert(i);
            }
            _ => {
                failed = true;
                let d = point_at_input(d, plan, pkg.runner_file, &pkg.claims_files);
                crate::show_diagnostic_multi(&d, &pkg.sources);
            }
        }
    }
    if failed {
        return None;
    }
    let mut i = 0;
    plan.claims.retain(|_| {
        let keep = !rejected.contains(&i);
        i += 1;
        keep
    });
    plan.write_runner();
    Some(())
}

/// Compile the claims sources and the plan's case functions as one package
/// that imports the package at `path`.
fn compile_claims(path: &str, claim_sources: &[(PathBuf, String)], plan: &mut Plan) -> Option<Compiled> {
    if !plan.generics.is_empty() {
        instantiate_generics(path, claim_sources, plan)?;
    }
    let (prepared, root_name) = prepare(path)?;
    let mut pkg = add_claims_package(prepared, &root_name, claim_sources, &plan.runner)?;
    let output = rask_compiler::compile_package_with(&mut pkg.ctx, &pkg.config, |_, _| {});
    for diag in &output.diagnostics {
        let diag = point_at_input(diag, plan, pkg.runner_file, &pkg.claims_files);
        crate::show_diagnostic_multi(&diag, &pkg.sources);
    }
    let result = output.result?;
    Some(Compiled { result, link_opts: pkg.link_opts, sources: pkg.sources, claims_files: pkg.claims_files })
}

/// An error inside a generated case function is about one of the claim's
/// inputs: say which, at the parameter that asked for it.
fn point_at_input(
    diag: &rask_diagnostics::Diagnostic,
    plan: &Plan,
    runner_file: u16,
    claims_files: &[u16],
) -> rask_diagnostics::Diagnostic {
    let mut diag = diag.clone();
    let mut input = None;
    for label in &mut diag.labels {
        if label.span.file_id != runner_file {
            continue;
        }
        let line = plan.runner.get(..label.span.start).map_or(0, |t| t.matches('\n').count());
        if let Some(Some((c, k))) = plan.input_lines.get(line) {
            let claim = &plan.claims[*c];
            let (name, span) = &claim.params[*k];
            label.span = Span { file_id: claims_files[claim.file], ..*span };
            input = Some((claim.name.clone(), name.clone()));
        }
    }
    if let Some((claim, name)) = input {
        diag.notes.push(format!(
            "the build makes input `{name}` of claim `{claim}` the way `json.decode` builds a value, \
             and `char`, tuples and enums too; a type it can't build is made inside the claim, \
             from inputs it can (type.claims/IN1)"
        ));
    }
    diag
}

fn claim_runs(plan: &Plan) -> Vec<rask_codegen::ClaimRun> {
    plan.claims
        .iter()
        .enumerate()
        .map(|(i, c)| rask_codegen::ClaimRun {
            name: c.name.clone(),
            fn_name: case_fn(i),
            seed: c.seed,
            budget: c.budget,
        })
        .collect()
}

/// Build and link the claim binary under sim. Returns its path.
fn link_claims(
    input: &rask_compiler::emit::BackEnd,
    entry: &rask_compiler::emit::Entry,
    lowered: rask_compiler::emit::Lowered,
    link: &super::link::LinkOptions,
) -> Option<PathBuf> {
    static NEXT_BIN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT_BIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let bin_path = std::env::temp_dir().join(format!("rask_claims_{}_{n}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();
    let obj_path = format!("{bin_str}.o");

    let emitted = rask_compiler::emit::emit_lowered(
        input,
        entry,
        lowered,
        None,
        rask_compiler::emit::BuildMode::Debug,
        &obj_path,
    );
    if !emitted.succeeded() {
        for e in &emitted.errors {
            eprintln!("{}: compile claims: {}", output::error_label(), e);
        }
        for d in &emitted.diagnostics {
            eprintln!("{}: compile claims: {}", output::error_label(), d.message);
        }
        let _ = std::fs::remove_file(&obj_path);
        return None;
    }
    let link_opts = super::link::LinkOptions {
        sim: true,
        libs: link.libs.clone(),
        objects: link.objects.clone(),
        search_paths: link.search_paths.clone(),
        keeps_binary: false,
    };
    let linked = super::link::link_executable_with(&obj_path, &bin_str, &link_opts, false, None);
    let _ = std::fs::remove_file(&obj_path);
    if let Err(e) = linked {
        eprintln!("{}: link claims: {}", output::error_label(), e);
        return None;
    }
    Some(bin_path)
}

// ─── Running ────────────────────────────────────────────────

/// One record the claim runner printed (claims.c).
#[derive(Default, Clone)]
struct Record {
    claim: String,
    done: bool,
    result: String,
    input: String,
    error: String,
    choices: String,
    index: i64,
    mode: String,
    tried: u64,
    distinct: u64,
    discarded: u64,
    over_budget: u64,
    /// Met another task, the clock or simulated I/O, so the schedule was sampled.
    scheduled: bool,
    budget_input: String,
    /// The line it came from, for the verdict cache.
    raw: String,
}

impl Record {
    fn parse(json: &str) -> Option<Record> {
        use super::run::{parse_json_i64, parse_json_str, unescape_json_str};
        let s = |k: &str| parse_json_str(json, k).map(unescape_json_str).unwrap_or_default();
        let n = |k: &str| parse_json_i64(json, k).map_or(0, |v| v.max(0) as u64);
        Some(Record {
            claim: unescape_json_str(parse_json_str(json, "claim")?),
            done: json.contains("\"done\":true"),
            result: s("result"),
            input: s("input"),
            error: s("error"),
            choices: s("choices"),
            index: parse_json_i64(json, "index").unwrap_or(-1),
            mode: s("mode"),
            tried: n("tried"),
            distinct: n("distinct"),
            discarded: n("discarded"),
            over_budget: n("over_budget"),
            scheduled: json.contains("\"scheduled\":true"),
            budget_input: s("budget_input"),
            raw: json.to_string(),
        })
    }
}

/// Run the claim binary, with `env` added, and read back its records and
/// what it wrote to stderr.
fn run_bin(bin: &Path, env: &[(&str, String)]) -> (Vec<Record>, String, process::ExitStatus) {
    let mut cmd = super::run::test_binary_command(bin);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            eprintln!("{}: running the claims: {e}", output::error_label());
            process::exit(1);
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let records = stdout
        .lines()
        .filter_map(|line| super::run::split_record(line).1)
        .filter_map(Record::parse)
        .collect();
    (records, String::from_utf8_lossy(&out.stderr).into_owned(), out.status)
}

/// How the claims came out, and the summaries of those that held.
struct RunOutcome {
    all_held: bool,
    held: Vec<Record>,
}

/// Report every claim from its records, cached ones included. `bin` is
/// there when anything ran: shrinking and extra seeds re-run it.
fn run_claims(
    bin: Option<&Path>,
    (records, stderr, status): (Vec<Record>, String, process::ExitStatus),
    plan: &Plan,
    compiled: &Compiled,
    root: &Path,
    format: Format,
) -> RunOutcome {
    let opts = options();
    let shown = |claim: &ClaimPlan| -> String {
        let path = &plan.files[claim.file];
        path.strip_prefix(root).unwrap_or(path).display().to_string()
    };
    if format == Format::Human {
        let files: Vec<String> = plan.files.iter().map(|f| f.display().to_string()).collect();
        println!("{} {}", "Claims".green().bold(), files.join(", "));
    }
    let mut all_held = true;
    let mut held = Vec::new();
    for (i, claim) in plan.claims.iter().enumerate() {
        let summary = records.iter().find(|r| r.done && r.claim == claim.name);
        let failure = records.iter().find(|r| !r.done && r.claim == claim.name && r.result == "fail");
        let Some(summary) = summary else {
            all_held = false;
            report_death(claim, &stderr, &status);
            break;
        };
        if let Some(fail) = failure {
            all_held = false;
            let (input, error) = match bin {
                Some(bin) => shrink(bin, i, fail, &[]),
                None => (fail.input.clone(), fail.error.clone()),
            };
            report_false(claim, &input, &error, &shown(claim), compiled, None);
            continue;
        }
        held.push(summary.clone());
        if format == Format::Human {
            println!("  {:<24} {}", claim.name, verdict(summary));
            if summary.over_budget > 0 {
                println!(
                    "  {:<24} {} ran past the step budget; the first was {}",
                    "",
                    count(summary.over_budget, "input"),
                    summary.budget_input
                );
            }
        }
        if opts.strict && !(summary.mode == "all" && summary.over_budget == 0) {
            all_held = false;
            report_not_strict(claim, summary, compiled);
        }
    }
    // A generic claim of the package's own that nothing instantiated says so,
    // rather than passing without a word.
    if format == Format::Human {
        for g in plan.generics.iter().filter(|g| g.law_of.is_none()) {
            let prefix = format!("{}<", g.claim.name);
            if !plan.claims.iter().any(|c| c.name.starts_with(&prefix)) {
                println!("  {:<24} no public type of the package meets its bounds and can be built", g.claim.name);
            }
        }
    }
    if let Some(bin) = bin.filter(|_| all_held && opts.extra_seeds > 0) {
        all_held = run_extra_seeds(bin, plan, compiled, &shown, format, opts.extra_seeds);
    }
    RunOutcome { all_held, held }
}

/// `--claim-seeds N`: every sampled claim again, on N seeds the build never
/// uses. A claim checked in full has nothing left to find, and the runtime
/// skips it.
fn run_extra_seeds(
    bin: &Path,
    plan: &Plan,
    compiled: &Compiled,
    shown: &dyn Fn(&ClaimPlan) -> String,
    format: Format,
    n: u64,
) -> bool {
    for k in 1..=n {
        let env = [("RASK_CLAIM_SEED_OFFSET", k.to_string())];
        let (records, stderr, status) = run_bin(bin, &env);
        for (i, claim) in plan.claims.iter().enumerate() {
            if !records.iter().any(|r| r.done && r.claim == claim.name) {
                report_death(claim, &stderr, &status);
                return false;
            }
            if let Some(fail) = records.iter().find(|r| !r.done && r.claim == claim.name && r.result == "fail") {
                let (input, error) = shrink(bin, i, fail, &env);
                let found = format!("found on extra seed {k} of {n} (`--claim-seeds`); a plain build doesn't draw it");
                report_false(claim, &input, &error, &shown(claim), compiled, Some(found));
                return false;
            }
        }
    }
    if format == Format::Human {
        println!("  {} more: every sampled claim held", count(n, "seed"));
    }
    true
}

fn count(n: u64, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{} {what}s", thousands(n)) }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The verdict line for a claim that held (type.claims/V2, V3).
fn verdict(r: &Record) -> String {
    let held = r.tried - r.discarded - r.over_budget;
    let mut line = if r.mode == "all" {
        format!("checked all {}", count(r.distinct, "input"))
    } else {
        format!("held for {} (sampled, not proven)", count(held, "input"))
    };
    if r.discarded > 0 {
        line.push_str(&format!(", {} discarded", r.discarded));
    }
    if r.scheduled {
        line.push_str(", schedules sampled too");
    }
    line
}

/// Cut a failing input down: the smallest choices that still fail. Returns
/// how that input prints and what it failed with.
fn shrink(bin: &Path, claim: usize, fail: &Record, env: &[(&str, String)]) -> (String, String) {
    // Built by an exhaustive walk: nothing smaller comes before it.
    if fail.choices.is_empty() {
        return (fail.input.clone(), fail.error.clone());
    }
    let mut best = Best { bytes: unhex(&fail.choices), input: fail.input.clone(), error: fail.error.clone() };
    let path = std::env::temp_dir().join(format!("rask_claims_shrink_{}", process::id()));
    for _round in 0..200 {
        let candidates = candidates(&best.bytes);
        if candidates.is_empty() {
            break;
        }
        let text: String = candidates.iter().map(|c| hex(c) + "\n").collect();
        if std::fs::write(&path, text).is_err() {
            break;
        }
        let spec = format!("{claim}:{}:{}", fail.index, path.display());
        let mut with = env.to_vec();
        with.push(("RASK_CLAIM_SHRINK", spec));
        let (records, _, _) = run_bin(bin, &with);
        let smaller = records
            .iter()
            .filter(|r| r.result == "fail")
            .map(|r| (unhex(&r.choices), r))
            .filter(|(bytes, _)| smaller_than(bytes, &best.bytes))
            .min_by(|(a, _), (b, _)| order(a, b));
        match smaller {
            Some((bytes, r)) => {
                if std::env::var_os("RASK_CLAIMS_DUMP").is_some() {
                    eprintln!("shrink: {} -> {} ({})", hex(&best.bytes), hex(&bytes), r.input);
                }
                best = Best { bytes, input: r.input.clone(), error: r.error.clone() };
            }
            None => break,
        }
    }
    let _ = std::fs::remove_file(&path);
    (best.input, best.error)
}

struct Best {
    bytes: Vec<u8>,
    input: String,
    error: String,
}

/// Shorter first, then lower. The runner reports every byte an input read,
/// zeros past the end included, so a shorter string read less: fewer list
/// elements, fewer branches taken.
fn order(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn smaller_than(a: &[u8], b: &[u8]) -> bool {
    order(a, b).is_lt()
}

/// Byte strings a step smaller than `b`: a run deleted, a byte zeroed,
/// halved or lowered by one. Bytes past the end read as zero, so all of them
/// make the input smaller or leave it alone.
fn candidates(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for size in [8usize, 4, 2, 1] {
        let mut i = 0;
        while i + size <= b.len() {
            let mut c = b[..i].to_vec();
            c.extend_from_slice(&b[i + size..]);
            out.push(c);
            i += size;
        }
    }
    for i in 0..b.len() {
        if b[i] != 0 {
            for v in [0u8, b[i] / 2, b[i] - 1] {
                if v < b[i] {
                    let mut c = b.to_vec();
                    c[i] = v;
                    out.push(c);
                }
            }
        }
    }
    // A wide integer spans several bytes, and lowering one byte at a time
    // can't get from 0x9d00 to 0x9c41. Lowering the run as one number by each
    // power of two can, in as many rounds as the number has bits.
    for width in [2usize, 4, 8] {
        for i in 0..b.len().saturating_sub(width - 1) {
            let v = b[i..i + width].iter().fold(0u64, |acc, x| (acc << 8) | *x as u64);
            for k in 0..(width * 8) as u32 {
                let Some(lower) = v.checked_sub(1u64 << k) else { break };
                let mut c = b.to_vec();
                for (j, byte) in lower.to_be_bytes()[8 - width..].iter().enumerate() {
                    c[i + j] = *byte;
                }
                out.push(c);
            }
        }
    }
    out.sort();
    out.dedup();
    out.sort_by(|x, y| order(x, y));
    out.truncate(6000);
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

/// E0950, with the counterexample.
fn report_false(claim: &ClaimPlan, input: &str, error: &str, shown: &str, compiled: &Compiled, found: Option<String>) {
    let span = Span { file_id: compiled.claims_files[claim.file], ..claim.span };
    let mut diag = rask_diagnostics::Diagnostic::error(format!("claim `{}` is false", claim.name))
        .with_code("E0950")
        .with_primary(span, "doesn't hold for this input")
        .with_note(format!("counterexample: {input}"))
        .with_note(error.to_string());
    if let Some(found) = found {
        diag = diag.with_note(found);
    }
    let help = if shown == LAWS_FILE {
        "this is the stdlib's law for the interface, and the type's own conformance breaks it: \
         fix its `implements` block"
            .to_string()
    } else {
        format!("the claim is in {shown}; if it's the claim that's wrong, change it there, in review")
    };
    let diag = diag.with_help(help);
    crate::show_diagnostic_multi(&diag, &compiled.sources);
}

/// E0953: under `--claims=strict`, a claim that held but wasn't checked in full.
fn report_not_strict(claim: &ClaimPlan, r: &Record, compiled: &Compiled) {
    let span = Span { file_id: compiled.claims_files[claim.file], ..claim.span };
    let why = if r.over_budget > 0 {
        format!("{} ran past the step budget, so it isn't known to hold for those", count(r.over_budget, "input"))
    } else if r.scheduled {
        "it meets another task, the clock or simulated I/O, so its schedules can only be sampled".to_string()
    } else {
        "its inputs are too many to walk, so they were sampled".to_string()
    };
    let diag = rask_diagnostics::Diagnostic::error(format!("claim `{}` held, but wasn't checked in full", claim.name))
        .with_code("E0953")
        .with_primary(span, verdict(r))
        .with_note(why)
        .with_help(
            "`--claims=strict` passes only claims checked for every input; narrow the inputs \
             (a `u8` instead of a `u32`, an enum instead of a string) or build without `strict`",
        );
    crate::show_diagnostic_multi(&diag, &compiled.sources);
}

/// The binary ended before it reported on `claim`: a deadlock under sim, or
/// a crash. Not shrinkable; reported with what it said.
fn report_death(claim: &ClaimPlan, stderr: &str, status: &process::ExitStatus) {
    eprintln!(
        "{}: the claims process {} while checking `{}`",
        output::error_label(),
        super::run::death_description(status),
        claim.name,
    );
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim_end());
    }
}

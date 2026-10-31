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
    let claims_path = root.join(rask_resolve::package::CLAIMS_FILE);
    let Ok(claims_src) = std::fs::read_to_string(&claims_path) else {
        return ClaimsOutcome::NoClaims;
    };
    let plan = match plan_claims(&claims_src) {
        Ok(plan) => plan,
        Err(diags) => {
            let sources = vec![(claims_path.clone(), claims_src.clone())];
            for d in &diags {
                crate::show_diagnostic_multi(d, &sources);
            }
            return ClaimsOutcome::Failed;
        }
    };
    let Some(compiled) = compile_claims(path, &claims_path, &claims_src, &plan) else {
        return ClaimsOutcome::Failed;
    };
    if plan.claims.is_empty() || mode == ClaimsMode::Check {
        return ClaimsOutcome::Held;
    }
    let Some(bin) = link_claims(&compiled, &plan) else {
        return ClaimsOutcome::Failed;
    };
    let ok = run_claims(&bin, &plan, &compiled, &claims_path, format);
    // Same switch as a test binary's: a claims process that dies takes the
    // evidence with it.
    if std::env::var_os("RASK_KEEP_TEST_BIN").is_some() {
        eprintln!("{}: claims binary kept at {}", output::warning_label(), bin.display());
    } else {
        let _ = std::fs::remove_file(&bin);
    }
    if ok { ClaimsOutcome::Held } else { ClaimsOutcome::Failed }
}

// ─── Planning: the case functions ───────────────────────────

/// One claim, as `claims.rk` declares it.
struct ClaimPlan {
    name: String,
    /// Within `claims.rk`; the file id is whatever the compile gives the file.
    span: Span,
    params: Vec<(String, Span)>,
    seed: u64,
    budget: i64,
}

struct Plan {
    claims: Vec<ClaimPlan>,
    /// The generated source: the end-of-body marker and a case function per claim.
    runner: String,
    /// For each line of `runner`, the claim and parameter it builds an input
    /// for, so an error there can point at the parameter instead.
    input_lines: Vec<Option<(usize, usize)>>,
}

fn case_fn(i: usize) -> String {
    format!("__claim_case_{i}")
}

/// The claims `claims.rk` declares and the code that calls them. A file that
/// doesn't parse plans nothing: the compile reports why.
fn plan_claims(claims_src: &str) -> Result<Plan, Vec<rask_diagnostics::Diagnostic>> {
    let mut lexer = rask_lexer::Lexer::new_with_file_id(claims_src, 0);
    let tokens = lexer.tokenize().tokens;
    let decls = rask_parser::Parser::new_with_file_id(tokens, 0, 0).parse().decls;

    let mut claims = Vec::new();
    let mut errors = Vec::new();
    let mut runner = format!("import claims\n\nfunc {}() {{\n    claims.reached_end()\n}}\n\n", rask_desugar::CLAIM_END_MARKER);
    let mut input_lines: Vec<Option<(usize, usize)>> = vec![None; runner.lines().count()];
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
        let i = claims.len();
        runner.push_str(&format!("func {}() {{\n    {}(\n", case_fn(i), f.name));
        input_lines.extend([None, None]);
        for (k, p) in f.params.iter().enumerate() {
            let sep = if k + 1 == f.params.len() { "" } else { "," };
            runner.push_str(&format!("        claims.input(\"{}\"){sep}\n", p.name));
            input_lines.push(Some((i, k)));
        }
        runner.push_str("    )\n}\n\n");
        input_lines.extend([None, None, None]);
        let text = claims_src.get(f.span.start..f.span.end).unwrap_or(&f.name);
        claims.push(ClaimPlan {
            name: f.name.clone(),
            span: f.span,
            params: f.params.iter().map(|p| (p.name.clone(), p.name_span)).collect(),
            seed: fnv1a(text),
            budget: claim_budget(f).unwrap_or(DEFAULT_BUDGET),
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // Monomorphization keeps what `main` reaches; the claim runner replaces it.
    runner.push_str("func main() {\n");
    for i in 0..claims.len() {
        runner.push_str(&format!("    {}()\n", case_fn(i)));
    }
    runner.push_str("}\n");
    if std::env::var_os("RASK_CLAIMS_DUMP").is_some() {
        eprintln!("{runner}");
    }
    Ok(Plan { claims, runner, input_lines })
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
    /// The id the compile gave `claims.rk`.
    claims_file: u16,
}

/// Compile `claims.rk` and the plan's case functions as a package that
/// imports the package at `path`.
fn compile_claims(path: &str, claims_path: &Path, claims_src: &str, plan: &Plan) -> Option<Compiled> {
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
    // Seen from the claims, the package is a dependency like any other: its
    // names are its own, and only what it made `public` is reachable.
    if let Some(root_pkg) = prepared.registry.get_mut(root_id) {
        root_pkg.is_external = true;
    }
    let claims_id = match prepared.registry.add_source_package(
        &format!("{root_name}_claims"),
        vec![
            (claims_path.to_path_buf(), claims_src.to_string()),
            (PathBuf::from(RUNNER_FILE), plan.runner.clone()),
        ],
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
    let (mut claims_file, mut runner_file) = (0u16, u16::MAX);
    for pkg in prepared.registry.packages() {
        for f in &pkg.files {
            let slot = f.file_id as usize;
            if sources.len() <= slot {
                sources.resize(slot + 1, (PathBuf::new(), String::new()));
            }
            sources[slot] = (f.path.clone(), f.source.clone());
            if pkg.id == claims_id && f.path == claims_path {
                claims_file = f.file_id;
            }
            if pkg.id == claims_id && f.path == Path::new(RUNNER_FILE) {
                runner_file = f.file_id;
            }
        }
    }
    let all_decls: Vec<Decl> = prepared.registry.get(claims_id)?.all_decls().cloned().collect();
    let cfg = rask_comptime::CfgConfig::from_host("debug", prepared.resolved_feature_names.clone());
    let config = crate::compiler_config(cfg).with_claims();
    let mut pkg_ctx = rask_compiler::PackageContext {
        registry: prepared.registry,
        root_id: claims_id,
        all_decls,
    };
    let output = rask_compiler::compile_package_with(&mut pkg_ctx, &config, |_, _| {});
    for diag in &output.diagnostics {
        let diag = point_at_input(diag, plan, runner_file, claims_file);
        crate::show_diagnostic_multi(&diag, &sources);
    }
    let result = output.result?;
    Some(Compiled { result, link_opts: prepared.link_opts, sources, claims_file })
}

/// An error inside a generated case function is about one of the claim's
/// inputs: say which, at the parameter that asked for it.
fn point_at_input(
    diag: &rask_diagnostics::Diagnostic,
    plan: &Plan,
    runner_file: u16,
    claims_file: u16,
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
            label.span = Span { file_id: claims_file, ..*span };
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

/// Build and link the claim binary under sim. Returns its path.
fn link_claims(compiled: &Compiled, plan: &Plan) -> Option<PathBuf> {
    static NEXT_BIN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT_BIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let bin_path = std::env::temp_dir().join(format!("rask_claims_{}_{n}", process::id()));
    let bin_str = bin_path.to_string_lossy().to_string();
    let obj_path = format!("{bin_str}.o");

    let runs: Vec<rask_codegen::ClaimRun> = plan
        .claims
        .iter()
        .enumerate()
        .map(|(i, c)| rask_codegen::ClaimRun {
            name: c.name.clone(),
            fn_name: case_fn(i),
            seed: c.seed,
            budget: c.budget,
        })
        .collect();
    let input = rask_compiler::emit::BackEnd::of(&compiled.result, None, None);
    if let Err(errors) = super::compile::emit_object(
        &input,
        &rask_compiler::emit::Entry::Claims(&runs),
        None,
        rask_compiler::emit::BuildMode::Debug,
        &obj_path,
    ) {
        for e in &errors {
            eprintln!("{}: compile claims: {}", output::error_label(), e);
        }
        let _ = std::fs::remove_file(&obj_path);
        return None;
    }
    let link_opts = super::link::LinkOptions {
        sim: true,
        libs: compiled.link_opts.libs.clone(),
        objects: compiled.link_opts.objects.clone(),
        search_paths: compiled.link_opts.search_paths.clone(),
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

fn run_claims(bin: &Path, plan: &Plan, compiled: &Compiled, claims_path: &Path, format: Format) -> bool {
    let opts = options();
    let (records, stderr, status) = run_bin(bin, &[]);
    let shown = claims_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if format == Format::Human {
        println!("{} {}", "Claims".green().bold(), claims_path.display());
    }
    let mut all_held = true;
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
            let (input, error) = shrink(bin, i, fail, &[]);
            report_false(claim, &input, &error, &shown, compiled, None);
            continue;
        }
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
    if all_held && opts.extra_seeds > 0 {
        all_held = run_extra_seeds(bin, plan, compiled, &shown, format, opts.extra_seeds);
    }
    all_held
}

/// `--claim-seeds N`: every sampled claim again, on N seeds the build never
/// uses. A claim checked in full has nothing left to find, and the runtime
/// skips it.
fn run_extra_seeds(bin: &Path, plan: &Plan, compiled: &Compiled, shown: &str, format: Format, n: u64) -> bool {
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
                report_false(claim, &input, &error, shown, compiled, Some(found));
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
    let span = Span { file_id: compiled.claims_file, ..claim.span };
    let mut diag = rask_diagnostics::Diagnostic::error(format!("claim `{}` is false", claim.name))
        .with_code("E0950")
        .with_primary(span, "doesn't hold for this input")
        .with_note(format!("counterexample: {input}"))
        .with_note(error.to_string());
    if let Some(found) = found {
        diag = diag.with_note(found);
    }
    let diag = diag.with_help(format!("the claim is in {shown}; if it's the claim that's wrong, change it there, in review"));
    crate::show_diagnostic_multi(&diag, &compiled.sources);
}

/// E0953: under `--claims=strict`, a claim that held but wasn't checked in full.
fn report_not_strict(claim: &ClaimPlan, r: &Record, compiled: &Compiled) {
    let span = Span { file_id: compiled.claims_file, ..claim.span };
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

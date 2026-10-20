// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask stdlib-coverage`: each stdlib spec's surface against what exists and runs.
//!
//! A function a spec promises is covered when `stdlib/*.rk` declares it public
//! with the same signature, it has a body (not `@unimplemented`), and a suite
//! program that agrees on both backends calls it. Anything short of that is a
//! gap, printed one per line so `tests/stdlib_coverage_gate.sh` can hold the
//! list to a registry.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use colored::Colorize;
use rask_ast::expr::ExprKind;
use rask_ast::ty::TypeExpr;
use rask_spec_test::surface::{self, FnKey, SpecSurface, SurfaceFn, SurfaceType};
use rask_types::{receiver_name, Callee};

use crate::output;

/// Why a promised item isn't covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Gap {
    /// The spec states no surface at all.
    NoSurface,
    /// A surface block that doesn't parse.
    BadSurface,
    Missing,
    /// Declared without `public`, so no program can call it.
    Private,
    /// Declared `@unimplemented`.
    Unbuilt,
    /// Declared with a different signature.
    Mismatch,
    /// An enum whose variants differ from the spec's.
    Variants,
    /// Declared and built, and nothing in the suite calls it.
    Untested,
}

impl Gap {
    fn label(self) -> &'static str {
        match self {
            Gap::NoSurface => "NO-SURFACE",
            Gap::BadSurface => "BAD-SURFACE",
            Gap::Missing => "MISSING",
            Gap::Private => "PRIVATE",
            Gap::Unbuilt => "UNBUILT",
            Gap::Mismatch => "MISMATCH",
            Gap::Variants => "VARIANTS",
            Gap::Untested => "UNTESTED",
        }
    }
}

struct Finding {
    gap: Gap,
    item: String,
    detail: String,
}

struct ModuleReport {
    id: String,
    path: PathBuf,
    promised: usize,
    covered: usize,
    findings: Vec<Finding>,
    /// Public stdlib functions on a type the spec covers that the spec never
    /// mentions. Reported, not counted: the spec may be the one that's behind.
    extras: Vec<String>,
}

pub struct CoverageOpts<'a> {
    pub specs_dir: &'a str,
    pub suite_dir: &'a str,
    /// Print only the gap lines, for the gate.
    pub gaps_only: bool,
}

pub fn cmd_stdlib_coverage(opts: CoverageOpts) {
    let stdlib = stdlib_surface();
    let specs = read_specs(Path::new(opts.specs_dir));
    let exercised = exercised_calls(Path::new(opts.suite_dir));

    let reports: Vec<ModuleReport> = specs.iter().map(|(path, spec)| judge(path, spec, &stdlib, &exercised)).collect();

    if opts.gaps_only {
        for r in &reports {
            for f in &r.findings {
                println!("{} {} {}", r.id, f.item, f.gap.label());
            }
        }
        println!("# {}", summary(&reports));
        return;
    }
    print_report(&reports);
}

// ── The stdlib's side ──────────────────────────────────────────────────

struct StdlibSurface {
    fns: HashMap<FnKey, (SurfaceFn, &'static str)>,
    types: HashMap<String, (SurfaceType, &'static str)>,
}

fn stdlib_surface() -> StdlibSurface {
    let mut all: Vec<(SurfaceFn, &'static str)> = Vec::new();
    let mut conformances = Vec::new();
    let mut types = HashMap::new();
    for (file, source, _) in rask_stdlib::stubs::stub_sources() {
        let s = surface::stdlib_source_surface(source, |decls| {
            let mut next_id = u32::MAX / 2;
            rask_stdlib::stubs::expand_width_families(decls, &mut next_id);
        })
        .unwrap_or_else(|e| {
            eprintln!("{}: stdlib/{} doesn't parse: {}", output::error_label(), file, e);
            process::exit(2);
        });
        all.extend(s.fns.into_iter().map(|f| (f, file)));
        conformances.extend(s.conformances.into_iter().map(|c| (c, file)));
        for t in s.types {
            types.insert(t.name.clone(), (t, file));
        }
    }
    // An inherited default is reported against the file of the conformance
    // that inherits it.
    let mut fns: Vec<SurfaceFn> = all.iter().map(|(f, _)| f.clone()).collect();
    let pairs: Vec<(String, String)> = conformances.iter().map(|(c, _)| c.clone()).collect();
    let before = fns.len();
    surface::inherit_defaults(&mut fns, &pairs);
    for f in fns.drain(before..) {
        let file = conformances
            .iter()
            .find(|((o, i), _)| *o == f.owner && Some(i) == f.via.as_ref())
            .map(|(_, file)| *file)
            .unwrap_or("");
        all.push((f, file));
    }
    let fns = all.into_iter().map(|(f, file)| (f.key(), (f, file))).collect();
    StdlibSurface { fns, types }
}

// ── The specs' side ────────────────────────────────────────────────────

fn read_specs(dir: &Path) -> Vec<(PathBuf, SpecSurface)> {
    let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "md")).collect(),
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), dir.display(), e);
            process::exit(2);
        }
    };
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(&path) else { continue };
        // A stdlib module spec carries a `std.` id. The README and the API
        // design rules are about the stdlib, not modules of it.
        if !text.lines().any(|l| l.trim().starts_with("<!-- id: std.")) || text.contains("<!-- id: std.api -->") {
            continue;
        }
        out.push((path.clone(), surface::spec_surface(&path, &text)));
    }
    out
}

fn judge(
    path: &Path,
    spec: &SpecSurface,
    stdlib: &StdlibSurface,
    exercised: &Exercised,
) -> ModuleReport {
    let mut r = ModuleReport {
        id: spec.id.clone(),
        path: path.to_path_buf(),
        promised: 0,
        covered: 0,
        findings: Vec::new(),
        extras: Vec::new(),
    };
    let at = |line: usize| format!("{}:{}", path.display(), line);

    if spec.blocks == 0 {
        r.findings.push(Finding { gap: Gap::NoSurface, item: "-".into(), detail: "no `<!-- surface -->` block".into() });
        return r;
    }
    for (line, msg) in &spec.errors {
        r.findings.push(Finding { gap: Gap::BadSurface, item: format!("line{line}"), detail: format!("{}: {msg}", at(*line)) });
    }

    for want in &spec.surface.fns {
        r.promised += 1;
        let gap = match stdlib.fns.get(&want.key()) {
            None => Some((Gap::Missing, format!("{}  {}", want.signature(), at(want.line)))),
            Some((have, file)) => {
                let here = format!("stdlib/{}:{}", file, have.line);
                if !have.matches(want) {
                    Some((Gap::Mismatch, format!("spec: {}  stdlib: {}  {}", want.signature(), have.signature(), here)))
                } else if !have.is_pub {
                    Some((Gap::Private, here))
                } else if have.unimplemented {
                    Some((Gap::Unbuilt, here))
                } else if !have.required && !exercised.reaches(have) {
                    Some((Gap::Untested, here))
                } else {
                    None
                }
            }
        };
        match gap {
            Some((gap, detail)) => r.findings.push(Finding { gap, item: want.qualified(), detail }),
            None => r.covered += 1,
        }
    }

    for want in &spec.surface.types {
        r.promised += 1;
        let gap = match stdlib.types.get(&want.name) {
            None => Some((Gap::Missing, format!("{} {}  {}", want.kind, want.name, at(want.line)))),
            Some((have, file)) => {
                let here = format!("stdlib/{}:{}", file, have.line);
                if !have.is_pub {
                    Some((Gap::Private, here))
                } else if have.variants != want.variants {
                    Some((Gap::Variants, format!("spec: {:?}  stdlib: {:?}  {}", want.variants, have.variants, here)))
                } else {
                    None
                }
            }
        };
        match gap {
            Some((gap, detail)) => r.findings.push(Finding { gap, item: want.name.clone(), detail }),
            None => r.covered += 1,
        }
    }

    // Only types the spec itself declares, and its own module: a spec that
    // adds one method to `Vec` isn't claiming to describe the rest of `Vec`.
    let module = spec.id.strip_prefix("std.").unwrap_or(&spec.id);
    let owners: HashSet<&str> =
        spec.surface.types.iter().map(|t| t.name.as_str()).chain(std::iter::once(module)).collect();
    let promised: HashSet<FnKey> = spec.surface.fns.iter().map(SurfaceFn::key).collect();
    for (key, (have, _)) in &stdlib.fns {
        if have.is_pub && owners.contains(key.owner.as_str()) && !promised.contains(key) {
            r.extras.push(have.qualified());
        }
    }
    r.extras.sort();
    r.findings.sort_by(|a, b| (a.gap, &a.item).cmp(&(b.gap, &b.item)));
    r
}

// ── What the suite calls ───────────────────────────────────────────────

/// The registries `tests/differential.sh` reads. A file in either is red on
/// some backend, so what it calls isn't known to run on both.
const RED_REGISTRIES: &[&str] = &["known_divergences.txt", "pending_features.txt"];

/// What green suite programs call.
#[derive(Default)]
struct Exercised {
    /// Ordinary calls, as (owner, name). Owner is empty for a free function.
    calls: HashSet<(String, String)>,
    /// Operators, as (owner, method, interface, right operand's type name).
    operators: HashSet<(String, String, String, Option<String>)>,
}

impl Exercised {
    fn reaches(&self, f: &SurfaceFn) -> bool {
        if self.calls.contains(&(f.owner.clone(), f.name.clone())) {
            return true;
        }
        let Some(iface) = &f.via else { return false };
        let rhs = match f.params.first() {
            Some((_, TypeExpr::Named { path, .. })) => path.last().cloned(),
            _ => None,
        };
        self.operators.contains(&(f.owner.clone(), f.name.clone(), iface.clone(), rhs))
    }
}

fn exercised_calls(suite: &Path) -> Exercised {
    let red: HashSet<String> = RED_REGISTRIES
        .iter()
        .filter_map(|f| fs::read_to_string(suite.parent().unwrap_or(suite).join(f)).ok())
        .flat_map(|text| {
            text.lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .filter_map(|l| l.split_whitespace().next().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut files: Vec<PathBuf> = match fs::read_dir(suite) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "rk"))
            .filter(|p| !p.file_name().is_some_and(|n| red.contains(n.to_string_lossy().as_ref())))
            .collect(),
        Err(e) => {
            eprintln!("{}: reading {}: {}", output::error_label(), suite.display(), e);
            process::exit(2);
        }
    };
    files.sort();

    let next = AtomicUsize::new(0);
    let found = Mutex::new(Exercised::default());
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            std::thread::Builder::new()
                // The checker recurses with the program; the default 2 MiB
                // thread stack is smaller than the main thread's.
                .stack_size(64 << 20)
                .spawn_scoped(scope, || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let one = calls_in(path);
                    let mut all = found.lock().unwrap();
                    all.calls.extend(one.calls);
                    all.operators.extend(one.operators);
                })
                .expect("spawn coverage worker");
        }
    });
    found.into_inner().unwrap()
}

/// The stdlib calls one program makes from its own code. A call inside a
/// stdlib body doesn't count: it shows the stdlib using itself, not a program
/// reaching it.
fn calls_in(path: &Path) -> Exercised {
    let config = rask_compiler::CompilerConfig { cfg: rask_compiler::CfgConfig::from_host("debug", vec![]) };
    let Some(checked) = rask_compiler::check_file(&path.to_string_lossy(), &config).result else {
        return Exercised::default();
    };
    let typed = &checked.typed;
    let mut out = Exercised::default();
    for decl in checked.decls.iter().filter(|d| d.span.file_id < rask_stdlib::stubs::STDLIB_FILE_ID_BASE) {
        rask_ast::visit::walk_decl(decl, &mut |e| {
            if !matches!(e.kind, ExprKind::Call { .. } | ExprKind::MethodCall { .. }) {
                return;
            }
            if let Some(op) = typed.operator_targets.get(&e.id) {
                if let (Some(owner), TypeExpr::Named { path, .. }) = (receiver_name(&op.recv, &typed.types), &op.applied) {
                    let iface = path.last().cloned().unwrap_or_default();
                    out.operators.insert((owner, op.operator.clone(), iface, op.rhs.clone()));
                }
                return;
            }
            match typed.call_targets.get(&e.id) {
                Some(Callee::Method { recv, method, .. }) => {
                    if let Some(owner) = receiver_name(recv, &typed.types) {
                        out.calls.insert((owner, method.clone()));
                    }
                }
                Some(Callee::Free(sym)) => {
                    if let Some(s) = typed.symbols.get(*sym) {
                        out.calls.insert((String::new(), s.name.clone()));
                    }
                }
                None => {}
            }
        });
    }
    out
}

// ── Output ─────────────────────────────────────────────────────────────

fn summary(reports: &[ModuleReport]) -> String {
    let promised: usize = reports.iter().map(|r| r.promised).sum();
    let covered: usize = reports.iter().map(|r| r.covered).sum();
    let full = reports.iter().filter(|r| r.findings.is_empty()).count();
    let measured = reports.iter().filter(|r| !r.findings.iter().any(|f| f.gap == Gap::NoSurface)).count();
    let mut by_gap: BTreeMap<Gap, usize> = BTreeMap::new();
    for f in reports.iter().flat_map(|r| &r.findings) {
        *by_gap.entry(f.gap).or_default() += 1;
    }
    let breakdown: Vec<String> = by_gap.iter().map(|(g, n)| format!("{} {}", n, g.label())).collect();
    format!(
        "{} of {} modules at 100% ({} state a surface). {} of {} promised items covered. Gaps: {}",
        full,
        reports.len(),
        measured,
        covered,
        promised,
        if breakdown.is_empty() { "none".to_string() } else { breakdown.join(", ") }
    )
}

fn print_report(reports: &[ModuleReport]) {
    for r in reports {
        let no_surface = r.findings.iter().any(|f| f.gap == Gap::NoSurface);
        let head = if no_surface {
            format!("{:<14} {}", r.id, "no surface stated".dimmed())
        } else {
            let pct = if r.promised == 0 { 100 } else { r.covered * 100 / r.promised };
            let line = format!("{:<14} {:>3}%  {}/{}", r.id, pct, r.covered, r.promised);
            if r.findings.is_empty() { line.green().to_string() } else { line }
        };
        println!("{head}  {}", output::file_path(&r.path.display().to_string()));
        for f in r.findings.iter().filter(|f| f.gap != Gap::NoSurface) {
            println!("  {:<11} {:<32} {}", f.gap.label().yellow(), f.item, f.detail.dimmed());
        }
        if !r.extras.is_empty() {
            println!("  {:<11} {}", "unspecced".dimmed(), r.extras.join(", ").dimmed());
        }
    }
    println!();
    println!("{}", summary(reports));
}

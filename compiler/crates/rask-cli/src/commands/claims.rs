// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Claims (type.claims): a package's `claims.rk`, checked by the build.
//!
//! Two passes over the package. The first compiles `claims.rk` as a package
//! of its own that imports the one it checks, which is what limits it to that
//! package's `public` items, and reads each claim's parameter types off the
//! checker. Those types say what the input code looks like, so the second pass
//! compiles the claims again with that code beside them and links the result
//! under sim. One process runs every claim; a counterexample is cut down by
//! re-running it on smaller choices before it's reported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process;

use colored::Colorize;
use rask_ast::decl::{Decl, DeclKind, FieldVisibility};
use rask_check::TypedProgram;
use rask_types::{GenericArg, Type, TypeDef};

use crate::output;
use crate::Format;

/// How many index values an exhaustive walk may take before a claim is
/// sampled instead (type.claims/V2).
const MAX_EXHAUSTIVE: u64 = 65_536;

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

/// Check the claims of the package at `path`, if it has any.
pub fn check_package_claims(path: &str, format: Format, mode: ClaimsMode) -> ClaimsOutcome {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let claims_path = root.join(rask_resolve::package::CLAIMS_FILE);
    let Ok(claims_src) = std::fs::read_to_string(&claims_path) else {
        return ClaimsOutcome::NoClaims;
    };

    // Pass 1: what the claims take.
    let probe_runner = format!("{}\nfunc main() {{}}\n", runner_prelude(""));
    let Some(probe) = compile_claims(path, &claims_path, &claims_src, &probe_runner, |decls, typed, ctx| {
        plan_claims(decls, typed, ctx, &claims_src)
    }) else {
        return ClaimsOutcome::Failed;
    };
    let (plan, _) = probe;
    let plan = match plan {
        Ok(plan) => plan,
        Err(errors) => {
            for e in errors {
                eprintln!("{}: {}", output::error_label(), e);
            }
            return ClaimsOutcome::Failed;
        }
    };
    if plan.claims.is_empty() || mode == ClaimsMode::Check {
        return ClaimsOutcome::Held;
    }

    // Pass 2: the claims with their input code, as a runnable binary.
    let Some(((), compiled)) = compile_claims(path, &claims_path, &claims_src, &plan.runner, |_, _, _| ()) else {
        return ClaimsOutcome::Failed;
    };
    let Some(bin) = link_claims(compiled, &plan) else {
        return ClaimsOutcome::Failed;
    };
    let ok = run_claims(&bin, &plan, &claims_path, format);
    // Same switch as a test binary's: a claims process that dies takes the
    // evidence with it.
    if std::env::var_os("RASK_KEEP_TEST_BIN").is_some() {
        eprintln!("{}: claims binary kept at {}", output::warning_label(), bin.display());
    } else {
        let _ = std::fs::remove_file(&bin);
    }
    if ok { ClaimsOutcome::Held } else { ClaimsOutcome::Failed }
}

// ─── Compiling ──────────────────────────────────────────────

/// What a hook over the claims package's checked program is told about the
/// package being checked.
struct PackageNames {
    /// The checked package's name, as `claims.rk` imports it.
    root: String,
    /// The name each of its declarations has once merged, back to the name
    /// it was declared with.
    declared: HashMap<String, String>,
}

/// The claims package's file of generated code.
const RUNNER_FILE: &str = "<claims runner>";

/// The alias the generated code imports the checked package under, so it
/// can't collide with however `claims.rk` imports it.
const PKG_ALIAS: &str = "__claims_pkg";

struct Compiled {
    result: rask_compiler::CompileResult,
    link_opts: super::link::LinkOptions,
}

/// Compile `claims.rk` plus `runner` as a package that imports the package at
/// `path`. `hook` sees the checked program before monomorphization.
fn compile_claims<T>(
    path: &str,
    claims_path: &Path,
    claims_src: &str,
    runner: &str,
    hook: impl FnOnce(&[Decl], &TypedProgram, &PackageNames) -> T,
) -> Option<(T, Compiled)> {
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
    let (root_name, declared) = {
        let root_pkg = prepared.registry.get(root_id)?;
        let declared = root_pkg
            .all_decls()
            .filter_map(|d| d.name())
            .map(|n| (rask_compiler::package_scope::qualified(&root_pkg.name, n), n.to_string()))
            .collect();
        (root_pkg.name.clone(), declared)
    };
    // Seen from the claims, the package is a dependency like any other: its
    // names are its own, and only what it made `public` is reachable.
    if let Some(root_pkg) = prepared.registry.get_mut(root_id) {
        root_pkg.is_external = true;
    }
    let claims_id = match prepared.registry.add_source_package(
        &format!("{root_name}_claims"),
        vec![
            (claims_path.to_path_buf(), claims_src.to_string()),
            (PathBuf::from(RUNNER_FILE), runner.to_string()),
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

    let mut source_files: Vec<(PathBuf, String)> = Vec::new();
    for pkg in prepared.registry.packages() {
        for f in &pkg.files {
            let slot = f.file_id as usize;
            if source_files.len() <= slot {
                source_files.resize(slot + 1, (PathBuf::new(), String::new()));
            }
            source_files[slot] = (f.path.clone(), f.source.clone());
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
    let names = PackageNames { root: root_name, declared };
    let mut answer = None;
    let output = rask_compiler::compile_package_with(&mut pkg_ctx, &config, |decls, typed| {
        answer = Some(hook(decls, typed, &names));
    });
    let sources = if output.source_files.is_empty() { source_files } else { output.source_files.clone() };
    for diag in &output.diagnostics {
        crate::show_diagnostic_multi(diag, &sources);
    }
    let result = output.result?;
    Some((answer?, Compiled { result, link_opts: prepared.link_opts }))
}

/// Build and link the claim binary under sim. Returns its path.
fn link_claims(compiled: Compiled, plan: &Plan) -> Option<PathBuf> {
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
            domain: c.walk.map_or(0, |w| w as i64),
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
    let link_opts = super::link::LinkOptions { sim: true, ..compiled.link_opts };
    let linked = super::link::link_executable_with(&obj_path, &bin_str, &link_opts, false, None);
    let _ = std::fs::remove_file(&obj_path);
    if let Err(e) = linked {
        eprintln!("{}: link claims: {}", output::error_label(), e);
        return None;
    }
    Some(bin_path)
}

// ─── Planning: the input code ───────────────────────────────

/// One claim and how its inputs are made.
struct ClaimPlan {
    name: String,
    span: rask_ast::Span,
    seed: u64,
    /// Index values an exhaustive walk takes, when the inputs can be walked.
    walk: Option<u64>,
    /// How many distinct inputs that walk covers.
    distinct: Option<u64>,
    budget: i64,
}

struct Plan {
    claims: Vec<ClaimPlan>,
    /// The generated source: the prelude, a generator per named type, and a
    /// case function per claim.
    runner: String,
}

fn case_fn(i: usize) -> String {
    format!("__claim_case_{i}")
}

/// What generated code the claims need, from the checked program.
fn plan_claims(
    decls: &[Decl],
    typed: &TypedProgram,
    names: &PackageNames,
    claims_src: &str,
) -> Result<Plan, Vec<String>> {
    let mut gen = Generators::new(typed, decls, names);
    let mut claims = Vec::new();
    let mut cases = String::new();
    let mut errors = Vec::new();

    let claim_fns = decls.iter().filter_map(|d| match &d.kind {
        DeclKind::Fn(f) if f.attrs.iter().any(|a| a.is("claim")) => Some(f),
        _ => None,
    });
    for f in claim_fns {
        let mut body = String::new();
        let mut walk: Option<u64> = Some(1);
        let mut distinct: Option<u64> = Some(1);
        let mut args = Vec::new();
        let mut ok = true;
        for (k, p) in f.params.iter().enumerate() {
            let key = (p.name_span.start, p.name_span.end, p.name_span.file_id);
            let Some(ty) = typed.span_types.get(&key) else {
                errors.push(format!("claim `{}`: no type for input `{}`", f.name, p.name));
                ok = false;
                continue;
            };
            match gen.expr(ty) {
                Ok((expr, dom)) => {
                    let binding = if p.is_mutate { "mut" } else { "let" };
                    body.push_str(&format!("    {binding} __arg{k} = {expr}\n"));
                    args.push(format!("__arg{k}"));
                    walk = walk.zip(dom.walk).and_then(|(a, b)| a.checked_mul(b));
                    distinct = distinct.zip(dom.distinct).and_then(|(a, b)| a.checked_mul(b));
                }
                Err(why) => {
                    errors.push(format!("claim `{}`: input `{}` can't be generated: {why}", f.name, p.name));
                    ok = false;
                }
            }
        }
        if !ok {
            continue;
        }
        // The input prints the way any value does, through its derived
        // `Debug`, before the claim can take it apart.
        let shown: Vec<String> = f.params.iter().enumerate().map(|(k, p)| format!("{} = {{__arg{k}:debug}}", p.name)).collect();
        body.push_str(&format!("    __claims.note(\"{}\")\n", shown.join(", ")));
        cases.push_str(&format!(
            "func {}() {{\n{body}    {}({})\n}}\n\n",
            case_fn(claims.len()),
            f.name,
            args.join(", ")
        ));
        let text = claims_src.get(f.span.start..f.span.end).unwrap_or(&f.name);
        let budget = claim_budget(f).unwrap_or(DEFAULT_BUDGET);
        claims.push(ClaimPlan {
            name: f.name.clone(),
            span: f.span,
            seed: fnv1a(text),
            walk: walk.filter(|w| *w <= MAX_EXHAUSTIVE),
            distinct,
            budget,
        });
    }
    errors.extend(gen.errors.drain(..));
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut runner = runner_prelude(&names.root);
    runner.push('\n');
    for g in &gen.functions {
        runner.push_str(g);
        runner.push('\n');
    }
    runner.push_str(&cases);
    // Monomorphization keeps what `main` reaches; the claim runner replaces it.
    runner.push_str("func main() {\n");
    for i in 0..claims.len() {
        runner.push_str(&format!("    {}()\n", case_fn(i)));
    }
    runner.push_str("}\n");
    if std::env::var_os("RASK_CLAIMS_DUMP").is_some() {
        eprintln!("{runner}");
    }
    Ok(Plan { claims, runner })
}

fn claim_budget(f: &rask_ast::decl::FnDecl) -> Option<i64> {
    let attr = f.attrs.iter().find(|a| a.is("claim_budget"))?;
    let arg = attr.args.as_ref()?.first()?;
    match arg.value.expr().map(|e| &e.kind) {
        Some(rask_ast::expr::ExprKind::Int(n, _)) => i64::try_from(*n).ok(),
        _ => None,
    }
}

/// The part of the runner every claims package gets: the end-of-body marker
/// the desugarer calls (type.claims/CL5) and the generic generators.
fn runner_prelude(root: &str) -> String {
    let import_root = if root.is_empty() { String::new() } else { format!("import {root} as {PKG_ALIAS}\n") };
    format!(
        r#"import claims as __claims
import string.StringBuilder
{import_root}
func {end}() {{
    __claims.reached_end()
}}

func __claim_bool() -> bool {{
    let coin: u64 = __claims.draw(2)
    return coin == 1
}}

func __claim_f64() -> f64 {{
    return __claims.float()
}}

func __claim_char() -> char {{
    let pick: u64 = __claims.draw(8)
    mut c = 'a'
    if pick == 1 {{ c = 'b' }}
    if pick == 2 {{ c = 'Z' }}
    if pick == 3 {{ c = '0' }}
    if pick == 4 {{ c = ' ' }}
    if pick == 5 {{ c = '\n' }}
    if pick == 6 {{ c = 'é' }}
    if pick == 7 {{ c = '"' }}
    return c
}}

func __claim_string() -> string {{
    mut s = StringBuilder.new()
    while __claims.draw(4) != 0 {{
        s.push_char(__claim_char())
    }}
    return s.build()
}}

func __claim_vec<T>(elem: func() -> T) -> Vec<T> {{
    mut v: Vec<T> = Vec.new()
    while __claims.draw(4) != 0 {{
        v.push(elem())
    }}
    return v
}}

func __claim_optional<T>(some: func() -> T) -> T? {{
    if __claims.draw(4) == 0 {{
        return none
    }}
    return some()
}}

func __claim_tuple2<A, B>(a: func() -> A, b: func() -> B) -> (A, B) {{
    let x = a()
    return (x, b())
}}

func __claim_tuple3<A, B, C>(a: func() -> A, b: func() -> B, c: func() -> C) -> (A, B, C) {{
    let x = a()
    let y = b()
    return (x, y, c())
}}
{ints}"#,
        end = rask_desugar::CLAIM_END_MARKER,
        ints = int_generators(),
    )
}

/// One generator per integer type: `__claim_u32()` and so on.
fn int_generators() -> String {
    let mut out = String::new();
    for (name, bits, signed) in INT_TYPES {
        out.push_str(&format!(
            "\nfunc __claim_{name}() -> {name} {{\n    return __claims.int({bits}, {}).wrap<{name}>()\n}}\n",
            *signed as i64,
        ));
    }
    out
}

const INT_TYPES: &[(&str, u32, bool)] = &[
    ("i8", 8, true),
    ("i16", 16, true),
    ("i32", 32, true),
    ("i64", 64, true),
    ("u8", 8, false),
    ("u16", 16, false),
    ("u32", 32, false),
    ("u64", 64, false),
];

/// The input space of a type, for an exhaustive walk (type.claims/V2).
#[derive(Clone, Copy)]
struct Domain {
    /// Index values the walk takes; a sum type's branches leave unused digits.
    walk: Option<u64>,
    /// Distinct inputs among them.
    distinct: Option<u64>,
}

impl Domain {
    const INFINITE: Domain = Domain { walk: None, distinct: None };

    fn finite(n: u64) -> Domain {
        Domain { walk: Some(n), distinct: Some(n) }
    }
}

/// Writes the generator code for each type a claim takes.
struct Generators<'a> {
    typed: &'a TypedProgram,
    names: &'a PackageNames,
    /// Field visibility of each struct, by its merged name.
    field_vis: HashMap<String, Vec<FieldVisibility>>,
    /// Generator function for each named type, once written.
    named: HashMap<rask_types::TypeId, (String, Domain)>,
    functions: Vec<String>,
    errors: Vec<String>,
}

impl<'a> Generators<'a> {
    fn new(typed: &'a TypedProgram, decls: &[Decl], names: &'a PackageNames) -> Self {
        let field_vis = decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Struct(s) => Some((s.name.clone(), s.fields.iter().map(|f| f.visibility).collect())),
                _ => None,
            })
            .collect();
        Generators { typed, names, field_vis, named: HashMap::new(), functions: Vec::new(), errors: Vec::new() }
    }

    /// An expression that builds a value of `ty`, and the type's domain.
    fn expr(&mut self, ty: &Type) -> Result<(String, Domain), String> {
        if std::env::var_os("RASK_CLAIMS_DUMP").is_some() {
            eprintln!("claim input type: {ty:?}");
        }
        let int = |name: &str, bits: u32| {
            let dom = if bits <= 8 { Domain::finite(1 << bits) } else { Domain::INFINITE };
            Ok((format!("__claim_{name}()"), dom))
        };
        match ty {
            Type::Bool => Ok(("__claim_bool()".to_string(), Domain::finite(2))),
            Type::I8 => int("i8", 8),
            Type::I16 => int("i16", 16),
            Type::I32 => int("i32", 32),
            Type::I64 => int("i64", 64),
            Type::U8 => int("u8", 8),
            Type::U16 => int("u16", 16),
            Type::U32 => int("u32", 32),
            Type::U64 => int("u64", 64),
            Type::F64 => Ok(("__claim_f64()".to_string(), Domain::INFINITE)),
            Type::Char => Ok(("__claim_char()".to_string(), Domain::INFINITE)),
            Type::String => Ok(("__claim_string()".to_string(), Domain::INFINITE)),
            Type::Tuple(elems) if elems.len() == 2 || elems.len() == 3 => {
                let mut parts = Vec::new();
                let mut dom = Domain::finite(1);
                for e in elems {
                    let (x, d) = self.expr(e)?;
                    parts.push(format!("|| {x}"));
                    dom = product(dom, d);
                }
                Ok((format!("__claim_tuple{}({})", elems.len(), parts.join(", ")), dom))
            }
            Type::Named(id) => self.named(*id),
            // `T?` is `T or none`.
            Type::Result { ok, err } if matches!(**err, Type::None) => {
                let (x, d) = self.expr(ok)?;
                Ok((format!("__claim_optional(|| {x})"), sum(&[Domain::finite(1), d])))
            }
            _ => self.wrapper(ty),
        }
    }

    /// `Vec<T>`: the stdlib's generic collection.
    fn wrapper(&mut self, ty: &Type) -> Result<(String, Domain), String> {
        let unsupported = || Err(format!("`{ty}` isn't a type claims can generate yet"));
        let Type::Generic { base, args } = ty else { return unsupported() };
        let elem = match args.as_slice() {
            [GenericArg::Type(t)] => (**t).clone(),
            _ => return unsupported(),
        };
        if self.typed.types.type_name(*base) != "Vec" {
            return unsupported();
        }
        let (x, _) = self.expr(&elem)?;
        Ok((format!("__claim_vec(|| {x})"), Domain::INFINITE))
    }

    /// How generated code names a user type: through the import for one the
    /// checked package declared, by its own name for one `claims.rk` did.
    fn path(&self, merged: &str) -> String {
        match self.names.declared.get(merged) {
            Some(declared) => format!("{PKG_ALIAS}.{declared}"),
            None => merged.to_string(),
        }
    }

    fn named(&mut self, id: rask_types::TypeId) -> Result<(String, Domain), String> {
        if let Some((f, d)) = self.named.get(&id) {
            return Ok((format!("{f}()"), *d));
        }
        let fn_name = format!("__claim_gen_{}", self.named.len());
        let def = self.typed.types.get(id).cloned().ok_or("unknown type")?;
        // Recursive types meet themselves here; the walk can't be finite then.
        self.named.insert(id, (fn_name.clone(), Domain::INFINITE));
        let (body, dom) = match &def {
            TypeDef::Struct { name, type_params, fields, private_fields, .. } => {
                if !type_params.is_empty() {
                    return Err(format!("`{name}` is generic, which claims can't generate yet"));
                }
                let visible = self.field_vis.get(name).cloned().unwrap_or_default();
                let foreign = self.names.declared.contains_key(name);
                let hidden = !private_fields.is_empty()
                    || (foreign && visible.iter().any(|v| *v != FieldVisibility::Public));
                if hidden {
                    return Err(format!(
                        "`{}` has fields claims can't see, so it can't be built from them; \
                         build it through its public API in the claim, from inputs that can \
                         be generated (type.claims/IN1)",
                        self.names.declared.get(name).unwrap_or(name)
                    ));
                }
                self.struct_body(name, fields)?
            }
            TypeDef::Enum { name, type_params, variants, .. } => {
                if !type_params.is_empty() {
                    return Err(format!("`{name}` is generic, which claims can't generate yet"));
                }
                self.enum_body(name, variants)?
            }
            _ => return Err(format!("`{}` isn't a type claims can generate yet", self.typed.types.type_name(id))),
        };
        let path = match &def {
            TypeDef::Struct { name, .. } | TypeDef::Enum { name, .. } => self.path(name),
            _ => unreachable!(),
        };
        self.functions.push(format!("func {fn_name}() -> {path} {{\n{body}}}\n"));
        self.named.insert(id, (fn_name.clone(), dom));
        Ok((format!("{fn_name}()"), dom))
    }

    fn struct_body(&mut self, name: &str, fields: &[(String, Type)]) -> Result<(String, Domain), String> {
        let mut body = String::new();
        let mut dom = Domain::finite(1);
        let mut args = Vec::new();
        for (i, (field, fty)) in fields.iter().enumerate() {
            let (x, d) = self.expr(fty)?;
            body.push_str(&format!("    let f{i} = {x}\n"));
            args.push(format!("{field}: f{i}"));
            dom = product(dom, d);
        }
        body.push_str(&format!("    return {}({})\n", self.path(name), args.join(", ")));
        Ok((body, dom))
    }

    fn enum_body(&mut self, name: &str, variants: &[(String, Vec<Type>)]) -> Result<(String, Domain), String> {
        if variants.is_empty() {
            return Err(format!("`{name}` has no variants, so there's nothing to generate"));
        }
        let path = self.path(name);
        let mut body = format!("    match __claims.draw({}) {{\n", variants.len());
        let mut doms = Vec::new();
        for (i, (variant, payload)) in variants.iter().enumerate() {
            let arm = if i + 1 == variants.len() { "_".to_string() } else { i.to_string() };
            body.push_str(&format!("        {arm} => {{\n"));
            if payload.is_empty() {
                body.push_str(&format!("            return {path}.{variant}\n"));
                doms.push(Domain::finite(1));
            } else {
                let mut dom = Domain::finite(1);
                let mut args = Vec::new();
                for (k, pty) in payload.iter().enumerate() {
                    let (x, d) = self.expr(pty)?;
                    body.push_str(&format!("            let p{k} = {x}\n"));
                    args.push(format!("p{k}"));
                    dom = product(dom, d);
                }
                body.push_str(&format!("            return {path}.{variant}({})\n", args.join(", ")));
                doms.push(dom);
            }
            body.push_str("        }\n");
        }
        body.push_str("    }\n");
        Ok((body, sum(&doms)))
    }
}

fn product(a: Domain, b: Domain) -> Domain {
    Domain {
        walk: a.walk.zip(b.walk).and_then(|(x, y)| x.checked_mul(y)),
        distinct: a.distinct.zip(b.distinct).and_then(|(x, y)| x.checked_mul(y)),
    }
}

/// A choice among branches: the walk spends one digit on the branch and
/// as many as the widest branch needs after it.
fn sum(branches: &[Domain]) -> Domain {
    let mut widest = Some(0u64);
    let mut distinct = Some(0u64);
    for b in branches {
        widest = widest.zip(b.walk).map(|(w, x)| w.max(x));
        distinct = distinct.zip(b.distinct).and_then(|(d, x)| d.checked_add(x));
    }
    Domain {
        walk: widest.and_then(|w| w.checked_mul(branches.len() as u64)),
        distinct,
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
    discarded: u64,
    over_budget: u64,
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
            discarded: n("discarded"),
            over_budget: n("over_budget"),
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

fn run_claims(bin: &Path, plan: &Plan, claims_path: &Path, format: Format) -> bool {
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
            let (input, error) = if fail.choices.is_empty() {
                (fail.input.clone(), fail.error.clone())
            } else {
                shrink(bin, i, fail)
            };
            report_false(claim, &input, &error, &shown, claims_path);
            continue;
        }
        if format == Format::Human {
            println!("  {:<24} {}", claim.name, verdict(summary, claim));
            if summary.over_budget > 0 {
                println!(
                    "  {:<24} {} ran past the step budget; the first was {}",
                    "",
                    count(summary.over_budget, "input"),
                    summary.budget_input
                );
            }
        }
    }
    all_held
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
fn verdict(r: &Record, claim: &ClaimPlan) -> String {
    let held = r.tried - r.discarded - r.over_budget;
    let mut line = if r.mode == "all" {
        format!("checked all {}", count(claim.distinct.unwrap_or(r.tried), "input"))
    } else {
        format!("held for {} (sampled, not proven)", count(held, "input"))
    };
    if r.discarded > 0 {
        line.push_str(&format!(", {} discarded", r.discarded));
    }
    line
}

/// Cut a failing input down: the smallest choices that still fail. Returns
/// how that input prints and what it failed with.
fn shrink(bin: &Path, claim: usize, fail: &Record) -> (String, String) {
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
        let (records, _, _) = run_bin(bin, &[("RASK_CLAIM_SHRINK", spec)]);
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
fn report_false(claim: &ClaimPlan, input: &str, error: &str, shown: &str, claims_path: &Path) {
    let diag = rask_diagnostics::Diagnostic::error(format!("claim `{}` is false", claim.name))
        .with_code("E0950")
        .with_primary(claim.span, "doesn't hold for this input")
        .with_note(format!("counterexample: {input}"))
        .with_note(error.to_string())
        .with_help(format!("the claim is in {shown}; if it's the claim that's wrong, change it there, in review"));
    let mut sources: Vec<(PathBuf, String)> = Vec::new();
    let slot = claim.span.file_id as usize;
    sources.resize(slot + 1, (PathBuf::new(), String::new()));
    sources[slot] = (claims_path.to_path_buf(), std::fs::read_to_string(claims_path).unwrap_or_default());
    crate::show_diagnostic_multi(&diag, &sources);
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

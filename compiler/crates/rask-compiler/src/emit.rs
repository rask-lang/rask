// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The back half: mono → MIR → passes → codegen → object file.
//!
//! One copy for every entry point. `rask compile`, `rask build`, `rask test`,
//! `rask bench` and the MIR dumps each used to write this out themselves, and
//! the copies drifted: benchmarks never registered vtables, left module consts
//! out of lowering, and then fell back to the interpreter when either one
//! mattered (#1563). `rask mir` printed MIR from before the passes.

use std::collections::{BTreeMap, HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind};
use rask_diagnostics::{Diagnostic, Severity};
use rask_mir::{Folded, MirFunction};
use rask_mono::MonoProgram;
use rask_check::TypedProgram;

pub use rask_codegen::BuildMode;

/// Everything the back half reads. Nothing in it is changed.
pub struct BackEnd<'a> {
    pub mono: &'a MonoProgram,
    pub typed: &'a TypedProgram,
    pub decls: &'a [Decl],
    pub folded: &'a Folded,
    pub package_modules: &'a HashSet<String>,
    /// For panic locations, try origins and DWARF.
    pub source_file: Option<&'a str>,
    pub source_text: Option<&'a str>,
}

impl<'a> BackEnd<'a> {
    pub fn of(
        result: &'a crate::CompileResult,
        source_file: Option<&'a str>,
        source_text: Option<&'a str>,
    ) -> Self {
        BackEnd {
            mono: &result.mono,
            typed: &result.typed,
            decls: &result.decls,
            folded: &result.folded,
            package_modules: &result.package_modules,
            source_file,
            source_text,
        }
    }
}

/// What the object file starts at. The test and benchmark runners replace
/// `main`, and run the module consts' initializers themselves.
pub enum Entry<'a> {
    Main,
    /// `(display name, function name)`, from `extract_tests`.
    Tests(&'a [(String, String)]),
    /// `(display name, function name)`, from `extract_benchmarks`.
    Benches(&'a [(String, String)]),
    /// The claims a claims build runs (type.claims).
    Claims(&'a [rask_codegen::ClaimRun]),
}

impl Entry<'_> {
    fn replaces_main(&self) -> bool {
        !matches!(self, Entry::Main)
    }
}

/// MIR as codegen sees it, after every pass.
pub struct Lowered {
    pub functions: Vec<MirFunction>,
    /// What each function referred to before the passes ran: inlining
    /// folds a callee into its caller, and a walk of the result alone
    /// would lose that the caller depended on it.
    pub calls_as_lowered: HashMap<String, Vec<String>>,
    pub pipeline: rask_mir::PipelineResult,
    /// Every layout's field types as MIR types, for codegen.
    pub field_types: rask_mir::FieldTypes,
}

/// Lower to MIR and run the pass pipeline.
pub fn lower(input: &BackEnd, entry: &Entry) -> Result<Lowered, Vec<String>> {
    let all_mono_decls = mono_decls(input.mono, input.decls);
    let line_map = input.source_text.map(rask_ast::LineMap::new);
    let type_names = input.typed.types.type_name_map();
    let interface_methods = rask_check::interface_vtable_methods(&input.typed.types);
    let extern_funcs = extern_func_names(input.decls);
    let nominal_underlying = nominal_underlying(input.typed);
    // Instantiated generic bodies have nodes the checker never saw; mono
    // carried its records onto them. Lowering wants one map for both.
    let records = input.mono.node_records(input.typed);
    let mut ctx = rask_mir::lower::MirContext::new(
        input.typed,
        &input.mono.struct_layouts,
        &input.mono.enum_layouts,
        &records,
        &type_names,
    )
    .with_folded(input.folded)
    .with_extern_funcs(&extern_funcs)
    .with_package_modules(input.package_modules)
    .with_interface_methods(interface_methods)
    .with_mono_calls(input.mono)
    .with_nominal_underlying(&nominal_underlying);
    ctx.line_map = line_map.as_ref();
    ctx.source_file = input.source_file;

    let mut errors = Vec::new();
    let mut functions = Vec::new();

    // Measure the module-level const slots before anything references them.
    rask_mir::lower::MirLowerer::compute_const_slot_types(&all_mono_decls, &ctx);

    // A runner that replaces `main` doesn't get the init thunks `main` would
    // have carried, so they're lowered on their own and the runner calls them.
    if entry.replaces_main() {
        match rask_mir::lower::MirLowerer::lower_const_init_thunks(&all_mono_decls, &ctx) {
            Ok(thunks) => functions.extend(thunks),
            Err(e) => errors.push(format!("MIR lowering const initializers: {}", e)),
        }
    }

    for mono_fn in &input.mono.functions {
        if entry.replaces_main() && mono_fn.name == "main" {
            continue;
        }
        // An empty-body stdlib stub (e.g. fs_write_bytes) is answered by the C
        // runtime dispatch table. Lowering it produces a no-op the inliner
        // substitutes for the call, silently dropping the operation.
        if let DeclKind::Fn(f) = &mono_fn.body.kind {
            if f.body.is_empty() && rask_stdlib::mir_metadata::lookup(&mono_fn.name).is_some() {
                continue;
            }
        }
        match rask_mir::lower::MirLowerer::lower_function_named(
            &mono_fn.body, &all_mono_decls, &ctx, Some(&mono_fn.name),
        ) {
            Ok(fns) => functions.extend(fns),
            Err(e) => errors.push(format!("MIR lowering '{}': {}", mono_fn.name, e)),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // A `@bits` field shares its storage unit; its stores become
    // read-modify-write before anything reasons about them.
    rask_mir::bitfields::expand_stores(&mut functions, &input.mono.struct_layouts);

    // A captured scalar has to live in memory before SSA runs, or SSA builds
    // phis over a variable whose real home is a stack slot (#1038).
    rask_mir::transform::addr_taken::run_all(&mut functions);
    for func in &mut functions {
        rask_mir::transform::ssa::construct(func);
    }

    rask_mir::dispatch_trace::report();
    rask_mir::fallback::report_coverage();

    let calls_as_lowered = functions.iter().map(|f| (f.name.clone(), referred_to(f, input))).collect();
    let pipeline = rask_mir::PassManager::default_pipeline().run(&mut functions);

    for func in &mut functions {
        rask_mir::transform::ssa::destruct(func);
    }
    if matches!(entry, Entry::Claims(_)) {
        rask_mir::transform::claim_steps::instrument(&mut functions);
    }

    // Every layout id has to name the layout it claims. One that doesn't sends
    // codegen walking another type's fields — the error then names a type the
    // program never mentions, and the version that doesn't error compares the
    // wrong bytes (#1062). Cheap: one walk of each function's locals.
    let mismatches = rask_mir::layout_check::check_layout_ids(
        &functions, &input.mono.struct_layouts, &input.mono.enum_layouts,
    );
    if !mismatches.is_empty() {
        return Err(mismatches
            .iter()
            .map(|m| format!(
                "MIR layout ids in '{}': {} — this is a compiler bug, not something \
                 the program did; please report it with the source that produced it",
                m.function, m.detail
            ))
            .collect());
    }

    Ok(Lowered { functions, calls_as_lowered, pipeline, field_types: ctx.field_types() })
}

/// What `emit` produced besides the object file.
pub struct Emitted {
    /// The MIR analyses' findings, warnings included. An error among them
    /// means no object was written.
    pub diagnostics: Vec<Diagnostic>,
    /// Failures that aren't diagnostics: a lowering or codegen error.
    pub errors: Vec<String>,
}

impl Emitted {
    pub fn succeeded(&self) -> bool {
        self.errors.is_empty() && !self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
}

/// Compile to an object file at `obj_path`.
pub fn emit(
    input: &BackEnd,
    entry: &Entry,
    target: Option<&str>,
    build_mode: BuildMode,
    obj_path: &str,
) -> Emitted {
    match lower(input, entry) {
        Ok(lowered) => emit_lowered(input, entry, lowered, target, build_mode, obj_path),
        Err(errors) => Emitted { diagnostics: Vec::new(), errors },
    }
}

/// `emit`, from MIR the caller already lowered and looked at.
pub fn emit_lowered(
    input: &BackEnd,
    entry: &Entry,
    lowered: Lowered,
    target: Option<&str>,
    build_mode: BuildMode,
    obj_path: &str,
) -> Emitted {
    let mut diagnostics = Vec::new();
    let errors = match emit_inner(input, entry, lowered, target, build_mode, obj_path, &mut diagnostics) {
        Ok(()) => Vec::new(),
        Err(errors) => errors,
    };
    Emitted { diagnostics, errors }
}

/// Every function `root` can reach, by name, in name order: what it calls,
/// takes the address of, or builds a closure or ensure hook from, before
/// the passes or after; the methods of any interface object it boxes; and
/// the helpers lowering synthesized for it (named `<fn>__…`). A claim's
/// verdict depends on these and on nothing else the program holds
/// (type.claims/D4).
pub fn reached_from<'f>(
    lowered: &'f Lowered,
    input: &BackEnd,
    root: &str,
) -> std::collections::BTreeMap<&'f str, &'f MirFunction> {
    let by_name: HashMap<&str, &MirFunction> =
        lowered.functions.iter().map(|f| (f.name.as_str(), f)).collect();
    let mut reached = std::collections::BTreeMap::new();
    let mut queue = vec![root.to_string()];
    while let Some(name) = queue.pop() {
        let Some(f) = by_name.get(name.as_str()) else { continue };
        if reached.insert(f.name.as_str(), *f).is_some() {
            continue;
        }
        let helpers = format!("{}__", f.name);
        queue.extend(lowered.functions.iter().filter(|g| g.name.starts_with(&helpers)).map(|g| g.name.clone()));
        queue.extend(referred_to(f, input));
        queue.extend(lowered.calls_as_lowered.get(&f.name).into_iter().flatten().cloned());
    }
    reached
}

/// The functions `f` names: calls, addresses taken, closures and ensure
/// hooks built, and the vtable methods of interface objects it boxes.
fn referred_to(f: &MirFunction, input: &BackEnd) -> Vec<String> {
    use rask_mir::{MirRValue, MirStmtKind};
    let mut out = Vec::new();
    let mut interface_methods = None;
    for stmt in f.blocks.iter().flat_map(|b| &b.statements) {
        match &stmt.kind {
            MirStmtKind::Call { func, .. } => out.push(func.name.clone()),
            MirStmtKind::Assign { rvalue: MirRValue::FuncAddr(n), .. } => out.push(n.clone()),
            MirStmtKind::ClosureCreate { func_name, .. } => out.push(func_name.clone()),
            MirStmtKind::EnsureHookRegister { thunk, .. } => out.push(thunk.clone()),
            MirStmtKind::InterfaceBox { concrete_type, interface_name, .. } => {
                let methods = interface_methods
                    .get_or_insert_with(|| rask_check::interface_vtable_methods(&input.typed.types));
                for method in methods.get(interface_name).into_iter().flatten() {
                    out.push(
                        input.mono.vtable_symbols
                            .get(&(concrete_type.clone(), method.clone()))
                            .cloned()
                            .unwrap_or_else(|| format!("{concrete_type}_{method}")),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

fn emit_inner(
    input: &BackEnd,
    entry: &Entry,
    lowered: Lowered,
    target: Option<&str>,
    build_mode: BuildMode,
    obj_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), Vec<String>> {
    let Lowered { mut functions, pipeline, field_types, .. } = lowered;
    let has_errors = pipeline.diagnostics.iter().any(|d| d.severity == Severity::Error);
    diagnostics.extend(pipeline.diagnostics);
    if has_errors {
        return Ok(());
    }
    if functions.is_empty() && matches!(entry, Entry::Main) {
        return Err(vec!["no functions to compile".to_string()]);
    }

    let mut codegen = match target {
        Some(t) => rask_codegen::CodeGenerator::new_with_target(t, build_mode),
        None => rask_codegen::CodeGenerator::new(build_mode),
    }
    .map_err(|e| vec![format!("codegen init: {}", e)])?;
    let fail = |e: rask_codegen::CodegenError| vec![e.to_string()];

    codegen.declare_runtime_functions().map_err(fail)?;
    codegen.declare_stdlib_functions().map_err(fail)?;
    let mut extern_sigs: Vec<_> = input.decls.iter().filter_map(|d| match &d.kind {
        // A C macro is no symbol; its calls are routed below (PP4).
        DeclKind::Extern(e) if rask_ast::attr::c_macro(&e.attrs).is_some() => None,
        DeclKind::Extern(e) => Some(rask_codegen::ExternFuncSig {
            name: e.name.clone(),
            link_name: crate::c_import::inline_header(e).map(|_| rask_c_parse::shim::wrapper_symbol(&e.name)),
            // The parser rejects an extern parameter without a type.
            param_types: e.params.iter().filter_map(|p| p.ty.clone()).collect(),
            ret_ty: e.ret_ty.clone(),
        }),
        _ => None,
    }).collect();
    let variadic = crate::variadic::route_calls(&mut functions, input.decls)?;
    extern_sigs.extend(variadic.iter().map(|w| w.sig.clone()));
    let macros = crate::c_macros::route_calls(&mut functions)?;
    extern_sigs.extend(macros.plain.iter().map(|w| w.sig.clone()));
    extern_sigs.extend(macros.with_header.iter().map(|(_, w)| w.sig.clone()));
    // Only the externs the program reaches. A header declares far more than
    // a program calls, and each declared import is a symbol the link must
    // find: some of a header's functions exist only on other platforms, or
    // in a library the program never links.
    let reached = reached_symbols(&functions);
    extern_sigs.retain(|s| reached.contains(&s.name));
    // Stale companions from an earlier build go; the set changes with the
    // headers a program's macro calls need.
    for stale in companion_sources(std::path::Path::new(obj_path)) {
        let _ = std::fs::remove_file(stale);
    }
    write_companion(obj_path, "va", variadic.iter().map(|w| w.source.as_str()).collect::<String>())?;
    write_companion(obj_path, "macro", macros.plain.iter().map(|w| w.source.as_str()).collect::<String>())?;
    codegen.declare_extern_functions(&extern_sigs, &input.mono.struct_layouts).map_err(fail)?;
    codegen.declare_functions(input.mono, &functions).map_err(fail)?;
    codegen.register_strings(&functions).map_err(fail)?;
    codegen.register_comptime_globals(&input.folded.data).map_err(fail)?;
    codegen.register_const_slots(&functions).map_err(fail)?;

    // Panic locations and try origins read the line map in every build; the
    // DWARF tables are codegen's call, by build mode.
    if let (Some(file), Some(text)) = (input.source_file, input.source_text) {
        codegen.set_debug_context(file, rask_ast::LineMap::new(text));
    }
    codegen.set_inline_regions(pipeline.inline_regions);
    codegen.set_field_types(field_types);

    let interface_methods = rask_check::interface_vtable_methods(&input.typed.types);
    let vtables = collect_vtables(&functions, &interface_methods, input.mono);
    if !vtables.is_empty() {
        codegen.register_vtables(&vtables).map_err(fail)?;
    }

    let mut errors = Vec::new();
    for mir_fn in &functions {
        if let Err(e) = codegen.gen_function(mir_fn) {
            errors.push(format!("codegen '{}': {}", mir_fn.name, e));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let const_names = module_const_names(input.decls);
    match entry {
        Entry::Main => {}
        Entry::Tests(tests) => codegen
            .gen_test_runner(tests, &const_names)
            .map_err(|e| vec![format!("test runner: {}", e)])?,
        Entry::Benches(benches) => codegen
            .gen_benchmark_runner(benches, &const_names)
            .map_err(|e| vec![format!("benchmark runner: {}", e)])?,
        Entry::Claims(claims) => codegen
            .gen_claim_runner(claims, &const_names)
            .map_err(|e| vec![format!("claim runner: {}", e)])?,
    }

    codegen.emit_object(obj_path).map_err(|e| vec![format!("emit object: {}", e)])?;
    write_inline_wrappers(input, target, obj_path, &macros.with_header)
}

/// The C companion of an object that calls a header's inline functions:
/// `<obj>.inline.c`, which the link step compiles beside it
/// (struct.c-interop/IN2). A stale one from an earlier build goes.
///
/// Its own file, apart from the variadic wrappers: those declare the C
/// functions they call without the headers, which a header's own
/// declaration would contradict.
fn write_inline_wrappers(
    input: &BackEnd,
    target: Option<&str>,
    obj_path: &str,
    macros: &[(String, crate::variadic::Wrapper)],
) -> Result<(), Vec<String>> {
    let (arch, os) = match target {
        Some(t) => rask_codegen::targets::arch_and_os(t).map_err(|e| vec![e])?,
        None => (std::env::consts::ARCH.to_string(), std::env::consts::OS.to_string()),
    };
    let dirs: HashMap<u16, std::path::PathBuf> = input
        .source_file
        .and_then(|f| std::path::Path::new(f).parent())
        .map(|d| (0u16, d.to_path_buf()))
        .into_iter()
        .collect();
    let cx = crate::c_import::Context { source_dirs: &dirs, arch: &arch, os: &os };
    let src = crate::c_import::inline_wrappers(input.decls, &cx)?.unwrap_or_default();
    write_companion(obj_path, "inline", src)?;
    // A C macro that reads memory or calls into C: compiled with its header,
    // which its call is made through (struct.c-interop/PP4). One file per
    // header, so two headers' macros of the same name each mean their own.
    let mut by_header: BTreeMap<std::path::PathBuf, String> = BTreeMap::new();
    let mut errors = Vec::new();
    for (header, w) in macros {
        match crate::c_import::find_header(header, &cx) {
            Ok(path) => {
                let src = by_header
                    .entry(path.clone())
                    .or_insert_with(|| format!("#include \"{}\"\n", path.display()));
                src.push_str(&w.source);
            }
            Err(msg) => errors.push(format!("the header `{}`, which its C macros are compiled with, isn't found: {}", header, msg)),
        }
    }
    if !errors.is_empty() {
        errors.dedup();
        return Err(errors);
    }
    for (i, src) in by_header.into_values().enumerate() {
        write_companion(obj_path, &format!("macro{}", i), src)?;
    }
    Ok(())
}

/// The C files beside an object, `<obj>.<kind>.c`: wrappers for a header's
/// inline functions, for variadic calls and for macro calls. The link step
/// compiles each with the object.
pub fn companion_sources(obj: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Some(name) = obj.file_name().and_then(|n| n.to_str()) else { return Vec::new() };
    let dir = match obj.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    };
    let prefix = format!("{}.", name);
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".c")))
        .map(|e| obj.with_file_name(e.file_name()))
        .collect();
    found.sort();
    found
}

/// Every function name a call or a function address in `functions` names.
fn reached_symbols(functions: &[MirFunction]) -> HashSet<String> {
    let mut names = HashSet::new();
    for f in functions {
        for block in &f.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    rask_mir::MirStmtKind::Call { func, .. } => {
                        names.insert(func.name.clone());
                    }
                    rask_mir::MirStmtKind::Assign { rvalue: rask_mir::MirRValue::FuncAddr(n), .. } => {
                        names.insert(n.clone());
                    }
                    _ => {}
                }
            }
        }
    }
    names
}

/// `<obj>.<kind>.c`, which the link step compiles beside the object; or no
/// file, when there's nothing to put in it.
fn write_companion(obj_path: &str, kind: &str, src: String) -> Result<(), Vec<String>> {
    let path = format!("{}.{}.c", obj_path, kind);
    if src.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    std::fs::write(&path, src).map_err(|e| vec![format!("writing {}: {}", path, e)])
}

/// Mono's function bodies under their mangled names, with the externs and
/// module consts lowering resolves names against.
fn mono_decls(mono: &MonoProgram, decls: &[Decl]) -> Vec<Decl> {
    let mut all: Vec<_> = mono.functions.iter().map(|f| {
        let mut decl = f.body.clone();
        if let DeclKind::Fn(ref mut fn_decl) = decl.kind {
            fn_decl.name = f.name.clone();
        }
        decl
    }).collect();
    all.extend(decls.iter()
        .filter(|d| matches!(&d.kind, DeclKind::Extern(_) | DeclKind::Const(_)))
        .cloned());
    all
}

/// Module-level constant names in declaration order — the order their init
/// thunks have to run in, since a later const can read an earlier one.
fn module_const_names(decls: &[Decl]) -> Vec<String> {
    decls.iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Const(c) => Some(c.name.clone()),
            _ => None,
        })
        .collect()
}

/// Nominal newtype name → the type it wraps.
///
/// `type Id = u64 implements …` has no layout: it *is* a u64 with its own
/// identity, so MIR treats it as transparent (#445).
fn nominal_underlying(typed: &TypedProgram) -> HashMap<String, rask_types::Type> {
    typed.types.iter()
        .filter_map(|def| match def {
            rask_types::TypeDef::NominalAlias { name, underlying, .. } => {
                Some((name.clone(), underlying.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Every `extern "C"` function, including the ones a C header import
/// translated: those are declarations like any other by now.
fn extern_func_names(decls: &[Decl]) -> HashSet<String> {
    decls.iter().filter_map(|d| match &d.kind {
        DeclKind::Extern(e) => Some(e.name.clone()),
        _ => None,
    }).collect()
}

/// One vtable per (concrete type, interface) pair some `InterfaceBox` builds.
fn collect_vtables(
    functions: &[MirFunction],
    interface_methods: &HashMap<String, Vec<String>>,
    mono: &MonoProgram,
) -> Vec<rask_codegen::vtable::VTableInfo> {
    let mut seen = HashSet::new();
    let mut vtables = Vec::new();
    for mir_fn in functions {
        for block in &mir_fn.blocks {
            for stmt in &block.statements {
                let rask_mir::MirStmtKind::InterfaceBox {
                    concrete_type, interface_name, concrete_size, concrete_align, vtable_name, ..
                } = &stmt.kind else {
                    continue;
                };
                if !seen.insert(vtable_name.clone()) {
                    continue;
                }
                let methods = interface_methods.get(interface_name).cloned().unwrap_or_default();
                let vt_methods = methods.iter().enumerate()
                    .map(|(i, method_name)| rask_codegen::vtable::VTableMethod {
                        name: method_name.clone(),
                        func_name: mono
                            .vtable_symbols
                            .get(&(concrete_type.clone(), method_name.clone()))
                            .cloned()
                            .unwrap_or_else(|| format!("{}_{}", concrete_type, method_name)),
                        vtable_offset: rask_codegen::vtable::method_offset(i),
                    })
                    .collect();
                // What the concrete value owns, for a box that owns it.
                // Containers and strings both: a moved-in box has no second
                // holder for either.
                let mut visited = HashSet::new();
                let owned = rask_codegen::drop_fields::owned_fields(
                    concrete_type, 0, &mono.struct_layouts, &mono.type_names, &mut visited,
                );
                vtables.push(rask_codegen::vtable::VTableInfo {
                    data_name: vtable_name.clone(),
                    concrete_type: concrete_type.clone(),
                    interface_name: interface_name.clone(),
                    concrete_size: *concrete_size,
                    concrete_align: *concrete_align,
                    methods: vt_methods,
                    owned,
                });
            }
        }
    }
    vtables
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The back half: mono → MIR → passes → codegen → object file.
//!
//! One copy for every entry point. `rask compile`, `rask build`, `rask test`,
//! `rask bench` and the MIR dumps each used to write this out themselves, and
//! the copies drifted: benchmarks never registered vtables, left module consts
//! out of lowering, and then fell back to the interpreter when either one
//! mattered (#1563). `rask mir` printed MIR from before the passes.

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind};
use rask_diagnostics::{Diagnostic, Severity};
use rask_mir::{Folded, MirFunction};
use rask_mono::MonoProgram;
use rask_types::TypedProgram;

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
}

impl Entry<'_> {
    fn replaces_main(&self) -> bool {
        !matches!(self, Entry::Main)
    }
}

/// MIR as codegen sees it, after every pass.
pub struct Lowered {
    pub functions: Vec<MirFunction>,
    pub pipeline: rask_mir::PipelineResult,
    /// Every layout's field types as MIR types, for codegen.
    pub field_types: rask_mir::FieldTypes,
}

/// Lower to MIR and run the pass pipeline.
pub fn lower(input: &BackEnd, entry: &Entry) -> Result<Lowered, Vec<String>> {
    let all_mono_decls = mono_decls(input.mono, input.decls);
    let line_map = input.source_text.map(rask_ast::LineMap::new);
    let type_names = input.typed.types.type_name_map();
    let interface_methods = rask_types::interface_vtable_methods(&input.typed.types);
    let extern_funcs = extern_func_names(input.decls, &input.typed.symbols);
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

    // A captured scalar has to live in memory before SSA runs, or SSA builds
    // phis over a variable whose real home is a stack slot (#1038).
    rask_mir::transform::addr_taken::run_all(&mut functions);
    for func in &mut functions {
        rask_mir::transform::ssa::construct(func);
    }

    rask_mir::dispatch_trace::report();
    rask_mir::fallback::report_coverage();

    let pipeline = rask_mir::PassManager::default_pipeline().run(&mut functions);

    for func in &mut functions {
        rask_mir::transform::ssa::destruct(func);
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

    Ok(Lowered { functions, pipeline, field_types: ctx.field_types() })
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
    let mut diagnostics = Vec::new();
    let errors = match emit_inner(input, entry, target, build_mode, obj_path, &mut diagnostics) {
        Ok(()) => Vec::new(),
        Err(errors) => errors,
    };
    Emitted { diagnostics, errors }
}

fn emit_inner(
    input: &BackEnd,
    entry: &Entry,
    target: Option<&str>,
    build_mode: BuildMode,
    obj_path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), Vec<String>> {
    let Lowered { functions, pipeline, field_types } = lower(input, entry)?;
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
        DeclKind::Extern(e) => Some(rask_codegen::ExternFuncSig {
            name: e.name.clone(),
            // The parser rejects an extern parameter without a type.
            param_types: e.params.iter().filter_map(|p| p.ty.clone()).collect(),
            ret_ty: e.ret_ty.clone(),
        }),
        _ => None,
    }).collect();
    extern_sigs.extend(c_import_extern_sigs(&input.typed.symbols));
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

    let interface_methods = rask_types::interface_vtable_methods(&input.typed.types);
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
    }

    codegen.emit_object(obj_path).map_err(|e| vec![format!("emit object: {}", e)])
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

/// Every `extern "C"` function: declared ones and those a C header import
/// brought in.
fn extern_func_names(decls: &[Decl], symbols: &rask_resolve::SymbolTable) -> HashSet<String> {
    let mut names: HashSet<String> = decls.iter().filter_map(|d| match &d.kind {
        DeclKind::Extern(e) => Some(e.name.clone()),
        _ => None,
    }).collect();
    for sym in symbols.iter() {
        if let rask_resolve::SymbolKind::CNamespace { members } = &sym.kind {
            for &member_id in members.values() {
                if let Some(member) = symbols.get(member_id) {
                    if let rask_resolve::SymbolKind::ExternFunction { abi, .. } = &member.kind {
                        if abi == "C" {
                            names.insert(member.name.clone());
                        }
                    }
                }
            }
        }
    }
    names
}

/// Signatures of the functions C header imports brought in.
fn c_import_extern_sigs(symbols: &rask_resolve::SymbolTable) -> Vec<rask_codegen::ExternFuncSig> {
    let mut sigs = Vec::new();
    for sym in symbols.iter() {
        if let rask_resolve::SymbolKind::CNamespace { members } = &sym.kind {
            // The header's struct types register under the qualified spelling —
            // `c.Rect` — because that is how a program writes them. A parameter
            // declared as one arrives here bare, so it wouldn't find the layout
            // that says how to pass it (#948).
            let structs: HashSet<&str> = members
                .iter()
                .filter(|(_, &id)| {
                    symbols
                        .get(id)
                        .is_some_and(|m| matches!(m.kind, rask_resolve::SymbolKind::Struct { .. }))
                })
                .map(|(name, _)| name.as_str())
                .collect();
            let qualify = |ty: &rask_ast::ty::TypeExpr| match ty.bare_name() {
                Some(name) if structs.contains(name) => rask_ast::ty::TypeExpr::Named {
                    path: vec![sym.name.clone(), name.to_string()],
                    args: Vec::new(),
                },
                _ => ty.clone(),
            };
            for &member_id in members.values() {
                if let Some(member) = symbols.get(member_id) {
                    if let rask_resolve::SymbolKind::ExternFunction { abi, params, ret_ty } = &member.kind {
                        if abi == "C" {
                            sigs.push(rask_codegen::ExternFuncSig {
                                name: member.name.clone(),
                                param_types: params.iter().map(qualify).collect(),
                                ret_ty: ret_ty.as_ref().map(qualify),
                            });
                        }
                    }
                }
            }
        }
    }
    sigs
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
                    concrete_type, interface_name, concrete_size, vtable_name, ..
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
                        func_name: format!("{}_{}", concrete_type, method_name),
                        vtable_offset: rask_codegen::vtable::method_offset(i),
                    })
                    .collect();
                let concrete_align = mono.struct_layouts.iter()
                    .find(|s| s.name == *concrete_type)
                    .map(|s| s.align)
                    .unwrap_or(8);
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
                    concrete_align,
                    methods: vt_methods,
                    owned,
                });
            }
        }
    }
    vtables
}

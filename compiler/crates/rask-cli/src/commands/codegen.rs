// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Code generation commands: mono, mir, compile.

use colored::Colorize;
use rask_mono::MonoProgram;
use std::path::{Path, PathBuf};
use std::process;

use crate::{output, Format};

type PipelineResult = (
    MonoProgram,
    rask_types::TypedProgram,
    Vec<rask_ast::decl::Decl>,
    std::collections::HashMap<String, rask_mir::ComptimeGlobalMeta>,
    Option<String>,
    Vec<String>,
);

/// Run the full front-end pipeline + monomorphize + comptime eval via
/// rask-compiler. Exits on error (including comptime hard errors, which are
/// pipeline diagnostics). Comptime globals are evaluated once, here.
fn run_pipeline(path: &str, format: Format) -> PipelineResult {
    let config = rask_compiler::CompilerConfig {
        cfg: rask_compiler::CfgConfig::from_host("debug", vec![]),
    };
    let output = rask_compiler::compile_file(path, vec![], &config);

    // Build source_files for display
    let source_files: Vec<(std::path::PathBuf, String)> = if let Some(ref r) = output.result {
        // Use the source from the compile result's check phase
        vec![] // compile doesn't track source_files directly — read if needed
    } else {
        match std::fs::read_to_string(path) {
            Ok(s) => vec![(std::path::PathBuf::from(path), s)],
            Err(_) => vec![],
        }
    };

    crate::display_pipeline_output(&output, &source_files, format);

    if output.has_errors() {
        let err_count = output.diagnostics.iter()
            .filter(|d| matches!(d.severity, rask_diagnostics::Severity::Error))
            .count();
        if format == Format::Human {
            eprintln!("\n{}", crate::output::banner_fail("Compile", err_count));
        }
        process::exit(1);
    }

    let result = output.result.unwrap();
    let source = std::fs::read_to_string(path).ok();
    let package_names = vec![]; // package_modules is in CompileResult but not as names
    (result.mono, result.typed, result.decls, result.comptime_globals, source, package_names)
}


/// Surface comptime hard-error diagnostics and abort. For the test/bench paths
/// that build their own monomorphized program and call
/// `rask_compiler::evaluate_comptime_globals` directly (the compile/build paths
/// get these through the normal pipeline diagnostics instead).
pub(crate) fn exit_on_comptime_errors(
    diags: &[rask_diagnostics::Diagnostic],
    source_files: &[(std::path::PathBuf, String)],
) {
    if diags.is_empty() {
        return;
    }
    for d in diags {
        crate::show_diagnostic_multi(d, source_files);
    }
    eprintln!("{}", output::banner_fail("Comptime", diags.len()));
    process::exit(1);
}

/// Dump monomorphization output for a single file.
pub fn cmd_mono(path: &str, format: Format) {
    let (mono, _typed, _decls, _comptime_globals, _source, _package_names) = run_pipeline(path, format);

    if format == Format::Human {
        println!(
            "{} Mono ({} function{}, {} struct layout{}, {} enum layout{}) {}\n",
            "===".dimmed(),
            mono.functions.len(),
            if mono.functions.len() == 1 { "" } else { "s" },
            mono.struct_layouts.len(),
            if mono.struct_layouts.len() == 1 {
                ""
            } else {
                "s"
            },
            mono.enum_layouts.len(),
            if mono.enum_layouts.len() == 1 {
                ""
            } else {
                "s"
            },
            "===".dimmed()
        );

        // Print reachable functions
        println!("{}", "Functions:".bold());
        for mono_fn in &mono.functions {
            let fn_decl = match &mono_fn.body.kind {
                rask_ast::decl::DeclKind::Fn(f) => f,
                _ => continue,
            };
            let params: Vec<String> = fn_decl
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty))
                .collect();
            let ret = fn_decl
                .ret_ty
                .as_deref()
                .map(|t| format!(" -> {}", t))
                .unwrap_or_default();
            let type_args = if mono_fn.type_args.is_empty() {
                String::new()
            } else {
                format!(
                    "<{}>",
                    mono_fn
                        .type_args
                        .iter()
                        .map(|t| format!("{:?}", t))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            println!(
                "  func {}{}({}){} [{} stmt{}]",
                mono_fn.name,
                type_args,
                params.join(", "),
                ret,
                fn_decl.body.len(),
                if fn_decl.body.len() == 1 { "" } else { "s" }
            );
        }

        // Print struct layouts
        if !mono.struct_layouts.is_empty() {
            println!();
            println!("{}", "Struct layouts:".bold());
            for layout in &mono.struct_layouts {
                println!(
                    "  {} (size: {}, align: {})",
                    layout.name, layout.size, layout.align
                );
                for field in &layout.fields {
                    println!(
                        "    .{}: {:?} (offset: {}, size: {})",
                        field.name, field.ty, field.offset, field.size
                    );
                }
            }
        }

        // Print enum layouts
        if !mono.enum_layouts.is_empty() {
            println!();
            println!("{}", "Enum layouts:".bold());
            for layout in &mono.enum_layouts {
                println!(
                    "  {} (size: {}, align: {}, tag: {:?})",
                    layout.name, layout.size, layout.align, layout.tag_ty
                );
                for variant in &layout.variants {
                    println!(
                        "    .{} = {} (payload offset: {}, size: {})",
                        variant.name, variant.tag, variant.payload_offset, variant.payload_size
                    );
                }
            }
        }

        println!();
        println!("{}", output::banner_ok("Monomorphization"));
    }
}

/// Dump MIR for a single file.
pub fn cmd_mir(path: &str, format: Format) {
    let (mono, typed, decls, comptime_globals, source, _package_names) = run_pipeline(path, format);

    // Lower each monomorphized function to MIR
    if format == Format::Human {
        println!(
            "{} MIR ({} function{}, {} struct layout{}, {} enum layout{}) {}\n",
            "===".dimmed(),
            mono.functions.len(),
            if mono.functions.len() == 1 { "" } else { "s" },
            mono.struct_layouts.len(),
            if mono.struct_layouts.len() == 1 {
                ""
            } else {
                "s"
            },
            mono.enum_layouts.len(),
            if mono.enum_layouts.len() == 1 {
                ""
            } else {
                "s"
            },
            "===".dimmed()
        );
    }

    // Collect all monomorphized function bodies + extern decls for signature table.
    // Patch FnDecl names to use the qualified monomorphized name (e.g. "GameState_new"
    // instead of bare "new") so func_sigs maps them correctly.
    let mut all_mono_decls: Vec<_> = mono.functions.iter().map(|f| {
        let mut decl = f.body.clone();
        if let rask_ast::decl::DeclKind::Fn(ref mut fn_decl) = decl.kind {
            fn_decl.name = f.name.clone();
        }
        decl
    }).collect();
    all_mono_decls.extend(decls.iter().filter(|d| matches!(&d.kind, rask_ast::decl::DeclKind::Extern(_))).cloned());
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    let extern_funcs = collect_extern_func_names(&decls, &typed.symbols);
    let empty_resource_types = std::collections::HashSet::new();
    let line_map = source.as_deref().map(rask_ast::LineMap::new);
    let type_names: std::collections::HashMap<rask_types::TypeId, String> = typed.types.iter()
        .enumerate()
        .map(|(i, def)| {
            let name = match def {
                rask_types::TypeDef::Struct { name, .. } => name.clone(),
                rask_types::TypeDef::Enum { name, .. } => name.clone(),
                rask_types::TypeDef::Trait { name, .. } => name.clone(),
                rask_types::TypeDef::Union { name, .. } => name.clone(),
                rask_types::TypeDef::NominalAlias { name, .. } => name.clone(),
            };
            (rask_types::TypeId(i as u32), name)
        })
        .collect();
    let trait_methods: std::collections::HashMap<String, Vec<String>> = typed.types.iter()
        .filter_map(|def| {
            if let rask_types::TypeDef::Trait { name, .. } = def {
                // Object-compatible methods only (TR1–TR3) — match vtable layout.
                Some((name.clone(), def.object_compatible_method_names()))
            } else {
                None
            }
        })
        .collect();
    let mut mir_interp = rask_comptime::ComptimeInterpreter::new();
    mir_interp.inject_cfg(&cfg);
    mir_interp.register_functions(&decls);
    let empty_packages = std::collections::HashSet::new();
    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
        type_names: &type_names,
        comptime_globals: &comptime_globals,
        extern_funcs: &extern_funcs,
        package_modules: &empty_packages,
        trait_methods,
        line_map: line_map.as_ref(),
        source_file: Some(path),
        shared_elem_types: std::cell::RefCell::new(std::collections::HashMap::new()),
        comptime_interp: Some(std::cell::RefCell::new(mir_interp)),
        trait_coercions: &typed.trait_coercions,
        call_rewrites: &mono.call_rewrites,
        resource_types: &empty_resource_types,
    };

    let mut mir_errors = 0;
    for mono_fn in &mono.functions {
        // Skip empty-body stdlib stubs (same filter as compile path).
        if let rask_ast::decl::DeclKind::Fn(f) = &mono_fn.body.kind {
            if f.body.is_empty()
                && rask_stdlib::mir_metadata::lookup(&mono_fn.name).is_some()
            {
                continue;
            }
        }
        match rask_mir::lower::MirLowerer::lower_function_named(&mono_fn.body, &all_mono_decls, &mir_ctx, Some(&mono_fn.name)) {
            Ok(mir_fns) => {
                if format == Format::Human {
                    for mir_fn in &mir_fns {
                        println!("{}", mir_fn);
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: lowering function '{}': {:?}",
                    output::error_label(),
                    mono_fn.name,
                    e
                );
                mir_errors += 1;
            }
        }
    }

    if format == Format::Human {
        println!();
        if mir_errors == 0 {
            println!("{}", output::banner_ok("MIR lowering"));
        } else {
            eprintln!("{}", output::banner_fail("MIR lowering", mir_errors));
            process::exit(1);
        }
    }
}

/// Dump MIR for a .rk file — runs the full pipeline up to MIR lowering
/// and prints the MIR functions to stderr. Used for debugging codegen issues.
pub fn cmd_dump_mir(path: &str, format: Format, release: bool) {
    let (mono, typed, decls, comptime_globals, source, package_names) = run_pipeline(path, format);
    let _ = release;
    let type_names = super::compile::build_type_names(&typed);
    let trait_methods = super::compile::build_trait_methods(&typed);
    let extern_funcs = collect_extern_func_names(&decls, &typed.symbols);
    let empty_resource_types = std::collections::HashSet::new();
    let line_map = source.as_deref().map(rask_ast::LineMap::new);
    let package_modules: std::collections::HashSet<String> = package_names.into_iter().collect();

    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
        type_names: &type_names,
        comptime_globals: &comptime_globals,
        extern_funcs: &extern_funcs,
        package_modules: &package_modules,
        trait_methods: trait_methods.clone(),
        line_map: line_map.as_ref(),
        source_file: Some(path),
        shared_elem_types: std::cell::RefCell::new(std::collections::HashMap::new()),
        comptime_interp: None,
        trait_coercions: &typed.trait_coercions,
        call_rewrites: &mono.call_rewrites,
        resource_types: &empty_resource_types,
    };

    let all_mono_decls = super::compile::build_mono_decls(&mono, &decls, true);
    for mono_fn in &mono.functions {
        if let rask_ast::decl::DeclKind::Fn(f) = &mono_fn.body.kind {
            if f.body.is_empty()
                && rask_stdlib::mir_metadata::lookup(&mono_fn.name).is_some()
            {
                continue;
            }
        }
        match rask_mir::lower::MirLowerer::lower_function_named(
            &mono_fn.body, &all_mono_decls, &mir_ctx, Some(&mono_fn.name)
        ) {
            Ok(mir_fns) => {
                for func in &mir_fns {
                    eprintln!("{}", func);
                }
            }
            Err(e) => {
                eprintln!("{}: MIR lowering '{}': {:?}", output::error_label(), mono_fn.name, e);
            }
        }
    }
}

/// Compile a single .rk file to a native executable.
/// Full pipeline: lex → parse → desugar → resolve → typecheck → ownership →
/// hidden-params → mono → MIR → Cranelift codegen → link with runtime.c.
pub fn cmd_compile(path: &str, output_path: Option<&str>, format: Format, quiet: bool, link_opts: &super::link::LinkOptions, release: bool, target: Option<&str>) {
    if let Some(t) = target {
        if let Err(e) = super::link::validate_target(t) {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }

    let (mono, typed, decls, comptime_globals, source, package_names) = run_pipeline(path, format);
    let profile = if release { "release" } else { "debug" };
    let cfg = rask_comptime::CfgConfig::from_target_or_host(target, profile, vec![]);
    let build_mode = if release { rask_codegen::BuildMode::Release } else { rask_codegen::BuildMode::Debug };

    // Determine output paths
    let bin_path = match output_path {
        Some(p) => p.to_string(),
        None => {
            let p = Path::new(path);
            let stem = p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("a.out");
            let base_dir = if let Some(project_root) = rask_compiler::find_project_root_from(path) {
                project_root
            } else {
                p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
            };
            let mut out_dir = base_dir.join("build");
            if let Some(t) = target {
                out_dir = out_dir.join(t);
            }
            let subdir = if release { "release" } else { "debug" };
            out_dir = out_dir.join(subdir);
            let _ = std::fs::create_dir_all(&out_dir);
            out_dir.join(stem).to_string_lossy().to_string()
        }
    };
    let obj_path = format!("{}.o", bin_path);

    let package_modules: std::collections::HashSet<String> = package_names.into_iter().collect();
    if let Err(errors) = super::compile::compile_to_object(
        &mono, &typed, &decls, &comptime_globals,
        Some(path), source.as_deref(), target, &obj_path, build_mode, Some(&cfg),
        &package_modules,
    ) {
        for e in &errors {
            eprintln!("{}: {}", output::error_label(), e);
        }
        process::exit(1);
    }

    if let Err(e) = super::link::link_executable_with(&obj_path, &bin_path, link_opts, release, target) {
        eprintln!("{}: link: {}", output::error_label(), e);
        process::exit(1);
    }

    let _ = std::fs::remove_file(&obj_path);

    if format == Format::Human && !quiet {
        eprintln!("{}", output::banner_ok(&format!("Compiled → {}", bin_path)));
    }
}

/// Extract the names of all `extern "C"` functions from parsed declarations
/// and C import namespaces.
pub fn collect_extern_func_names(
    decls: &[rask_ast::decl::Decl],
    symbols: &rask_resolve::SymbolTable,
) -> std::collections::HashSet<String> {
    let mut names: std::collections::HashSet<String> = decls.iter().filter_map(|d| {
        if let rask_ast::decl::DeclKind::Extern(e) = &d.kind {
            Some(e.name.clone())
        } else {
            None
        }
    }).collect();

    // Add functions from C import namespaces
    for sym in symbols.iter() {
        if let rask_resolve::SymbolKind::CNamespace { members } = &sym.kind {
            for (_, &member_id) in members {
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

/// Extract extern function signatures from C import namespaces in the symbol table.
pub fn collect_c_import_extern_sigs(
    symbols: &rask_resolve::SymbolTable,
) -> Vec<rask_codegen::ExternFuncSig> {
    let mut sigs = Vec::new();
    for sym in symbols.iter() {
        if let rask_resolve::SymbolKind::CNamespace { members } = &sym.kind {
            for (_, &member_id) in members {
                if let Some(member) = symbols.get(member_id) {
                    if let rask_resolve::SymbolKind::ExternFunction { abi, params, ret_ty } = &member.kind {
                        if abi == "C" {
                            sigs.push(rask_codegen::ExternFuncSig {
                                name: member.name.clone(),
                                param_types: params.clone(),
                                ret_ty: ret_ty.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
    sigs
}

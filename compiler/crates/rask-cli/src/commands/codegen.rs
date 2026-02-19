// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Code generation commands: mono, mir, compile.

use colored::Colorize;
use rask_mono::MonoProgram;
use std::path::{Path, PathBuf};
use std::process;

use crate::{output, Format};

/// Run the full front-end pipeline + monomorphize. Exits on error.
/// Returns (mono, typed, decls, source) — source is the original .rk text.
fn run_pipeline(path: &str, format: Format) -> (MonoProgram, rask_types::TypedProgram, Vec<rask_ast::decl::Decl>, Option<String>) {
    let mut result = super::pipeline::run_frontend(path, format);

    // Hidden parameter pass — desugar `using` clauses into explicit params
    rask_hidden_params::desugar_hidden_params(&mut result.decls);

    let decls = result.decls.clone();
    let source = result.source.clone();

    // Monomorphize
    let mono = match rask_mono::monomorphize(&result.typed, &result.decls) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}: monomorphization failed: {:?}", output::error_label(), e);
            process::exit(1);
        }
    };

    (mono, result.typed, decls, source)
}

/// Evaluate comptime const declarations and return serialized data.
pub fn evaluate_comptime_globals(decls: &[rask_ast::decl::Decl]) -> std::collections::HashMap<String, rask_mir::ComptimeGlobalMeta> {
    use rask_ast::decl::DeclKind;
    use rask_ast::stmt::StmtKind;

    let mut comptime_interp = rask_comptime::ComptimeInterpreter::new();
    comptime_interp.register_functions(decls);

    let mut globals = std::collections::HashMap::new();

    // Collect (name, init_expr) pairs from both top-level consts and function-body consts
    let mut comptime_consts: Vec<(String, &rask_ast::expr::Expr)> = Vec::new();

    for decl in decls {
        match &decl.kind {
            // Top-level const with comptime initializer
            DeclKind::Const(c) => {
                if is_comptime_init(&c.init, decls) {
                    comptime_consts.push((c.name.clone(), &c.init));
                }
            }
            // Const inside a function body
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
        match comptime_interp.eval_expr(init) {
            Ok(val) => {
                let type_prefix = val.type_prefix().to_string();
                let elem_count = val.elem_count();
                if let Some(bytes) = val.serialize() {
                    globals.insert(name, rask_mir::ComptimeGlobalMeta { bytes, elem_count, type_prefix });
                }
            }
            Err(e) => {
                eprintln!("warning: comptime eval '{}' failed: {:?}", name, e);
            }
        }
    }

    globals
}

/// Check if an expression is a comptime initializer.
fn is_comptime_init(init: &rask_ast::expr::Expr, decls: &[rask_ast::decl::Decl]) -> bool {
    use rask_ast::decl::DeclKind;
    use rask_ast::expr::ExprKind;

    matches!(&init.kind, ExprKind::Comptime { .. })
        || matches!(&init.kind, ExprKind::Call { func, .. }
            if matches!(&func.kind, ExprKind::Ident(name)
                if decls.iter().any(|d| matches!(&d.kind,
                    DeclKind::Fn(f) if f.name == *name && f.is_comptime))))
}

/// Dump monomorphization output for a single file.
pub fn cmd_mono(path: &str, format: Format) {
    let (mono, _typed, _decls, _source) = run_pipeline(path, format);

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
    let (mono, typed, decls, source) = run_pipeline(path, format);

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
    let comptime_globals = evaluate_comptime_globals(&decls);
    let extern_funcs = collect_extern_func_names(&decls);
    let line_map = source.as_deref().map(rask_ast::LineMap::new);
    let type_names: std::collections::HashMap<rask_types::TypeId, String> = typed.types.iter()
        .enumerate()
        .map(|(i, def)| {
            let name = match def {
                rask_types::TypeDef::Struct { name, .. } => name.clone(),
                rask_types::TypeDef::Enum { name, .. } => name.clone(),
                rask_types::TypeDef::Trait { name, .. } => name.clone(),
                rask_types::TypeDef::Union { name, .. } => name.clone(),
            };
            (rask_types::TypeId(i as u32), name)
        })
        .collect();
    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
        type_names: &type_names,
        comptime_globals: &comptime_globals,
        extern_funcs: &extern_funcs,
        line_map: line_map.as_ref(),
        source_file: Some(path),
        shared_elem_types: std::cell::RefCell::new(std::collections::HashMap::new()),
    };

    let mut mir_errors = 0;
    for mono_fn in &mono.functions {
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

/// Compile a single .rk file to a native executable.
/// Full pipeline: lex → parse → desugar → resolve → typecheck → ownership →
/// hidden-params → mono → MIR → Cranelift codegen → link with runtime.c.
pub fn cmd_compile(path: &str, output_path: Option<&str>, format: Format, quiet: bool, link_opts: &super::link::LinkOptions, release: bool) {
    let (mono, typed, decls, source) = run_pipeline(path, format);
    let comptime_globals = evaluate_comptime_globals(&decls);
    let build_mode = if release { rask_codegen::BuildMode::Release } else { rask_codegen::BuildMode::Debug };

    // Determine output paths
    let bin_path = match output_path {
        Some(p) => p.to_string(),
        None => {
            let p = Path::new(path);
            let stem = p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("a.out");
            let base_dir = if let Some(project_root) = super::pipeline::find_project_root_from(path) {
                project_root
            } else {
                p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
            };
            let subdir = if release { "release" } else { "debug" };
            let out_dir = base_dir.join("build").join(subdir);
            let _ = std::fs::create_dir_all(&out_dir);
            out_dir.join(stem).to_string_lossy().to_string()
        }
    };
    let obj_path = format!("{}.o", bin_path);

    if let Err(errors) = super::compile::compile_to_object(
        &mono, &typed, &decls, &comptime_globals,
        Some(path), source.as_deref(), None, &obj_path, build_mode,
    ) {
        for e in &errors {
            eprintln!("{}: {}", output::error_label(), e);
        }
        process::exit(1);
    }

    if let Err(e) = super::link::link_executable_with(&obj_path, &bin_path, link_opts, release) {
        eprintln!("{}: link: {}", output::error_label(), e);
        process::exit(1);
    }

    let _ = std::fs::remove_file(&obj_path);

    if format == Format::Human && !quiet {
        eprintln!("{}", output::banner_ok(&format!("Compiled → {}", bin_path)));
    }
}

/// Extract the names of all `extern "C"` functions from parsed declarations.
pub fn collect_extern_func_names(decls: &[rask_ast::decl::Decl]) -> std::collections::HashSet<String> {
    decls.iter().filter_map(|d| {
        if let rask_ast::decl::DeclKind::Extern(e) = &d.kind {
            Some(e.name.clone())
        } else {
            None
        }
    }).collect()
}

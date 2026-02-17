// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Code generation commands: mono, mir, compile.

use colored::Colorize;
use rask_mono::MonoProgram;
use std::path::Path;
use std::process;

use crate::{output, Format};

/// Run the full front-end pipeline + monomorphize. Exits on error.
fn run_pipeline(path: &str, format: Format) -> (MonoProgram, rask_types::TypedProgram) {
    let mut result = super::pipeline::run_frontend(path, format);

    // Hidden parameter pass — desugar `using` clauses into explicit params
    rask_hidden_params::desugar_hidden_params(&mut result.decls);

    // Monomorphize
    let mono = match rask_mono::monomorphize(&result.typed, &result.decls) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}: monomorphization failed: {:?}", output::error_label(), e);
            process::exit(1);
        }
    };

    (mono, result.typed)
}

/// Dump monomorphization output for a single file.
pub fn cmd_mono(path: &str, format: Format) {
    let (mono, _typed) = run_pipeline(path, format);

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
    let (mono, typed) = run_pipeline(path, format);

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

    // Collect all monomorphized function bodies for signature table
    let all_mono_decls: Vec<_> = mono.functions.iter().map(|f| f.body.clone()).collect();
    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
    };

    let mut mir_errors = 0;
    for mono_fn in &mono.functions {
        match rask_mir::lower::MirLowerer::lower_function(&mono_fn.body, &all_mono_decls, &mir_ctx) {
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
pub fn cmd_compile(path: &str, output_path: Option<&str>, format: Format, quiet: bool) {
    let (mono, typed) = run_pipeline(path, format);

    // MIR lowering
    let all_mono_decls: Vec<_> = mono.functions.iter().map(|f| f.body.clone()).collect();
    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
    };

    let mut mir_functions = Vec::new();
    for mono_fn in &mono.functions {
        match rask_mir::lower::MirLowerer::lower_function(&mono_fn.body, &all_mono_decls, &mir_ctx) {
            Ok(mir_fns) => mir_functions.extend(mir_fns),
            Err(e) => {
                eprintln!("{}: MIR lowering '{}': {:?}", output::error_label(), mono_fn.name, e);
                process::exit(1);
            }
        }
    }

    if mir_functions.is_empty() {
        eprintln!("{}: no functions to compile", output::error_label());
        process::exit(1);
    }

    // Closure optimization: escape analysis + cross-function ownership transfer + drop insertion
    rask_mir::optimize_all_closures(&mut mir_functions);

    // Cranelift codegen
    let mut codegen = match rask_codegen::CodeGenerator::new() {
        Ok(cg) => cg,
        Err(e) => {
            eprintln!("{}: codegen init: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    if let Err(e) = codegen.declare_runtime_functions() {
        eprintln!("{}: {}", output::error_label(), e);
        process::exit(1);
    }
    if let Err(e) = codegen.declare_stdlib_functions() {
        eprintln!("{}: {}", output::error_label(), e);
        process::exit(1);
    }
    if let Err(e) = codegen.declare_functions(&mono, &mir_functions) {
        eprintln!("{}: {}", output::error_label(), e);
        process::exit(1);
    }
    if let Err(e) = codegen.register_strings(&mir_functions) {
        eprintln!("{}: {}", output::error_label(), e);
        process::exit(1);
    }
    for mir_fn in &mir_functions {
        if let Err(e) = codegen.gen_function(mir_fn) {
            eprintln!("{}: codegen '{}': {}", output::error_label(), mir_fn.name, e);
            process::exit(1);
        }
    }

    // Emit object and link
    let bin_path = match output_path {
        Some(p) => p.to_string(),
        None => {
            let p = Path::new(path);
            let stem = p.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("a.out");

            // If in a project context (build.rk found), output to build/debug/
            if let Some(project_root) = super::pipeline::find_project_root_from(path) {
                let out_dir = project_root.join("build").join("debug");
                let _ = std::fs::create_dir_all(&out_dir);
                out_dir.join(stem).to_string_lossy().to_string()
            } else {
                stem.to_string()
            }
        }
    };
    let obj_path = format!("{}.o", bin_path);

    if let Err(e) = codegen.emit_object(&obj_path) {
        eprintln!("{}: emit object: {}", output::error_label(), e);
        process::exit(1);
    }

    if let Err(e) = super::link::link_executable(&obj_path, &bin_path) {
        eprintln!("{}: link: {}", output::error_label(), e);
        process::exit(1);
    }

    if format == Format::Human && !quiet {
        eprintln!("{}", output::banner_ok(&format!("Compiled → {}", bin_path)));
    }
}

// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Code generation commands: mono, mir, compile.

use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process;

use crate::{output, Format};

/// Run the full front-end pipeline + monomorphize + comptime eval via
/// rask-compiler. Exits on error (including comptime hard errors, which are
/// pipeline diagnostics). Comptime globals are evaluated once, here.
///
/// `cfg` is the build's, not the host's: `comptime if cfg.os` is decided in
/// here, so a cfg built afterwards for codegen came too late and every
/// cross-compile took the host's branch (#1315).
fn run_pipeline(path: &str, format: Format, cfg: &rask_comptime::CfgConfig) -> (rask_compiler::CompileResult, Option<String>) {
    run_pipeline_with(path, format, cfg, |_, _| {})
}

/// `run_pipeline` with a rewrite of the declarations before lowering — how
/// test blocks become functions.
fn run_pipeline_with(
    path: &str,
    format: Format,
    cfg: &rask_comptime::CfgConfig,
    transform: impl FnOnce(&mut Vec<rask_ast::decl::Decl>, &rask_check::TypedProgram),
) -> (rask_compiler::CompileResult, Option<String>) {
    let config = crate::compiler_config(cfg.clone());
    let output = rask_compiler::compile_file_with(path, &config, transform);

    // Build source_files for display
    let source_files: Vec<(std::path::PathBuf, String)> = if !output.source_files.is_empty() {
        output.source_files.clone()
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

    let source = std::fs::read_to_string(path).ok();
    (output.result.unwrap(), source)
}


/// Dump monomorphization output for a single file.
pub fn cmd_mono(path: &str, format: Format) {
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    let (result, _source) = run_pipeline(path, format, &cfg);
    let mono = result.mono;

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
                .map(|p| match &p.ty {
                    Some(ty) => format!("{}: {}", p.name, ty.source()),
                    None => p.name.clone(),
                })
                .collect();
            let ret = fn_decl
                .ret_ty
                .as_ref()
                .map(|t| format!(" -> {}", t.source()))
                .unwrap_or_default();
            let type_args = if mono_fn.type_args.is_empty() {
                String::new()
            } else {
                format!(
                    "<{}>",
                    mono_fn
                        .type_args
                        .iter()
                        .map(|b| format!("{}={}", b.param, b.ty))
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
    let cfg = rask_comptime::CfgConfig::from_host("debug", vec![]);
    let (result, source) = run_pipeline(path, format, &cfg);
    let functions = lower_or_exit(&result, path, source.as_deref(), &rask_compiler::emit::Entry::Main);
    if format == Format::Human {
        println!(
            "{} MIR ({} function{}) {}\n",
            "===".dimmed(),
            functions.len(),
            if functions.len() == 1 { "" } else { "s" },
            "===".dimmed()
        );
        for func in &functions {
            println!("{}", func);
        }
        println!();
        println!("{}", output::banner_ok("MIR lowering"));
    }
}

/// Dump MIR for a .rk file to stderr — the MIR codegen sees, after every pass.
/// Used for debugging codegen issues.
pub fn cmd_dump_mir(path: &str, format: Format, release: bool, target: Option<&str>) {
    let cfg = target_cfg_or_exit(target, release);
    let (result, source) = run_pipeline(path, format, &cfg);
    for func in &lower_or_exit(&result, path, source.as_deref(), &rask_compiler::emit::Entry::Main) {
        eprintln!("{}", func);
    }
}

/// `cmd_dump_mir` for a file's tests: the MIR `rask test` would compile, for
/// any target. Lowering needs no backend, so a target codegen can't reach yet
/// still gets its frontend and MIR checked (tests/target32_gate.sh).
pub fn cmd_test_dump_mir(path: &str, filter: Option<&str>, format: Format, target: Option<&str>) {
    let cfg = target_cfg_or_exit(target, false);
    let mut tests = Vec::new();
    let (result, source) = run_pipeline_with(path, format, &cfg, |decls, _typed| {
        tests = rask_compiler::runners::extract_tests(decls, filter);
    });
    if tests.is_empty() {
        eprintln!("{}: no runtime tests in {} to lower", output::error_label(), path);
        process::exit(1);
    }
    let entry = rask_compiler::emit::Entry::Tests(&tests);
    for func in &lower_or_exit(&result, path, source.as_deref(), &entry) {
        eprintln!("{}", func);
    }
}

/// MIR after the passes, or exit with what stopped it.
fn lower_or_exit(
    result: &rask_compiler::CompileResult,
    path: &str,
    source: Option<&str>,
    entry: &rask_compiler::emit::Entry,
) -> Vec<rask_mir::MirFunction> {
    let input = rask_compiler::emit::BackEnd::of(result, Some(path), source);
    match rask_compiler::emit::lower(&input, entry) {
        Ok(lowered) => lowered.functions,
        Err(errors) => {
            for e in &errors {
                eprintln!("{}: {}", output::error_label(), e);
            }
            eprintln!("{}", output::banner_fail("MIR lowering", errors.len()));
            process::exit(1);
        }
    }
}

/// The cfg for a single-file build, or exit on a target name nobody declared.
fn target_cfg_or_exit(target: Option<&str>, release: bool) -> rask_comptime::CfgConfig {
    let profile = if release { "release" } else { "debug" };
    let cfg = target
        .map_or(Ok(()), super::link::validate_target)
        .and_then(|()| super::link::build_cfg(target, profile, vec![]));
    match cfg {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}

/// Compile a single .rk file to a native executable.
/// Full pipeline: lex → parse → desugar → resolve → typecheck → ownership →
/// hidden-params → mono → MIR → Cranelift codegen → link with runtime.c.
pub fn cmd_compile(path: &str, output_path: Option<&str>, format: Format, quiet: bool, link_opts: &super::link::LinkOptions, release: bool, target: Option<&str>) {
    if let Some(Err(e)) = target.map(super::link::validate_build_target) {
        eprintln!("{}: {}", output::error_label(), e);
        process::exit(1);
    }
    let cfg = target_cfg_or_exit(target, release);
    let (result, source) = run_pipeline(path, format, &cfg);
    let build_mode = if release { rask_compiler::emit::BuildMode::Release } else { rask_compiler::emit::BuildMode::Debug };

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

    let input = rask_compiler::emit::BackEnd::of(&result, Some(path), source.as_deref());
    if let Err(errors) = super::compile::emit_object(
        &input, &rask_compiler::emit::Entry::Main, target, build_mode, &obj_path,
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

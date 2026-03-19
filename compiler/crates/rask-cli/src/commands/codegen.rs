// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Code generation commands: mono, mir, compile.

use colored::Colorize;
use rask_mono::MonoProgram;
use std::path::{Path, PathBuf};
use std::process;

use crate::{output, Format};

/// Run the full front-end pipeline + monomorphize. Exits on error.
/// Returns (mono, typed, decls, source) — source is the original .rk text.
fn run_pipeline(path: &str, format: Format) -> (MonoProgram, rask_types::TypedProgram, Vec<rask_ast::decl::Decl>, Option<String>, Vec<String>) {
    let mut result = super::pipeline::run_frontend(path, format);

    // Hidden parameter pass — desugar `using` clauses into explicit params
    rask_hidden_params::desugar_hidden_params(&mut result.decls);

    // Generate synthetic function bodies for auto-derived methods (compare, etc.)
    super::derive::generate_derived_methods(&mut result.decls, &result.typed);

    // Inject compiled stdlib functions + struct defs AFTER typechecking.
    // Type signatures come from BuiltinModule stubs during resolve/typecheck.
    // Function bodies and struct layouts are only needed at mono/codegen time.
    let stdlib_fn_decls = rask_stdlib::StubRegistry::compilable_decls();
    let stdlib_struct_defs = rask_stdlib::StubRegistry::compilable_struct_defs();
    if !stdlib_fn_decls.is_empty() {
        result.decls.extend(stdlib_fn_decls);
    }
    if !stdlib_struct_defs.is_empty() {
        result.decls.extend(stdlib_struct_defs);
    }

    let decls = result.decls.clone();
    let source = result.source.clone();
    let package_names = result.package_names.clone();

    // Monomorphize
    let mono = match rask_mono::monomorphize(&result.typed, &result.decls) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{}: monomorphization failed: {:?}", output::error_label(), e);
            process::exit(1);
        }
    };

    (mono, result.typed, decls, source, package_names)
}

/// Evaluate comptime const declarations and return serialized data.
///
/// When `mir_ctx` is provided, tries MIR-based evaluation first (via rask-miri),
/// falling back to the AST interpreter on failure.
pub fn evaluate_comptime_globals(
    decls: &[rask_ast::decl::Decl],
    cfg: Option<&rask_comptime::CfgConfig>,
    mir_ctx: Option<MirEvalContext<'_>>,
) -> std::collections::HashMap<String, rask_mir::ComptimeGlobalMeta> {
    use rask_ast::decl::DeclKind;
    use rask_ast::stmt::StmtKind;

    let mut comptime_interp = rask_comptime::ComptimeInterpreter::new();
    if let Some(c) = cfg {
        comptime_interp.inject_cfg(c);
    }
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
        // Try MIR-based evaluation first
        if let Some(ref ctx) = mir_ctx {
            if let Some(meta) = try_eval_comptime_mir(&name, init, ctx, decls) {
                globals.insert(name, meta);
                continue;
            }
        }

        // Fallback: AST interpreter
        comptime_interp.reset_branch_count();
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

/// Context for MIR-based comptime evaluation.
pub struct MirEvalContext<'a> {
    pub mono: &'a MonoProgram,
    pub typed: &'a rask_types::TypedProgram,
}

/// Try to evaluate a comptime expression via MIR lowering + MiriEngine.
/// Returns None on any failure (fallback to AST interpreter).
fn try_eval_comptime_mir(
    name: &str,
    init: &rask_ast::expr::Expr,
    ctx: &MirEvalContext<'_>,
    decls: &[rask_ast::decl::Decl],
) -> Option<rask_mir::ComptimeGlobalMeta> {
    use rask_ast::decl::{DeclKind, FnDecl};
    use rask_ast::expr::ExprKind;
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    // Extract the comptime body
    let body = match &init.kind {
        ExprKind::Comptime { body } => body.clone(),
        ExprKind::Call { func, args } => {
            // Comptime function call — find the function and use its body
            if let ExprKind::Ident(func_name) = &func.kind {
                let fn_decl = decls.iter().find_map(|d| match &d.kind {
                    DeclKind::Fn(f) if f.name == *func_name && f.is_comptime => Some(f),
                    _ => None,
                })?;
                // For comptime functions with args, bail to AST interpreter for now
                if !args.is_empty() {
                    return None;
                }
                fn_decl.body.clone()
            } else {
                return None;
            }
        }
        _ => return None,
    };

    // Determine return type from type checker
    let ret_ty_str = ctx.typed.node_types.get(&init.id)
        .map(|ty| format!("{ty:?}"))
        .and_then(|s| {
            // Map Type debug format to a type string the MIR lowerer understands
            match s.as_str() {
                "I64" => Some("i64"),
                "I32" => Some("i32"),
                "I16" => Some("i16"),
                "I8" => Some("i8"),
                "U64" => Some("u64"),
                "U32" => Some("u32"),
                "U16" => Some("u16"),
                "U8" => Some("u8"),
                "F64" => Some("f64"),
                "F32" => Some("f32"),
                "Bool" => Some("bool"),
                "Char" => Some("char"),
                "String" => Some("string"),
                "Unit" => None,
                _ => None,
            }
        });

    // Build a synthetic function wrapping the comptime block.
    // Add explicit `return <last_expr>` so MIR lowering captures the value.
    let mut synth_body = body;
    if let Some(last) = synth_body.last() {
        if let StmtKind::Expr(_) = &last.kind {
            // Last statement is an expression — wrap it in a return
            let last_owned = synth_body.pop().unwrap();
            if let StmtKind::Expr(e) = last_owned.kind {
                synth_body.push(Stmt {
                    id: NodeId(u32::MAX - 1),
                    kind: StmtKind::Return(Some(e)),
                    span: last_owned.span,
                });
            }
        }
    }

    let synth_name = format!("__comptime_{name}");
    let synth_decl = rask_ast::decl::Decl {
        id: NodeId(u32::MAX),
        kind: DeclKind::Fn(FnDecl {
            name: synth_name.clone(),
            type_params: vec![],
            params: vec![],
            ret_ty: ret_ty_str.map(|s| s.to_string()),
            context_clauses: vec![],
            body: synth_body,
            is_pub: false,
            is_private: false,
            is_comptime: true,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: Span::new(0, 0),
        }),
        span: Span::new(0, 0),
    };

    // Create a minimal MirContext for comptime lowering
    let empty_comptime = std::collections::HashMap::new();
    let empty_externs = std::collections::HashSet::new();
    let empty_packages = std::collections::HashSet::new();
    let empty_coercions = std::collections::HashMap::new();
    let empty_rewrites = std::collections::HashMap::new();
    let type_names: std::collections::HashMap<rask_types::TypeId, String> = ctx.typed.types.iter()
        .enumerate()
        .map(|(i, def)| {
            let tname = match def {
                rask_types::TypeDef::Struct { name, .. } => name.clone(),
                rask_types::TypeDef::Enum { name, .. } => name.clone(),
                rask_types::TypeDef::Trait { name, .. } => name.clone(),
                rask_types::TypeDef::Union { name, .. } => name.clone(),
                rask_types::TypeDef::NominalAlias { name, .. } => name.clone(),
            };
            (rask_types::TypeId(i as u32), tname)
        })
        .collect();

    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &ctx.mono.struct_layouts,
        enum_layouts: &ctx.mono.enum_layouts,
        node_types: &ctx.typed.node_types,
        type_names: &type_names,
        comptime_globals: &empty_comptime,
        extern_funcs: &empty_externs,
        package_modules: &empty_packages,
        trait_methods: std::collections::HashMap::new(),
        line_map: None,
        source_file: None,
        shared_elem_types: std::cell::RefCell::new(std::collections::HashMap::new()),
        comptime_interp: None,
        trait_coercions: &empty_coercions,
        call_rewrites: &empty_rewrites,
    };

    // Lower the synthetic function to MIR
    let mir_fns = rask_mir::lower::MirLowerer::lower_function(
        &synth_decl, decls, &mir_ctx,
    ).ok()?;

    let mir_fn = mir_fns.into_iter().next()?;

    // Also lower any comptime functions that might be called
    let mut engine = rask_miri::MiriEngine::new(Box::new(rask_miri::PureStdlib));
    engine.set_struct_layouts(ctx.mono.struct_layouts.clone());
    engine.set_enum_layouts(ctx.mono.enum_layouts.clone());

    // Register comptime-callable functions
    for decl in decls {
        if let DeclKind::Fn(f) = &decl.kind {
            if f.is_comptime {
                let fn_decl = rask_ast::decl::Decl {
                    id: decl.id,
                    kind: DeclKind::Fn(f.clone()),
                    span: decl.span,
                };
                if let Ok(fns) = rask_mir::lower::MirLowerer::lower_function(
                    &fn_decl, decls, &mir_ctx,
                ) {
                    for f in fns {
                        engine.register_function(f);
                    }
                }
            }
        }
    }

    engine.register_function(mir_fn);

    // Execute
    let result = engine.execute(&synth_name, vec![]).ok()?;

    // Convert to ComptimeGlobalMeta
    let type_prefix = result.type_prefix().to_string();
    let elem_count = result.elem_count();
    let bytes = result.serialize()?;

    Some(rask_mir::ComptimeGlobalMeta { bytes, elem_count, type_prefix })
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
    let (mono, _typed, _decls, _source, _package_names) = run_pipeline(path, format);

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
    let (mono, typed, decls, source, _package_names) = run_pipeline(path, format);

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
    let comptime_globals = evaluate_comptime_globals(&decls, Some(&cfg), Some(MirEvalContext { mono: &mono, typed: &typed }));
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
                rask_types::TypeDef::NominalAlias { name, .. } => name.clone(),
            };
            (rask_types::TypeId(i as u32), name)
        })
        .collect();
    let trait_methods: std::collections::HashMap<String, Vec<String>> = typed.types.iter()
        .filter_map(|def| {
            if let rask_types::TypeDef::Trait { name, methods, .. } = def {
                Some((name.clone(), methods.iter().map(|m| m.name.clone()).collect()))
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
pub fn cmd_compile(path: &str, output_path: Option<&str>, format: Format, quiet: bool, link_opts: &super::link::LinkOptions, release: bool, target: Option<&str>) {
    if let Some(t) = target {
        if let Err(e) = super::link::validate_target(t) {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }

    let (mono, typed, decls, source, package_names) = run_pipeline(path, format);
    let profile = if release { "release" } else { "debug" };
    let cfg = rask_comptime::CfgConfig::from_target_or_host(target, profile, vec![]);
    let comptime_globals = evaluate_comptime_globals(&decls, Some(&cfg), Some(MirEvalContext { mono: &mono, typed: &typed }));
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

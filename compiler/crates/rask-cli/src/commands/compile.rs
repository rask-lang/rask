// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared compilation pipeline: mono → MIR → Cranelift → object file.
//! Used by both `rask compile` (single file) and `rask build` (multi-package).

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_mono::MonoProgram;
use std::collections::HashMap;

/// Compile monomorphized program to an object file.
///
/// Takes ownership of the mono program and typed info, produces an object file
/// at `obj_path`. Returns the number of errors encountered (0 = success).
pub fn compile_to_object(
    mono: &MonoProgram,
    typed: &rask_types::TypedProgram,
    decls: &[rask_ast::decl::Decl],
    comptime_globals: &HashMap<String, rask_mir::ComptimeGlobalMeta>,
    source_file: Option<&str>,
    source_text: Option<&str>,
    target: Option<&str>,
    obj_path: &str,
    build_mode: rask_codegen::BuildMode,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    // Build mono decls with qualified names + extern decls
    let mut all_mono_decls: Vec<_> = mono.functions.iter().map(|f| {
        let mut decl = f.body.clone();
        if let rask_ast::decl::DeclKind::Fn(ref mut fn_decl) = decl.kind {
            fn_decl.name = f.name.clone();
        }
        decl
    }).collect();
    all_mono_decls.extend(
        decls.iter()
            .filter(|d| matches!(&d.kind, rask_ast::decl::DeclKind::Extern(_)))
            .cloned()
    );

    let extern_funcs = super::codegen::collect_extern_func_names(decls);
    let line_map = source_text.map(rask_ast::LineMap::new);
    let type_names: HashMap<rask_types::TypeId, String> = typed.types.iter()
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
        comptime_globals,
        extern_funcs: &extern_funcs,
        line_map: line_map.as_ref(),
        source_file,
        shared_elem_types: std::cell::RefCell::new(std::collections::HashMap::new()),
    };

    // MIR lowering
    let mut mir_functions = Vec::new();
    for mono_fn in &mono.functions {
        match rask_mir::lower::MirLowerer::lower_function_named(
            &mono_fn.body, &all_mono_decls, &mir_ctx, Some(&mono_fn.name)
        ) {
            Ok(mir_fns) => mir_functions.extend(mir_fns),
            Err(e) => errors.push(format!("MIR lowering '{}': {:?}", mono_fn.name, e)),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    if mir_functions.is_empty() {
        return Err(vec!["no functions to compile".to_string()]);
    }

    // Closure optimization
    rask_mir::optimize_all_closures(&mut mir_functions);

    // Self-concat → in-place append (eliminates O(n²) string building)
    rask_mir::optimize_string_concat(&mut mir_functions);

    // Generation check coalescing — eliminate redundant pool access checks
    rask_mir::coalesce_generation_checks(&mut mir_functions);

    // Cranelift codegen
    let mut codegen = match target {
        Some(t) => rask_codegen::CodeGenerator::new_with_target(t, build_mode),
        None => rask_codegen::CodeGenerator::new(build_mode),
    }.map_err(|e| vec![format!("codegen init: {}", e)])?;

    codegen.declare_runtime_functions()
        .map_err(|e| vec![e.to_string()])?;
    codegen.declare_stdlib_functions()
        .map_err(|e| vec![e.to_string()])?;

    // Declare extern functions
    let extern_sigs: Vec<_> = decls.iter().filter_map(|d| {
        if let rask_ast::decl::DeclKind::Extern(e) = &d.kind {
            Some(rask_codegen::ExternFuncSig {
                name: e.name.clone(),
                param_types: e.params.iter().map(|p| p.ty.clone()).collect(),
                ret_ty: e.ret_ty.clone(),
            })
        } else {
            None
        }
    }).collect();
    codegen.declare_extern_functions(&extern_sigs)
        .map_err(|e| vec![e.to_string()])?;

    codegen.declare_functions(mono, &mir_functions)
        .map_err(|e| vec![e.to_string()])?;
    codegen.register_strings(&mir_functions)
        .map_err(|e| vec![e.to_string()])?;
    codegen.register_comptime_globals(comptime_globals)
        .map_err(|e| vec![e.to_string()])?;

    for mir_fn in &mir_functions {
        if let Err(e) = codegen.gen_function(mir_fn) {
            errors.push(format!("codegen '{}': {}", mir_fn.name, e));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    codegen.emit_object(obj_path)
        .map_err(|e| vec![format!("emit object: {}", e)])?;

    Ok(())
}

/// Transform benchmark declarations into compilable functions, returning
/// (modified_decls, benchmark_metadata) where metadata is (display_name, fn_name).
pub fn extract_benchmarks(decls: &mut Vec<Decl>, filter: Option<&str>) -> Vec<(String, String)> {
    let mut benchmarks = Vec::new();
    let mut bench_fns = Vec::new();

    for decl in decls.iter() {
        if let DeclKind::Benchmark(b) = &decl.kind {
            if let Some(pat) = filter {
                if !b.name.contains(pat) {
                    continue;
                }
            }
            let fn_name = format!("__bench_{}", benchmarks.len());
            benchmarks.push((b.name.clone(), fn_name.clone()));
            bench_fns.push(Decl {
                id: decl.id,
                kind: DeclKind::Fn(FnDecl {
                    name: fn_name,
                    type_params: vec![],
                    params: vec![],
                    ret_ty: None,
                    context_clauses: vec![],
                    body: b.body.clone(),
                    is_pub: false,
                    is_comptime: false,
                    is_unsafe: false,
                    abi: None,
                    attrs: vec![],
                    doc: None,
                    span: decl.span,
                }),
                span: decl.span,
            });
        }
    }

    // Remove benchmark decls and user's main() — the runner replaces main
    decls.retain(|d| {
        !matches!(&d.kind, DeclKind::Benchmark(_))
            && !matches!(&d.kind, DeclKind::Fn(f) if f.name == "main")
    });

    // Add benchmark body functions
    decls.extend(bench_fns);

    // Generate a synthetic main() that calls each __bench_N() so monomorphization
    // marks them reachable. The real entry point (rask_main) is generated by codegen.
    if !benchmarks.is_empty() {
        let dummy_span = Span { start: 0, end: 0 };
        let mut body: Vec<Stmt> = benchmarks.iter().map(|(_, fn_name)| {
            Stmt {
                id: NodeId(0),
                kind: StmtKind::Expr(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Call {
                        func: Box::new(Expr {
                            id: NodeId(0),
                            kind: ExprKind::Ident(fn_name.clone()),
                            span: dummy_span,
                        }),
                        args: vec![],
                    },
                    span: dummy_span,
                }),
                span: dummy_span,
            }
        }).collect();
        body.push(Stmt {
            id: NodeId(0),
            kind: StmtKind::Return(None),
            span: dummy_span,
        });

        decls.push(Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: "main".to_string(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![],
                body,
                is_pub: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: dummy_span,
            }),
            span: dummy_span,
        });
    }

    benchmarks
}

/// Compile benchmarks to a native object file.
///
/// Like `compile_to_object` but transforms benchmark blocks into functions
/// and generates a runner entry point that calls `rask_bench_run` for each.
pub fn compile_benchmarks_to_object(
    mono: &MonoProgram,
    typed: &rask_types::TypedProgram,
    decls: &[Decl],
    comptime_globals: &HashMap<String, rask_mir::ComptimeGlobalMeta>,
    benchmarks: &[(String, String)],
    source_file: Option<&str>,
    source_text: Option<&str>,
    obj_path: &str,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let mut all_mono_decls: Vec<_> = mono.functions.iter().map(|f| {
        let mut decl = f.body.clone();
        if let DeclKind::Fn(ref mut fn_decl) = decl.kind {
            fn_decl.name = f.name.clone();
        }
        decl
    }).collect();
    all_mono_decls.extend(
        decls.iter()
            .filter(|d| matches!(&d.kind, DeclKind::Extern(_)))
            .cloned()
    );

    let extern_funcs = super::codegen::collect_extern_func_names(decls);
    let line_map = source_text.map(rask_ast::LineMap::new);
    let type_names: HashMap<rask_types::TypeId, String> = typed.types.iter()
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
        comptime_globals,
        extern_funcs: &extern_funcs,
        line_map: line_map.as_ref(),
        source_file,
        shared_elem_types: std::cell::RefCell::new(std::collections::HashMap::new()),
    };

    // MIR lowering — skip the synthetic main() since gen_benchmark_runner replaces it
    let mut mir_functions = Vec::new();
    for mono_fn in &mono.functions {
        if mono_fn.name == "main" {
            continue;
        }
        match rask_mir::lower::MirLowerer::lower_function_named(
            &mono_fn.body, &all_mono_decls, &mir_ctx, Some(&mono_fn.name)
        ) {
            Ok(mir_fns) => mir_functions.extend(mir_fns),
            Err(e) => errors.push(format!("MIR lowering '{}': {:?}", mono_fn.name, e)),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    if mir_functions.is_empty() && benchmarks.is_empty() {
        return Err(vec!["no functions or benchmarks to compile".to_string()]);
    }

    rask_mir::optimize_all_closures(&mut mir_functions);
    rask_mir::optimize_string_concat(&mut mir_functions);
    rask_mir::coalesce_generation_checks(&mut mir_functions);

    // Insert cleanup calls for benchmark functions to prevent memory leaks.
    // Scans for Vec_new/Map_new/Pool_new/string_new calls and inserts
    // matching free calls before return terminators.
    let bench_names: Vec<String> = benchmarks.iter().map(|(_, fn_name)| fn_name.clone()).collect();
    for mir_fn in &mut mir_functions {
        if bench_names.contains(&mir_fn.name) {
            insert_bench_cleanup(mir_fn);
        }
    }

    // Cranelift codegen — benchmarks always use release mode
    let mut codegen = rask_codegen::CodeGenerator::new(rask_codegen::BuildMode::Release)
        .map_err(|e| vec![format!("codegen init: {}", e)])?;

    codegen.declare_runtime_functions()
        .map_err(|e| vec![e.to_string()])?;
    codegen.declare_stdlib_functions()
        .map_err(|e| vec![e.to_string()])?;

    let extern_sigs: Vec<_> = decls.iter().filter_map(|d| {
        if let DeclKind::Extern(e) = &d.kind {
            Some(rask_codegen::ExternFuncSig {
                name: e.name.clone(),
                param_types: e.params.iter().map(|p| p.ty.clone()).collect(),
                ret_ty: e.ret_ty.clone(),
            })
        } else {
            None
        }
    }).collect();
    codegen.declare_extern_functions(&extern_sigs)
        .map_err(|e| vec![e.to_string()])?;

    codegen.declare_functions(mono, &mir_functions)
        .map_err(|e| vec![e.to_string()])?;
    codegen.register_strings(&mir_functions)
        .map_err(|e| vec![e.to_string()])?;
    codegen.register_comptime_globals(comptime_globals)
        .map_err(|e| vec![e.to_string()])?;

    for mir_fn in &mir_functions {
        if let Err(e) = codegen.gen_function(mir_fn) {
            errors.push(format!("codegen '{}': {}", mir_fn.name, e));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    // Generate the benchmark runner entry point
    codegen.gen_benchmark_runner(benchmarks)
        .map_err(|e| vec![format!("benchmark runner: {}", e)])?;

    codegen.emit_object(obj_path)
        .map_err(|e| vec![format!("emit object: {}", e)])?;

    Ok(())
}

/// Insert free calls at the end of benchmark functions for collection locals.
///
/// Scans for Call statements that create collections (Vec_new, Map_new, etc.)
/// and inserts corresponding free calls before return terminators.
fn insert_bench_cleanup(mir_fn: &mut rask_mir::MirFunction) {
    use rask_mir::{MirStmt, MirTerminator, MirOperand, FunctionRef};

    // Map: constructor name → free function name
    let ctor_to_free: &[(&str, &str)] = &[
        ("Vec_new", "Vec_free"),
        ("Vec_with_capacity", "Vec_free"),
        ("Vec_from", "Vec_free"),
        ("Map_new", "Map_free"),
        ("Map_from", "Map_free"),
        ("Pool_new", "Pool_free"),
        ("Pool_with_capacity", "Pool_free"),
        ("string_new", "string_free"),
        ("string_from", "string_free"),
    ];

    // Find locals that hold collection values (assigned from a constructor call)
    let mut to_free: Vec<(rask_mir::LocalId, String)> = Vec::new();
    for block in &mir_fn.blocks {
        for stmt in &block.statements {
            if let MirStmt::Call { dst: Some(dst_id), func, .. } = stmt {
                for (ctor, free_fn) in ctor_to_free {
                    if func.name == *ctor {
                        to_free.push((*dst_id, free_fn.to_string()));
                        break;
                    }
                }
            }
        }
    }

    if to_free.is_empty() {
        return;
    }

    // Insert free calls before every Return terminator
    for block in &mut mir_fn.blocks {
        if matches!(block.terminator, MirTerminator::Return { .. }) {
            for (local_id, free_fn) in to_free.iter().rev() {
                block.statements.push(MirStmt::Call {
                    dst: None,
                    func: FunctionRef { name: free_fn.clone(), is_extern: false },
                    args: vec![MirOperand::Local(*local_id)],
                });
            }
        }
    }
}

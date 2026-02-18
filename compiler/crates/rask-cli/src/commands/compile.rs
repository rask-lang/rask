// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared compilation pipeline: mono → MIR → Cranelift → object file.
//! Used by both `rask compile` (single file) and `rask build` (multi-package).

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
    let mir_ctx = rask_mir::lower::MirContext {
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
        comptime_globals,
        extern_funcs: &extern_funcs,
        line_map: line_map.as_ref(),
        source_file,
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

    // Cranelift codegen
    let mut codegen = match target {
        Some(t) => rask_codegen::CodeGenerator::new_with_target(t),
        None => rask_codegen::CodeGenerator::new(),
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

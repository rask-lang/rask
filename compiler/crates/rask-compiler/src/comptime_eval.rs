// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Comptime global evaluation — the single source of truth.
//!
//! Runs once as part of the compile pipeline (`finalize_compile`) and is also
//! called directly by the CLI's test/bench paths, which build their own
//! monomorphized program. It folds each comptime-initialized const, trying the
//! MIR/Miri fast path first and falling back to the AST interpreter, and
//! reports hard errors (overflow, divide-by-zero — type.overflow CT1/OV2) as
//! `Diagnostic`s so they flow through the normal pipeline error path.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_diagnostics::Diagnostic;
use rask_mir::ComptimeGlobalMeta;
use rask_mono::MonoProgram;
use rask_types::TypedProgram;

use crate::{is_comptime_init, CfgConfig};

/// Evaluate every comptime-initialized const. Returns the folded globals plus
/// any hard-error diagnostics (empty on success). Soft failures — an
/// expression that simply can't be folded at comptime — are not errors; that
/// value is left to run at runtime.
pub fn evaluate_comptime_globals(
    decls: &[Decl],
    typed: &TypedProgram,
    mono: &MonoProgram,
    cfg: Option<&CfgConfig>,
) -> (HashMap<String, ComptimeGlobalMeta>, Vec<Diagnostic>) {
    let mut comptime_interp = rask_comptime::ComptimeInterpreter::new();
    if let Some(c) = cfg {
        comptime_interp.inject_cfg(c);
    }
    comptime_interp.register_functions(decls);

    // Collect (name, init) from top-level consts and function-body consts.
    let mut comptime_consts: Vec<(String, &rask_ast::expr::Expr)> = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Const(c) => {
                if is_comptime_init(&c.init, decls) {
                    comptime_consts.push((c.name.clone(), &c.init));
                }
            }
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

    let mut globals = HashMap::new();
    let mut diags = Vec::new();

    for (name, init) in comptime_consts {
        // MIR/Miri fast path.
        let mut hard = None;
        if let Some(meta) = try_eval_comptime_mir(&name, init, typed, mono, decls, &mut hard) {
            globals.insert(name, meta);
            continue;
        }
        if let Some(err) = hard {
            let div0 = matches!(err, rask_miri::MiriError::DivisionByZero);
            diags.push(comptime_diagnostic(&err.to_string(), div0, init.span));
            continue;
        }

        // AST-interpreter fallback.
        comptime_interp.reset_branch_count();
        match comptime_interp.eval_expr(init) {
            Ok(val) => {
                if let Some(bytes) = val.serialize() {
                    globals.insert(name, ComptimeGlobalMeta {
                        bytes,
                        elem_count: val.elem_count(),
                        type_prefix: val.type_prefix().to_string(),
                    });
                }
            }
            Err(e) if e.is_hard() => {
                let div0 = matches!(e, rask_comptime::ComptimeError::DivisionByZero);
                diags.push(comptime_diagnostic(&e.to_string(), div0, init.span));
            }
            Err(_) => {} // soft: not foldable → runs at runtime
        }
    }

    (globals, diags)
}

/// Build a diagnostic for a hard comptime error at `span`. Overflow shares the
/// R0010 code with the interpreter's runtime check; divide-by-zero shares R0001.
fn comptime_diagnostic(message: &str, div_by_zero: bool, span: Span) -> Diagnostic {
    let (code, why) = if div_by_zero {
        ("R0001", "division by zero is undefined")
    } else {
        ("R0010", "comptime overflow is a compile error (type.overflow/CT1)")
    };
    Diagnostic::error(message.to_string())
        .with_code(code)
        .with_primary(span, "evaluated here")
        .with_why(why)
}

/// Try to fold a comptime const via MIR lowering + MiriEngine. Returns None on
/// soft failure (caller falls back to the AST interpreter); sets `hard_err` on a
/// genuine compile error (overflow, divide-by-zero).
fn try_eval_comptime_mir(
    name: &str,
    init: &rask_ast::expr::Expr,
    typed: &TypedProgram,
    mono: &MonoProgram,
    decls: &[Decl],
    hard_err: &mut Option<rask_miri::MiriError>,
) -> Option<ComptimeGlobalMeta> {
    use rask_ast::expr::ExprKind;

    // Extract the comptime body.
    let body = match &init.kind {
        ExprKind::Comptime { body } => body.clone(),
        ExprKind::Call { func, args } => {
            if let ExprKind::Ident(func_name) = &func.kind {
                let fn_decl = decls.iter().find_map(|d| match &d.kind {
                    DeclKind::Fn(f) if f.name == *func_name && f.is_comptime => Some(f),
                    _ => None,
                })?;
                // Comptime functions with args go to the AST interpreter.
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

    // Return type from the checker, mapped to a MIR type string.
    let ret_ty_str = typed.node_types.get(&init.id)
        .map(|ty| format!("{ty:?}"))
        .and_then(|s| match s.as_str() {
            "I64" => Some("i64"), "I32" => Some("i32"), "I16" => Some("i16"), "I8" => Some("i8"),
            "U64" => Some("u64"), "U32" => Some("u32"), "U16" => Some("u16"), "U8" => Some("u8"),
            "F64" => Some("f64"), "F32" => Some("f32"),
            "Bool" => Some("bool"), "Char" => Some("char"), "String" => Some("string"),
            _ => None,
        });

    // Wrap the block in a synthetic function whose last expression is returned.
    let mut synth_body = body;
    if let Some(last) = synth_body.last() {
        if let StmtKind::Expr(_) = &last.kind {
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
    let synth_decl = Decl {
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

    let empty_comptime = HashMap::new();
    let empty_externs = std::collections::HashSet::new();
    let empty_packages = std::collections::HashSet::new();
    let empty_coercions = HashMap::new();
    let empty_rewrites = HashMap::new();
    let empty_resource_types = std::collections::HashSet::new();
    let type_names: HashMap<rask_types::TypeId, String> = typed.types.iter()
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
        struct_layouts: &mono.struct_layouts,
        enum_layouts: &mono.enum_layouts,
        node_types: &typed.node_types,
        type_names: &type_names,
        comptime_globals: &empty_comptime,
        extern_funcs: &empty_externs,
        package_modules: &empty_packages,
        trait_methods: HashMap::new(),
        line_map: None,
        source_file: None,
        shared_elem_types: std::cell::RefCell::new(HashMap::new()),
        comptime_interp: None,
        trait_coercions: &empty_coercions,
        call_rewrites: &empty_rewrites,
        resource_types: &empty_resource_types,
    };

    let mir_fn = rask_mir::lower::MirLowerer::lower_function(&synth_decl, decls, &mir_ctx)
        .ok()?
        .into_iter()
        .next()?;

    let mut engine = rask_miri::MiriEngine::new(Box::new(rask_miri::PureStdlib));
    engine.set_struct_layouts(mono.struct_layouts.clone());
    engine.set_enum_layouts(mono.enum_layouts.clone());

    // Register comptime-callable functions the block may invoke.
    for decl in decls {
        if let DeclKind::Fn(f) = &decl.kind {
            if f.is_comptime {
                let fn_decl = Decl { id: decl.id, kind: DeclKind::Fn(f.clone()), span: decl.span };
                if let Ok(fns) = rask_mir::lower::MirLowerer::lower_function(&fn_decl, decls, &mir_ctx) {
                    for f in fns {
                        engine.register_function(f);
                    }
                }
            }
        }
    }
    engine.register_function(mir_fn);

    // A hard error (overflow, divide-by-zero) is a compile error — surface it
    // rather than silently falling back to the AST interpreter.
    let result = match engine.execute(&synth_name, vec![]) {
        Ok(r) => r,
        Err(e) if e.is_hard() => {
            *hard_err = Some(e);
            return None;
        }
        Err(_) => return None,
    };

    Some(ComptimeGlobalMeta {
        type_prefix: result.type_prefix().to_string(),
        elem_count: result.elem_count(),
        bytes: result.serialize()?,
    })
}

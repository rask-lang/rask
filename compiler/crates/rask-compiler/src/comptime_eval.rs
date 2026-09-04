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
/// the diagnostics.
///
/// Two kinds come back. A **hard** failure is a compile error: the evaluator
/// ran the block and the block panicked, ran past its branch quota, indexed off
/// the end, or asked for something that only exists at run time. CT2 says the
/// value is computed at compile time, so there is nothing to fall back to —
/// running the same block at startup would reach the same panic.
///
/// A **soft** failure is the evaluator's own gap: a construct it can't model
/// yet, like `Map.new`. The block runs at runtime instead and the const still
/// works, but the guarantee `comptime` was written for is gone, so it warns
/// (W0303) rather than saying nothing. It used to say nothing (#1072).
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

    let mut diags = Vec::new();

    // Collect (key, name, init, quota) from top-level consts, function bodies
    // and test bodies. The key is what the globals map is looked up by; the
    // name is what a diagnostic calls it, which is the local's own name and
    // not the mangled key — nobody wrote `__test_3$local$which`.
    //
    // Only a top-level const can carry `@comptime_quota`: a `let … = comptime`
    // inside a body has nowhere to write an attribute.
    let mut comptime_consts: Vec<(String, String, &rask_ast::expr::Expr, Option<usize>)> = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Const(c) => {
                if is_comptime_init(&c.init, decls) {
                    let mut quota = None;
                    if let Some(arg) = c.comptime_quota_arg() {
                        match arg.replace('_', "").parse::<usize>() {
                            Ok(n) if n > 0 => quota = Some(n),
                            _ => diags.push(
                                Diagnostic::error(format!(
                                    "`@comptime_quota` wants a positive number of branches, found `{arg}`"
                                ))
                                .with_code("E0383")
                                .with_primary(decl.span, "on this const")
                                .with_why("the quota is a branch count (ctrl.comptime/CT35); the default is 1,000"),
                            ),
                        }
                    }
                    comptime_consts.push((c.name.clone(), c.name.clone(), &c.init, quota));
                }
            }
            // Keyed by the body it lives in. A bare local name isn't unique
            // across a program, and this map is shared by the whole of it: two
            // functions each with a `let v = f()` collided, and one silently
            // read the other's value (#825).
            DeclKind::Fn(f) => {
                for (name, init) in comptime_lets(&f.body, decls) {
                    comptime_consts.push((
                        rask_mir::lower::comptime_local_key(&f.name, name),
                        name.clone(),
                        init,
                        None,
                    ));
                }
            }
            // A `test` block is a function by the time the compile path folds
            // (the runner rewrites it to `__test_N`), but not yet on the check
            // path — and check has to see the same consts or one backend warns
            // and the other doesn't. The key here is never looked up: check
            // throws the folded globals away and keeps the diagnostics.
            DeclKind::Test(t) => {
                for (name, init) in comptime_lets(&t.body, decls) {
                    comptime_consts.push((
                        rask_mir::lower::comptime_local_key(&format!("test${}", t.name), name),
                        name.clone(),
                        init,
                        None,
                    ));
                }
            }
            _ => {}
        }
    }

    let mut globals = HashMap::new();

    for (key, name, init, quota) in comptime_consts {
        // MIR/Miri fast path.
        let mut hard = None;
        if let Some(meta) = try_eval_comptime_mir(&key, init, typed, mono, decls, quota, &mut hard) {
            globals.insert(key, meta);
            continue;
        }
        if let Some(err) = hard {
            diags.push(comptime_diagnostic(&err.to_string(), miri_code(&err), init.span));
            continue;
        }

        // AST-interpreter fallback.
        comptime_interp.reset_branch_count();
        comptime_interp.set_quota(quota.unwrap_or(DEFAULT_BRANCH_QUOTA));
        match comptime_interp.eval_expr(init) {
            Ok(val) => match val.serialize() {
                Some(bytes) => {
                    globals.insert(key, ComptimeGlobalMeta {
                        bytes,
                        elem_count: val.elem_count(),
                        type_prefix: val.type_prefix().to_string(),
                        elem_type: val.elem_type_name().map(str::to_string),
                    });
                }
                // Folded, but the value has no constant representation — a
                // gap like any other, and the const runs at runtime.
                None => diags.push(no_constant_form(&name, val.type_name(), init.span)),
            },
            Err(e) if e.is_hard() => {
                diags.push(comptime_diagnostic(&e.to_string(), comptime_code(&e), init.span));
            }
            Err(e) => diags.push(fallback_warning(&name, &e, init.span)),
        }
    }

    (globals, diags)
}

/// Every `let x = comptime …` directly in a body, as (name, initializer).
fn comptime_lets<'a>(
    body: &'a [Stmt],
    decls: &[Decl],
) -> Vec<(&'a String, &'a rask_ast::expr::Expr)> {
    body.iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Let { name, init, .. } if is_comptime_init(init, decls) => Some((name, init)),
            _ => None,
        })
        .collect()
}

/// CT35: backwards branches allowed per fold unless `@comptime_quota` says more.
const DEFAULT_BRANCH_QUOTA: usize = 1_000;

/// Which code a hard AST-interpreter failure carries. Overflow and
/// divide-by-zero keep the R-codes they share with the same check at runtime;
/// everything else is a comptime-only compile error.
fn comptime_code(e: &rask_comptime::ComptimeError) -> (&'static str, &'static str) {
    use rask_comptime::ComptimeError as C;
    match e {
        C::DivisionByZero => ("R0001", "division by zero is undefined"),
        C::IntegerOverflow(_) => ("R0010", "comptime overflow is a compile error (type.overflow/CT1)"),
        _ => ("E0383", "a `comptime` const is computed while compiling, so there is no runtime to retry on (ctrl.comptime/CT2)"),
    }
}

/// Same, for the MIR/Miri path.
fn miri_code(e: &rask_miri::MiriError) -> (&'static str, &'static str) {
    use rask_miri::MiriError as M;
    match e {
        M::DivisionByZero => ("R0001", "division by zero is undefined"),
        M::IntegerOverflow(_) => ("R0010", "comptime overflow is a compile error (type.overflow/CT1)"),
        _ => ("E0383", "a `comptime` const is computed while compiling, so there is no runtime to retry on (ctrl.comptime/CT2)"),
    }
}

/// Build a diagnostic for a hard comptime error at `span`.
fn comptime_diagnostic(message: &str, (code, why): (&str, &str), span: Span) -> Diagnostic {
    Diagnostic::error(message.to_string())
        .with_code(code)
        .with_primary(span, "evaluated here")
        .with_why(why)
}

/// The evaluator couldn't model the block, so it runs at startup instead. Not
/// an error — the program is correct — but the const isn't a compile-time
/// constant any more, and saying nothing hid that for as long as it was true.
fn fallback_warning(name: &str, e: &rask_comptime::ComptimeError, span: Span) -> Diagnostic {
    use rask_comptime::ComptimeError as C;
    let (message, why) = match e {
        // `todo()` is deliberate — the block hasn't been written. Nothing is
        // missing from the evaluator, so don't blame it.
        C::Unimplemented(what) => (
            format!("`{name}` is `todo(\"{what}\")`, so it can't fold"),
            "the const runs at runtime instead, where the same `todo()` panics",
        ),
        _ => (
            format!("`{name}` runs at runtime: {e}"),
            "the comptime evaluator doesn't cover this yet; the value is still correct, it's just computed at startup",
        ),
    };
    Diagnostic::warning(message)
        .with_code("W0303")
        .with_primary(span, "not folded")
        .with_why(why)
}

/// Folded, but the result has no bytes to put in the binary.
fn no_constant_form(name: &str, ty: &str, span: Span) -> Diagnostic {
    Diagnostic::warning(format!("`{name}` folded to a `{ty}`, which has no constant form"))
        .with_code("W0303")
        .with_primary(span, "not stored as a constant")
        .with_why("only values with a constant representation become const data; this one is rebuilt at startup")
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
    quota: Option<usize>,
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

    // Miri's values stop at 64 bits — `MiriValue` has no `I128`/`U128` and
    // `impl_int_binop!` isn't instantiated for them — so a 128-bit fold computed
    // at the wrong width and handed back a truncated number. Hand those to the AST
    // interpreter, which carries them exactly (#824).
    let checker_ty = typed.node_types.get(&init.id).map(|ty| format!("{ty:?}"));
    if matches!(checker_ty.as_deref(), Some("I128") | Some("U128")) {
        return None;
    }

    // Return type from the checker, mapped to a MIR type string.
    let ret_ty_str = checker_ty
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

    // Everything else defaults to empty via MirContext::new, which is what a
    // comptime block wants: no package modules, no extern functions, no
    // comptime globals of its own. Before the constructor existed this was 21
    // fields written by hand, and `call_targets` had silently become an empty
    // map (#425, #727).
    let comptime_call_targets = mono.all_call_targets(typed);
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

    let mir_ctx = rask_mir::lower::MirContext::new(
        typed,
        &mono.struct_layouts,
        &mono.enum_layouts,
        &typed.node_types,
        &comptime_call_targets,
        &type_names,
    );

    let mir_fn = rask_mir::lower::MirLowerer::lower_function(&synth_decl, decls, &mir_ctx)
        .ok()?
        .into_iter()
        .next()?;

    let mut engine = rask_miri::MiriEngine::new(Box::new(rask_miri::PureStdlib));
    engine.set_branch_limit(quota.unwrap_or(DEFAULT_BRANCH_QUOTA) as u64);
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
        Err(e) => {
            if std::env::var_os("RASK_DBG_COMPTIME").is_some() {
                eprintln!("[comptime] MIR path gave up: {}", e);
            }
            return None;
        }
    };

    Some(ComptimeGlobalMeta {
        type_prefix: result.type_prefix().to_string(),
        elem_count: result.elem_count(),
        elem_type: result.elem_type_name().map(str::to_string),
        bytes: result.serialize()?,
    })
}

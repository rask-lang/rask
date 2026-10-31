// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Calls of C function-like macros (struct.c-interop/PP4).
//!
//! A macro has no symbol to call, and no types until a call gives it some. The
//! checker typed each call from its arguments by C's rules and lowering
//! carried that onto the call (`FunctionRef::c_macro`). Here a call whose
//! arguments are all constants becomes its value, computed the way C would,
//! so `c.SDL_BUTTON_MASK(c.SDL_BUTTON_LEFT)` costs nothing. Any other call
//! goes to a C function written for those argument types, compiled at link
//! like the variadic wrappers: `c.SDL_NS_TO_MS(t)` with a `u64` calls
//! `rask_macro_SDL_NS_TO_MS__u64(t)`, whose body is the macro's, so C's
//! promotions and wrapping are C's own.

use std::collections::BTreeMap;

use rask_ast::ty::TypeExpr;
use rask_c_parse::cexpr::{self, CVal, MacroCall};
use rask_mir::{MirConst, MirFunction, MirOperand, MirRValue, MirStmtKind};

use crate::variadic::Wrapper;

/// The constant a call folds to, when every argument is one and C defines
/// the result.
fn fold(call: &MacroCall, args: &[MirOperand]) -> Option<MirConst> {
    let mut vals = Vec::with_capacity(args.len());
    for (a, ty) in args.iter().zip(&call.args) {
        let v = match a {
            MirOperand::Constant(MirConst::Int(i)) => CVal::Int(*i as i128, ty.clone()),
            MirOperand::Constant(MirConst::Bool(b)) => CVal::Int(*b as i128, ty.clone()),
            MirOperand::Constant(MirConst::Float(f)) => CVal::Float(*f, ty.clone()),
            _ => return None,
        };
        vals.push(cexpr::convert(&v, ty)?);
    }
    let body = cexpr::parse_body(&call.body, &call.params).ok()?;
    Some(match cexpr::convert(&cexpr::eval(&body, &vals)?, &call.result)? {
        CVal::Int(v, ty) if rask_c_parse::cexpr::rask_scalar(&ty) == Some("bool") => MirConst::Bool(v != 0),
        // A `u64` above `i64::MAX` keeps its bits.
        CVal::Int(v, _) => MirConst::Int(v as i64),
        CVal::Float(f, _) => MirConst::Float(f),
        CVal::Str(_) => return None,
    })
}

/// The C function a call with these argument types goes to.
fn wrapper(call: &MacroCall) -> Option<Wrapper> {
    let arg_names: Vec<&str> = call.args.iter().map(cexpr::rask_scalar).collect::<Option<_>>()?;
    let ret = cexpr::rask_scalar(&call.result)?;
    let name = if arg_names.is_empty() {
        format!("rask_macro_{}", call.name)
    } else {
        format!("rask_macro_{}__{}", call.name, arg_names.join("_"))
    };
    let params: Vec<String> = call
        .params
        .iter()
        .zip(&call.args)
        .map(|(p, t)| format!("{} {}", cexpr::spell(t), p))
        .collect();
    let source = format!(
        "{ret} {name}({params}) {{ return ({ret})({body}); }}\n",
        ret = cexpr::spell(&call.result),
        name = name,
        params = if params.is_empty() { "void".to_string() } else { params.join(", ") },
        body = call.body,
    );
    Some(Wrapper {
        source,
        sig: rask_codegen::ExternFuncSig {
            name: name.clone(),
            link_name: None,
            param_types: arg_names.iter().map(|n| TypeExpr::named(*n)).collect(),
            ret_ty: Some(TypeExpr::named(ret)),
        },
    })
}

/// Fold or route every macro call in `functions`, and answer the C functions
/// the routed ones need (one per macro and argument types).
pub fn route_calls(functions: &mut [MirFunction]) -> Result<Vec<Wrapper>, Vec<String>> {
    let mut wrappers: BTreeMap<String, Wrapper> = BTreeMap::new();
    let mut errors = Vec::new();
    for f in functions.iter_mut() {
        for block in &mut f.blocks {
            for stmt in &mut block.statements {
                let MirStmtKind::Call { dst, func, args } = &mut stmt.kind else { continue };
                let Some(call) = func.c_macro.take() else { continue };
                if let (Some(d), Some(value)) = (*dst, fold(&call, args)) {
                    stmt.kind = MirStmtKind::Assign { dst: d, rvalue: MirRValue::Use(MirOperand::Constant(value)) };
                    continue;
                }
                let Some(w) = wrapper(&call) else {
                    errors.push(format!("`{}` is a C macro whose types have no C spelling here", call.name));
                    continue;
                };
                func.name = w.sig.name.clone();
                func.is_extern = true;
                wrappers.entry(w.sig.name.clone()).or_insert(w);
            }
        }
    }
    if errors.is_empty() { Ok(wrappers.into_values().collect()) } else { Err(errors) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_c_parse::CType;

    fn mask(arg: CType) -> MacroCall {
        MacroCall {
            name: "MASK".into(),
            body: "(((unsigned int)1) << (X - 1))".into(),
            params: vec!["X".into()],
            args: vec![arg],
            result: CType::UnsignedInt,
        }
    }

    #[test]
    fn constant_arguments_fold() {
        let folded = fold(&mask(CType::Int), &[MirOperand::Constant(MirConst::Int(3))]);
        assert!(matches!(folded, Some(MirConst::Int(4))), "{folded:?}");
    }

    #[test]
    fn what_c_leaves_undefined_doesnt_fold() {
        let folded = fold(&mask(CType::Int), &[MirOperand::Constant(MirConst::Int(40))]);
        assert!(folded.is_none());
    }

    #[test]
    fn a_run_time_call_gets_a_function_for_its_types() {
        let w = wrapper(&mask(CType::UnsignedChar)).unwrap();
        assert_eq!(w.sig.name, "rask_macro_MASK__u8");
        assert_eq!(
            w.source,
            "unsigned int rask_macro_MASK__u8(unsigned char X) { return (unsigned int)((((unsigned int)1) << (X - 1))); }\n"
        );
    }
}

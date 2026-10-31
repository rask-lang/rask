// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Calls to C variadic functions (struct.c-interop).
//!
//! A variadic call isn't an ordinary call on every target: x86-64 wants the
//! number of vector registers in `%al`, Apple's arm64 puts every variadic
//! argument on the stack, Windows duplicates floats into integer registers.
//! The C compiler knows all of that, so each shape a program calls with goes
//! through a C function with fixed parameters that makes the variadic call
//! itself: `printf(fmt, 2, 3.5)` calls `rask_va_printf__p_i32_f64(fmt, 2, 3.5)`.
//! It's written to the object's `.va.c` companion and compiled at link, the
//! way inline-function wrappers are. A wrapper parameter has the argument's
//! own type, so C's default promotions happen in C.

use std::collections::{BTreeMap, HashMap};

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::ty::TypeExpr;
use rask_mir::{MirConst, MirFunction, MirOperand, MirStmtKind, MirType};

/// The C spelling of a scalar a wrapper passes, its code in the wrapper's
/// name, and the Rask type codegen declares it with.
fn scalar(ty: &MirType) -> Option<(&'static str, &'static str, TypeExpr)> {
    let named = |n: &str| TypeExpr::named(n);
    Some(match ty {
        MirType::Bool => ("_Bool", "b", named("bool")),
        MirType::I8 => ("signed char", "i8", named("i8")),
        MirType::U8 => ("unsigned char", "u8", named("u8")),
        MirType::I16 => ("short", "i16", named("i16")),
        MirType::U16 => ("unsigned short", "u16", named("u16")),
        MirType::I32 => ("int", "i32", named("i32")),
        MirType::U32 => ("unsigned int", "u32", named("u32")),
        MirType::I64 => ("long long", "i64", named("i64")),
        MirType::U64 => ("unsigned long long", "u64", named("u64")),
        MirType::F32 => ("float", "f32", named("f32")),
        MirType::F64 => ("double", "f64", named("f64")),
        MirType::Ptr | MirType::FuncPtr(_) => {
            ("void *", "p", TypeExpr::RawPtr(Box::new(TypeExpr::Unit)))
        }
        _ => return None,
    })
}

fn operand_type(f: &MirFunction, op: &MirOperand) -> Option<MirType> {
    match op {
        MirOperand::Local(id) => f.local_ty(*id).cloned(),
        MirOperand::Constant(MirConst::Int(_)) => Some(MirType::I64),
        MirOperand::Constant(MirConst::Float(_)) => Some(MirType::F64),
        MirOperand::Constant(MirConst::Bool(_)) => Some(MirType::Bool),
        MirOperand::Constant(MirConst::String(_)) => Some(MirType::Ptr),
        _ => None,
    }
}

/// One wrapper: its C definition, and its declaration for codegen.
pub struct Wrapper {
    pub source: String,
    pub sig: rask_codegen::ExternFuncSig,
}

/// Point each variadic call in `functions` at a fixed-arity wrapper, and
/// answer the wrappers (one per distinct shape).
pub fn route_calls(functions: &mut [MirFunction], decls: &[Decl]) -> Result<Vec<Wrapper>, Vec<String>> {
    let variadic: HashMap<&str, (usize, Option<&TypeExpr>)> = decls
        .iter()
        .filter_map(|d| match &d.kind {
            DeclKind::Extern(e) if e.is_variadic => Some((e.name.as_str(), (e.params.len(), e.ret_ty.as_ref()))),
            _ => None,
        })
        .collect();
    if variadic.is_empty() {
        return Ok(Vec::new());
    }
    let mut wrappers: BTreeMap<String, Wrapper> = BTreeMap::new();
    let mut errors = Vec::new();
    for f in functions.iter_mut() {
        let f_ro = f.clone();
        for block in &mut f.blocks {
            for stmt in &mut block.statements {
                let MirStmtKind::Call { dst, func, args } = &mut stmt.kind else { continue };
                let Some(&(fixed, ret_te)) = variadic.get(func.name.as_str()).filter(|_| func.is_extern) else { continue };
                let mut parts = Vec::new();
                for a in args.iter() {
                    match operand_type(&f_ro, a).as_ref().and_then(scalar) {
                        Some(s) => parts.push(s),
                        None => {
                            errors.push(format!(
                                "a call to `{}` passes a value C's `...` can't take (a struct or a Rask aggregate)",
                                func.name
                            ));
                            parts.clear();
                            break;
                        }
                    }
                }
                if parts.len() != args.len() {
                    continue;
                }
                let codes: Vec<&str> = parts.iter().map(|p| p.1).collect();
                let name = format!("rask_va_{}__{}", func.name, codes.join("_"));
                let ret = dst.and_then(|d| f_ro.local_ty(d).cloned()).filter(|t| *t != MirType::Void);
                let ret_c = match ret.as_ref() {
                    None => Some("void"),
                    Some(t) => scalar(t).map(|s| s.0),
                };
                let Some(ret_c) = ret_c else {
                    errors.push(format!("`{}` returns something a variadic wrapper can't hand back", func.name));
                    continue;
                };
                if !wrappers.contains_key(&name) {
                    let fixed_c: Vec<&str> = parts[..fixed].iter().map(|p| p.0).collect();
                    let params: Vec<String> = parts.iter().enumerate().map(|(i, p)| format!("{} a{}", p.0, i)).collect();
                    let call_args: Vec<String> = (0..parts.len()).map(|i| format!("a{}", i)).collect();
                    let proto = if fixed_c.is_empty() { "...".to_string() } else { format!("{}, ...", fixed_c.join(", ")) };
                    let call = format!("{}({})", func.name, call_args.join(", "));
                    let body = if ret_c == "void" { format!("{};", call) } else { format!("return {};", call) };
                    let source = format!(
                        "{ret} {target}({proto});\n{ret} {name}({params}) {{ {body} }}\n",
                        ret = ret_c,
                        target = func.name,
                        proto = proto,
                        name = name,
                        params = if params.is_empty() { "void".to_string() } else { params.join(", ") },
                        body = body,
                    );
                    wrappers.insert(
                        name.clone(),
                        Wrapper {
                            source,
                            sig: rask_codegen::ExternFuncSig {
                                name: name.clone(),
                                link_name: None,
                                param_types: parts.iter().map(|p| p.2.clone()).collect(),
                                ret_ty: ret_te.cloned(),
                            },
                        },
                    );
                }
                func.name = name;
            }
        }
    }
    if errors.is_empty() { Ok(wrappers.into_values().collect()) } else { Err(errors) }
}

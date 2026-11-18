// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Say what an assignment into an optional does.
//!
//! Lowering writes `dst = src` whether `src` is already a `T?` (copy it) or a
//! plain `T` (build `Some(src)`). Codegen used to tell the two apart from the
//! types on both sides, and every reader of MIR had to repeat that guess. Each
//! case the guess missed was a bug: `a[0] ?? d` handed back an address (#783),
//! an optional copied from constant data came out `Some(<address>)` (#1554).
//!
//! This runs after the passes, which reason about `dst = src` as a copy, and
//! rewrites every wrapping assignment to `dst = WrapSome(src)`. Whatever is
//! left with an optional destination copies a whole optional (#1555).

use std::collections::HashMap;

use crate::{LocalId, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt, MirStmtKind, MirType};

/// Rewrite the wrapping assignments. Returns the assignments whose two sides
/// are different shapes with no wrap that could join them: a result assigned
/// from something that isn't one. Codegen would copy result-sized bytes out of
/// it, so each is a lowering bug, reported before codegen runs.
pub fn make_wraps_explicit(functions: &mut [MirFunction]) -> Vec<String> {
    let mut mismatches = Vec::new();
    for func in functions {
        make_wraps_explicit_in(func, &mut mismatches);
    }
    mismatches
}

fn make_wraps_explicit_in(func: &mut MirFunction, mismatches: &mut Vec<String>) {
    let types: HashMap<LocalId, MirType> = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| (l.id, l.ty.clone()))
        .collect();
    let mut next_id = types.keys().map(|id| id.0 + 1).max().unwrap_or(0);
    let mut new_locals: Vec<MirLocal> = Vec::new();

    for block in &mut func.blocks {
        let mut out: Vec<MirStmt> = Vec::with_capacity(block.statements.len());
        for stmt in block.statements.drain(..) {
            let MirStmtKind::Assign { dst, rvalue } = &stmt.kind else {
                out.push(stmt);
                continue;
            };
            if let (Some(MirType::Result { .. }), MirRValue::Use(MirOperand::Local(src))) = (types.get(dst), rvalue) {
                if !matches!(types.get(src), Some(MirType::Result { .. })) {
                    mismatches.push(format!(
                        "'{}': _{} is a result, and _{} assigned to it isn't",
                        func.name, dst.0, src.0
                    ));
                }
            }
            let Some(MirType::Option(payload)) = types.get(dst) else {
                out.push(stmt);
                continue;
            };
            if !wraps(rvalue, &types) {
                out.push(stmt);
                continue;
            }
            let dst = *dst;
            let operand = match rvalue {
                MirRValue::Use(op) => op.clone(),
                // A computed payload (`a + b`, a cast) goes through a local of
                // its own, so `WrapSome` only ever holds a value.
                other => {
                    let tmp = LocalId(next_id);
                    next_id += 1;
                    new_locals.push(MirLocal {
                        id: tmp,
                        name: None,
                        ty: payload.as_ref().clone(),
                        is_param: false,
                    });
                    out.push(MirStmt::new(
                        MirStmtKind::Assign { dst: tmp, rvalue: other.clone() },
                        stmt.span,
                    ));
                    MirOperand::Local(tmp)
                }
            };
            out.push(MirStmt::new(
                MirStmtKind::Assign { dst, rvalue: MirRValue::WrapSome(operand) },
                stmt.span,
            ));
        }
        block.statements = out;
    }
    func.locals.extend(new_locals);
}

/// Whether an assignment into an optional builds one around its value, as
/// opposed to copying one the rvalue already is.
fn wraps(rvalue: &MirRValue, types: &HashMap<LocalId, MirType>) -> bool {
    match rvalue {
        MirRValue::WrapSome(_) => false,
        MirRValue::Use(MirOperand::Local(src)) => !matches!(types.get(src), Some(MirType::Option(_))),
        MirRValue::Use(MirOperand::Constant(_)) => true,
        // A field read of an optional field hands back the field's address.
        MirRValue::Field { .. } => false,
        // An element stored inline is its whole wrapper, at an address (#783).
        MirRValue::ArrayIndex { base: MirOperand::Local(base), .. } => !matches!(
            types.get(base),
            Some(MirType::Array { elem, .. }) if elem.stored_inline_in_array()
        ),
        // `try convert` builds the optional itself.
        MirRValue::Convert { kind, .. } => !kind.is_optional(),
        _ => true,
    }
}

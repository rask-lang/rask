// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Address aliasing — which locals hold a raw address of another local's storage.
//!
//! `unsafe { native_fn(fd, s as i64) }` casts a string to an integer. The
//! integer is the address of `s`'s storage, so everything the native call reads
//! through it belongs to `s`. Liveness only sees a scalar, and without help it
//! calls the cast the last use of `s` and releases the buffer one statement
//! before the call that reads it (#1036).
//!
//! The map here says "this scalar points into that local", so liveness can keep
//! the pointee alive for as long as the address is.

use std::collections::HashMap;

use crate::analysis::uses;
use crate::{LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirType};

/// Scalar locals that hold the address of an aggregate local's storage.
#[derive(Debug, Default)]
pub struct AddrAliases {
    /// Address local → every aggregate its address may point into.
    ///
    /// More than one is normal before SSA, where `p = a as i64` in one arm and
    /// `p = b as i64` in the other both reach the same name. Keeping only the
    /// last one seen would leave whichever lost the race releasable while `p`
    /// is still live, which is the bug this whole map exists to stop.
    pointees: HashMap<LocalId, Vec<LocalId>>,
}

impl AddrAliases {
    pub fn build(func: &MirFunction) -> Self {
        let types: HashMap<LocalId, &MirType> = func
            .locals
            .iter()
            .chain(func.params.iter())
            .map(|l| (l.id, &l.ty))
            .collect();

        let mut pointees: HashMap<LocalId, Vec<LocalId>> = HashMap::new();

        // An address escapes into a scalar either by casting the aggregate
        // directly or by copying an address that already exists. Copies can
        // precede the cast in block order, so run to a fixed point.
        let mut changed = true;
        while changed {
            changed = false;
            for block in &func.blocks {
                for stmt in &block.statements {
                    let MirStmtKind::Assign { dst, rvalue } = &stmt.kind else { continue };
                    // The destination has to be a scalar — a cast that produces
                    // another aggregate is a conversion, not an address.
                    if types.get(dst).is_some_and(|t| t.passed_by_address()) {
                        continue;
                    }
                    let found: Vec<LocalId> = match rvalue {
                        MirRValue::Cast { value: MirOperand::Local(src), .. } => types
                            .get(src)
                            .is_some_and(|t| t.passed_by_address())
                            .then(|| vec![*src])
                            .unwrap_or_default(),
                        MirRValue::Ref(src) => vec![*src],
                        MirRValue::Use(MirOperand::Local(src)) => {
                            pointees.get(src).cloned().unwrap_or_default()
                        }
                        _ => Vec::new(),
                    };
                    for p in found {
                        let entry = pointees.entry(*dst).or_default();
                        if !entry.contains(&p) {
                            entry.push(p);
                            changed = true;
                        }
                    }
                }
            }
        }

        Self { pointees }
    }

    pub fn is_empty(&self) -> bool {
        self.pointees.is_empty()
    }

    /// Every local whose storage `local` may point into.
    pub fn pointees(&self, local: LocalId) -> &[LocalId] {
        self.pointees.get(&local).map_or(&[], |v| v.as_slice())
    }

    fn any_points_into(&self, addrs: &[LocalId], target: LocalId) -> bool {
        !self.pointees.is_empty()
            && addrs.iter().any(|a| self.pointees(*a).contains(&target))
    }

    /// True if `stmt` reads `local`, either by name or through an address of it.
    pub fn stmt_reads(&self, stmt: &MirStmt, local: LocalId) -> bool {
        uses::stmt_reads(stmt, local)
            || self.any_points_into(&uses::stmt_uses(stmt), local)
    }

    /// True if `term` reads `local`, either by name or through an address of it.
    pub fn terminator_reads(&self, term: &MirTerminator, local: LocalId) -> bool {
        uses::terminator_reads(term, local)
            || self.any_points_into(&uses::terminator_uses(term), local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockId, MirBlock, MirConst, MirLocal, MirStmt, MirTerminator, MirTerminatorKind,
    };

    fn func_with(locals: Vec<(u32, MirType)>, statements: Vec<MirStmt>) -> MirFunction {
        MirFunction {
            name: "t".into(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: locals
                .into_iter()
                .map(|(id, ty)| MirLocal { id: LocalId(id), name: None, ty, is_param: false })
                .collect(),
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements,
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn cast(dst: u32, src: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: LocalId(dst),
            rvalue: MirRValue::Cast {
                value: MirOperand::Local(LocalId(src)),
                target_ty: MirType::I64,
            },
        })
    }

    fn copy(dst: u32, src: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: LocalId(dst),
            rvalue: MirRValue::Use(MirOperand::Local(LocalId(src))),
        })
    }

    #[test]
    fn string_cast_to_int_is_an_address() {
        let f = func_with(
            vec![(0, MirType::String), (1, MirType::I64)],
            vec![cast(1, 0)],
        );
        let a = AddrAliases::build(&f);
        assert_eq!(a.pointees(LocalId(1)), &[LocalId(0)]);
    }

    #[test]
    fn address_survives_a_copy() {
        let f = func_with(
            vec![(0, MirType::String), (1, MirType::I64), (2, MirType::I64)],
            vec![cast(1, 0), copy(2, 1)],
        );
        let a = AddrAliases::build(&f);
        assert_eq!(a.pointees(LocalId(2)), &[LocalId(0)]);
    }

    #[test]
    fn scalar_casts_are_not_addresses() {
        let f = func_with(
            vec![(0, MirType::I32), (1, MirType::I64)],
            vec![cast(1, 0)],
        );
        assert!(AddrAliases::build(&f).is_empty());
    }

    /// Before SSA the same name can be assigned an address twice. Both
    /// strings have to stay alive — keeping only the last one seen would
    /// release the other while the address is still in use.
    #[test]
    fn two_definitions_keep_both_strings() {
        let f = func_with(
            vec![(0, MirType::String), (1, MirType::String), (2, MirType::I64)],
            vec![cast(2, 0), cast(2, 1)],
        );
        let a = AddrAliases::build(&f);
        assert_eq!(a.pointees(LocalId(2)), &[LocalId(0), LocalId(1)]);
    }

    #[test]
    fn a_call_on_the_address_reads_the_string() {
        let call = MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: crate::FunctionRef::extern_c("rask_io_write_string".into()),
            args: vec![
                MirOperand::Constant(MirConst::Int(1)),
                MirOperand::Local(LocalId(1)),
            ],
        });
        let f = func_with(
            vec![(0, MirType::String), (1, MirType::I64)],
            vec![cast(1, 0), call.clone()],
        );
        let a = AddrAliases::build(&f);
        assert!(a.stmt_reads(&call, LocalId(0)));
    }
}

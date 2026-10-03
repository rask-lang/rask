// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR function representation - control-flow graph of basic blocks.

use crate::{MirStmt, MirTerminator, MirType};

/// MIR function
#[derive(Debug, Clone)]
pub struct MirFunction {
    pub name: String,
    pub params: Vec<MirLocal>,
    pub ret_ty: MirType,
    pub locals: Vec<MirLocal>,
    pub blocks: Vec<MirBlock>,
    pub entry_block: BlockId,
    /// If true, export with C ABI (no name mangling)
    pub is_extern_c: bool,
    /// Source file path for runtime error messages (None in tests)
    pub source_file: Option<String>,
}

impl MirFunction {
    /// Every local of this type, each one once.
    ///
    /// `params` is a *subset* of `locals` — `BlockBuilder::add_param` pushes a
    /// parameter into both. Code that iterated `locals` chained with `params`
    /// therefore saw every parameter twice, which is how a string parameter
    /// ended up with two RcDecs for one RcInc: the buffer was freed while the
    /// caller still held it (#698).
    pub fn locals_of_type(&self, ty: &MirType) -> Vec<LocalId> {
        let mut seen = std::collections::HashSet::new();
        self.locals
            .iter()
            .chain(self.params.iter())
            .filter(|l| l.ty == *ty)
            .filter(|l| seen.insert(l.id))
            .map(|l| l.id)
            .collect()
    }

    /// The declared type of one local, parameter or not.
    pub fn local_ty(&self, id: LocalId) -> Option<&MirType> {
        self.locals
            .iter()
            .chain(self.params.iter())
            .find(|l| l.id == id)
            .map(|l| &l.ty)
    }
}

/// Basic block in CFG
#[derive(Debug, Clone)]
pub struct MirBlock {
    pub id: BlockId,
    pub statements: Vec<MirStmt>,
    pub terminator: MirTerminator,
}

/// Local variable or temporary
#[derive(Debug, Clone)]
pub struct MirLocal {
    pub id: LocalId,
    pub name: Option<String>,
    pub ty: MirType,
    pub is_param: bool,
    /// This local's type with its containers still named, when `ty` lost any.
    ///
    /// `ty` calls every container a bare `Ptr`, so a local holding
    /// `Vec<Point> or JsonError` or `(Vec<i64>, i64)` says nothing about the
    /// vector inside it, and releasing it walked straight past it. Lowering
    /// hands the full type to `BlockBuilder`, which keeps it here and keeps
    /// `ty` the plain spelling everything else compares against
    /// (`MirType::Container` says why that matters).
    pub unerased: Option<MirType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalId(pub u32);

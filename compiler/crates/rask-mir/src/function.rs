// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR function representation - control-flow graph of basic blocks.

use crate::{MirStmt, MirTerminator, MirType, Span};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

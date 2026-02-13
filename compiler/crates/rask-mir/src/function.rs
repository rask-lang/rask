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

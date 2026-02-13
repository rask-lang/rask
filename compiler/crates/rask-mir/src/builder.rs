// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! BlockBuilder - helper for CFG construction during lowering.

use crate::{BlockId, LocalId, MirBlock, MirFunction, MirLocal, MirStmt, MirTerminator, MirType};

pub struct BlockBuilder {
    function: MirFunction,
    current_block: BlockId,
    next_local_id: u32,
    next_block_id: u32,
}

impl BlockBuilder {
    pub fn new(name: String, ret_ty: MirType) -> Self {
        let entry_block = BlockId(0);
        let function = MirFunction {
            name,
            params: Vec::new(),
            ret_ty,
            locals: Vec::new(),
            blocks: vec![MirBlock {
                id: entry_block,
                statements: Vec::new(),
                terminator: MirTerminator::Unreachable,
            }],
            entry_block,
        };

        Self {
            function,
            current_block: entry_block,
            next_local_id: 0,
            next_block_id: 1,
        }
    }

    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        self.function.blocks.push(MirBlock {
            id,
            statements: Vec::new(),
            terminator: MirTerminator::Unreachable,
        });
        id
    }

    pub fn switch_to_block(&mut self, block: BlockId) {
        self.current_block = block;
    }

    pub fn alloc_temp(&mut self, ty: MirType) -> LocalId {
        let id = LocalId(self.next_local_id);
        self.next_local_id += 1;
        self.function.locals.push(MirLocal {
            id,
            name: None,
            ty,
            is_param: false,
        });
        id
    }

    pub fn alloc_local(&mut self, name: String, ty: MirType) -> LocalId {
        let id = LocalId(self.next_local_id);
        self.next_local_id += 1;
        self.function.locals.push(MirLocal {
            id,
            name: Some(name),
            ty,
            is_param: false,
        });
        id
    }

    pub fn add_param(&mut self, name: String, ty: MirType) -> LocalId {
        let id = LocalId(self.next_local_id);
        self.next_local_id += 1;
        let local = MirLocal {
            id,
            name: Some(name),
            ty,
            is_param: true,
        };
        self.function.params.push(local.clone());
        self.function.locals.push(local);
        id
    }

    pub fn push_stmt(&mut self, stmt: MirStmt) {
        let block = &mut self.function.blocks[self.current_block.0 as usize];
        block.statements.push(stmt);
    }

    pub fn terminate(&mut self, term: MirTerminator) {
        let block = &mut self.function.blocks[self.current_block.0 as usize];
        block.terminator = term;
    }

    /// Check if the current block still has the default Unreachable terminator.
    pub fn current_block_unterminated(&self) -> bool {
        matches!(
            self.function.blocks[self.current_block.0 as usize].terminator,
            MirTerminator::Unreachable
        )
    }

    pub fn finish(self) -> MirFunction {
        self.function
    }
}

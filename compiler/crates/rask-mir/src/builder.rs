// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! BlockBuilder - helper for CFG construction during lowering.

use crate::{BlockId, LocalId, MirBlock, MirFunction, MirLocal, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType};
use rask_ast::Span;

pub struct BlockBuilder {
    function: MirFunction,
    current_block: BlockId,
    next_local_id: u32,
    next_block_id: u32,
    /// Current source span — stamped onto statements/terminators with dummy spans.
    current_span: Span,
}

impl BlockBuilder {
    /// Return type of the function being built (read-only).
    pub fn ret_ty(&self) -> &MirType {
        &self.function.ret_ty
    }

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
                terminator: MirTerminator::dummy(MirTerminatorKind::Unreachable),
            }],
            entry_block,
            is_extern_c: false,
            source_file: None,
        };

        Self {
            function,
            current_block: entry_block,
            next_local_id: 0,
            next_block_id: 1,
            current_span: Span::new(0, 0),
        }
    }

    pub fn create_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        self.function.blocks.push(MirBlock {
            id,
            statements: Vec::new(),
            terminator: MirTerminator::dummy(MirTerminatorKind::Unreachable),
        });
        id
    }

    pub fn switch_to_block(&mut self, block: BlockId) {
        self.current_block = block;
    }

    /// The block statements are currently being appended to. Needed when a
    /// lowering builds several blocks and has to come back to terminate the
    /// one it started from.
    pub fn current_block(&self) -> BlockId {
        self.current_block
    }

    /// How many locals this function has so far. Used to mint unique synthetic
    /// binding names.
    pub fn local_count(&self) -> u32 {
        self.next_local_id
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

    /// Give an existing local a name. Used when a binding takes over a temp
    /// rather than copying out of it, so the dump still shows the source name.
    pub fn name_local(&mut self, id: LocalId, name: String) {
        if let Some(local) = self.function.locals.iter_mut().find(|l| l.id == id) {
            local.name = Some(name);
        }
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

    /// Look up the MIR type of a local by its ID.
    pub fn local_type(&self, id: LocalId) -> Option<MirType> {
        self.function.locals.iter()
            .find(|l| l.id == id)
            .map(|l| l.ty.clone())
    }

    /// Retype an already-allocated local.
    ///
    /// For inlining a closure body that returns early: the `return` needs a
    /// destination local before the body is lowered, but the body's type isn't
    /// known until after.
    pub fn set_local_type(&mut self, id: LocalId, ty: MirType) {
        if let Some(local) = self.function.locals.iter_mut().find(|l| l.id == id) {
            local.ty = ty;
        }
    }

    /// Set the current source span. Subsequent push_stmt/terminate calls
    /// will stamp this span onto any statement/terminator with a dummy span.
    pub fn set_span(&mut self, span: Span) {
        self.current_span = span;
    }

    pub fn current_span(&self) -> Span {
        self.current_span
    }

    pub fn push_stmt(&mut self, mut stmt: MirStmt) {
        // Stamp current span onto dummy-spanned statements
        if stmt.span.start == 0 && stmt.span.end == 0 && self.current_span.end > 0 {
            stmt.span = self.current_span;
        }
        let block = &mut self.function.blocks[self.current_block.0 as usize];
        block.statements.push(stmt);
    }

    pub fn terminate(&mut self, mut term: MirTerminator) {
        if term.span.start == 0 && term.span.end == 0 && self.current_span.end > 0 {
            term.span = self.current_span;
        }
        let block = &mut self.function.blocks[self.current_block.0 as usize];
        block.terminator = term;
    }

    /// Rewrite the function name of the last Call statement in the current block.
    /// Returns true if a Call was found and rewritten.
    pub fn rewrite_last_call(&mut self, from: &str, to: &str) -> bool {
        let block = &mut self.function.blocks[self.current_block.0 as usize];
        for stmt in block.statements.iter_mut().rev() {
            if let MirStmtKind::Call { func, .. } = &mut stmt.kind {
                if func.name == from {
                    func.name = to.to_string();
                    return true;
                }
            }
        }
        false
    }

    /// Replace the args of the call to `name` at `(block, index)`. Callers that
    /// only learn a call's argument later — `collect()` doesn't know its element
    /// size until the loop body has been lowered — record the position when they
    /// emit the call and fill it in afterwards.
    pub fn set_call_args(&mut self, block: BlockId, index: usize, name: &str, args: Vec<crate::MirOperand>) -> bool {
        let Some(stmt) = self.function.blocks
            .get_mut(block.0 as usize)
            .and_then(|b| b.statements.get_mut(index))
        else {
            return false;
        };
        match &mut stmt.kind {
            MirStmtKind::Call { func, args: call_args, .. } if func.name == name => {
                *call_args = args;
                true
            }
            _ => false,
        }
    }

    /// Block and index the next pushed statement will land at.
    pub fn next_stmt_pos(&self) -> (BlockId, usize) {
        (
            self.current_block,
            self.function.blocks[self.current_block.0 as usize].statements.len(),
        )
    }

    /// Read statements from a block (for inlining cleanup at exit points).
    pub fn block_stmts(&self, block: BlockId) -> &[MirStmt] {
        &self.function.blocks[block.0 as usize].statements
    }

    /// Read terminator kind from a block (to check if cleanup has sub-CFG).
    pub fn block_terminator_kind(&self, block: BlockId) -> Option<MirTerminatorKind> {
        self.function.blocks.get(block.0 as usize)
            .map(|b| b.terminator.kind.clone())
    }

    /// Get the destination local of the last Call statement in the current block.
    /// Used to check the result of an ensure body's cleanup call.
    pub fn last_call_dst(&self) -> Option<LocalId> {
        let block = &self.function.blocks[self.current_block.0 as usize];
        for stmt in block.statements.iter().rev() {
            if let MirStmtKind::Call { dst, .. } = &stmt.kind {
                return *dst;
            }
        }
        None
    }

    /// Check if the current block still has the default Unreachable terminator.
    pub fn current_block_unterminated(&self) -> bool {
        matches!(
            self.function.blocks[self.current_block.0 as usize].terminator.kind,
            MirTerminatorKind::Unreachable
        )
    }

    pub fn finish(self) -> MirFunction {
        self.function
    }
}

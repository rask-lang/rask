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

    /// Look up the MIR type of a local by its ID.
    pub fn local_type(&self, id: LocalId) -> Option<MirType> {
        self.function.locals.iter()
            .find(|l| l.id == id)
            .map(|l| l.ty.clone())
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

    /// Read statements from a block (for inlining cleanup at exit points).
    pub fn block_stmts(&self, block: BlockId) -> &[MirStmt] {
        &self.function.blocks[block.0 as usize].statements
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

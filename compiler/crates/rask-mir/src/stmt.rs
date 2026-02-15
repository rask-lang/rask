// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR statements and terminators.

use crate::{BlockId, FunctionRef, LocalId, MirOperand, MirRValue};

/// MIR statement - no control flow
#[derive(Debug, Clone)]
pub enum MirStmt {
    Assign {
        dst: LocalId,
        rvalue: MirRValue,
    },
    Store {
        addr: LocalId,
        offset: u32,
        value: MirOperand,
    },
    Call {
        dst: Option<LocalId>,
        func: FunctionRef,
        args: Vec<MirOperand>,
    },
    ResourceRegister {
        dst: LocalId,
        type_name: String,
        scope_depth: u32,
    },
    ResourceConsume {
        resource_id: LocalId,
    },
    ResourceScopeCheck {
        scope_depth: u32,
    },
    EnsurePush {
        cleanup_block: BlockId,
    },
    EnsurePop,
    PoolCheckedAccess {
        dst: LocalId,
        pool: LocalId,
        handle: LocalId,
    },
    SourceLocation {
        line: u32,
        col: u32,
    },
    /// Create a closure value: { func_ptr, env_ptr }.
    /// `captures` lists the locals whose values are stored into the environment.
    ClosureCreate {
        dst: LocalId,
        func_name: String,
        captures: Vec<ClosureCapture>,
    },
    /// Call through a closure value (indirect call with env_ptr prepended).
    ClosureCall {
        dst: Option<LocalId>,
        closure: LocalId,
        args: Vec<MirOperand>,
    },
    /// Load a captured variable from the closure environment pointer.
    LoadCapture {
        dst: LocalId,
        env_ptr: LocalId,
        offset: u32,
    },
}

/// A captured variable in a closure environment.
#[derive(Debug, Clone)]
pub struct ClosureCapture {
    pub local_id: LocalId,
    pub offset: u32,
    pub size: u32,
}

/// MIR terminator - ends a basic block
#[derive(Debug, Clone)]
pub enum MirTerminator {
    Return {
        value: Option<MirOperand>,
    },
    Goto {
        target: BlockId,
    },
    Branch {
        cond: MirOperand,
        then_block: BlockId,
        else_block: BlockId,
    },
    Switch {
        value: MirOperand,
        cases: Vec<(u64, BlockId)>,
        default: BlockId,
    },
    Unreachable,
    CleanupReturn {
        value: Option<MirOperand>,
        cleanup_chain: Vec<BlockId>,
    },
}

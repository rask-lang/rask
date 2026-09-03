// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR statements and terminators.

use crate::{BlockId, FunctionRef, LocalId, MirOperand, MirRValue};

pub use rask_ast::Span;

/// MIR statement kind — no control flow
#[derive(Debug, Clone)]
pub enum MirStmtKind {
    Assign {
        dst: LocalId,
        rvalue: MirRValue,
    },
    Store {
        addr: LocalId,
        offset: u32,
        value: MirOperand,
        /// Byte size of the store (e.g. 4 for f32, 1 for bool).
        /// When None, codegen uses the natural size of the value.
        store_size: Option<u32>,
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
    /// Register a runtime ensure hook so the body runs if the scope unwinds on
    /// panic (ctrl.panic/U1). `thunk` is a synthesized `fn(env_ptr)`; each
    /// capture occupies an 8-byte env slot holding the local's value — for an
    /// aggregate that value is its address, so the hook sees the live resource.
    EnsureHookRegister {
        thunk: String,
        captures: Vec<ClosureCapture>,
    },
    /// Deregister the most recent ensure hook. Emitted at the top of the inline
    /// cleanup block, so a normal scope exit removes the hook (the inline path
    /// runs the body) and only a panic reaches it through `rask_ensure_run_all`.
    EnsureHookPop,
    PoolCheckedAccess {
        dst: LocalId,
        pool: LocalId,
        handle: LocalId,
    },
    /// Create a closure value: heap-allocated `[func_ptr | captures...]`.
    /// `captures` lists the locals whose values are stored into the environment.
    /// `heap` controls allocation strategy: true = heap (escaping), false = stack (local-only).
    ClosureCreate {
        dst: LocalId,
        func_name: String,
        captures: Vec<ClosureCapture>,
        heap: bool,
    },
    /// Call through a closure value (indirect call with env_ptr prepended).
    ClosureCall {
        dst: Option<LocalId>,
        closure: LocalId,
        args: Vec<MirOperand>,
    },
    /// Get at a captured variable through the closure environment pointer.
    LoadCapture {
        dst: LocalId,
        env_ptr: LocalId,
        offset: u32,
        access: CaptureAccess,
    },
    /// Free a heap-allocated closure. Emitted before returns for owned closures.
    ClosureDrop {
        closure: LocalId,
    },
    /// Store into a fixed-size array element: base_ptr[index * elem_size] = value
    ArrayStore {
        base: LocalId,
        index: MirOperand,
        elem_size: u32,
        value: MirOperand,
    },
    /// Load the address of a comptime global data section.
    GlobalRef {
        dst: LocalId,
        name: String,
    },
    /// Box a concrete value into a trait object: heap-allocate, copy data, build fat pointer.
    TraitBox {
        dst: LocalId,
        value: MirOperand,
        concrete_type: String,
        trait_name: String,
        concrete_size: u32,
        vtable_name: String,
    },
    /// Call a method through a trait object's vtable.
    TraitCall {
        dst: Option<LocalId>,
        trait_object: LocalId,
        method_name: String,
        vtable_offset: u32,
        args: Vec<MirOperand>,
    },
    /// Drop a trait object: call vtable drop_fn, then free heap allocation.
    TraitDrop {
        trait_object: LocalId,
    },
    /// SSA phi node — selects a value based on which predecessor block was executed.
    /// Always appears at the start of a block; removed by de-SSA before codegen.
    Phi {
        dst: LocalId,
        args: Vec<(BlockId, MirOperand)>,
    },
    /// Increment refcount on a string value. Inserted by the RC insertion pass
    /// for string-typed copies. Codegen lowers to `rask_string_clone`.
    RcInc {
        local: LocalId,
    },
    /// Decrement refcount on a string value. Inserted by the RC insertion pass
    /// at last-use points. Codegen lowers to `rask_string_free`.
    RcDec {
        local: LocalId,
    },
    /// Release the strings an aggregate holds, at the point the aggregate dies.
    ///
    /// A struct field, an optional's payload, a `T or E`'s payload: the
    /// aggregate owns a reference to each of them, and without this nothing
    /// ever gives it back. `RcDec` can't do the job because it takes a value,
    /// and where the strings sit inside one of these depends on the layout —
    /// and on a tag, for anything with variants. Codegen walks the type and
    /// emits the branches; the pass that inserts this doesn't have the layouts
    /// and doesn't need them.
    ///
    /// A no-op for an aggregate holding no strings, which is most of them.
    RcDecContents {
        local: LocalId,
    },
}

/// MIR statement — wraps a kind with source span.
#[derive(Debug, Clone)]
pub struct MirStmt {
    pub kind: MirStmtKind,
    pub span: Span,
}

impl MirStmt {
    pub fn new(kind: MirStmtKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// Dummy span (0..0) for tests and synthetic transforms.
    pub fn dummy(kind: MirStmtKind) -> Self {
        Self { kind, span: Span::new(0, 0) }
    }
}

/// A captured variable in a closure environment.
#[derive(Debug, Clone)]
pub struct ClosureCapture {
    pub local_id: LocalId,
    pub offset: u32,
    pub size: u32,
    /// The env slot holds the variable's *address*, not a copy of it.
    ///
    /// This is what makes a scope-limited closure borrow rather than copy: the
    /// body reads and writes through the pointer, so a write inside the closure
    /// is a write to the enclosing variable (#1038). `own` and `spawn` captures
    /// are by value — neither may alias the definer.
    ///
    /// A by-ref slot is 8 bytes whatever the variable's type.
    pub by_ref: bool,
}

/// How a closure body reaches one of its captures.
///
/// The three answers differ in what the environment slot holds and therefore
/// in where a write inside the body lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAccess {
    /// The slot holds the value and the body only reads it. Spawn bodies run
    /// once from a state machine that rebuilds the environment itself.
    Value,
    /// The slot holds a pointer into the frame that built the closure, so a
    /// write through it is a write to that frame's variable (mem.closures/MC1).
    Borrowed,
    /// The slot *is* the variable: an `own` closure moved the value in, so the
    /// environment is its home for as long as the closure lives. Loading the
    /// value out instead threw every write away, and a counter closure answered
    /// 1 forever however many times it was called.
    Owned,
}

impl CaptureAccess {
    /// True when the body works through an address rather than a loaded value.
    pub fn is_addressed(self) -> bool {
        matches!(self, CaptureAccess::Borrowed | CaptureAccess::Owned)
    }
}

/// MIR terminator kind — ends a basic block
#[derive(Debug, Clone)]
pub enum MirTerminatorKind {
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

/// MIR terminator — wraps a kind with source span.
#[derive(Debug, Clone)]
pub struct MirTerminator {
    pub kind: MirTerminatorKind,
    pub span: Span,
}

impl MirTerminator {
    pub fn new(kind: MirTerminatorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// Dummy span (0..0) for tests and synthetic transforms.
    pub fn dummy(kind: MirTerminatorKind) -> Self {
        Self { kind, span: Span::new(0, 0) }
    }
}

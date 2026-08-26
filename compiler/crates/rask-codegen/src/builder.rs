// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, Function, GlobalValue, InstBuilder, MemFlags, SourceLoc, StackSlot, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::{HashMap, HashSet};

use rask_mir::FieldAccess;
use rask_mir::{BinOp, BlockId, LocalId, MirConst, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType, UnaryOp};
use rask_mono::{StructLayout, EnumLayout};
use rask_types::Type as RaskType;
use crate::dispatch::{ArgAdapt, RetAdapt};
use crate::types::mir_to_cranelift_type;
use crate::{BuildMode, CodegenError, CodegenResult};

/// Copy `size` bytes from `src_ptr + src_off` to `dst_ptr + dst_off`.
///
/// The one canonical aggregate byte-copy in the backend. It emits the full
/// 8→4→2→1 ladder, so every trailing size is covered. A hand-inlined copy that
/// skipped the 2-byte step silently dropped a byte for any size ≡ 2,3,6,7
/// (mod 8) (#365) — which is exactly the kind of bug that stays invisible
/// until someone adds a struct with an odd layout.
///
/// Route ALL aggregate copies through this. Do not re-inline the ladder.
pub(crate) fn copy_bytes(
    builder: &mut ClifFunctionBuilder,
    src_ptr: Value,
    src_off: i32,
    dst_ptr: Value,
    dst_off: i32,
    size: u32,
) {
    let size = size as i32;
    let mut off = 0i32;
    for (chunk, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
        while size - off >= chunk {
            let val = builder.ins().load(ty, MemFlags::new(), src_ptr, src_off + off);
            builder.ins().store(MemFlags::new(), val, dst_ptr, dst_off + off);
            off += chunk;
        }
    }
}

// Checked-arithmetic panic messages (type.overflow). Registered as string
// globals unconditionally (see `register_strings`) so the message prints in
// both debug and release builds — OV4 requires consistent behavior.
//
// Each names the type that overflowed and the range it holds. Native used to
// print "integer overflow in addition" and nothing else, where the interpreter
// printed "integer overflow: 200 + 100 exceeds u8 range [0, 255]" for the same
// event — a user who hit one natively had no way to tell which of the
// expression's widths ran out. The operand values can't be in a static message,
// but the type and its range can, and those are what the reader needs.
pub(crate) const OV_DIV_ZERO: &str = "division by zero";

/// Which check fired, for picking the message.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OvKind {
    Add,
    Sub,
    Mul,
    Neg,
    DivMinByNegOne,
    Shift,
}

/// The operator symbols the runtime formatter splices between the operands.
/// Registered as string globals alongside the messages below.
pub(crate) const OVERFLOW_OP_SYMBOLS: &[&str] = &["+", "-", "*", "/"];

/// Build the seven messages for one integer type, and the accessor over them.
macro_rules! overflow_messages {
    ($($ty:literal, $bits:literal, $unsigned:literal, $range:literal;)*) => {
        /// Every overflow message codegen can emit, registered up front.
        ///
        /// The full sentences are what the 128-bit helper path and the
        /// unknown-width fallback still emit whole. Everything else emits a
        /// `tail` (below) and lets the runtime splice the operands in front.
        pub(crate) const OVERFLOW_MESSAGES: &[&str] = &[
            OV_DIV_ZERO,
            $(
                concat!("integer overflow: addition exceeds ", $ty, " range ", $range),
                concat!("integer overflow: subtraction exceeds ", $ty, " range ", $range),
                concat!("integer overflow: multiplication exceeds ", $ty, " range ", $range),
                concat!("integer overflow: negation exceeds ", $ty, " range ", $range),
                concat!("integer overflow: dividing ", $ty, " MIN by -1 exceeds ", $ty, " range ", $range),
                concat!("shift amount exceeds ", $ty, " bit width (", stringify!($bits), ")"),
                // F3 tails: the static half of an operand-carrying message.
                concat!($ty, " range ", $range),
                concat!($ty, " bit width (", stringify!($bits), ")"),
            )*
        ];

        /// The "<type> range [min, max]" half of an overflow message, for the
        /// runtime formatter to put the operands in front of.
        ///
        /// Every width has one; the guards pick the i64 or the i128 formatter
        /// from the operand's own Cranelift type.
        pub(crate) fn overflow_range_tail(bits: u32, unsigned: bool) -> Option<&'static str> {
            match (bits, unsigned) {
                $( ($bits, $unsigned) => Some(concat!($ty, " range ", $range)), )*
                _ => None,
            }
        }

        /// The "<type> bit width (n)" half of a shift-amount message. Capped at
        /// 64 bits: a 128-bit shift amount is itself an i128, and the formatter
        /// for that case would have no caller worth its weight.
        pub(crate) fn shift_width_tail(bits: u32, unsigned: bool) -> Option<&'static str> {
            if bits > 64 {
                return None;
            }
            match (bits, unsigned) {
                $( ($bits, $unsigned) => Some(concat!($ty, " bit width (", stringify!($bits), ")")), )*
                _ => None,
            }
        }

        /// The message for one check on one integer type.
        ///
        /// `bits` and `unsigned` come from the reconciled operand type at the
        /// operation, so they name the width that actually ran out rather than
        /// the widest one in the expression.
        pub(crate) fn overflow_message(kind: OvKind, bits: u32, unsigned: bool) -> &'static str {
            match (bits, unsigned) {
                $(
                    ($bits, $unsigned) => match kind {
                        OvKind::Add => concat!("integer overflow: addition exceeds ", $ty, " range ", $range),
                        OvKind::Sub => concat!("integer overflow: subtraction exceeds ", $ty, " range ", $range),
                        OvKind::Mul => concat!("integer overflow: multiplication exceeds ", $ty, " range ", $range),
                        OvKind::Neg => concat!("integer overflow: negation exceeds ", $ty, " range ", $range),
                        OvKind::DivMinByNegOne => concat!("integer overflow: dividing ", $ty, " MIN by -1 exceeds ", $ty, " range ", $range),
                        OvKind::Shift => concat!("shift amount exceeds ", $ty, " bit width (", stringify!($bits), ")"),
                    },
                )*
                // Not a width the language has. Falling back to the i64 wording
                // beats printing a range that isn't this type's.
                _ => match kind {
                    OvKind::Add => "integer overflow in addition",
                    OvKind::Sub => "integer overflow in subtraction",
                    OvKind::Mul => "integer overflow in multiplication",
                    OvKind::Neg => "integer overflow in negation",
                    OvKind::DivMinByNegOne => "integer overflow in division (MIN / -1)",
                    OvKind::Shift => "shift amount exceeds bit width",
                },
            }
        }
    };
}

overflow_messages! {
    "i8",   8,   false, "[-128, 127]";
    "i16",  16,  false, "[-32768, 32767]";
    "i32",  32,  false, "[-2147483648, 2147483647]";
    "i64",  64,  false, "[-9223372036854775808, 9223372036854775807]";
    "i128", 128, false, "[-170141183460469231731687303715884105728, 170141183460469231731687303715884105727]";
    "u8",   8,   true,  "[0, 255]";
    "u16",  16,  true,  "[0, 65535]";
    "u32",  32,  true,  "[0, 4294967295]";
    "u64",  64,  true,  "[0, 18446744073709551615]";
    "u128", 128, true,  "[0, 340282366920938463463374607431768211455]";
}

/// The fallbacks, also registered so an unexpected width still finds a global.
pub(crate) const OVERFLOW_FALLBACKS: &[&str] = &[
    "integer overflow in addition",
    "integer overflow in subtraction",
    "integer overflow in multiplication",
    "integer overflow in negation",
    "integer overflow in division (MIN / -1)",
    "shift amount exceeds bit width",
];

/// Read-only context bundling parameters for lowering functions.
struct CodegenCtx<'a> {
    var_map: &'a HashMap<LocalId, Variable>,
    locals: &'a [rask_mir::MirLocal],
    /// Parameters live in their own list, so `locals` alone never resolves one.
    params: &'a [rask_mir::MirLocal],
    /// Declared param types of every Rask function, by MIR name
    fn_param_types: &'a HashMap<String, Vec<MirType>>,
    func_refs: &'a HashMap<String, FuncRef>,
    struct_layouts: &'a [StructLayout],
    enum_layouts: &'a [EnumLayout],
    string_globals: &'a HashMap<String, GlobalValue>,
    comptime_globals: &'a HashMap<String, GlobalValue>,
    vtable_globals: &'a HashMap<String, GlobalValue>,
    panicking_fns: &'a HashSet<String>,
    internal_fns: &'a HashSet<String>,
    stack_slot_map: &'a HashMap<LocalId, (StackSlot, u32)>,
    block_map: &'a HashMap<BlockId, Block>,
    build_mode: BuildMode,
    source_file: Option<&'a str>,
    line_map: Option<&'a rask_ast::LineMap>,
    current_line: u32,
    current_col: u32,
    /// Byte offset of the current MIR statement being lowered
    current_span_start: u32,
    ret_ty: &'a MirType,
    is_main: bool,
    /// An `extern "C"` export: its body is bracketed with the FFI panic
    /// boundary, so a panic inside aborts rather than unwinding into C frames.
    is_extern_c: bool,
    adapt_table: &'a HashMap<String, (ArgAdapt, RetAdapt)>,
}

/// How to compare one slot of an aggregate. Struct and enum-payload fields
/// carry a Rask type from the layout; tuple and array elements carry a MIR one.
enum FieldKind {
    Rask(RaskType, u32),
    Mir(MirType),
}

/// Result of adapting a stdlib call for the typed runtime API.
enum CallAdapt {
    /// No special post-call handling needed
    None,
    /// Result is void* — load the i64 value from the returned pointer
    DerefResult,
    /// Result is void* — wrap as Option: NULL→None(tag=1), non-NULL→Some(tag=0, deref)
    DerefOption,
    /// Pop-style: value written to this stack slot by callee
    PopOutParam(StackSlot),
    /// The callee wrote the payload straight into the destination `T?`'s slot
    /// and returned 1 (wrote) or 0 (nothing). Only the tag is left to set.
    OptionOutParam(StackSlot),
    /// String out-param: callee wrote 16-byte RaskStr to this slot.
    /// Result is the slot address (pointer), not a loaded value.
    StringOutParam(StackSlot),
    /// Result is void* pointing to 16-byte string element in Vec.
    /// Copy to dst's stack slot.
    DerefStringElement,
    /// Receiver.try_recv: call returned a channel status; the payload was
    /// written into the given slot. Build a `T or E` Result in dst —
    /// status==OK → Ok(payload of `elem_size` bytes), else → Err.
    TryRecvResult(StackSlot, u32),
    /// Receiver.receive on a struct element: the call wrote the value into the
    /// buffer it returns (and panicked if the channel was closed), so the result
    /// is always Ok. Copy `elem_size` bytes out of it into dst's payload.
    RecvStructOk(u32),
    /// parse: the call returned 0/1; the value was written into the given slot.
    /// Build a `T or ParseError` — status==0 → Ok(value), else Err.
    /// Carries (slot, type the runtime wrote, type the destination wants).
    ParseResult(StackSlot, Type, Type),
    /// Same idea for a string out-param: the call returned 0/1 and wrote a
    /// 16-byte RaskStr into the given slot. Build a `string or E` — status==0
    /// → Ok(string), else Err. Used by `io.read_line`, where the error case is
    /// end of input. The second slot carries the failure message, so a real
    /// `IoError.Other(msg)` can be built instead of a guessed variant.
    StringResult(StackSlot, StackSlot),
    /// join/cancel: the call returned how the task ended and wrote its value
    /// into the first slot and any panic message into the second (a 16-byte
    /// RaskStr). Build a `T or JoinError` in dst.
    JoinOutcome(StackSlot, StackSlot),
}

/// `IoError.UnexpectedEof`'s variant index, counting the declaration order in
/// stdlib/io.rk. Reordering that enum has to be reflected here.
const IO_ERROR_UNEXPECTED_EOF: i64 = 6;

/// How a joined task ended, as the runtime reports it. Mirrors the
/// `RASK_JOIN_*` defines in runtime/rask_runtime.h.
const RASK_JOIN_OK: i64 = 0;
const RASK_JOIN_PANICKED: i64 = 1;

/// How a string-out-param call ended, as the runtime reports it. Mirrors the
/// `RASK_STROUT_*` defines in runtime/rask_runtime.h.
const RASK_STROUT_EOF: i64 = 2;

pub struct FunctionBuilder<'a> {
    func: &'a mut Function,
    builder_ctx: FunctionBuilderContext,
    mir_fn: &'a MirFunction,
    /// Pre-imported function references (MIR name → Cranelift FuncRef)
    func_refs: &'a HashMap<String, FuncRef>,
    /// Struct layouts from monomorphization
    struct_layouts: &'a [StructLayout],
    /// Enum layouts from monomorphization
    enum_layouts: &'a [EnumLayout],
    /// String literal data (content → GlobalValue for the data address)
    string_globals: &'a HashMap<String, GlobalValue>,
    /// Comptime global data (const name → GlobalValue for the data address)
    comptime_globals: &'a HashMap<String, GlobalValue>,
    /// VTable data globals (vtable name → GlobalValue for the vtable address)
    vtable_globals: &'a HashMap<String, GlobalValue>,
    /// MIR names of stdlib functions that can panic at runtime
    panicking_fns: &'a HashSet<String>,
    /// Names of functions compiled as Rask code (vs C stdlib)
    internal_fns: &'a HashSet<String>,
    /// Declared param types of every Rask function, by MIR name
    fn_param_types: &'a HashMap<String, Vec<MirType>>,
    /// Debug vs Release — controls whether pool access is inlined
    build_mode: BuildMode,

    /// Map MIR block IDs to Cranelift blocks
    block_map: HashMap<BlockId, Block>,
    /// Map MIR locals to Cranelift variables
    var_map: HashMap<LocalId, Variable>,

    /// Stack slots allocated for aggregate locals (struct, enum, result, etc.)
    /// Maps LocalId → (StackSlot, byte_size) so calls returning aggregates can
    /// memcpy into the caller's slot instead of storing a dangling callee pointer.
    stack_slot_map: HashMap<LocalId, (StackSlot, u32)>,

    /// Current source location tracked from SourceLocation statements
    current_line: u32,
    current_col: u32,

    /// Line map for converting byte offsets → line:col
    line_map: Option<&'a rask_ast::LineMap>,

    /// Table-driven call adaptation (populated from dispatch::stdlib_entries)
    adapt_table: HashMap<String, (ArgAdapt, RetAdapt)>,
}

impl<'a> FunctionBuilder<'a> {
    pub fn new(
        func: &'a mut Function,
        mir_fn: &'a MirFunction,
        func_refs: &'a HashMap<String, FuncRef>,
        struct_layouts: &'a [StructLayout],
        enum_layouts: &'a [EnumLayout],
        string_globals: &'a HashMap<String, GlobalValue>,
        comptime_globals: &'a HashMap<String, GlobalValue>,
        vtable_globals: &'a HashMap<String, GlobalValue>,
        panicking_fns: &'a HashSet<String>,
        internal_fns: &'a HashSet<String>,
        fn_param_types: &'a HashMap<String, Vec<MirType>>,
        build_mode: BuildMode,
    ) -> CodegenResult<Self> {
        Ok(FunctionBuilder {
            func,
            builder_ctx: FunctionBuilderContext::new(),
            mir_fn,
            func_refs,
            struct_layouts,
            enum_layouts,
            string_globals,
            comptime_globals,
            vtable_globals,
            panicking_fns,
            internal_fns,
            fn_param_types,
            build_mode,
            block_map: HashMap::new(),
            var_map: HashMap::new(),
            stack_slot_map: HashMap::new(),
            current_line: 0,
            current_col: 0,
            line_map: None,
            adapt_table: crate::dispatch::build_adapt_table(),
        })
    }

    /// Set the line map for converting byte offsets to line:col in assert messages.
    pub fn set_line_map(&mut self, line_map: &'a rask_ast::LineMap) {
        self.line_map = Some(line_map);
    }

    /// Build the Cranelift IR from MIR.
    pub fn build(&mut self) -> CodegenResult<()> {
        // Pre-compute stack allocation sizes before builder borrows self.func.
        // Entries: (local_id, byte size) for each aggregate local.
        let stack_allocs: Vec<(LocalId, u32)> = self.mir_fn.locals.iter()
            .filter(|l| !l.is_param)
            .filter_map(|l| {
                let size = Self::resolve_type_alloc_size(
                    &l.ty, self.struct_layouts, self.enum_layouts,
                );
                size.filter(|&s| s > 0).map(|s| (l.id, s))
            })
            .collect();

        // Collect cleanup-only blocks (appear in CleanupReturn chains)
        // and their transitive sub-blocks (handler/done blocks reachable
        // from cleanup blocks). These are excluded from normal codegen
        // and processed as part of shared cleanup blocks instead.
        //
        // A block is cleanup-only IFF every path reaching it goes through a
        // CleanupReturn terminator. Blocks reachable from both cleanup and
        // normal paths (e.g. post-cleanup continuation points listed in a
        // chain) must stay in the normal block_map so non-cleanup Gotos can
        // still target them.
        let chain_members: HashSet<BlockId> = self.mir_fn.blocks.iter()
            .filter_map(|b| {
                if let MirTerminatorKind::CleanupReturn { cleanup_chain, .. } = &b.terminator.kind {
                    Some(cleanup_chain.iter().copied())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        // Walk forward from the entry over ordinary edges. Anything found this
        // way is on a normal path and keeps its place in the main block map.
        let mut normal_reachable: HashSet<BlockId> = HashSet::new();
        {
            let mut queue = vec![self.mir_fn.entry_block];
            while let Some(bid) = queue.pop() {
                if !normal_reachable.insert(bid) {
                    continue;
                }
                if let Some(b) = self.mir_fn.blocks.iter().find(|b| b.id == bid) {
                    // A CleanupReturn's chain is the cleanup path, not a normal
                    // successor — that's the whole distinction being drawn here.
                    if !matches!(b.terminator.kind, MirTerminatorKind::CleanupReturn { .. }) {
                        queue.extend(rask_mir::analysis::cfg::successors(&b.terminator));
                    }
                }
            }
        }

        // Then forward from each cleanup chain member. Everything reachable
        // that isn't also on a normal path belongs to the cleanup sub-CFG.
        //
        // This used to run backwards — a block joined the set once *all* its
        // predecessors were in it. A loop inside the cleanup path defeats that:
        // the loop header's back edge comes from a block that can't be admitted
        // until the header is, so neither ever is. The body stayed in the main
        // block map while its entry moved to the cleanup map, and the jump
        // between them was emitted into nothing — leaving a block with no
        // terminator for Cranelift to reject (#538).
        let mut cleanup_only: HashSet<BlockId> = HashSet::new();
        {
            let mut queue: Vec<BlockId> = chain_members.iter().copied().collect();
            while let Some(bid) = queue.pop() {
                if normal_reachable.contains(&bid) || !cleanup_only.insert(bid) {
                    continue;
                }
                if let Some(b) = self.mir_fn.blocks.iter().find(|b| b.id == bid) {
                    queue.extend(rask_mir::analysis::cfg::successors(&b.terminator));
                }
            }
        }

        // Deduplicate cleanup chains: map each unique chain to a shared block.
        let mut cleanup_chain_blocks: HashMap<Vec<BlockId>, cranelift_codegen::ir::Block> =
            HashMap::new();

        let mut builder = ClifFunctionBuilder::new(self.func, &mut self.builder_ctx);

        // Create blocks (skip cleanup-only blocks — handled via shared cleanup blocks)
        for mir_block in &self.mir_fn.blocks {
            if cleanup_only.contains(&mir_block.id) {
                continue;
            }
            let block = builder.create_block();
            self.block_map.insert(mir_block.id, block);
        }

        // Create shared cleanup blocks for each unique chain.
        for mir_block in &self.mir_fn.blocks {
            if let MirTerminatorKind::CleanupReturn { cleanup_chain, .. } = &mir_block.terminator.kind {
                if !cleanup_chain.is_empty() && !cleanup_chain_blocks.contains_key(cleanup_chain) {
                    let shared_block = builder.create_block();
                    cleanup_chain_blocks.insert(cleanup_chain.clone(), shared_block);
                }
            }
        }

        // Declare all variables (locals)
        for (idx, local) in self.mir_fn.locals.iter().enumerate() {
            let var = Variable::new(idx);
            let ty = mir_to_cranelift_type(&local.ty)?;
            builder.declare_var(var, ty);
            self.var_map.insert(local.id, var);
        }

        // Entry block - add parameters as block params
        let entry_block = self.block_map.get(&self.mir_fn.entry_block)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Entry block not found".to_string()))?;
        builder.switch_to_block(*entry_block);

        // Append parameters to entry block and bind to variables
        for param in &self.mir_fn.params {
            let param_ty = mir_to_cranelift_type(&param.ty)?;
            let block_param = builder.append_block_param(*entry_block, param_ty);
            let var = self.var_map.get(&param.id)
                .ok_or_else(|| CodegenError::UnsupportedFeature("Parameter variable not found".to_string()))?;
            builder.def_var(*var, block_param);
        }

        // Allocate stack slots for aggregate locals (structs, enums, arrays).
        // These types are represented as pointers (i64) — the variable holds
        // the address of the stack-allocated storage.
        for (local_id, size) in &stack_allocs {
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                *size,
                0, // align_shift: natural alignment
            ));
            self.stack_slot_map.insert(*local_id, (ss, *size));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            let var = self.var_map[local_id];
            builder.def_var(var, addr);
        }

        // For main(): emit rask_set_origin_file(source_file) so .origin() includes file name
        if self.mir_fn.name == "main" {
            if let Some(file_name) = self.mir_fn.source_file.as_deref() {
                if let Some(gv) = self.string_globals.get(file_name) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    if let Some(func_ref) = self.func_refs.get("rask_set_origin_file") {
                        builder.ins().call(*func_ref, &[file_ptr]);
                    }
                }
            }
        }

        let mut ctx = CodegenCtx {
            var_map: &self.var_map,
            locals: &self.mir_fn.locals,
            params: &self.mir_fn.params,
            fn_param_types: self.fn_param_types,
            func_refs: self.func_refs,
            struct_layouts: self.struct_layouts,
            enum_layouts: self.enum_layouts,
            string_globals: self.string_globals,
            comptime_globals: self.comptime_globals,
            vtable_globals: self.vtable_globals,
            panicking_fns: self.panicking_fns,
            internal_fns: self.internal_fns,
            stack_slot_map: &self.stack_slot_map,
            block_map: &self.block_map,
            build_mode: self.build_mode,
            source_file: self.mir_fn.source_file.as_deref(),
            line_map: self.line_map,
            current_line: self.current_line,
            current_col: self.current_col,
            current_span_start: 0,
            ret_ty: &self.mir_fn.ret_ty,
            is_main: self.mir_fn.name == "main",
            is_extern_c: self.mir_fn.is_extern_c,
            adapt_table: &self.adapt_table,
        };

        // ctrl.panic/A1: an exported symbol is entered from C, so the frames
        // between here and any panic handler belong to the C caller. Mark the
        // boundary on the way in — `rask_panic` aborts instead of longjmping
        // over them. `lower_terminator` clears it on every normal return.
        if ctx.is_extern_c {
            if let Some(fr) = ctx.func_refs.get("rask_ffi_boundary_enter") {
                builder.ins().call(*fr, &[]);
            }
        }

        // Lower each block (skip cleanup-only blocks)
        for mir_block in &self.mir_fn.blocks {
            if cleanup_only.contains(&mir_block.id) {
                continue;
            }

            let cl_block = self.block_map[&mir_block.id];

            if mir_block.id != self.mir_fn.entry_block {
                builder.switch_to_block(cl_block);
            }

            // Lower statements
            for stmt in &mir_block.statements {
                Self::apply_srcloc(&mut builder, stmt.span);
                ctx.current_span_start = stmt.span.start as u32;
                // Update line:col from span if we have a line map
                if let Some(lm) = ctx.line_map {
                    let (line, col) = lm.offset_to_line_col(stmt.span.start);
                    ctx.current_line = line as u32;
                    ctx.current_col = col as u32;
                }
                Self::lower_stmt(&mut builder, stmt, &ctx)?;
            }

            // Lower terminator
            Self::apply_srcloc(&mut builder, mir_block.terminator.span);
            Self::lower_terminator(&mut builder, &mir_block.terminator, &ctx, &cleanup_chain_blocks)?;
        }

        // Emit shared cleanup blocks. Each unique cleanup chain gets one
        // entry Cranelift block. Cleanup blocks may contain sub-CFGs
        // (e.g. else handler branching for ER2), which get their own
        // Cranelift blocks.
        //
        // Create Cranelift blocks for all cleanup sub-blocks first so
        // Branch terminators can reference them.
        //
        // Per chain, not once for the whole function. The same `ensure` shows
        // up in several chains — an inner `with` puts its own release in front
        // of the outer one — and with one Cranelift block per MIR block the
        // second chain lowered its statements and its `return` on top of the
        // first chain's, right after that block's terminator (#836).
        let mut all_cleanup_blocks: Vec<cranelift_codegen::ir::Block> = Vec::new();

        for (chain, &shared_block) in &cleanup_chain_blocks {
            // What this chain can reach: its own members, plus the sub-blocks
            // (handler and done blocks) hanging off them.
            let mut used: HashSet<BlockId> = HashSet::new();
            {
                let mut queue: Vec<BlockId> = chain.clone();
                while let Some(bid) = queue.pop() {
                    if !used.insert(bid) {
                        continue;
                    }
                    if let Some(b) = self.mir_fn.blocks.iter().find(|b| b.id == bid) {
                        for succ in rask_mir::analysis::cfg::successors(&b.terminator) {
                            if cleanup_only.contains(&succ) {
                                queue.push(succ);
                            }
                        }
                    }
                }
            }
            let mut cleanup_block_map: HashMap<BlockId, cranelift_codegen::ir::Block> =
                HashMap::new();
            for &bid in &used {
                let cl_block = builder.create_block();
                all_cleanup_blocks.push(cl_block);
                cleanup_block_map.insert(bid, cl_block);
            }

            builder.switch_to_block(shared_block);

            // Add return value as block parameter if function returns a value
            // (main is called from C as void — never returns a value)
            let is_main = self.mir_fn.name == "main";
            let ret_param = if !matches!(self.mir_fn.ret_ty, MirType::Void) && !is_main {
                let ret_cl_ty = mir_to_cranelift_type(&self.mir_fn.ret_ty)?;
                Some(builder.append_block_param(shared_block, ret_cl_ty))
            } else {
                None
            };

            let cleanup_ctx = CodegenCtx {
                source_file: None,
                line_map: None,
                current_line: 0,
                current_col: 0,
                current_span_start: 0,
                ..ctx
            };

            // Jump from shared entry to the first cleanup block in the chain
            if let Some(&first_block) = chain.first().and_then(|bid| cleanup_block_map.get(bid)) {
                builder.ins().jump(first_block, &[]);
            } else {
                // Empty chain — just return
                if let Some(val) = ret_param {
                    builder.ins().return_(&[val]);
                } else {
                    builder.ins().return_(&[]);
                }
                continue;
            }

            // Process each cleanup block in the chain as a real CFG.
            // Unreachable sentinels → jump to next chain block or return.
            for (i, block_id) in chain.iter().enumerate() {
                let Some(mir_block) = self.mir_fn.blocks.iter().find(|b| b.id == *block_id) else {
                    continue;
                };
                let Some(&cl_block) = cleanup_block_map.get(block_id) else {
                    continue;
                };

                builder.switch_to_block(cl_block);

                // Lower statements
                for stmt in &mir_block.statements {
                    Self::lower_stmt(&mut builder, stmt, &cleanup_ctx)?;
                }

                // Lower terminator — Unreachable means "continue chain or return"
                match &mir_block.terminator.kind {
                    MirTerminatorKind::Unreachable => {
                        // End of this ensure's sub-CFG. Jump to next chain block or return.
                        if let Some(next_bid) = chain.get(i + 1) {
                            if let Some(&next_cl) = cleanup_block_map.get(next_bid) {
                                builder.ins().jump(next_cl, &[]);
                            } else if let Some(val) = ret_param {
                                builder.ins().return_(&[val]);
                            } else {
                                builder.ins().return_(&[]);
                            }
                        } else if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                    MirTerminatorKind::Branch { cond, then_block, else_block } => {
                        let cond_val = Self::lower_operand_typed(&mut builder, cond, Some(types::I8), &cleanup_ctx)?;
                        let actual_ty = builder.func.dfg.value_type(cond_val);
                        let cond_final = if actual_ty != types::I8 {
                            Self::convert_value(&mut builder, cond_val, actual_ty, types::I8, None)
                        } else {
                            cond_val
                        };
                        let then_cl = cleanup_block_map.get(then_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        let else_cl = cleanup_block_map.get(else_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        builder.ins().brif(cond_final, then_cl, &[], else_cl, &[]);
                    }
                    MirTerminatorKind::Goto { target } => {
                        // Cleanup blocks first, then the main map. A target in
                        // neither would leave this block with no terminator,
                        // which Cranelift rejects with a message that says
                        // nothing about where it came from — so say it here.
                        let tgt = cleanup_block_map.get(target)
                            .or_else(|| self.block_map.get(target))
                            .copied()
                            .ok_or_else(|| CodegenError::UnsupportedFeature(format!(
                                "cleanup block jumps to {:?}, which has no Cranelift block",
                                target,
                            )))?;
                        builder.ins().jump(tgt, &[]);
                    }
                    _ => {
                        // Other terminators in cleanup blocks: treat as return
                        if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                }
            }

            // Process sub-blocks (handler blocks, done blocks) that aren't
            // in the chain but are reachable from chain blocks.
            let chain_set: HashSet<BlockId> = chain.iter().copied().collect();
            for &bid in &used {
                if chain_set.contains(&bid) {
                    continue; // Already processed above
                }
                // Only process sub-blocks reachable from THIS chain's blocks
                let Some(mir_block) = self.mir_fn.blocks.iter().find(|b| b.id == bid) else {
                    continue;
                };
                let Some(&cl_block) = cleanup_block_map.get(&bid) else {
                    continue;
                };

                // Check if this sub-block is reachable from any block in THIS chain
                let reachable = chain.iter().any(|chain_bid| {
                    let mut visited = HashSet::new();
                    let mut queue = vec![*chain_bid];
                    while let Some(qid) = queue.pop() {
                        if qid == bid { return true; }
                        if !visited.insert(qid) { continue; }
                        if let Some(qb) = self.mir_fn.blocks.iter().find(|b| b.id == qid) {
                            for succ in rask_mir::analysis::cfg::successors(&qb.terminator) {
                                if cleanup_only.contains(&succ) {
                                    queue.push(succ);
                                }
                            }
                        }
                    }
                    false
                });
                if !reachable { continue; }

                builder.switch_to_block(cl_block);
                for stmt in &mir_block.statements {
                    Self::lower_stmt(&mut builder, stmt, &cleanup_ctx)?;
                }

                match &mir_block.terminator.kind {
                    MirTerminatorKind::Unreachable => {
                        // End of sub-CFG — jump to next chain block or return.
                        // Find which chain block this sub-block belongs to.
                        let chain_idx = chain.iter().position(|cid| {
                            let mut visited = HashSet::new();
                            let mut queue = vec![*cid];
                            while let Some(qid) = queue.pop() {
                                if qid == bid { return true; }
                                if !visited.insert(qid) { continue; }
                                if let Some(qb) = self.mir_fn.blocks.iter().find(|b| b.id == qid) {
                                    for succ in rask_mir::analysis::cfg::successors(&qb.terminator) {
                                        if cleanup_only.contains(&succ) {
                                            queue.push(succ);
                                        }
                                    }
                                }
                            }
                            false
                        });
                        let next_chain_idx = chain_idx.map(|i| i + 1);
                        if let Some(next_bid) = next_chain_idx.and_then(|i| chain.get(i)) {
                            if let Some(&next_cl) = cleanup_block_map.get(next_bid) {
                                builder.ins().jump(next_cl, &[]);
                            } else if let Some(val) = ret_param {
                                builder.ins().return_(&[val]);
                            } else {
                                builder.ins().return_(&[]);
                            }
                        } else if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                    MirTerminatorKind::Goto { target } => {
                        // Cleanup blocks first, then the main map. A target in
                        // neither would leave this block with no terminator,
                        // which Cranelift rejects with a message that says
                        // nothing about where it came from — so say it here.
                        let tgt = cleanup_block_map.get(target)
                            .or_else(|| self.block_map.get(target))
                            .copied()
                            .ok_or_else(|| CodegenError::UnsupportedFeature(format!(
                                "cleanup block jumps to {:?}, which has no Cranelift block",
                                target,
                            )))?;
                        builder.ins().jump(tgt, &[]);
                    }
                    MirTerminatorKind::Branch { cond, then_block, else_block } => {
                        let cond_val = Self::lower_operand_typed(&mut builder, cond, Some(types::I8), &cleanup_ctx)?;
                        let actual_ty = builder.func.dfg.value_type(cond_val);
                        let cond_final = if actual_ty != types::I8 {
                            Self::convert_value(&mut builder, cond_val, actual_ty, types::I8, None)
                        } else {
                            cond_val
                        };
                        let then_cl = cleanup_block_map.get(then_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        let else_cl = cleanup_block_map.get(else_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        builder.ins().brif(cond_final, then_cl, &[], else_cl, &[]);
                    }
                    _ => {
                        if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                }
            }
        }

        // Now seal all blocks (all predecessors are known)
        for mir_block in &self.mir_fn.blocks {
            if let Some(&cl_block) = self.block_map.get(&mir_block.id) {
                builder.seal_block(cl_block);
            }
        }
        for &shared_block in cleanup_chain_blocks.values() {
            builder.seal_block(shared_block);
        }
        for &cl_block in &all_cleanup_blocks {
            builder.seal_block(cl_block);
        }

        builder.finalize();
        Ok(())
    }

    /// Set Cranelift source location from a MIR span.
    /// Real spans (end > 0) encode as SourceLoc(start + 1) to avoid the
    /// SourceLoc(0) value which Cranelift reserves internally.
    /// Dummy spans (0..0) clear the location.
    fn apply_srcloc(builder: &mut ClifFunctionBuilder, span: rask_mir::Span) {
        if span.end > 0 {
            // +1 so that byte offset 0 becomes SourceLoc(1), avoiding any
            // ambiguity with "no location" values.
            builder.set_srcloc(SourceLoc::new(span.start as u32 + 1));
        } else {
            builder.set_srcloc(SourceLoc::default());
        }
    }

    fn lower_stmt(
        builder: &mut ClifFunctionBuilder,
        stmt: &MirStmt,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue } => Self::lower_assign(builder, dst, rvalue, ctx)?,

            MirStmtKind::Store { addr, offset, value, store_size } => Self::lower_store(builder, addr, offset, value, store_size, ctx)?,

            // Array element store: base_ptr[index * elem_size] = value
            MirStmtKind::ArrayStore { base, index, elem_size, value } => {
                let base_val = builder.use_var(*ctx.var_map.get(base)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("ArrayStore: base not found".to_string()))?);
                let idx_val = Self::lower_operand_typed(builder, index, Some(types::I64), ctx)?;
                let val = Self::lower_operand(builder, value, ctx)?;
                let elem_sz = builder.ins().iconst(types::I64, *elem_size as i64);
                let offset = builder.ins().imul(idx_val, elem_sz);
                let addr = builder.ins().iadd(base_val, offset);
                // An element that lives *in* its slot is a value, and the operand
                // is its address — copy the bytes. Storing the pointer put the
                // constructing slot's address where the element belongs, so
                // `a[1] = 5` on a `[i64?; 3]` read back as an address (#783). Same
                // `stored_inline_in_array` rule the read and the literal's store
                // use; the three only work as a set.
                let elem_is_inline = ctx.locals.iter().find(|l| l.id == *base)
                    .is_some_and(|l| matches!(&l.ty,
                        MirType::Array { elem, .. } if elem.stored_inline_in_array()));
                if elem_is_inline {
                    Self::copy_bytes(builder, val, 0, addr, 0, *elem_size);
                } else if let 1 | 2 | 4 = *elem_size {
                    // The address above already accounts for `elem_size`; the
                    // store has to as well. A full-word store into a 4-byte
                    // element wrote over the next one, so `a[1] = 9` on a
                    // `[i32; 4]` blanked `a[2]` (#902).
                    Self::store_narrow(builder, val, addr, 0, *elem_size);
                } else {
                    let flags = MemFlags::new();
                    builder.ins().store(flags, val, addr, 0);
                }
            }

            MirStmtKind::Call { dst, func, args } => {
                Self::lower_call(builder, dst.as_ref(), func, args, ctx)?;
            }

            // ── Resource tracking ──────────────────────────────────────
            // Calls C runtime functions for runtime must-consume checks.

            MirStmtKind::ResourceRegister { dst, scope_depth, .. } => {
                // rask_resource_register(scope_depth) → resource_id
                let func_ref = ctx.func_refs.get("rask_resource_register")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_register".to_string()))?;
                let depth_val = builder.ins().iconst(types::I64, *scope_depth as i64);
                let call_inst = builder.ins().call(*func_ref, &[depth_val]);

                let results = builder.inst_results(call_inst);
                if !results.is_empty() {
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Resource register destination not found".to_string()
                        ))?;
                    builder.def_var(*var, results[0]);
                }
            }

            MirStmtKind::ResourceConsume { resource_id } => {
                // rask_resource_consume(resource_id)
                let func_ref = ctx.func_refs.get("rask_resource_consume")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_consume".to_string()))?;
                let id_val = builder.use_var(*ctx.var_map.get(resource_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Resource ID variable not found".to_string()
                    ))?);
                builder.ins().call(*func_ref, &[id_val]);
            }

            MirStmtKind::ResourceScopeCheck { scope_depth } => {
                // rask_resource_scope_check(scope_depth)
                let func_ref = ctx.func_refs.get("rask_resource_scope_check")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_scope_check".to_string()))?;
                let depth_val = builder.ins().iconst(types::I64, *scope_depth as i64);
                builder.ins().call(*func_ref, &[depth_val]);
            }

            // ── Cleanup stack ──────────────────────────────────────────
            // EnsurePush/Pop track the cleanup scope during MIR construction.
            // At codegen time, the cleanup chain is already materialized in
            // CleanupReturn terminators, so these are no-ops.
            MirStmtKind::EnsurePush { .. } | MirStmtKind::EnsurePop => {}

            // ── Runtime ensure hooks (panic unwind) ────────────────────
            // Register a hook so the cleanup runs if the scope unwinds on a
            // panic (ctrl.panic/U1). The env is a frame-local array of 8-byte
            // slots holding each capture's value (aggregate → address).
            MirStmtKind::EnsureHookRegister { thunk, captures } => {
                let slots = captures.len().max(1) as u32;
                let env_ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slots * 8,
                    3, // 8-byte alignment
                ));
                for c in captures {
                    let var = ctx.var_map.get(&c.local_id).ok_or_else(|| {
                        CodegenError::UnsupportedFeature("EnsureHookRegister capture not found".to_string())
                    })?;
                    let val = builder.use_var(*var);
                    let vty = builder.func.dfg.value_type(val);
                    let val64 = if vty == types::I64 {
                        val
                    } else if vty.is_int() && vty.bytes() < 8 {
                        builder.ins().uextend(types::I64, val)
                    } else {
                        val
                    };
                    builder.ins().stack_store(val64, env_ss, c.offset as i32);
                }
                let env_addr = builder.ins().stack_addr(types::I64, env_ss, 0);
                let thunk_ref = ctx.func_refs.get(thunk)
                    .ok_or_else(|| CodegenError::FunctionNotFound(thunk.clone()))?;
                let thunk_ptr = builder.ins().func_addr(types::I64, *thunk_ref);
                let push_ref = ctx.func_refs.get("rask_ensure_push")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_ensure_push".to_string()))?;
                builder.ins().call(*push_ref, &[thunk_ptr, env_addr]);
            }

            // Deregister the most recent hook (normal exit runs the inline path).
            MirStmtKind::EnsureHookPop => {
                let pop_ref = ctx.func_refs.get("rask_ensure_pop")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_ensure_pop".to_string()))?;
                builder.ins().call(*pop_ref, &[]);
            }

            // ── Pool checked access ────────────────────────────────────
            MirStmtKind::PoolCheckedAccess { dst, pool, handle } => Self::lower_pool_checked_access(builder, dst, pool, handle, ctx)?,

            // ── Closure support ──────────────────────────────────────────

            MirStmtKind::ClosureCreate { dst, func_name, captures, heap } => Self::lower_closure_create(builder, dst, func_name, captures, heap, ctx)?,

            MirStmtKind::ClosureCall { dst, closure, args } => Self::lower_closure_call(builder, dst, closure, args, ctx)?,

            MirStmtKind::LoadCapture { dst, env_ptr, offset, by_ref } => {
                let env_val = builder.use_var(*ctx.var_map.get(env_ptr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture env_ptr not found".to_string()
                    ))?);
                let dst_local = ctx.locals.iter().find(|l| l.id == *dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination not found".to_string()
                    ))?;
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination variable not found".to_string()
                    ))?;

                if *by_ref {
                    // Ensure-hook capture: the env slot holds an 8-byte pointer to
                    // the original value. Point the local's variable straight at it
                    // (for aggregates the variable already IS the address), so
                    // cleanup runs against the live resource rather than a copy.
                    let val = crate::closures::load_capture(builder, env_val, *offset, types::I64);
                    builder.def_var(*var, val);
                } else if let Some((ss, size)) = ctx.stack_slot_map.get(dst) {
                    // Aggregate types (String, Struct, etc.) were deep-copied into
                    // the closure environment. Copy into the local stack slot and
                    // set the variable to the local slot address.
                    let env_addr = builder.ins().iadd_imm(env_val, *offset as i64);
                    Self::copy_aggregate(builder, env_addr, *ss, *size);
                    let local_addr = builder.ins().stack_addr(types::I64, *ss, 0);
                    builder.def_var(*var, local_addr);
                } else {
                    // Scalar: load the value directly
                    let load_ty = mir_to_cranelift_type(&dst_local.ty)?;
                    let val = crate::closures::load_capture(builder, env_val, *offset, load_ty);
                    builder.def_var(*var, val);
                }
            }

            MirStmtKind::ClosureDrop { closure } => {
                let closure_val = builder.use_var(*ctx.var_map.get(closure)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "ClosureDrop closure variable not found".to_string()
                    ))?);
                let free_ref = ctx.func_refs.get("rask_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_free".to_string()))?;
                crate::closures::free_closure(builder, closure_val, *free_ref);
            }

            MirStmtKind::GlobalRef { dst, name } => {
                let gv = ctx.comptime_globals.get(name.as_str())
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        format!("GlobalRef: comptime global '{}' not found", name)
                    ))?;
                let addr = builder.ins().global_value(types::I64, *gv);
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "GlobalRef destination not found".to_string()
                    ))?;
                builder.def_var(*var, addr);
            }

            // ── Trait object support ──────────────────────────────────

            MirStmtKind::TraitBox { dst, value, vtable_name, concrete_size, .. } => Self::lower_trait_box(builder, dst, value, vtable_name, concrete_size, ctx)?,

            MirStmtKind::TraitCall { dst, trait_object, method_name, vtable_offset, args } => Self::lower_trait_call(builder, dst, trait_object, method_name, vtable_offset, args, ctx)?,

            MirStmtKind::TraitDrop { trait_object } => {
                let obj_val = builder.use_var(*ctx.var_map.get(trait_object)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitDrop: trait object variable not found".to_string()
                    ))?);

                // Load data_ptr and vtable_ptr
                let data_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_DATA_OFFSET);
                let vtable_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_VTABLE_OFFSET);

                // Load drop_fn from vtable
                let drop_fn = builder.ins().load(types::I64, MemFlags::new(), vtable_ptr, crate::vtable::VTABLE_DROP_OFFSET as i32);

                // If drop_fn != null, call it
                let null = builder.ins().iconst(types::I64, 0);
                let is_null = builder.ins().icmp(IntCC::Equal, drop_fn, null);

                let drop_block = builder.create_block();
                let free_block = builder.create_block();

                builder.ins().brif(is_null, free_block, &[], drop_block, &[]);

                // Drop block: call drop_fn(data_ptr), then fall through to free.
                // Its only predecessor is the brif above, already emitted —
                // safe to seal right away, matching every other conditional
                // block pair in this file (bounds checks, tag comparisons, …).
                builder.switch_to_block(drop_block);
                builder.seal_block(drop_block);
                let mut drop_sig = Signature::new(isa::CallConv::SystemV);
                drop_sig.params.push(AbiParam::new(types::I64));
                let sig_ref = builder.import_signature(drop_sig);
                builder.ins().call_indirect(sig_ref, drop_fn, &[data_ptr]);
                builder.ins().jump(free_block, &[]);

                // Free block: rask_free(data_ptr). Both predecessors (the
                // brif's null arm and drop_block's jump) are already emitted.
                builder.switch_to_block(free_block);
                builder.seal_block(free_block);
                let free_ref = ctx.func_refs.get("rask_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_free".to_string()))?;
                builder.ins().call(*free_ref, &[data_ptr]);
            }

            MirStmtKind::Phi { .. } => {
                panic!("Phi nodes must be lowered by de-SSA before codegen");
            }

            MirStmtKind::RcInc { local } => {
                // Increment string refcount: rask_string_clone(local)
                let val = builder.use_var(*ctx.var_map.get(local)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "RcInc local variable not found".to_string()
                    ))?);
                let clone_ref = ctx.func_refs.get("rask_string_clone")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_string_clone".to_string()))?;
                builder.ins().call(*clone_ref, &[val]);
            }

            MirStmtKind::RcDec { local } => {
                // Decrement string refcount: rask_string_free(local)
                let val = builder.use_var(*ctx.var_map.get(local)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "RcDec local variable not found".to_string()
                    ))?);
                let free_ref = ctx.func_refs.get("rask_string_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_string_free".to_string()))?;
                builder.ins().call(*free_ref, &[val]);
            }
        }
        Ok(())
    }

    /// Lower a `MirStmtKind::Call` — dispatches builtins, extern calls, and regular calls.
    fn lower_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        if Self::try_lower_builtin_call(builder, dst, func, args, ctx)? {
            return Ok(());
        }
        if func.is_extern {
            Self::lower_extern_call(builder, dst, func, args, ctx)
        } else {
            Self::lower_ordinary_call(builder, dst, func, args, ctx)
        }
    }

    /// Convert a value between Cranelift types (integer widening/narrowing, float conversion).
    /// `from_mir` is the source's MIR type where the caller has it. A Cranelift
    /// type carries no signedness, so without it every widening sign-extends,
    /// and `x as u64` on a `u16` holding 60000 came out as 2^64 - 5536 (#326).
    /// Callers used to patch that themselves; two did, the rest didn't.
    fn convert_value(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        from_ty: Type,
        to_ty: Type,
        from_mir: Option<&MirType>,
    ) -> Value {
        if from_ty == to_ty {
            return val;
        }

        if from_ty.is_int() && to_ty.is_int() {
            let from_bits = from_ty.bits();
            let to_bits = to_ty.bits();
            if from_bits == 1 {
                builder.ins().uextend(to_ty, val)
            } else if to_bits == 1 {
                builder.ins().icmp_imm(IntCC::NotEqual, val, 0)
            } else if from_bits > to_bits {
                builder.ins().ireduce(to_ty, val)
            } else if from_mir.is_some_and(|t| t.is_unsigned()) {
                builder.ins().uextend(to_ty, val)
            } else {
                builder.ins().sextend(to_ty, val)
            }
        } else if from_ty.is_float() && to_ty.is_float() {
            if from_ty.bits() > to_ty.bits() {
                builder.ins().fdemote(to_ty, val)
            } else {
                builder.ins().fpromote(to_ty, val)
            }
        } else if from_ty.is_int() && to_ty.is_float() {
            // Signedness decides this the same way it decides the widening
            // above — and for the same reason, since Cranelift's integer types
            // carry a width and not a sign. Converting unsigned bits as signed
            // read `u8 255` as -1 and `u32 4000000000` as -294967296 (#907).
            // `lower_convert` has always asked; this one didn't.
            if from_mir.is_some_and(|t| t.is_unsigned()) {
                builder.ins().fcvt_from_uint(to_ty, val)
            } else {
                builder.ins().fcvt_from_sint(to_ty, val)
            }
        } else if from_ty.is_float() && to_ty.is_int() {
            builder.ins().fcvt_to_sint_sat(to_ty, val)
        } else {
            builder.ins().bitcast(to_ty, MemFlags::new(), val)
        }
    }

    /// Lower an explicit conversion form (type.primitives CV5–CV10).
    fn lower_convert(
        builder: &mut ClifFunctionBuilder,
        value: &MirOperand,
        source_ty: &MirType,
        target_ty: &MirType,
        kind: rask_ast::expr::ConvertKind,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        use rask_ast::expr::ConvertKind::*;

        // Normalize the operand to the source type's Cranelift width.
        let src_clif = mir_to_cranelift_type(source_ty)?;
        let raw = Self::lower_operand(builder, value, ctx)?;
        let raw_ty = builder.func.dfg.value_type(raw);
        let val = if raw_ty != src_clif {
            Self::convert_value(builder, raw, raw_ty, src_clif, None)
        } else {
            raw
        };
        let tgt_clif = mir_to_cranelift_type(target_ty)?;

        match kind {
            // CV12: bit-preserving resize.
            Wrap => Ok(Self::resize_int(builder, val, source_ty, target_ty)),
            // CV13: clamp to target range.
            Clamp => Ok(Self::saturate_int(builder, val, source_ty, target_ty)),
            // Compiler-internal: build Option<T> in a stack slot, branchlessly.
            CheckedOption => {
                // char.from_u32 lowers here with a Char target — validity is
                // "valid Unicode scalar", not a contiguous integer range.
                if matches!(target_ty, MirType::Char) {
                    let valid = Self::char_is_valid(builder, val);
                    return Ok(Self::build_option(builder, val, valid, tgt_clif));
                }
                let payload = Self::resize_int(builder, val, source_ty, target_ty);
                let in_range = Self::int_in_range(builder, val, source_ty, target_ty);
                Ok(Self::build_option(builder, payload, in_range, tgt_clif))
            }
            // CV14 to a float target is the one method form that can't fail:
            // there's always a nearest representable float, and an out-of-range
            // `f64` → `f32` gives ±infinity, which is IEEE's answer rather than
            // an error.
            Round if matches!(target_ty, MirType::F32 | MirType::F64) => {
                Ok(Self::to_float(builder, val, source_ty, tgt_clif))
            }
            To | Round | Floor | Ceil => Ok(Self::lower_convert_result(
                builder, val, source_ty, target_ty, tgt_clif, kind,
            )),
        }
    }

    /// `ConvertError`'s variants, in declaration order — `stdlib/builtins.rk`.
    /// Stored as the error payload of the result, which is how every fieldless
    /// error enum travels.
    const CONVERT_OK: i64 = 0;
    const CONVERT_OUT_OF_RANGE: i64 = 1;
    const CONVERT_NOT_EXACT: i64 = 2;
    const CONVERT_NOT_FINITE: i64 = 3;

    /// A numeric value at a float width, whatever it started as.
    fn to_float(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        tgt_clif: Type,
    ) -> Value {
        let src_clif = builder.func.dfg.value_type(val);
        if src_clif.is_float() {
            return match (src_clif.bits(), tgt_clif.bits()) {
                (a, b) if a == b => val,
                (64, _) => builder.ins().fdemote(tgt_clif, val),
                _ => builder.ins().fpromote(tgt_clif, val),
            };
        }
        if source_ty.is_unsigned() {
            builder.ins().fcvt_from_uint(tgt_clif, val)
        } else {
            builder.ins().fcvt_from_sint(tgt_clif, val)
        }
    }

    /// CV11/CV14–CV16: the conversion that can fail, as `T or ConvertError`.
    ///
    /// One shape for all four: work out the converted value and a failure code
    /// side by side, then branch once to write either `Ok(value)` or
    /// `Err(variant)` into a result slot. The codes are applied in priority
    /// order — a `NaN` is `NotFinite` rather than `OutOfRange`, even though it
    /// fails the range test too — which is the order the interpreter uses.
    fn lower_convert_result(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
        tgt_clif: Type,
        kind: rask_ast::expr::ConvertKind,
    ) -> Value {
        use rask_ast::expr::ConvertKind::*;
        let src_clif = builder.func.dfg.value_type(val);
        let zero = builder.ins().iconst(types::I64, Self::CONVERT_OK);

        let (payload, err_code) = if tgt_clif.is_float() {
            // → float, and only `to` gets here: exact or it fails. An integer
            // source is exact when the round trip survives; a float source is
            // exact when narrowing loses nothing. NaN is neither — it converts
            // fine, it just isn't equal to itself, so it's excused explicitly.
            let converted = Self::to_float(builder, val, source_ty, tgt_clif);
            let exact = if src_clif.is_float() {
                let back = if src_clif.bits() > tgt_clif.bits() {
                    builder.ins().fpromote(src_clif, converted)
                } else {
                    converted
                };
                let same = builder.ins().fcmp(FloatCC::Equal, back, val);
                let is_nan = builder.ins().fcmp(FloatCC::Unordered, val, val);
                builder.ins().bor(same, is_nan)
            } else {
                let back = if source_ty.is_unsigned() {
                    builder.ins().fcvt_to_uint_sat(src_clif, converted)
                } else {
                    builder.ins().fcvt_to_sint_sat(src_clif, converted)
                };
                builder.ins().icmp(IntCC::Equal, back, val)
            };
            let not_exact = builder.ins().iconst(types::I64, Self::CONVERT_NOT_EXACT);
            (converted, builder.ins().select(exact, zero, not_exact))
        } else if src_clif.is_float() {
            // float → int. The fraction is handled by the verb; what's left is
            // whether the result is finite and whether it fits.
            let rounded = match kind {
                Floor => builder.ins().floor(val),
                Ceil => builder.ins().ceil(val),
                Round => builder.ins().nearest(val),
                // `to` doesn't round — a fraction is a failure, checked below.
                _ => val,
            };
            let converted = if target_ty.is_unsigned() {
                builder.ins().fcvt_to_uint_sat(tgt_clif, rounded)
            } else {
                builder.ins().fcvt_to_sint_sat(tgt_clif, rounded)
            };

            let in_range = Self::float_in_range(builder, rounded, target_ty);
            let out_of_range = builder.ins().iconst(types::I64, Self::CONVERT_OUT_OF_RANGE);
            let mut code = builder.ins().select(in_range, zero, out_of_range);

            if matches!(kind, To) {
                // A fraction can't survive into an integer, so `to` fails on it
                // rather than picking a rounding mode nobody asked for.
                let truncated = builder.ins().trunc(val);
                let whole = builder.ins().fcmp(FloatCC::Equal, truncated, val);
                let not_exact = builder.ins().iconst(types::I64, Self::CONVERT_NOT_EXACT);
                let fract_code = builder.ins().select(whole, code, not_exact);
                code = fract_code;
            }

            // NaN and infinity outrank both: they're not a value that missed
            // the range, they're not a value at all.
            let is_nan = builder.ins().fcmp(FloatCC::Unordered, val, val);
            let magnitude = builder.ins().fabs(val);
            let inf = if src_clif == types::F32 {
                let d = builder.ins().f64const(f64::INFINITY);
                builder.ins().fdemote(types::F32, d)
            } else {
                builder.ins().f64const(f64::INFINITY)
            };
            let is_inf = builder.ins().fcmp(FloatCC::Equal, magnitude, inf);
            let not_finite_cond = builder.ins().bor(is_nan, is_inf);
            let not_finite = builder.ins().iconst(types::I64, Self::CONVERT_NOT_FINITE);
            (converted, builder.ins().select(not_finite_cond, not_finite, code))
        } else {
            // int → int: the only way to lose the value is for it not to fit.
            let converted = Self::resize_int(builder, val, source_ty, target_ty);
            let in_range = Self::int_in_range(builder, val, source_ty, target_ty);
            let out_of_range = builder.ins().iconst(types::I64, Self::CONVERT_OUT_OF_RANGE);
            (converted, builder.ins().select(in_range, zero, out_of_range))
        };

        Self::build_convert_result(builder, payload, err_code, tgt_clif, target_ty)
    }

    /// Write `Ok(payload)` or `Err(variant)` into a result slot and hand back
    /// its address, which is how every aggregate travels here.
    fn build_convert_result(
        builder: &mut ClifFunctionBuilder,
        payload: Value,
        err_code: Value,
        tgt_clif: Type,
        target_ty: &MirType,
    ) -> Value {
        let payload_size = target_ty.size().max(8);
        let slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            crate::layouts::RESULT_PAYLOAD_OFFSET as u32 + payload_size,
            3,
        ));
        let ok_block = builder.create_block();
        let err_block = builder.create_block();
        let merge = builder.create_block();
        builder.ins().brif(err_code, err_block, &[], ok_block, &[]);

        builder.switch_to_block(ok_block);
        builder.seal_block(ok_block);
        // Sub-word payloads widen into the 8-byte slot, same as an Option's.
        let stored = if tgt_clif.is_int() && tgt_clif.bits() < 64 {
            builder.ins().uextend(types::I64, payload)
        } else {
            payload
        };
        Self::build_ok(builder, slot, stored);
        builder.ins().jump(merge, &[]);

        builder.switch_to_block(err_block);
        builder.seal_block(err_block);
        // The code is the variant index plus one, so that zero can mean "no
        // failure" — take the one back off before storing it.
        let variant = builder.ins().iadd_imm(err_code, -1);
        Self::build_err(builder, slot, variant);
        builder.ins().jump(merge, &[]);

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        builder.ins().stack_addr(types::I64, slot, 0)
    }

    /// Bit-preserving integer resize. Widening extends by the source's signedness.
    fn resize_int(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let from = mir_to_cranelift_type(source_ty).unwrap_or(types::I64);
        let to = mir_to_cranelift_type(target_ty).unwrap_or(types::I64);
        if from == to {
            return val;
        }
        if from.bits() > to.bits() {
            builder.ins().ireduce(to, val)
        } else if source_ty.is_unsigned() {
            builder.ins().uextend(to, val)
        } else {
            builder.ins().sextend(to, val)
        }
    }

    /// Widen an integer value to a width both sides of a range check survive:
    /// at least I64, at least as wide as the value, and at least as wide as
    /// the target whose bounds it is about to be compared against.
    ///
    /// Each of those three came from a bug. Extending anything that wasn't
    /// already I64 meant narrowing *out* of 128 bits emitted `sextend.i64`
    /// against an i128 — a widening instruction on a value wider than its
    /// target — and the verifier rejected the function. Truncating to i64
    /// instead would be worse: the check that decides whether `to<i64>()`
    /// answers an error would be comparing the value against bounds it has
    /// already been forced inside of. And comparing an i64 at 64 bits against
    /// `i128`'s bounds truncated `i128::MIN` to zero, so `(-5).to<i128>()`
    /// reported out of range for a conversion that cannot fail (#933).
    fn widen_for_compare(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let have = builder.func.dfg.value_type(val);
        let target_bits = mir_to_cranelift_type(target_ty)
            .map(|t| t.bits())
            .unwrap_or(64);
        let want = have.bits().max(64).max(target_bits);
        if have.bits() >= want {
            return val;
        }
        let to = if want >= 128 { types::I128 } else { types::I64 };
        if source_ty.is_unsigned() {
            builder.ins().uextend(to, val)
        } else {
            builder.ins().sextend(to, val)
        }
    }


    /// Clamp `val` (interpreted per source signedness) to the target range.
    fn saturate_int(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let (min, max) = Self::int_bounds(target_ty);
        // Compare at at least I64. An i128 source stays at its own width —
        // see `widen_for_compare`.
        let wide = Self::widen_for_compare(builder, val, source_ty, target_ty);
        let cmp_ty = builder.func.dfg.value_type(wide);
        let to = mir_to_cranelift_type(target_ty).unwrap_or(types::I64);
        let (cmp_signed_max, cmp_unsigned_max) = Self::compare_ceilings(cmp_ty);

        // Source and target don't have to share a signedness, and one
        // comparison mode for both is wrong whenever they don't. Clamping
        // `u64` to `i64` compared unsigned against `i64::MIN`, which reads as
        // 2^63 unsigned — so every value below it, meaning every ordinary
        // value, "underflowed" and came out as `i64::MIN`. `42 saturate to
        // i64` was -9223372036854775808 (#495).
        //
        // Nothing unsigned is below a target minimum, so only the ceiling can
        // bite there. A ceiling the comparison width can't represent is out of
        // the source's reach anyway, so there's nothing to clamp against.
        let too_small = if source_ty.is_unsigned() {
            None
        } else {
            let minc = Self::iconst_at(builder, cmp_ty, min);
            Some(builder.ins().icmp(IntCC::SignedLessThan, wide, minc))
        };
        let too_big = if source_ty.is_unsigned() {
            (max < cmp_unsigned_max).then(|| {
                let maxc = Self::iconst_at(builder, cmp_ty, max);
                builder.ins().icmp(IntCC::UnsignedGreaterThan, wide, maxc)
            })
        } else {
            (max <= cmp_signed_max).then(|| {
                let maxc = Self::iconst_at(builder, cmp_ty, max);
                builder.ins().icmp(IntCC::SignedGreaterThan, wide, maxc)
            })
        };

        // Narrow first, then substitute the limit — both at the target's
        // width. Selecting between two i128s instead left Cranelift's egraph
        // free to rewrite `icmp` + `select` into `smin.i128`, which the x64
        // backend has no lowering for, so `big.clamp<i64>()` panicked the
        // compiler rather than producing code (#933). Narrowing against the
        // comparison width rather than a hardcoded 64 is the other half: a
        // clamped i128 is still 128 bits until it's reduced.
        let mut out = if to.bits() < cmp_ty.bits() {
            builder.ins().ireduce(to, wide)
        } else {
            wide
        };
        if let Some(cond) = too_small {
            let lim = Self::iconst_at(builder, to, min);
            out = builder.ins().select(cond, lim, out);
        }
        if let Some(cond) = too_big {
            let lim = Self::iconst_at(builder, to, max);
            out = builder.ins().select(cond, lim, out);
        }
        out
    }

    /// The largest signed and unsigned values a comparison at `ty` can carry
    /// as a constant. Guards that used to name `i64::MAX`/`u64::MAX` directly
    /// were reading "the comparison happens in 64 bits", which stopped being
    /// true once an i128 source compared at its own width: converting one to
    /// `u64` then skipped the ceiling check and called every value in range.
    fn compare_ceilings(ty: Type) -> (i128, i128) {
        if ty.bits() >= 128 {
            (i128::MAX, i128::MAX)
        } else {
            (i64::MAX as i128, u64::MAX as i128)
        }
    }

    /// True when `val` fits in the target integer range (for `try convert`).
    fn int_in_range(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let (min, max) = Self::int_bounds(target_ty);
        let v64 = Self::widen_for_compare(builder, val, source_ty, target_ty);
        let t = builder.func.dfg.value_type(v64);
        let (cmp_signed_max, cmp_unsigned_max) = Self::compare_ceilings(t);
        let always = |b: &mut ClifFunctionBuilder| b.ins().iconst(types::I8, 1);

        // Same asymmetry as the saturating form: which comparison is right
        // depends on the *source*'s signedness, and which bound can bite at
        // all depends on the target's.
        let (ge_min, le_max) = if source_ty.is_unsigned() {
            let ge_min = always(builder); // never below a target minimum
            let le_max = if max < cmp_unsigned_max {
                let maxc = Self::iconst_at(builder, t, max);
                builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, v64, maxc)
            } else {
                always(builder)
            };
            (ge_min, le_max)
        } else {
            let minc = Self::iconst_at(builder, t, min);
            let ge_min = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, v64, minc);
            let le_max = if max <= cmp_signed_max {
                let maxc = Self::iconst_at(builder, t, max);
                builder.ins().icmp(IntCC::SignedLessThanOrEqual, v64, maxc)
            } else {
                always(builder)
            };
            (ge_min, le_max)
        };
        builder.ins().band(ge_min, le_max)
    }

    /// True when the float is finite and its truncation fits the target range.
    fn float_in_range(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        target_ty: &MirType,
    ) -> Value {
        let (min, max) = Self::int_bounds_i64(target_ty);
        let ft = builder.func.dfg.value_type(val);
        let minf = builder.ins().f64const(min as f64);
        let maxf = builder.ins().f64const(max as f64);
        let (minf, maxf) = if ft == types::F32 {
            (builder.ins().fdemote(types::F32, minf), builder.ins().fdemote(types::F32, maxf))
        } else {
            (minf, maxf)
        };
        // NaN fails both comparisons (ordered), so `ge_min & le_max` is false.
        let ge_min = builder.ins().fcmp(FloatCC::GreaterThanOrEqual, val, minf);
        let le_max = builder.ins().fcmp(FloatCC::LessThanOrEqual, val, maxf);
        builder.ins().band(ge_min, le_max)
    }

    /// True (I64 0/1) when `val` is a valid Unicode scalar (≤ 0x10FFFF, not a
    /// surrogate 0xD800..=0xDFFF).
    fn char_is_valid(builder: &mut ClifFunctionBuilder, val: Value) -> Value {
        let vt = builder.func.dfg.value_type(val);
        let n64 = if vt == types::I64 { val } else { builder.ins().uextend(types::I64, val) };
        let max = builder.ins().iconst(types::I64, 0x10FFFF);
        let sur_lo = builder.ins().iconst(types::I64, 0xD800);
        let sur_hi = builder.ins().iconst(types::I64, 0xDFFF);
        let le_max = builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, n64, max);
        let ge_lo = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, n64, sur_lo);
        let le_hi = builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, n64, sur_hi);
        let le_max = builder.ins().uextend(types::I64, le_max);
        let ge_lo = builder.ins().uextend(types::I64, ge_lo);
        let le_hi = builder.ins().uextend(types::I64, le_hi);
        let in_sur = builder.ins().band(ge_lo, le_hi);
        let not_sur = builder.ins().bxor_imm(in_sur, 1);
        builder.ins().band(le_max, not_sur)
    }

    /// Build `Option<T>` in a stack slot: tag 0 (Some) if `present`, else 1 (None).
    /// Layout matches `build_some`: [tag:8][payload:8].
    fn build_option(
        builder: &mut ClifFunctionBuilder,
        payload: Value,
        present: Value,
        payload_ty: Type,
    ) -> Value {
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 16, 0,
        ));
        let some_tag = builder.ins().iconst(types::I64, 0);
        let none_tag = builder.ins().iconst(types::I64, 1);
        let tag = builder.ins().select(present, some_tag, none_tag);
        builder.ins().stack_store(tag, ss, crate::layouts::TAG_OFFSET);
        // Store the payload at its natural width; widen scalars < 8 bytes to I64 slot.
        let store_val = if payload_ty.is_int() && payload_ty.bits() < 64 {
            builder.ins().uextend(types::I64, payload)
        } else {
            payload
        };
        builder.ins().stack_store(store_val, ss, crate::layouts::PAYLOAD_OFFSET);
        builder.ins().stack_addr(types::I64, ss, 0)
    }

    /// Target integer range as i64 constants. 64-bit unsigned upper bound is
    /// approximated by i64::MAX (saturate/try to u64 from ≤64-bit rarely overflows).
    fn int_bounds_i64(t: &MirType) -> (i64, i64) {
        let (lo, hi) = Self::int_bounds(t);
        (lo as i64, hi.min(i64::MAX as i128) as i64)
    }

    /// A target integer type's true range. `u64`'s maximum doesn't fit in an
    /// `i64`, so the i64-shaped version above reported `i64::MAX` for it —
    /// which is a real clamp point rather than the type's actual ceiling.
    fn int_bounds(t: &MirType) -> (i128, i128) {
        match t {
            MirType::I8 => (i8::MIN as i128, i8::MAX as i128),
            MirType::I16 => (i16::MIN as i128, i16::MAX as i128),
            MirType::I32 => (i32::MIN as i128, i32::MAX as i128),
            MirType::I64 => (i64::MIN as i128, i64::MAX as i128),
            MirType::U8 => (0, u8::MAX as i128),
            MirType::U16 => (0, u16::MAX as i128),
            MirType::U32 => (0, u32::MAX as i128),
            MirType::U64 => (0, u64::MAX as i128),
            MirType::I128 => (i128::MIN, i128::MAX),
            // `u128::MAX` doesn't fit the `i128` this returns. Nothing narrows
            // *into* a `u128` today — the only conversions that reach here are
            // widening, which can't clamp — so the ceiling stands in rather
            // than widening the whole bounds API for one unreachable case.
            MirType::U128 => (0, i128::MAX),
            _ => (i64::MIN as i128, i64::MAX as i128),
        }
    }

    /// Pick the runtime print function based on the MIR operand.
    fn runtime_print_for_operand(op: &MirOperand, locals: &[rask_mir::MirLocal]) -> &'static str {
        match op {
            MirOperand::Constant(c) => match c {
                MirConst::String(_) => "rask_print_string",
                MirConst::Bool(_) => "rask_print_bool",
                MirConst::Float(_) => "rask_print_f64",
                MirConst::Int128(_) => "rask_print_i128",
                _ => "rask_print_i64",
            },
            MirOperand::Local(id) => {
                if let Some(local) = locals.iter().find(|l| l.id == *id) {
                    match &local.ty {
                        MirType::Bool => "rask_print_bool",
                        MirType::F32 => "rask_print_f32",
                        MirType::F64 => "rask_print_f64",
                        MirType::Char => "rask_print_char",
                        MirType::String => "rask_print_string",
                        MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 => "rask_print_u64",
                        // The 64-bit printers take the low half and print a
                        // different number (#762).
                        MirType::I128 => "rask_print_i128",
                        MirType::U128 => "rask_print_u128",
                        _ => "rask_print_i64",
                    }
                } else {
                    "rask_print_i64"
                }
            }
        }
    }

    /// If the operand is a constant integer that's a power of 2, return the exponent.
    fn const_power_of_two(operand: &MirOperand) -> Option<u32> {
        if let MirOperand::Constant(MirConst::Int(n)) = operand {
            let n = *n;
            if n > 0 && (n & (n - 1)) == 0 {
                return Some(n.trailing_zeros());
            }
        }
        None
    }

    /// Look up the MirType of an operand from the locals table.
    /// Is this operand 128-bit — the one scalar width that doesn't fit a word?
    fn operand_is_wide(operand: &MirOperand, locals: &[rask_mir::MirLocal]) -> bool {
        match operand {
            MirOperand::Constant(MirConst::Int128(_)) => true,
            _ => matches!(
                Self::operand_mir_type(operand, locals),
                Some(MirType::I128 | MirType::U128)
            ),
        }
    }

    fn operand_mir_type(operand: &MirOperand, locals: &[rask_mir::MirLocal]) -> Option<MirType> {
        match operand {
            MirOperand::Local(id) => locals.iter().find(|l| l.id == *id).map(|l| l.ty.clone()),
            MirOperand::Constant(_) => None,
        }
    }

    /// True when a struct field's declared type uses stack-slot (aggregate)
    /// representation in codegen. These fields return a pointer into the parent
    /// struct rather than a loaded scalar.
    /// Byte size of a user struct/enum by name, or 0 if there's no layout for
    /// it (a runtime-opaque handle, or a type that never got one).
    fn named_layout_size(name: &str, ctx: &CodegenCtx) -> u32 {
        if let Some(l) = ctx.struct_layouts.iter().find(|l| l.name == name) {
            return l.size;
        }
        if let Some(l) = ctx.enum_layouts.iter().find(|l| l.name == name) {
            return l.size;
        }
        0
    }

    fn is_aggregate_field_type(ty: &RaskType, ctx: &CodegenCtx) -> bool {
        match ty {
            // Primitives, opaque pointers — scalar
            RaskType::Unit | RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64 | RaskType::I128
            | RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64 | RaskType::U128
            | RaskType::F32 | RaskType::F64
            | RaskType::Char
            | RaskType::Fn { .. } | RaskType::Slice(_) => false,
            // Runtime-opaque pointer types (Vec, Map, Pool, Handle, Channel, ...)
            RaskType::UnresolvedGeneric { .. } | RaskType::Generic { .. } => false,
            // A named type is an aggregate when it's a user struct or enum —
            // one with real bytes. Runtime-opaque handles (TcpListener, File,
            // Instant) have empty layouts and stay pointer-sized scalars.
            RaskType::UnresolvedNamed(n) => Self::named_layout_size(n, ctx) > 0,
            RaskType::Named(_) => false,
            // The niche options — `Handle<T>?` and `Link<T>?` — are one word
            // with a sentinel for `none`, so they load like a scalar. Answering
            // "aggregate" here handed back the field's *address*, and a root
            // edge read as a stack address instead of the node it named.
            ty if ty.is_option() && matches!(ty.as_option().unwrap(),
                RaskType::UnresolvedGeneric { name, .. } if name == "Handle" || name == "Link")
                => false,
            // The same option with its payload already resolved to a `TypeId`.
            // `Generic`/`Named` payloads are the runtime-opaque pointer types
            // (line above), and an option over one of those is the niche — one
            // word, no tag — so it loads like a scalar.
            ty if ty.is_option() && matches!(ty.as_option().unwrap(),
                RaskType::Generic { .. }) => false,
            // User-defined enums/structs, tuples, arrays, Option (T or none), Result — aggregate
            _ => true,
        }
    }

    /// Get actual (size, align) for a MirType, looking up struct/enum layouts.
    fn real_type_size_align(ty: &MirType, ctx: &CodegenCtx) -> (u32, u32) {
        match ty {
            MirType::Struct(id) => {
                if let Some(layout) = ctx.struct_layouts.get(id.id as usize) {
                    (layout.size as u32, layout.align as u32)
                } else {
                    (8, 8)
                }
            }
            MirType::Enum(id) => {
                if let Some(layout) = ctx.enum_layouts.get(id.id as usize) {
                    (layout.size as u32, layout.align as u32)
                } else {
                    (8, 8)
                }
            }
            _ => (ty.size(), ty.align()),
        }
    }

    fn lower_rvalue(
        builder: &mut ClifFunctionBuilder,
        rvalue: &MirRValue,
        expected_ty: Option<Type>,
        dst_mir_ty: Option<&MirType>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        match rvalue {
            MirRValue::Use(op) => {
                // A 64-bit constant going into an unsigned 128-bit slot is a
                // bit pattern, not a number — `u64::MAX` is carried as -1 —
                // and the generic constant path sign-extends, which would make
                // it `u128::MAX`. The destination's MIR type is the only place
                // the signedness is still visible at this point (#762).
                if let (Some(MirType::U128), MirOperand::Constant(MirConst::Int(n))) =
                    (dst_mir_ty, op)
                {
                    return Ok(Self::iconst_i128(builder, *n as u64 as i128));
                }
                Self::lower_operand_typed(builder, op, expected_ty, ctx)
            }

            MirRValue::BinaryOp { op, left, right } => Self::lower_binary_op(builder, op, left, right, expected_ty, dst_mir_ty, ctx),

            MirRValue::UnaryOp { op, operand } => {
                let val = Self::lower_operand_typed(builder, operand, expected_ty, ctx)?;
                let val_ty = builder.func.dfg.value_type(val);

                let result = match op {
                    UnaryOp::Neg if val_ty.is_float() => builder.ins().fneg(val),
                    // A negated integer literal (e.g. `-2147483648`) is a valid
                    // constant, not a runtime negation — fold it so the literal
                    // form of a type's MIN doesn't trip the overflow guard.
                    UnaryOp::Neg if matches!(operand, MirOperand::Constant(MirConst::Int(_))) => {
                        let n = match operand {
                            MirOperand::Constant(MirConst::Int(n)) => *n,
                            _ => unreachable!(),
                        };
                        Self::iconst_at(builder, val_ty, (n as i128).wrapping_neg())
                    }
                    UnaryOp::Neg => {
                        // OV1: negation overflows at signed MIN (and for any
                        // nonzero unsigned value).
                        let unsigned = Self::operand_mir_type(operand, ctx.locals)
                            .map(|t| t.is_unsigned())
                            .unwrap_or(false);
                        let overflowed = if unsigned {
                            let zero = Self::iconst_at(builder, val_ty, 0);
                            builder.ins().icmp(IntCC::NotEqual, val, zero)
                        } else {
                            let min = Self::emit_type_min(builder, val_ty);
                            builder.ins().icmp(IntCC::Equal, val, min)
                        };
                        Self::guard_overflow_unary(
                            builder, ctx, overflowed, OvKind::Neg,
                            val_ty.bits(), unsigned, val,
                        );
                        builder.ins().ineg(val)
                    }
                    // Logical NOT: XOR with 1 to flip the boolean bit.
                    // bnot flips all bits which is wrong for booleans
                    // (e.g. bnot(1) = 0xFE, not 0).
                    UnaryOp::Not => {
                        let val_ty = builder.func.dfg.value_type(val);
                        let one = builder.ins().iconst(val_ty, 1);
                        builder.ins().bxor(val, one)
                    }
                    UnaryOp::BitNot => builder.ins().bnot(val),
                    // Counts come back as the operand's own type, so a
                    // `count_ones()` on an i32 answers in i32 — matching the
                    // checker, which gives these the receiver's type.
                    UnaryOp::CountOnes => builder.ins().popcnt(val),
                    UnaryOp::LeadingZeros => builder.ins().clz(val),
                    UnaryOp::TrailingZeros => builder.ins().ctz(val),
                    UnaryOp::ReverseBits => builder.ins().bitrev(val),
                    UnaryOp::SwapBytes => builder.ins().bswap(val),
                };
                Ok(result)
            }

            MirRValue::Cast { value, target_ty } => {
                if target_ty == &MirType::String {
                    // Casting to string requires a runtime call that writes 16-byte
                    // RaskStr into an out-param. A plain type-cast (convert_value)
                    // would return a scalar, which copy_aggregate would dereference
                    // as a pointer — causing a segfault.
                    let src_mir_ty = Self::operand_mir_type(value, ctx.locals);
                    let (fn_name, arg_ty) = match src_mir_ty.as_ref() {
                        Some(MirType::Bool) => ("rask_bool_to_string", types::I64),
                        Some(MirType::F64) => ("rask_f64_to_string", types::F64),
                        Some(MirType::F32) => ("rask_f32_to_string", types::F32),
                        Some(MirType::Char) => ("rask_char_to_string", types::I32),
                        _ => match value {
                            MirOperand::Constant(MirConst::Bool(_)) => ("rask_bool_to_string", types::I64),
                            MirOperand::Constant(MirConst::Float(_)) => ("rask_f64_to_string", types::F64),
                            MirOperand::Constant(MirConst::Char(_)) => ("rask_char_to_string", types::I32),
                            _ => ("rask_i64_to_string", types::I64),
                        }
                    };

                    let fr = ctx.func_refs.get(fn_name)
                        .ok_or_else(|| CodegenError::FunctionNotFound(fn_name.to_string()))?;

                    let out_ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_ss, 0);

                    let mut val = Self::lower_operand_typed(builder, value, Some(arg_ty), ctx)?;
                    let val_ty = builder.func.dfg.value_type(val);
                    if val_ty != arg_ty {
                        let src_mir = Self::operand_mir_type(value, ctx.locals);
                        val = Self::convert_value(builder, val, val_ty, arg_ty, src_mir.as_ref());
                    }

                    builder.ins().call(*fr, &[out_ptr, val]);
                    return Ok(out_ptr);
                }

                let val = Self::lower_operand(builder, value, ctx)?;
                let target = mir_to_cranelift_type(target_ty)?;
                let val_ty = builder.func.dfg.value_type(val);
                let src_mir = Self::operand_mir_type(value, ctx.locals);
                Ok(Self::convert_value(builder, val, val_ty, target, src_mir.as_ref()))
            }

            MirRValue::Convert { value, source_ty, target_ty, kind } => {
                Self::lower_convert(builder, value, source_ty, target_ty, *kind, ctx)
            }

            // Struct/enum field access: load from base pointer + field offset
            MirRValue::Field { base, field_index, byte_offset, access } => Self::field_address_and_load(builder, base, field_index, byte_offset, access, expected_ty, ctx),

            // Enum discriminant extraction: load tag byte from base pointer
            MirRValue::EnumTag { value } => {
                let ptr_val = Self::lower_operand(builder, value, ctx)?;
                let base_ty = Self::operand_mir_type(value, ctx.locals);

                let (tag_offset, tag_cranelift_ty) = match &base_ty {
                    Some(MirType::Enum(id)) => {
                        if let Some(layout) = ctx.enum_layouts.get(id.id as usize) {
                            let offset = layout.tag_offset as i32;
                            // Derive Cranelift type from tag type's size
                            let (tag_size, _) = rask_mono::type_size_align(&layout.tag_ty, &Default::default());
                            let ty = match tag_size {
                                2 => types::I16,
                                _ => types::I8,
                            };
                            (offset, ty)
                        } else {
                            (0, types::I8)
                        }
                    }
                    _ => (0, types::I8),
                };

                let flags = MemFlags::new();
                Ok(builder.ins().load(tag_cranelift_ty, flags, ptr_val, tag_offset))
            }

            // Address-of: return the pointer that the local already holds (for aggregates)
            // or spill a scalar to a stack slot and return its address.
            MirRValue::Ref(local_id) => {
                let var = ctx.var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Ref: local not found".to_string()))?;
                let val = builder.use_var(*var);

                // For aggregate types the variable already IS a pointer
                let local_ty = ctx.locals.iter().find(|l| l.id == *local_id).map(|l| &l.ty);
                let is_aggregate = matches!(
                    local_ty,
                    Some(MirType::Struct(_) | MirType::Enum(_) | MirType::Array { .. }
                         | MirType::Tuple(_) | MirType::Slice(_) | MirType::Option(_)
                         | MirType::Result { .. } | MirType::Union(_))
                );

                if is_aggregate {
                    Ok(val)
                } else {
                    // Scalar: spill to a stack slot, return the address
                    let val_ty = builder.func.dfg.value_type(val);
                    let size = val_ty.bytes();
                    let ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        size,
                        0, // align_shift: natural alignment
                    ));
                    let addr = builder.ins().stack_addr(types::I64, ss, 0);
                    builder.ins().store(MemFlags::new(), val, addr, 0);
                    Ok(addr)
                }
            }

            // Pointer dereference: load the value pointed to by the operand
            MirRValue::Deref(operand) => {
                let ptr_val = Self::lower_operand(builder, operand, ctx)?;
                let load_ty = expected_ty.unwrap_or(types::I64);
                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, ptr_val, 0))
            }

            // Array element access: base_ptr + index * elem_size → load
            MirRValue::ArrayIndex { base, index, elem_size } => {
                let base_val = Self::lower_operand(builder, base, ctx)?;
                let idx_val = Self::lower_operand_typed(builder, index, Some(types::I64), ctx)?;
                let elem_sz = builder.ins().iconst(types::I64, *elem_size as i64);
                let offset = builder.ins().imul(idx_val, elem_sz);
                let addr = builder.ins().iadd(base_val, offset);
                // An element that lives *in* its slot comes back as an address —
                // everything downstream (field reads, tag reads, method
                // receivers) wants that. Loading eight bytes and treating them as
                // the value's pointer is what made every element read of
                // `[Pt { x: 1, y: 2 }, …]` segfault. The rule is
                // `MirType::stored_inline_in_array`, shared with the store in MIR
                // lowering, because the two only work as a pair.
                let elem_is_inline = Self::operand_mir_type(base, ctx.locals)
                    .and_then(|t| match t {
                        MirType::Array { elem, .. } => Some(elem.stored_inline_in_array()),
                        _ => None,
                    })
                    .unwrap_or(false);
                if elem_is_inline {
                    return Ok(addr);
                }
                let load_ty = expected_ty.unwrap_or(types::I64);
                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, addr, 0))
            }
        }
    }

    /// How deep a wrapper gets is not decided here.
    ///
    /// MIR lowering builds the layers, at every position listed in
    /// `rask_ast::coercion::CoercionSite`, so an assignment that reaches codegen
    /// should already agree with its destination's depth. The multi-layer
    /// widener that used to live here is gone — it was the only thing that could
    /// add a second layer, and it typed the payload by peeling exactly one,
    /// which is what made `f32??` read back as 0 (#493, #637, #701).
    ///
    /// The single-layer wrap below stays as a net for MIR that still builds its
    /// own slots rather than going through the shared coercion. To re-measure
    /// what still needs it, put an `eprintln!` in each wrap branch and compile
    /// `tests/suite/*.rk examples/*.rk benchmarks/*.rk projects/*/*.rk`; as of
    /// this change only the aggregate branch and the return terminator's Ok/Err
    /// fire at all, and only from `tiered_store`, `text_editor`,
    /// `18_resource_types` and `t_try_error_wrap`.
    fn lower_assign(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        rvalue: &MirRValue,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let dst_local = ctx.locals.iter().find(|l| l.id == *dst)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Destination variable not found".to_string()))?;
        let container_ty = mir_to_cranelift_type(&dst_local.ty)?;

        // Which shape this assignment takes is decided by the destination type
        // and the rvalue alone — never by the lowered value — so it's settled
        // first. The value's type depends on it: a scalar being wrapped as a
        // payload is typed by the payload, not by the container.

        // When dest is Option(T) and the source is already Option-typed,
        // copy the struct. When the source is a scalar, wrap as Some.
        let src_option_ty = if let MirType::Option(_) = &dst_local.ty {
            if let MirRValue::Use(MirOperand::Local(src_id)) = rvalue {
                ctx.locals.iter().find(|l| l.id == *src_id)
                    .map(|l| matches!(l.ty, MirType::Option(_)))
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        };

        // Aggregate assignment: when the destination has a stack slot and
        // the rvalue produces a pointer to aggregate data, copy the data
        // into the destination's stack slot rather than aliasing pointers.
        // This covers String (always 16 bytes) and Field extractions from
        // Struct/Tuple/Result/Option that return aggregate sub-fields.
        //
        // Whole-aggregate assignment (`p = q` where both are Struct/Tuple/etc.)
        // also needs a memcpy: aliasing the pointers means a subsequent
        // `mutate p` write would land in `q`'s storage. mem.borrowing/M-rules
        // require `mutate` writes to flow back to the caller, which only
        // works if `p = ...` copies bytes into `*p`'s slot.
        let needs_copy = match (&dst_local.ty, rvalue) {
            (MirType::String, _) => true,
            // Field on aggregate base returns pointer for aggregate elements.
            // TraitObject belongs here: it's a 16-byte fat pointer, so reading one
            // out of a `T?` payload as a scalar took the data half and left the
            // vtable behind. The call through it then read the concrete value's
            // first word as a vtable — for `Circle { r: 2.0 }` that meant
            // jumping through 2.0 (#552).
            (MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) |
             MirType::Result { .. } | MirType::Option(_) |
             MirType::TraitObject { .. }, MirRValue::Field { .. }) => true,
            // Whole-aggregate copy: rvalue produces a pointer to the source
            // aggregate, dst has its own storage (either a stack slot or an
            // external pointer for mutate-params).
            (MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_),
             MirRValue::Use(MirOperand::Local(_))) => true,
            // Result/Option whole-aggregate copy: only when src and dst
            // have the same general shape. Avoid clobbering layout when
            // src is Result and dst is Option (different payload offsets).
            (MirType::Result { .. }, MirRValue::Use(MirOperand::Local(src_id))) => {
                ctx.locals.iter().find(|l| l.id == *src_id)
                    .map_or(false, |l| matches!(l.ty, MirType::Result { .. }))
            }
            (MirType::Option(_), MirRValue::Use(MirOperand::Local(src_id))) => {
                ctx.locals.iter().find(|l| l.id == *src_id)
                    .map_or(false, |l| matches!(l.ty, MirType::Option(_)))
            }
            // An array element that lives in its slot comes back as an address,
            // and for a wrapper element that address *is* a whole wrapper. Copy
            // it. Without this arm the wrap-as-Some path below claimed the
            // assignment and built `Some(<element address>)`, so `a[0] ?? d` on a
            // `[i64?; 3]` handed back the address (#783).
            (MirType::Option(_) | MirType::Result { .. },
             MirRValue::ArrayIndex { base, .. }) => {
                Self::operand_mir_type(base, ctx.locals).is_some_and(|t| {
                    matches!(t, MirType::Array { elem, .. } if elem.stored_inline_in_array())
                })
            }
            // `try convert`/`try float to int` builds an Option slot and
            // returns its pointer — copy the 16-byte struct into dst.
            (MirType::Option(_), MirRValue::Convert { kind, .. }) => kind.is_optional(),
            // Option(T) assigned from an Option-typed local: copy the 16-byte struct
            (MirType::Option(_), _) if src_option_ty => true,
            _ => false,
        };

        // Option(T) assigned from a non-Option source: wrap as Some
        // in the stack slot. Scalars need this so `const x: i32? = 42`
        // doesn't overwrite x's slot-address with the scalar 42 (later
        // tag loads would dereference 42 as a pointer and SIGSEGV).
        // Aggregates (Struct/Enum/Tuple/String) need it so the bytes
        // land at PAYLOAD_OFFSET of the Option slot, not just the
        // pointer in the first 8 bytes — otherwise field reads
        // through the Option's payload return garbage.
        let wrap_as_some = matches!(&dst_local.ty, MirType::Option(_))
            && !needs_copy
            && ctx.stack_slot_map.contains_key(dst);
        // If the source is an aggregate and dst is Option<aggregate>,
        // we need full-aggregate wrap (tag + memcpy payload), not the
        // scalar wrap.
        // A string constant lowers to the address of its 16 bytes, same as a
        // string local — so it takes the aggregate wrap too. Treated as a
        // scalar, `let a: string? = "hello"` stored the *pointer* at the
        // payload offset and reading the payload back handed out that slot's
        // address instead of the string (#613).
        let src_is_addressed = match rvalue {
            MirRValue::Use(MirOperand::Local(_)) => true,
            MirRValue::Use(MirOperand::Constant(MirConst::String(_))) => true,
            _ => false,
        };
        let wrap_as_some_aggregate = wrap_as_some
            && src_is_addressed
            && if let MirType::Option(inner) = &dst_local.ty {
                matches!(inner.as_ref(),
                    MirType::Struct(_) | MirType::Enum(_) |
                    MirType::Tuple(_) | MirType::String)
            } else { false };

        // On the scalar-wrap path the value becomes the Option's payload, so it
        // is typed by the payload — not by the container, which maps to I64
        // because an Option is addressed by pointer. Coercing to the container's
        // type ran a float through `fcvt_to_sint_sat`: `let x: f64? = 2.5`
        // stored the integer 2, and the payload read (a plain float load) turned
        // those bits back into 1e-323 (#608).
        let dst_ty = if wrap_as_some && !wrap_as_some_aggregate {
            match &dst_local.ty {
                MirType::Option(inner) => Self::scalar_payload_store_type(inner)?
                    .unwrap_or(container_ty),
                _ => container_ty,
            }
        } else {
            container_ty
        };

        let mut val = Self::lower_rvalue(builder, rvalue, Some(dst_ty), Some(&dst_local.ty), ctx)?;

        let val_ty = builder.func.dfg.value_type(val);
        if val_ty != dst_ty {
            // The source's MIR type carries the signedness a Cranelift integer
            // doesn't. Without it every widening sign-extends, so an implicit
            // `u32` → `i64` (CV1a) would turn 3000000000 into a negative
            // (#326's family, reached by a new route).
            let src_mir = match rvalue {
                MirRValue::Use(op) => Self::operand_mir_type(op, ctx.locals),
                _ => None,
            };
            val = Self::convert_value(builder, val, val_ty, dst_ty, src_mir.as_ref());
        }

        if needs_copy {
            if let Some((dst_ss, dst_size)) = ctx.stack_slot_map.get(dst) {
                Self::copy_aggregate(builder, val, *dst_ss, *dst_size);
            } else if matches!(&dst_local.ty,
                MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_))
            {
                // Dst variable holds an external pointer (mutate-param) —
                // copy bytes through it instead of overwriting the pointer.
                let size = Self::resolve_type_alloc_size(
                    &dst_local.ty, ctx.struct_layouts, ctx.enum_layouts,
                ).unwrap_or(0);
                if size > 0 {
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                    let dst_ptr = builder.use_var(*var);
                    Self::copy_aggregate_to_ptr(builder, val, dst_ptr, size);
                } else {
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                    builder.def_var(*var, val);
                }
            } else {
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                builder.def_var(*var, val);
            }
        } else if wrap_as_some_aggregate {
            // Some(aggregate): tag + payload bytes copied at PAYLOAD_OFFSET.
            let (dst_ss, _) = ctx.stack_slot_map.get(dst).unwrap();
            let inner_size = if let MirType::Option(inner) = &dst_local.ty {
                Self::resolve_type_alloc_size(
                    inner.as_ref(), ctx.struct_layouts, ctx.enum_layouts,
                ).unwrap_or(inner.size())
            } else { 0 };
            Self::build_wrapped_aggregate(builder, *dst_ss, false, 0, val, inner_size);
        } else if wrap_as_some {
            let (dst_ss, _) = ctx.stack_slot_map.get(dst).unwrap();
            Self::build_some(builder, *dst_ss, val);
        } else {
            let var = ctx.var_map.get(dst)
                .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
            builder.def_var(*var, val);
        }
        Ok(())
    }

    /// Store a scalar into a slot narrower than a word.
    ///
    /// A slot the layout packed into 1, 2 or 4 bytes gets a store that wide. An
    /// 8-byte store into a 4-byte slot walks into whatever follows: as a struct
    /// field it took the return address with it (#548), and as an array element
    /// it silently overwrote the next element (#902). Both callers share this so
    /// the two can't drift apart again.
    fn store_narrow(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        addr: Value,
        offset: i32,
        size: u32,
    ) {
        let narrow = match size {
            1 => types::I8,
            2 => types::I16,
            _ => types::I32,
        };
        let val_ty = builder.func.dfg.value_type(val);
        let val = if val_ty.is_float() {
            // How wide this slot holds a float is `rask_mono::abi`'s to say, not
            // this function's — a word takes the promoted f64, anything narrower
            // takes the f32. The value can arrive either way, so bring it to
            // whichever the slot wants before storing its bits.
            let want_bytes = rask_mono::abi::slot_scalar_bytes(
                true, val_ty.bytes(), size,
            );
            let val = if want_bytes < 8 && val_ty.bits() > 32 {
                builder.ins().fdemote(types::F32, val)
            } else {
                val
            };
            builder.ins().bitcast(types::I32, MemFlags::new(), val)
        } else if val_ty.bits() > narrow.bits() {
            builder.ins().ireduce(narrow, val)
        } else if val_ty.bits() < narrow.bits() {
            builder.ins().uextend(narrow, val)
        } else {
            val
        };
        builder.ins().store(MemFlags::new(), val, addr, offset);
    }

    fn lower_store(
        builder: &mut ClifFunctionBuilder,
        addr: &LocalId,
        offset: &u32,
        value: &MirOperand,
        store_size: &Option<u32>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let addr_val = builder.use_var(*ctx.var_map.get(addr)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Address variable not found".to_string()))?);

        // If the value is a stack-allocated aggregate (struct/enum), copy its
        // data instead of storing the pointer. This handles Ok(struct_val) where
        // the struct data must be embedded in the Result's payload area.
        // Use the variable's current value (not the stack_slot address) because
        // the variable may alias another slot (e.g., p = struct_literal result).
        let is_aggregate = if let MirOperand::Local(src_id) = value {
            if let Some((_src_slot, src_size)) = ctx.stack_slot_map.get(src_id) {
                // Use store_size when available to avoid overflowing the
                // destination.
                let effective_size = store_size
                    .map(|ss| ss.min(*src_size))
                    .unwrap_or(*src_size);
                // Copy the bytes even when the aggregate fits in 8 bytes.
                // Storing the pointer instead left the field aimed at the
                // constructing function's frame; it read back fine until
                // something reused that frame, which is how `Request.method`
                // arrived at the middleware with an out-of-range tag (#474).
                // Fields larger than 8 bytes were always copied — this makes
                // the small ones behave the same.
                let src_var = ctx.var_map.get(src_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Aggregate source not found".to_string()))?;
                let src_addr = builder.use_var(*src_var);
                Self::copy_bytes(builder, src_addr, 0, addr_val, *offset as i32, effective_size);
                true
            } else { false }
        } else { false };

        // An aggregate-typed *parameter* is a pointer to the caller's data.
        // Params get no stack slot of their own, so they miss the copy above and
        // the pointer itself would land in the field — `create_task(priority:
        // Priority)` stored the caller's stack address where the enum tag
        // belongs, and reading it back trapped on an out-of-range tag. Copy the
        // pointee, same as a slot-backed aggregate.
        let is_aggregate = is_aggregate || Self::copy_from_aggregate_param(
            builder, addr_val, *offset, value, store_size, ctx,
        );

        if !is_aggregate {
            let val = Self::lower_operand(builder, value, ctx)?;

            let val_ty = builder.func.dfg.value_type(val);

            // store_size > 8: the lowered value is a pointer to aggregate data
            // (e.g., string constant → 16-byte SSO). Copy word-by-word from
            // the source pointer instead of storing the pointer itself.
            //
            // Only when it really is a pointer, though, and an address is
            // always a word. An i128 is the one scalar wider than that:
            // Cranelift keeps it in a register pair, so the value *is* the
            // data. Copying from it emitted `load.i64` against an i128 and the
            // verifier rejected the function before anything ran — every
            // `struct S { balance: i128 }` and `Vec<i128>` failed to build
            // (#933). A plain store handles it, the same as any other scalar.
            if store_size.map_or(false, |s| s > 8) && val_ty == types::I64 {
                let size = store_size.unwrap();
                Self::copy_bytes(builder, val, 0, addr_val, *offset as i32, size);
            } else {
                let flags = MemFlags::new();

                // A field the layout packed into fewer than 8 bytes gets a
                // store that wide. An 8-byte store into a 4-byte slot walks
                // into whatever follows: a two-i32 tuple wrote its second
                // element across the frame's edge and took the return address
                // with it, so the test binary jumped into nowhere (#548).
                if let Some(size @ (1 | 2 | 4)) = *store_size {
                    Self::store_narrow(builder, val, addr_val, *offset as i32, size);
                    return Ok(());
                }

                // Otherwise the slot is a full word. Widen sub-8-byte values to
                // fill it — a 4-byte f32 store would leave stale upper bytes
                // that corrupt the f64 read-back.
                let val = if val_ty == types::F32 {
                    builder.ins().fpromote(types::F64, val)
                } else if val_ty.is_int() && val_ty.bits() < 64 {
                    let src_mir = Self::operand_mir_type(value, ctx.locals);
                    Self::convert_value(builder, val, val_ty, types::I64, src_mir.as_ref())
                } else {
                    val
                };

                builder.ins().store(flags, val, addr_val, *offset as i32);
            }
        }
        Ok(())
    }

    /// Aggregate params are passed by pointer. Where the callee declares a
    /// struct/enum/tuple but the caller's own local is a scalar, spill the value
    /// into a stack slot and pass its address, so the callee's "this is a
    /// pointer" assumption always holds.
    ///
    /// MIR sometimes types an 8-byte struct as a bare `i64` — a payload
    /// extraction that had no checker type to read falls back to it — and then a
    /// value reaches a by-pointer param. Fixing every such fallback is the real
    /// cure; this makes the boundary safe either way.
    fn spill_scalars_for_aggregate_params(
        builder: &mut ClifFunctionBuilder,
        callee: &str,
        arg_vals: &mut [Value],
        mir_args: &[MirOperand],
        ctx: &CodegenCtx,
    ) {
        let Some(param_tys) = ctx.fn_param_types.get(callee) else { return };
        for (i, param_ty) in param_tys.iter().enumerate() {
            if i >= arg_vals.len() {
                break;
            }
            if !matches!(
                param_ty,
                MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. }
            ) {
                continue;
            }
            // The operand's own type decides. Aggregate-typed operands already
            // hold a pointer; a scalar-typed one holds the value itself.
            let arg_is_aggregate = Self::operand_arg_type(&mir_args[i], ctx).is_none_or(|ty| {
                matches!(
                    ty,
                    MirType::Struct(_)
                        | MirType::Enum(_)
                        | MirType::Tuple(_)
                        | MirType::Array { .. }
                        | MirType::Ptr
                        | MirType::Handle
                )
            });
            if arg_is_aggregate {
                continue;
            }
            let (size, _) = Self::real_type_size_align(param_ty, ctx);
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size.max(8),
                0,
            ));
            builder.ins().stack_store(arg_vals[i], ss, 0);
            arg_vals[i] = builder.ins().stack_addr(types::I64, ss, 0);
        }
    }

    /// MIR type of a call argument, looking in both locals and params.
    fn operand_arg_type(operand: &MirOperand, ctx: &CodegenCtx) -> Option<MirType> {
        match operand {
            MirOperand::Local(id) => ctx
                .locals
                .iter()
                .chain(ctx.params.iter())
                .find(|l| l.id == *id)
                .map(|l| l.ty.clone()),
            MirOperand::Constant(_) => None,
        }
    }

    /// Copy a struct/enum/tuple parameter's bytes into a field. Returns false
    /// when `value` isn't such a parameter and the caller should store a scalar.
    ///
    /// Only parameters qualify: every other aggregate local is slot-backed, and
    /// one that isn't means the MIR type doesn't match the representation —
    /// dereferencing it would turn a wrong value into a crash.
    fn copy_from_aggregate_param(
        builder: &mut ClifFunctionBuilder,
        addr_val: Value,
        offset: u32,
        value: &MirOperand,
        store_size: &Option<u32>,
        ctx: &CodegenCtx,
    ) -> bool {
        let MirOperand::Local(src_id) = value else { return false };
        if ctx.stack_slot_map.contains_key(src_id) {
            return false;
        }
        let Some(local) = ctx.params.iter().find(|l| l.id == *src_id) else { return false };
        if !matches!(
            local.ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. }
        ) {
            return false;
        }
        let (real_size, _) = Self::real_type_size_align(&local.ty, ctx);
        // Layout gives fields 8-byte slots, so store_size can exceed the type's
        // real size. Copy only what the value actually has.
        let size = store_size.map_or(real_size, |ss| ss.min(real_size));
        if size == 0 {
            return false;
        }
        let Some(src_var) = ctx.var_map.get(src_id) else { return false };
        let src_addr = builder.use_var(*src_var);
        Self::copy_bytes(builder, src_addr, 0, addr_val, offset as i32, size);
        true
    }

    fn lower_pool_checked_access(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        pool: &LocalId,
        handle: &LocalId,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let pool_val = builder.use_var(*ctx.var_map.get(pool)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "Pool variable not found".to_string()
            ))?);
        let handle_val = builder.use_var(*ctx.var_map.get(handle)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "Handle variable not found".to_string()
            ))?);

        // Determine result type before emitting IR
        let is_struct = ctx.locals.iter()
            .find(|l| l.id == *dst)
            .map(|l| matches!(&l.ty, MirType::Struct(_)))
            .unwrap_or(false);
        let load_ty = ctx.locals.iter()
            .find(|l| l.id == *dst)
            .and_then(|l| mir_to_cranelift_type(&l.ty).ok())
            .unwrap_or(types::I64);

        if ctx.build_mode == BuildMode::Release {
            // ── Inline pool access (release mode) ──────────────
            // Emits bounds check + generation check + data load directly
            // as Cranelift IR, avoiding the C function call overhead.
            //
            // Pool layout (verified by _Static_assert in pool.c):
            //   offset 16: slot_stride (i64)
            //   offset 24: cap (i64)
            //   offset 40: slots (ptr)
            // Slot layout (stride varies by elem_size):
            //   offset 0: generation (u32)
            //   offset 8: data (elem_size bytes)
            use crate::layouts::*;

            // 1. Extract index and generation from packed i64 handle
            //    handle = index:32 | generation:32
            let index = builder.ins().band_imm(handle_val, 0xFFFF_FFFF_i64);
            let gen_i64 = builder.ins().ushr_imm(handle_val, 32);
            let gen = builder.ins().ireduce(types::I32, gen_i64);

            // 2. Bounds check: index < cap
            let cap = builder.ins().load(types::I64, MemFlags::new(), pool_val, POOL_CAP_OFFSET);
            let oob = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, index, cap);

            let panic_block = builder.create_block();
            let bounds_ok = builder.create_block();
            builder.ins().brif(oob, panic_block, &[], bounds_ok, &[]);

            Self::emit_panic_block(builder, panic_block, "pool access with invalid handle", ctx);

            // bounds_ok: load slots pointer and stride, compute slot address
            builder.switch_to_block(bounds_ok);
            builder.seal_block(bounds_ok);
            let slots = builder.ins().load(types::I64, MemFlags::new(), pool_val, POOL_SLOTS_OFFSET);
            let stride = builder.ins().load(types::I64, MemFlags::new(), pool_val, POOL_STRIDE_OFFSET);
            let slot_offset = builder.ins().imul(index, stride);
            let slot_addr = builder.ins().iadd(slots, slot_offset);

            // 3. Generation check
            let slot_gen = builder.ins().load(types::I32, MemFlags::new(), slot_addr, SLOT_GEN_OFFSET);
            let gen_mismatch = builder.ins().icmp(IntCC::NotEqual, gen, slot_gen);

            let gen_panic_block = builder.create_block();
            let ok_block = builder.create_block();
            builder.ins().brif(gen_mismatch, gen_panic_block, &[], ok_block, &[]);

            Self::emit_panic_block(builder, gen_panic_block, "pool access with invalid handle", ctx);

            // ok_block: load data (single predecessor, seal immediately)
            builder.switch_to_block(ok_block);
            builder.seal_block(ok_block);
            let var = ctx.var_map.get(dst)
                .ok_or_else(|| CodegenError::UnsupportedFeature(
                    "Pool access destination not found".to_string()
                ))?;
            // Always return pointer to slot data — pool[h] is used
            // for mutation, so callers need the address.
            let data_ptr = builder.ins().iadd_imm(slot_addr, SLOT_DATA_OFFSET as i64);
            builder.def_var(*var, data_ptr);
        } else {
            // ── Debug mode: call C function ──────────────────────
            let call_inst = if let Some(file_str) = ctx.source_file {
                if let (Some(func_ref), Some(gv)) = (
                    ctx.func_refs.get("pool_get_checked"),
                    ctx.string_globals.get(file_str),
                ) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                    let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                    builder.ins().call(*func_ref, &[pool_val, handle_val, file_ptr, line_val, col_val])
                } else {
                    let func_ref = ctx.func_refs.get("Pool_checked_access")
                        .ok_or_else(|| CodegenError::FunctionNotFound("Pool_checked_access".to_string()))?;
                    builder.ins().call(*func_ref, &[pool_val, handle_val])
                }
            } else {
                let func_ref = ctx.func_refs.get("Pool_checked_access")
                    .ok_or_else(|| CodegenError::FunctionNotFound("Pool_checked_access".to_string()))?;
                builder.ins().call(*func_ref, &[pool_val, handle_val])
            };

            let results = builder.inst_results(call_inst);
            if !results.is_empty() {
                let ptr = results[0];
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Pool access destination not found".to_string()
                    ))?;
                // Always return raw pointer — pool[h] is used for
                // mutation (pool[h].field = val), so callers need
                // the address, not the loaded value.
                builder.def_var(*var, ptr);
            }
        }
        Ok(())
    }

    fn lower_closure_create(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        func_name: &String,
        captures: &[rask_mir::ClosureCapture],
        heap: &bool,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        // Build environment layout from captures, using real aggregate
        // sizes from codegen layouts instead of MIR fallbacks.
        // MirType::Struct.size() returns 8 (pointer), but actual structs
        // may be 16+ bytes. Escaping closures must deep-copy aggregate
        // data so it survives after the parent's stack is reused.
        let mut env_layout = crate::closures::ClosureEnvLayout::new();
        for c in captures {
            let local = ctx.locals.iter().find(|l| l.id == c.local_id);
            let (real_size, is_aggregate) = if let Some(l) = local {
                if let Some(alloc_size) = Self::resolve_type_alloc_size(
                    &l.ty, ctx.struct_layouts, ctx.enum_layouts,
                ) {
                    (alloc_size, true)
                } else {
                    (c.size, false)
                }
            } else {
                (c.size, false)
            };
            env_layout.add_capture(c.local_id, real_size, is_aggregate);
        }

        // Get function pointer for the closure function
        let func_ref = ctx.func_refs.get(func_name)
            .ok_or_else(|| CodegenError::FunctionNotFound(func_name.clone()))?;
        let func_ptr = builder.ins().func_addr(types::I64, *func_ref);

        let closure_ptr = if *heap {
            // Escaping closure: heap-allocate via rask_alloc
            let alloc_ref = ctx.func_refs.get("rask_alloc")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_alloc".to_string()))?;
            crate::closures::allocate_closure_heap(
                builder, func_ptr, &env_layout, ctx.var_map, *alloc_ref,
            )?
        } else {
            // Non-escaping closure: stack-allocate
            crate::closures::allocate_closure_stack(
                builder, func_ptr, &env_layout, ctx.var_map,
            )?
        };

        let var = ctx.var_map.get(dst)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "ClosureCreate destination not found".to_string()
            ))?;
        builder.def_var(*var, closure_ptr);
        Ok(())
    }

    fn lower_closure_call(
        builder: &mut ClifFunctionBuilder,
        dst: &Option<LocalId>,
        closure: &LocalId,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let closure_val = builder.use_var(*ctx.var_map.get(closure)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "Closure variable not found".to_string()
            ))?);

        // Lower arg values
        let mut arg_vals = Vec::new();
        for a in args {
            let val = Self::lower_operand(builder, a, ctx)?;
            arg_vals.push(val);
        }

        // Build signature: (args...) -> ret
        // call_closure will prepend env_ptr automatically
        let mut sig = builder.func.signature.clone();
        sig.params.clear();
        sig.returns.clear();

        for val in &arg_vals {
            let ty = builder.func.dfg.value_type(*val);
            sig.params.push(AbiParam::new(ty));
        }

        if let Some(dst_id) = dst {
            let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
            if let Some(local) = dst_local {
                let cl_ret_ty = mir_to_cranelift_type(&local.ty)?;
                sig.returns.push(AbiParam::new(cl_ret_ty));
            }
        }

        let call_inst = crate::closures::call_closure(
            builder, closure_val, sig, &arg_vals,
        );

        if let Some(dst_id) = dst {
            let result = builder.inst_results(call_inst).first().copied();
            if let Some(result) = result {
                let var = ctx.var_map.get(dst_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "ClosureCall destination not found".to_string()
                    ))?;
                // A closure body is an ordinary Rask function, so it returns an
                // aggregate the way the Return terminator does: the bare value
                // when it fits in a word, otherwise a pointer into its own frame.
                // Neither survives being parked in the destination variable —
                // the word gets dereferenced as an address (#633) and the
                // pointer is overwritten by the next call's frame (#611). Copy
                // into the destination's own slot here, like every other call
                // site does.
                if let Some((ss, size)) = ctx.stack_slot_map.get(dst_id) {
                    match *size {
                        8 => { builder.ins().stack_store(result, *ss, 0); }
                        // A slot narrower than a word still gets a whole word
                        // back, because the callee loaded one. Only the low
                        // `size` bytes mean anything, and storing all 8 would
                        // run off the end of a 2-byte slot into whatever sits
                        // next to it. Park the word in a scratch slot that can
                        // hold it and copy out just the meaningful bytes.
                        n if n < 8 => {
                            let scratch = builder.create_sized_stack_slot(
                                StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 0),
                            );
                            builder.ins().stack_store(result, scratch, 0);
                            let scratch_ptr = builder.ins().stack_addr(types::I64, scratch, 0);
                            Self::copy_aggregate(builder, scratch_ptr, *ss, n);
                        }
                        n => { Self::copy_aggregate(builder, result, *ss, n); }
                    }
                    let addr = builder.ins().stack_addr(types::I64, *ss, 0);
                    builder.def_var(*var, addr);
                } else {
                    builder.def_var(*var, result);
                }
            }
        }
        Ok(())
    }

    fn lower_trait_box(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        value: &MirOperand,
        vtable_name: &String,
        concrete_size: &u32,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let alloc_ref = ctx.func_refs.get("rask_alloc")
            .ok_or_else(|| CodegenError::FunctionNotFound("rask_alloc".to_string()))?;

        // Allocate heap memory for the concrete value (min 8 to avoid null from zero-size alloc)
        let alloc_size = std::cmp::max(*concrete_size, 8) as i64;
        let size_val = builder.ins().iconst(types::I64, alloc_size);
        let call_inst = builder.ins().call(*alloc_ref, &[size_val]);
        let data_ptr = builder.inst_results(call_inst)[0];

        // Copy concrete value to heap
        if let MirOperand::Local(src_id) = value {
            if ctx.stack_slot_map.contains_key(src_id) {
                // Aggregate: memcpy from the source pointer the var holds —
                // not the local's own slot, which may be uninitialized when
                // the var aliases another slot (e.g. `_1 = _0`).
                let src_var = ctx.var_map.get(src_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitBox: source variable not found".to_string()
                    ))?;
                let src_ptr = builder.use_var(*src_var);
                let sz = *concrete_size;
                let mut off = 0i32;
                while (off as u32) + 8 <= sz {
                    let word = builder.ins().load(types::I64, MemFlags::new(), src_ptr, off);
                    builder.ins().store(MemFlags::new(), word, data_ptr, off);
                    off += 8;
                }
                if (off as u32) < sz {
                    let word = builder.ins().load(types::I64, MemFlags::new(), src_ptr, off);
                    builder.ins().store(MemFlags::new(), word, data_ptr, off);
                }
            } else {
                // Scalar: load from variable, store to heap
                let src_val = builder.use_var(*ctx.var_map.get(src_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitBox: source variable not found".to_string()
                    ))?);
                builder.ins().store(MemFlags::new(), src_val, data_ptr, 0);
            }
        } else {
            // Constant: lower and store
            let src_val = Self::lower_operand(builder, value, ctx)?;
            builder.ins().store(MemFlags::new(), src_val, data_ptr, 0);
        }

        // Get vtable address
        let gv = ctx.vtable_globals.get(vtable_name.as_str())
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                format!("TraitBox: vtable '{}' not found", vtable_name)
            ))?;
        let vtable_ptr = builder.ins().global_value(types::I64, *gv);

        // Store fat pointer into destination stack slot: [data_ptr, vtable_ptr]
        let (ss, _) = ctx.stack_slot_map.get(dst)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "TraitBox destination stack slot not found".to_string()
            ))?;
        let dst_addr = builder.ins().stack_addr(types::I64, *ss, 0);
        builder.ins().store(MemFlags::new(), data_ptr, dst_addr, crate::layouts::FAT_PTR_DATA_OFFSET);
        builder.ins().store(MemFlags::new(), vtable_ptr, dst_addr, crate::layouts::FAT_PTR_VTABLE_OFFSET);

        // Set the variable to point to the stack slot
        let var = ctx.var_map.get(dst)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "TraitBox destination variable not found".to_string()
            ))?;
        builder.def_var(*var, dst_addr);
        Ok(())
    }

    fn lower_trait_call(
        builder: &mut ClifFunctionBuilder,
        dst: &Option<LocalId>,
        trait_object: &LocalId,
        method_name: &String,
        vtable_offset: &u32,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        // Load fat pointer components from trait object stack slot
        let obj_val = builder.use_var(*ctx.var_map.get(trait_object)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "TraitCall: trait object variable not found".to_string()
            ))?);
        let data_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_DATA_OFFSET);
        let vtable_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_VTABLE_OFFSET);

        // Load function pointer from vtable
        let func_ptr = builder.ins().load(
            types::I64, MemFlags::new(), vtable_ptr, *vtable_offset as i32,
        );

        // Build signature: (data_ptr, args...) -> ret.
        //
        // From the operands' own types, not I64 for everything. An aggregate
        // travels as a pointer, but a float travels in a float register — an
        // `f64`-returning trait method declared as returning I64 put the ABI
        // and the callee on different registers entirely.
        let abi_ty = |ty: Option<&MirType>| -> Type {
            match ty {
                Some(
                    MirType::Struct(_)
                    | MirType::Enum(_)
                    | MirType::Tuple(_)
                    | MirType::Array { .. }
                    | MirType::Result { .. }
                    | MirType::Option(_)
                    | MirType::String
                    | MirType::TraitObject { .. },
                ) => types::I64,
                Some(t) => mir_to_cranelift_type(t).unwrap_or(types::I64),
                None => types::I64,
            }
        };

        let mut sig = Signature::new(isa::CallConv::SystemV);
        sig.params.push(AbiParam::new(types::I64)); // data_ptr (self)
        let arg_tys: Vec<MirType> = args
            .iter()
            .map(|a| Self::operand_mir_type(a, ctx.locals).unwrap_or(MirType::I64))
            .collect();
        for ty in &arg_tys {
            sig.params.push(AbiParam::new(abi_ty(Some(ty))));
        }
        let ret_mir = dst
            .as_ref()
            .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| l.ty.clone());
        if !matches!(ret_mir, Some(MirType::Void)) {
            sig.returns.push(AbiParam::new(abi_ty(ret_mir.as_ref())));
        }

        // Build argument values
        let mut call_args = Vec::with_capacity(1 + args.len());
        call_args.push(data_ptr);
        for (arg, want) in args.iter().zip(arg_tys.iter()) {
            let want_cl = abi_ty(Some(want));
            let val = Self::lower_operand_typed(builder, arg, Some(want_cl), ctx)?;
            let val_ty = builder.func.dfg.value_type(val);
            let val = if val_ty != want_cl {
                Self::convert_value(builder, val, val_ty, want_cl, Some(want))
            } else {
                val
            };
            call_args.push(val);
        }

        let sig_ref = builder.import_signature(sig);
        let call_inst = builder.ins().call_indirect(sig_ref, func_ptr, &call_args);

        if let Some(dst_id) = dst {
            let result = builder.inst_results(call_inst)[0];
            let var = ctx.var_map.get(dst_id)
                .ok_or_else(|| CodegenError::UnsupportedFeature(
                    format!("TraitCall destination for '{}' not found", method_name)
                ))?;
            // Aggregate-returning methods hand back a pointer to data in the
            // callee frame, so copy into the dst's slot before that frame goes
            // away — except when the aggregate fits in a word, where a Rask
            // function returns the value itself (see the Return terminator).
            // Dereferencing that was the crash: a fieldless enum through a
            // vtable read address 1 (#474).
            if let Some((dst_ss, dst_size)) = ctx.stack_slot_map.get(dst_id) {
                if *dst_size <= 8 {
                    builder.ins().stack_store(result, *dst_ss, 0);
                } else {
                    Self::copy_aggregate(builder, result, *dst_ss, *dst_size);
                }
                let addr = builder.ins().stack_addr(types::I64, *dst_ss, 0);
                builder.def_var(*var, addr);
            } else {
                builder.def_var(*var, result);
            }
        }
        Ok(())
    }

    fn lower_binary_op(
        builder: &mut ClifFunctionBuilder,
        op: &BinOp,
        left: &MirOperand,
        right: &MirOperand,
        expected_ty: Option<Type>,
        dst_mir_ty: Option<&MirType>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        let is_comparison = matches!(op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );
        // Ops that answer a bool from operands of their own width. The
        // destination type can't set the operand width for these: an
        // `OverflowAdd` writing into a Bool slot would truncate both operands to
        // one byte, and `(2147483647 as i32).checked_add(1)` then asked whether
        // -1 + 0 overflows an i8 — no — so the overflow went unnoticed.
        let answers_bool = is_comparison || matches!(op,
            BinOp::OverflowAdd | BinOp::OverflowSub
            | BinOp::OverflowMul | BinOp::OverflowDiv
        );

        // Enums, structs, tuples and unions are held in stack slots and passed
        // around by pointer. A plain `icmp` on the two operands would compare
        // the slot addresses — always unequal for distinct values — so `==`
        // and `!=` must walk the contents instead (#399). This matches the
        // interpreter's `value_eq`: tags then payloads for enums, every field
        // for structs/tuples, and content (not pointer) equality for strings.
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let agg_ty = Self::operand_mir_type(left, ctx.locals)
                .filter(|t| Self::is_structural_eq_type(t))
                .or_else(|| Self::operand_mir_type(right, ctx.locals)
                    .filter(|t| Self::is_structural_eq_type(t)));
            if let Some(ty) = agg_ty {
                let lhs_ptr = Self::lower_operand(builder, left, ctx)?;
                let rhs_ptr = Self::lower_operand(builder, right, ctx)?;
                let eq = Self::emit_aggregate_eq(builder, ctx, lhs_ptr, rhs_ptr, &ty)?;
                return Ok(if matches!(op, BinOp::Ne) {
                    builder.ins().bxor_imm(eq, 1)
                } else {
                    eq
                });
            }
        }

        // Same reasoning for `<`/`<=`/`>`/`>=`: comparing the two slot
        // addresses answered from allocation order, so `a < b` on a struct was
        // whichever local was declared first. Walk the contents instead —
        // fields in declaration order, enum variant first then payload.
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
            let agg_ty = Self::operand_mir_type(left, ctx.locals)
                .filter(|t| Self::is_structural_ord_type(t))
                .or_else(|| Self::operand_mir_type(right, ctx.locals)
                    .filter(|t| Self::is_structural_ord_type(t)));
            if let Some(ty) = agg_ty {
                let lhs_ptr = Self::lower_operand(builder, left, ctx)?;
                let rhs_ptr = Self::lower_operand(builder, right, ctx)?;
                let cmp = Self::emit_aggregate_cmp(builder, ctx, lhs_ptr, rhs_ptr, &ty)?;
                let cc = match op {
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok(builder.ins().icmp_imm(cc, cmp, 0));
            }
        }

        let operand_ty = if answers_bool { None } else { expected_ty };
        let lhs_val = Self::lower_operand_typed(builder, left, operand_ty, ctx)?;
        let lhs_ty = builder.func.dfg.value_type(lhs_val);
        let rhs_val = Self::lower_operand_typed(builder, right, Some(lhs_ty), ctx)?;
        let rhs_ty = builder.func.dfg.value_type(rhs_val);

        let is_float = lhs_ty.is_float() || rhs_ty.is_float();

        // A mixed-signedness comparison is answered by *value* (#308). Both
        // operands widen to i64, but they don't read the same way there: the
        // unsigned side is a bit pattern and the signed side a number, so one
        // `icmp` can only be right for one of them. Native compared as unsigned
        // and said `5 > -1` was false; the interpreter compared as signed and
        // said `u64::MAX > 1` was false. Each got the other's case wrong.
        //
        // A negative signed operand is below every unsigned one, and once it
        // isn't negative both sides are non-negative and unsigned order is
        // right — a sign check and a compare, which is what the design predicted.
        if is_comparison {
            let lt = Self::operand_mir_type(left, ctx.locals);
            let rt = Self::operand_mir_type(right, ctx.locals);
            if let (Some(lty), Some(rty)) = (&lt, &rt) {
                let mixed = lty.is_int_like() && rty.is_int_like()
                    && lty.is_unsigned() != rty.is_unsigned();
                if mixed {
                    let signed_on_left = !lty.is_unsigned();
                    // Widen each side the way its own type reads — zero-extend an
                    // unsigned operand, sign-extend a signed one. Branch on the
                    // value's *actual* Cranelift type: a local's declared MIR
                    // width and the width it's held at don't always agree, and
                    // extending an i64 is a verifier error.
                    let widen = |b: &mut ClifFunctionBuilder, v: Value, ty: &MirType| -> Value {
                        let have = b.func.dfg.value_type(v);
                        if have == types::I64 || !have.is_int() {
                            v
                        } else if ty.is_unsigned() {
                            b.ins().uextend(types::I64, v)
                        } else {
                            b.ins().sextend(types::I64, v)
                        }
                    };
                    let l64 = widen(builder, lhs_val, lty);
                    let r64 = widen(builder, rhs_val, rty);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let neg = if signed_on_left {
                        builder.ins().icmp(IntCC::SignedLessThan, l64, zero)
                    } else {
                        builder.ins().icmp(IntCC::SignedLessThan, r64, zero)
                    };
                    let ucc = match op {
                        BinOp::Eq => IntCC::Equal,
                        BinOp::Ne => IntCC::NotEqual,
                        BinOp::Lt => IntCC::UnsignedLessThan,
                        BinOp::Le => IntCC::UnsignedLessThanOrEqual,
                        BinOp::Gt => IntCC::UnsignedGreaterThan,
                        BinOp::Ge => IntCC::UnsignedGreaterThanOrEqual,
                        _ => unreachable!("checked by is_comparison"),
                    };
                    let ucmp = builder.ins().icmp(ucc, l64, r64);
                    // What the answer is when the signed side *is* negative.
                    let when_neg = match (op, signed_on_left) {
                        (BinOp::Eq, _) => false,
                        (BinOp::Ne, _) => true,
                        (BinOp::Lt, true) | (BinOp::Le, true) => true,
                        (BinOp::Gt, true) | (BinOp::Ge, true) => false,
                        (BinOp::Lt, false) | (BinOp::Le, false) => false,
                        (BinOp::Gt, false) | (BinOp::Ge, false) => true,
                        _ => unreachable!("checked by is_comparison"),
                    };
                    let fixed = builder.ins().iconst(types::I8, i64::from(when_neg));
                    return Ok(builder.ins().select(neg, fixed, ucmp));
                }
            }
        }

        // Signedness lives in the MIR type — a Cranelift integer carries none.
        // A constant operand has no type to read, so take it from whichever side
        // does; both operands of an arithmetic operator share a type anyway.
        // Reading only the left meant `200 / b` on a `u8` divided signed, and
        // 200 in an i8 is -56, so it answered 0 (#630).
        // When *both* operands are constants (e.g. `200 + 100`), neither has a
        // local to read a type from — fall back to the destination's MIR type,
        // which for arithmetic ops is the same type as the operands. That
        // fallback doesn't hold for comparisons (destination is always Bool),
        // so it's skipped there; a bare `200 < 100` has no width-dependent
        // signedness to get wrong anyway (#328).
        let is_unsigned = Self::operand_mir_type(left, ctx.locals)
            .or_else(|| Self::operand_mir_type(right, ctx.locals))
            .map(|t| t.is_unsigned())
            .unwrap_or_else(|| {
                if answers_bool { false } else { dst_mir_ty.is_some_and(|t| t.is_unsigned()) }
            });

        // Reconcile operand types
        let (lhs_val, rhs_val) = if lhs_ty == rhs_ty {
            (lhs_val, rhs_val)
        } else if lhs_ty.is_int() && rhs_ty.is_int() {
            // Widen narrower integer
            if lhs_ty.bits() < rhs_ty.bits() {
                (Self::convert_value(builder, lhs_val, lhs_ty, rhs_ty, None), rhs_val)
            } else {
                (lhs_val, Self::convert_value(builder, rhs_val, rhs_ty, lhs_ty, None))
            }
        } else if lhs_ty.is_float() && rhs_ty.is_float() {
            // Promote narrower float
            if lhs_ty.bits() < rhs_ty.bits() {
                (builder.ins().fpromote(rhs_ty, lhs_val), rhs_val)
            } else {
                (lhs_val, builder.ins().fpromote(lhs_ty, rhs_val))
            }
        } else if lhs_ty.is_int() && rhs_ty.is_float() {
            // Convert int to float to match rhs
            (builder.ins().fcvt_from_sint(rhs_ty, lhs_val), rhs_val)
        } else if lhs_ty.is_float() && rhs_ty.is_int() {
            // Convert int to float to match lhs
            (lhs_val, builder.ins().fcvt_from_sint(lhs_ty, rhs_val))
        } else {
            (lhs_val, rhs_val)
        };

        let result = if is_float {
            match op {
                BinOp::Add => builder.ins().fadd(lhs_val, rhs_val),
                BinOp::Sub => builder.ins().fsub(lhs_val, rhs_val),
                BinOp::Mul => builder.ins().fmul(lhs_val, rhs_val),
                BinOp::Div => builder.ins().fdiv(lhs_val, rhs_val),
                BinOp::Mod => {
                    // fmod: a - trunc(a/b) * b
                    let div = builder.ins().fdiv(lhs_val, rhs_val);
                    let trunc = builder.ins().trunc(div);
                    let prod = builder.ins().fmul(trunc, rhs_val);
                    builder.ins().fsub(lhs_val, prod)
                }
                BinOp::Eq => builder.ins().fcmp(FloatCC::Equal, lhs_val, rhs_val),
                BinOp::Ne => builder.ins().fcmp(FloatCC::NotEqual, lhs_val, rhs_val),
                BinOp::Lt => builder.ins().fcmp(FloatCC::LessThan, lhs_val, rhs_val),
                BinOp::Le => builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs_val, rhs_val),
                BinOp::Gt => builder.ins().fcmp(FloatCC::GreaterThan, lhs_val, rhs_val),
                BinOp::Ge => builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lhs_val, rhs_val),
                BinOp::And => builder.ins().band(lhs_val, rhs_val),
                BinOp::Or => builder.ins().bor(lhs_val, rhs_val),
                _ => return Err(CodegenError::UnsupportedFeature(format!("Bitwise op {:?} not valid on floats", op))),
            }
        } else {
            // Checked integer arithmetic (type.overflow OV1–OV4, SH1).
            // The *_overflow instructions give a result + overflow flag;
            // div/shift guards branch to a panic block. Type is the
            // reconciled operand type, so checks are width-correct.
            let int_ty = builder.func.dfg.value_type(lhs_val);
            // Cranelift lowers `iadd`/`isub` and their overflow forms at 128
            // bits, so checked `+` and `-` need nothing special. `imul`'s
            // overflow forms have no rule — the verifier rejects them outright —
            // and division and remainder have no rule at all, so those three go
            // through the runtime and come back with a status (#762).
            if int_ty == types::I128 && matches!(op, BinOp::Mul | BinOp::Div | BinOp::Mod) {
                let (name, kind, symbol) = match (op, is_unsigned) {
                    (BinOp::Mul, false) => ("rask_i128_mul", OvKind::Mul, "*"),
                    (BinOp::Mul, true) => ("rask_u128_mul", OvKind::Mul, "*"),
                    (BinOp::Div, false) => ("rask_i128_div", OvKind::DivMinByNegOne, "/"),
                    (BinOp::Div, true) => ("rask_u128_div", OvKind::DivMinByNegOne, "/"),
                    (BinOp::Mod, false) => ("rask_i128_rem", OvKind::DivMinByNegOne, "/"),
                    _ => ("rask_u128_rem", OvKind::DivMinByNegOne, "/"),
                };
                return Self::emit_i128_helper(
                    builder, ctx, name, lhs_val, rhs_val, kind, symbol, is_unsigned,
                );
            }
            match op {
                BinOp::Add => {
                    let (res, of) = if is_unsigned {
                        builder.ins().uadd_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().sadd_overflow(lhs_val, rhs_val)
                    };
                    Self::guard_overflow_binary(
                        builder, ctx, of, OvKind::Add, "+",
                        int_ty.bits(), is_unsigned, lhs_val, rhs_val,
                    );
                    res
                }
                BinOp::Sub => {
                    let (res, of) = if is_unsigned {
                        builder.ins().usub_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().ssub_overflow(lhs_val, rhs_val)
                    };
                    Self::guard_overflow_binary(
                        builder, ctx, of, OvKind::Sub, "-",
                        int_ty.bits(), is_unsigned, lhs_val, rhs_val,
                    );
                    res
                }
                BinOp::Mul => {
                    let (res, of) = if is_unsigned {
                        builder.ins().umul_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().smul_overflow(lhs_val, rhs_val)
                    };
                    Self::guard_overflow_binary(
                        builder, ctx, of, OvKind::Mul, "*",
                        int_ty.bits(), is_unsigned, lhs_val, rhs_val,
                    );
                    res
                }
                BinOp::Div if is_unsigned => {
                    if let Some(k) = Self::const_power_of_two(right) {
                        builder.ins().ushr_imm(lhs_val, k as i64)
                    } else {
                        Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                        builder.ins().udiv(lhs_val, rhs_val)
                    }
                }
                BinOp::Div => {
                    if let Some(k) = Self::const_power_of_two(right) {
                        // Signed div by 2^k: (value + ((value >> 63) >>> (64-k))) >> k
                        let bits = builder.func.dfg.value_type(lhs_val).bits() as i64;
                        let sign = builder.ins().sshr_imm(lhs_val, bits - 1);
                        let correction = builder.ins().ushr_imm(sign, bits - k as i64);
                        let adjusted = builder.ins().iadd(lhs_val, correction);
                        builder.ins().sshr_imm(adjusted, k as i64)
                    } else {
                        Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                        Self::guard_div_overflow(builder, ctx, lhs_val, rhs_val, int_ty);
                        builder.ins().sdiv(lhs_val, rhs_val)
                    }
                }
                BinOp::Mod if is_unsigned => {
                    if let Some(k) = Self::const_power_of_two(right) {
                        let ty = builder.func.dfg.value_type(lhs_val);
                        let mask = Self::iconst_at(builder, ty, (1i128 << k) - 1);
                        builder.ins().band(lhs_val, mask)
                    } else {
                        Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                        builder.ins().urem(lhs_val, rhs_val)
                    }
                }
                BinOp::Mod => {
                    Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                    Self::guard_div_overflow(builder, ctx, lhs_val, rhs_val, int_ty);
                    builder.ins().srem(lhs_val, rhs_val)
                }
                BinOp::BitAnd => builder.ins().band(lhs_val, rhs_val),
                BinOp::BitOr => builder.ins().bor(lhs_val, rhs_val),
                BinOp::BitXor => builder.ins().bxor(lhs_val, rhs_val),
                BinOp::Shl => {
                    Self::guard_shift(builder, ctx, rhs_val, int_ty, is_unsigned);
                    builder.ins().ishl(lhs_val, rhs_val)
                }
                BinOp::Shr if is_unsigned => {
                    Self::guard_shift(builder, ctx, rhs_val, int_ty, is_unsigned);
                    builder.ins().ushr(lhs_val, rhs_val)
                }
                BinOp::Shr => {
                    Self::guard_shift(builder, ctx, rhs_val, int_ty, is_unsigned);
                    builder.ins().sshr(lhs_val, rhs_val)
                }
                // Rotation wraps within the width, so it needs no shift guard:
                // any amount is well-defined.
                BinOp::RotateLeft => builder.ins().rotl(lhs_val, rhs_val),
                BinOp::RotateRight => builder.ins().rotr(lhs_val, rhs_val),
                // OV5 — the checked forms above with the guard taken off. The
                // machine already wraps; the panic was the extra part.
                BinOp::WrappingAdd => builder.ins().iadd(lhs_val, rhs_val),
                BinOp::WrappingSub => builder.ins().isub(lhs_val, rhs_val),
                BinOp::WrappingMul => builder.ins().imul(lhs_val, rhs_val),
                // SH2 — the amount is masked to the width instead of trapping,
                // so every amount means something. `x.wrapping_shl(9)` on a
                // `u8` shifts by 1.
                BinOp::WrappingShl => {
                    let amount = Self::mask_shift_amount(builder, rhs_val, int_ty);
                    builder.ins().ishl(lhs_val, amount)
                }
                BinOp::WrappingShr => {
                    let amount = Self::mask_shift_amount(builder, rhs_val, int_ty);
                    if is_unsigned {
                        builder.ins().ushr(lhs_val, amount)
                    } else {
                        builder.ins().sshr(lhs_val, amount)
                    }
                }
                BinOp::SaturatingAdd | BinOp::SaturatingSub | BinOp::SaturatingMul => {
                    Self::emit_saturating(builder, *op, lhs_val, rhs_val, int_ty, is_unsigned)
                }
                // The flag on its own, for the lowering that builds a `T?` or a
                // `(T, bool)` around it.
                BinOp::OverflowAdd => {
                    let (_, of) = if is_unsigned {
                        builder.ins().uadd_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().sadd_overflow(lhs_val, rhs_val)
                    };
                    of
                }
                BinOp::OverflowSub => {
                    let (_, of) = if is_unsigned {
                        builder.ins().usub_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().ssub_overflow(lhs_val, rhs_val)
                    };
                    of
                }
                BinOp::OverflowMul => {
                    let (_, of) = if is_unsigned {
                        builder.ins().umul_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().smul_overflow(lhs_val, rhs_val)
                    };
                    of
                }
                // Division fails two ways: a zero divisor, and the one signed
                // pair whose quotient isn't representable.
                BinOp::OverflowDiv => {
                    let zero = Self::iconst_at(builder, int_ty, 0);
                    let by_zero = builder.ins().icmp(IntCC::Equal, rhs_val, zero);
                    if is_unsigned {
                        by_zero
                    } else {
                        let min = Self::emit_type_min(builder, int_ty);
                        let neg_one = Self::iconst_at(builder, int_ty, -1);
                        let lhs_is_min = builder.ins().icmp(IntCC::Equal, lhs_val, min);
                        let rhs_is_neg_one = builder.ins().icmp(IntCC::Equal, rhs_val, neg_one);
                        let no_quotient = builder.ins().band(lhs_is_min, rhs_is_neg_one);
                        builder.ins().bor(by_zero, no_quotient)
                    }
                }
                BinOp::Eq => builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val),
                BinOp::Ne => builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val),
                BinOp::Lt if is_unsigned => builder.ins().icmp(IntCC::UnsignedLessThan, lhs_val, rhs_val),
                BinOp::Lt => builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val),
                BinOp::Le if is_unsigned => builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, lhs_val, rhs_val),
                BinOp::Le => builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val),
                BinOp::Gt if is_unsigned => builder.ins().icmp(IntCC::UnsignedGreaterThan, lhs_val, rhs_val),
                BinOp::Gt => builder.ins().icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val),
                BinOp::Ge if is_unsigned => builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, lhs_val, rhs_val),
                BinOp::Ge => builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val),
                BinOp::And => builder.ins().band(lhs_val, rhs_val),
                BinOp::Or => builder.ins().bor(lhs_val, rhs_val),
            }
        };
        Ok(result)
    }

    // ─── Structural equality for aggregates (#399) ──────────────────────
    // `==`/`!=` on an aggregate compares contents, not the slot address.
    // Every comparison returns an i8 (0/1). Scalars load at their storage
    // width and `icmp`/`fcmp`; strings and other heap values go through the
    // runtime content comparison; nested aggregates recurse.

    /// Aggregate types whose `==`/`!=` needs a structural (not pointer)
    /// comparison. Strings are already broken into `string_eq` calls during
    /// MIR lowering, so they never reach here as a top-level operand.
    fn is_structural_eq_type(ty: &MirType) -> bool {
        match ty {
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
            | MirType::Union(_) | MirType::Array { .. } => true,
            // A niche option — `Handle<T>?` or `Link<T>?` — is one word: the
            // value itself, with a reserved word for `none`. There's no slot to
            // walk, and comparing the two words directly is already right.
            // Walking one loaded a tag byte through the value, and for a `none`
            // link that value is the null address (#959).
            MirType::Option(inner) => !inner.is_niche_payload(),
            _ => false,
        }
    }

    /// Compare two aggregates (pointed to by `lhs`/`rhs`) for structural
    /// equality. Returns an i8 (1 = equal).
    fn emit_aggregate_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::Struct(id) => Self::emit_struct_eq(builder, ctx, lhs, rhs, id.id as usize),
            MirType::Enum(id) => Self::emit_enum_eq(builder, ctx, lhs, rhs, id.id as usize),
            MirType::Option(inner) => Self::emit_option_eq(builder, ctx, lhs, rhs, inner),
            MirType::Tuple(elems) => {
                // Tuple elements are packed at their natural offsets (see the
                // tuple-literal lowering); mirror that packing here.
                let elems = elems.clone();
                let mut acc = builder.ins().iconst(types::I8, 1);
                let mut offset = 0u32;
                for e in &elems {
                    let align = e.align().max(1);
                    offset = (offset + align - 1) & !(align - 1);
                    let l = builder.ins().iadd_imm(lhs, offset as i64);
                    let r = builder.ins().iadd_imm(rhs, offset as i64);
                    let eeq = Self::emit_field_eq_mir(builder, ctx, l, r, e)?;
                    acc = builder.ins().band(acc, eeq);
                    offset += e.size();
                }
                Ok(acc)
            }
            MirType::Array { elem, len } => {
                let stride = elem.size();
                let mut acc = builder.ins().iconst(types::I8, 1);
                for i in 0..*len {
                    let off = (i * stride) as i64;
                    let l = builder.ins().iadd_imm(lhs, off);
                    let r = builder.ins().iadd_imm(rhs, off);
                    let eeq = Self::emit_field_eq_mir(builder, ctx, l, r, elem)?;
                    acc = builder.ins().band(acc, eeq);
                }
                Ok(acc)
            }
            // A union's bytes start with its member index, so a byte-wise
            // comparison can't call two different members equal.
            MirType::Union(variants) => {
                let size = rask_mono::abi::UNION_PAYLOAD_OFFSET
                    + variants.iter().map(|v| v.size()).max().unwrap_or(0);
                Ok(Self::emit_bytes_eq(builder, lhs, rhs, size))
            }
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, ty.size())),
        }
    }

    /// Compare two `T?` slots.
    ///
    /// Two absent optionals are equal, an absent and a present one are not, and
    /// two present ones compare their payloads. That last step is the one that
    /// was missing: `Option` wasn't in the structural-equality set at all, so
    /// `a == b` on two `f32?` compared the two *stack slot addresses*, which are
    /// never equal — every optional comparison in a compiled program answered
    /// false, including `none == none` (#638).
    ///
    /// The absent case has to short-circuit before the payload. A `none` slot's
    /// payload is never written, so comparing those 8 bytes reads whatever the
    /// stack held, and two `none` values would agree or disagree depending on
    /// what ran before them.
    ///
    /// Only the low byte of the tag carries meaning — `EnumTag` loads an i8 —
    /// so the comparison reads the same width the constructors and the branch
    /// tests do. Tag 0 is present.
    fn emit_option_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        payload_ty: &MirType,
    ) -> CodegenResult<Value> {
        let tag_off = rask_mono::abi::OPTION_TAG_OFFSET as i32;
        let payload_off = rask_mono::abi::OPTION_PAYLOAD_OFFSET as i64;

        let tag_l = builder.ins().load(types::I8, MemFlags::new(), lhs, tag_off);
        let tag_r = builder.ins().load(types::I8, MemFlags::new(), rhs, tag_off);
        let present_l = builder.ins().icmp_imm(IntCC::Equal, tag_l, 0);
        let present_r = builder.ins().icmp_imm(IntCC::Equal, tag_r, 0);
        let same_shape = builder.ins().icmp(IntCC::Equal, present_l, present_r);

        let merge = builder.create_block();
        builder.append_block_param(merge, types::I8);
        let both_same = builder.create_block();
        let cmp_payload = builder.create_block();

        let false_v = builder.ins().iconst(types::I8, 0);
        builder.ins().brif(same_shape, both_same, &[], merge, &[false_v.into()]);

        // Same shape: both absent is equal outright, both present compares.
        builder.switch_to_block(both_same);
        builder.seal_block(both_same);
        let true_v = builder.ins().iconst(types::I8, 1);
        builder.ins().brif(present_l, cmp_payload, &[], merge, &[true_v.into()]);

        builder.switch_to_block(cmp_payload);
        builder.seal_block(cmp_payload);
        let l = builder.ins().iadd_imm(lhs, payload_off);
        let r = builder.ins().iadd_imm(rhs, payload_off);
        let payload_eq = Self::emit_wrapper_payload_eq(builder, ctx, l, r, payload_ty)?;
        builder.ins().jump(merge, &[payload_eq.into()]);

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// Compare the payload of a wrapper slot.
    ///
    /// Separate from `emit_field_eq_mir` because a wrapper's scalar payload
    /// fills the whole 8-byte slot — floats widened to f64, integers to their
    /// full width — which is the same rule the constructors write by and the
    /// peels read by. Loading an `f32?`'s payload as an f32 would take four
    /// bytes of an eight-byte write.
    fn emit_wrapper_payload_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::F32 | MirType::F64 => {
                let a = builder.ins().load(types::F64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::F64, MemFlags::new(), rhs, 0);
                Ok(builder.ins().fcmp(FloatCC::Equal, a, b))
            }
            MirType::Bool | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
            | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
            | MirType::Char | MirType::Handle | MirType::Ptr => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                Ok(builder.ins().icmp(IntCC::Equal, a, b))
            }
            MirType::String => Self::emit_string_eq(builder, ctx, lhs, rhs),
            // An aggregate payload is memcpy'd into the slot, so it is right
            // there to walk — including a nested `T??`, which recurses back
            // into the option comparison rather than byte-comparing a slot
            // whose absent half is uninitialised.
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
            | MirType::Union(_) | MirType::Array { .. } | MirType::Option(_) => {
                Self::emit_aggregate_eq(builder, ctx, lhs, rhs, ty)
            }
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, ty.size())),
        }
    }

    /// Compare every field of the struct at layout index `idx`.
    fn emit_struct_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        // Snapshot field descriptors so no borrow of `ctx` is held across the
        // recursive field comparisons (which also read `ctx`).
        let fields: Vec<(u32, RaskType, u32)> = {
            let layout = ctx.struct_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("struct layout missing for equality".into())
            })?;
            layout.fields.iter().map(|f| (f.offset, f.ty.clone(), f.size)).collect()
        };
        let mut acc = builder.ins().iconst(types::I8, 1);
        for (off, fty, sz) in fields {
            let l = builder.ins().iadd_imm(lhs, off as i64);
            let r = builder.ins().iadd_imm(rhs, off as i64);
            let feq = Self::emit_field_eq_rask(builder, ctx, l, r, &fty, sz)?;
            acc = builder.ins().band(acc, feq);
        }
        Ok(acc)
    }

    /// Compare two enums: tags first, then the payload of the shared variant.
    /// Different tags short-circuit to "not equal".
    fn emit_enum_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        // (tag value, payload offset, [(field offset, field type, field size)])
        let (tag_off, variants): (i32, Vec<(u64, u32, Vec<(u32, RaskType, u32)>)>) = {
            let layout = ctx.enum_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("enum layout missing for equality".into())
            })?;
            let vs = layout.variants.iter().map(|v| {
                let fields = v.fields.iter()
                    .map(|f| (f.offset, f.ty.clone(), f.size))
                    .collect::<Vec<_>>();
                (v.tag, v.payload_offset, fields)
            }).collect();
            (layout.tag_offset as i32, vs)
        };

        let tag_l = builder.ins().load(types::I64, MemFlags::new(), lhs, tag_off);
        let tag_r = builder.ins().load(types::I64, MemFlags::new(), rhs, tag_off);
        let tags_eq = builder.ins().icmp(IntCC::Equal, tag_l, tag_r);

        // Fieldless enum (plain tag union): equality is just tag equality.
        if variants.iter().all(|(_, _, f)| f.is_empty()) {
            return Ok(tags_eq);
        }

        // result = tags_eq && payload matches for the (now shared) variant.
        let merge = builder.create_block();
        builder.append_block_param(merge, types::I8);
        let cmp_payload = builder.create_block();
        let false_v = builder.ins().iconst(types::I8, 0);
        builder.ins().brif(tags_eq, cmp_payload, &[], merge, &[false_v]);

        builder.switch_to_block(cmp_payload);
        builder.seal_block(cmp_payload);
        // `equal_v` dominates the whole chain below (all reached only through
        // cmp_payload), so it is valid to reuse in the trailing jump.
        let equal_v = builder.ins().iconst(types::I8, 1);
        let mut chain_blocks = Vec::new();
        for (tag_val, poff, fields) in variants.iter().filter(|(_, _, f)| !f.is_empty()) {
            let var_block = builder.create_block();
            let next_block = builder.create_block();
            let tv = builder.ins().iconst(types::I64, *tag_val as i64);
            let is_this = builder.ins().icmp(IntCC::Equal, tag_l, tv);
            builder.ins().brif(is_this, var_block, &[], next_block, &[]);

            builder.switch_to_block(var_block);
            builder.seal_block(var_block);
            let mut acc = builder.ins().iconst(types::I8, 1);
            for (foff, fty, sz) in fields {
                let field_off = (*poff + *foff) as i64;
                let l = builder.ins().iadd_imm(lhs, field_off);
                let r = builder.ins().iadd_imm(rhs, field_off);
                let feq = Self::emit_field_eq_rask(builder, ctx, l, r, fty, *sz)?;
                acc = builder.ins().band(acc, feq);
            }
            builder.ins().jump(merge, &[acc]);

            builder.switch_to_block(next_block);
            chain_blocks.push(next_block);
        }
        // Tags matched but the variant carries no payload → equal.
        builder.ins().jump(merge, &[equal_v]);
        for b in chain_blocks {
            builder.seal_block(b);
        }

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// Compare a value of MIR type `ty` held at `lhs`/`rhs`.
    fn emit_field_eq_mir(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::F32 | MirType::F64 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                Ok(builder.ins().fcmp(FloatCC::Equal, a, b))
            }
            MirType::String => Self::emit_string_eq(builder, ctx, lhs, rhs),
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
            | MirType::Union(_) | MirType::Array { .. } => {
                Self::emit_aggregate_eq(builder, ctx, lhs, rhs, ty)
            }
            MirType::Bool | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
            | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
            | MirType::Char | MirType::Handle | MirType::Ptr => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                Ok(builder.ins().icmp(IntCC::Equal, a, b))
            }
            // Option/Result/Slice and friends as a nested element: compare the
            // raw slot bytes. Correct for POD payloads; heap payloads nested
            // this deep aren't content-compared yet.
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, ty.size())),
        }
    }

    /// Compare a struct/enum field of Rask type `ty` held at `lhs`/`rhs`.
    /// Struct and enum-payload fields sit in 8-byte slots, so scalars load as
    /// i64/f64 (see `lower_store`). `size` is the field's slot size, used for
    /// the byte-compare fallback.
    fn emit_field_eq_rask(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &RaskType,
        size: u32,
    ) -> CodegenResult<Value> {
        match ty {
            RaskType::F32 | RaskType::F64 => {
                // Stored promoted to f64 in the 8-byte slot.
                let a = builder.ins().load(types::F64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::F64, MemFlags::new(), rhs, 0);
                Ok(builder.ins().fcmp(FloatCC::Equal, a, b))
            }
            RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64
            | RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64
            | RaskType::Char => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                Ok(builder.ins().icmp(IntCC::Equal, a, b))
            }
            RaskType::String => Self::emit_string_eq(builder, ctx, lhs, rhs),
            // Nested struct or enum — look it up by name and recurse.
            RaskType::UnresolvedNamed(name) => {
                if let Some(sidx) = ctx.struct_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_struct_eq(builder, ctx, lhs, rhs, sidx)
                } else if let Some(eidx) = ctx.enum_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_enum_eq(builder, ctx, lhs, rhs, eidx)
                } else {
                    // Opaque named type (runtime pointer) — compare the slot.
                    Ok(Self::emit_bytes_eq(builder, lhs, rhs, size))
                }
            }
            // Tuples/arrays/options/opaque pointers: the field occupies exactly
            // `size` bytes and equal values produce identical bytes, so a byte
            // compare is a safe default. Heap contents nested here aren't
            // content-compared yet.
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, size)),
        }
    }

    /// Aggregate types with a defined ordering: structs lexicographically by
    /// declaration order (CO3), enums by variant order then payload (CO1),
    /// tuples and arrays elementwise. Unions have no active-variant tag, so
    /// there's nothing to order them by.
    fn is_structural_ord_type(ty: &MirType) -> bool {
        matches!(ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. })
    }

    /// A field with no ordering stops the whole comparison. Returning "equal"
    /// instead would let `<` on the containing struct quietly skip that field
    /// and answer from the next one — a wrong sort with nothing to notice.
    fn unorderable_field(what: &str) -> CodegenError {
        CodegenError::UnsupportedFeature(format!(
            "ordering comparison on a field of type {what} — there is no order \
             defined for it, so the struct can't be ordered either"
        ))
    }

    /// Three-way compare of two aggregates behind `lhs`/`rhs`. Returns i64:
    /// negative, zero, positive. Without this, `<` on a struct compared the two
    /// stack-slot addresses, so the answer depended on allocation order.
    fn emit_aggregate_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::Struct(id) => Self::emit_struct_cmp(builder, ctx, lhs, rhs, id.id as usize),
            MirType::Tuple(elems) => {
                let elems = elems.clone();
                let mut parts = Vec::new();
                let mut offset = 0u32;
                for e in &elems {
                    let align = e.align().max(1);
                    offset = (offset + align - 1) & !(align - 1);
                    parts.push((offset as i64, FieldKind::Mir(e.clone())));
                    offset += e.size();
                }
                Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)
            }
            MirType::Array { elem, len } => {
                let stride = elem.size();
                let parts = (0..*len)
                    .map(|i| ((i * stride) as i64, FieldKind::Mir((**elem).clone())))
                    .collect::<Vec<_>>();
                Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)
            }
            MirType::Enum(id) => Self::emit_enum_cmp(builder, ctx, lhs, rhs, id.id as usize),
            _ => Err(CodegenError::UnsupportedFeature(
                "ordering comparison on this aggregate type".into(),
            )),
        }
    }

    /// Compare every field of the struct at layout index `idx`, in declaration
    /// order, stopping at the first that differs (CO3).
    fn emit_struct_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        let parts: Vec<(i64, FieldKind)> = {
            let layout = ctx.struct_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("struct layout missing for ordering".into())
            })?;
            layout.fields.iter()
                .map(|f| (f.offset as i64, FieldKind::Rask(f.ty.clone(), f.size)))
                .collect()
        };
        Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)
    }

    /// Walk `parts` in order, stopping at the first that differs. Each entry is
    /// a byte offset from the aggregate base plus how to compare what's there.
    fn emit_lexicographic_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        parts: &[(i64, FieldKind)],
    ) -> CodegenResult<Value> {
        let merge = builder.create_block();
        builder.append_block_param(merge, types::I64);

        let mut pending = Vec::new();
        for (off, kind) in parts {
            let l = builder.ins().iadd_imm(lhs, *off);
            let r = builder.ins().iadd_imm(rhs, *off);
            let c = match kind {
                FieldKind::Rask(fty, sz) => {
                    Self::emit_field_cmp_rask(builder, ctx, l, r, fty, *sz)?
                }
                FieldKind::Mir(mty) => Self::emit_field_cmp_mir(builder, ctx, l, r, mty)?,
            };
            let next = builder.create_block();
            let is_eq = builder.ins().icmp_imm(IntCC::Equal, c, 0);
            builder.ins().brif(is_eq, next, &[], merge, &[c]);
            builder.switch_to_block(next);
            pending.push(next);
        }

        let equal = builder.ins().iconst(types::I64, 0);
        builder.ins().jump(merge, &[equal]);
        for b in pending {
            builder.seal_block(b);
        }
        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// Enums order by variant first, then by the shared variant's payload (CO1).
    fn emit_enum_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        let (tag_off, variants): (i32, Vec<(u64, u32, Vec<(u32, RaskType, u32)>)>) = {
            let layout = ctx.enum_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("enum layout missing for ordering".into())
            })?;
            let vs = layout.variants.iter().map(|v| {
                let fields = v.fields.iter()
                    .map(|f| (f.offset, f.ty.clone(), f.size))
                    .collect::<Vec<_>>();
                (v.tag, v.payload_offset, fields)
            }).collect();
            (layout.tag_offset as i32, vs)
        };

        let tag_l = builder.ins().load(types::I64, MemFlags::new(), lhs, tag_off);
        let tag_r = builder.ins().load(types::I64, MemFlags::new(), rhs, tag_off);
        let tag_cmp = Self::emit_signed_three_way(builder, tag_l, tag_r);

        if variants.iter().all(|(_, _, f)| f.is_empty()) {
            return Ok(tag_cmp);
        }

        let merge = builder.create_block();
        builder.append_block_param(merge, types::I64);
        let same_tag = builder.create_block();
        let tags_eq = builder.ins().icmp_imm(IntCC::Equal, tag_cmp, 0);
        builder.ins().brif(tags_eq, same_tag, &[], merge, &[tag_cmp]);

        builder.switch_to_block(same_tag);
        builder.seal_block(same_tag);
        let mut chain_blocks = Vec::new();
        for (tag_val, poff, fields) in variants.iter().filter(|(_, _, f)| !f.is_empty()) {
            let var_block = builder.create_block();
            let next_block = builder.create_block();
            let tv = builder.ins().iconst(types::I64, *tag_val as i64);
            let is_this = builder.ins().icmp(IntCC::Equal, tag_l, tv);
            builder.ins().brif(is_this, var_block, &[], next_block, &[]);

            builder.switch_to_block(var_block);
            builder.seal_block(var_block);
            let parts = fields
                .iter()
                .map(|(foff, fty, sz)| ((*poff + *foff) as i64, FieldKind::Rask(fty.clone(), *sz)))
                .collect::<Vec<_>>();
            let c = Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)?;
            builder.ins().jump(merge, &[c]);

            builder.switch_to_block(next_block);
            chain_blocks.push(next_block);
        }
        // Same tag, and that variant has no payload → equal.
        let equal = builder.ins().iconst(types::I64, 0);
        builder.ins().jump(merge, &[equal]);
        for b in chain_blocks {
            builder.seal_block(b);
        }

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// `(a > b) - (a < b)` as an i64.
    fn emit_signed_three_way(builder: &mut ClifFunctionBuilder, a: Value, b: Value) -> Value {
        let gt = builder.ins().icmp(IntCC::SignedGreaterThan, a, b);
        let lt = builder.ins().icmp(IntCC::SignedLessThan, a, b);
        let gt = builder.ins().uextend(types::I64, gt);
        let lt = builder.ins().uextend(types::I64, lt);
        builder.ins().isub(gt, lt)
    }

    /// Three-way compare of a struct/enum field of Rask type `ty`. Scalars sit
    /// in 8-byte slots, same as `emit_field_eq_rask`.
    fn emit_field_cmp_rask(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &RaskType,
        size: u32,
    ) -> CodegenResult<Value> {
        match ty {
            RaskType::F32 | RaskType::F64 => {
                let a = builder.ins().load(types::F64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::F64, MemFlags::new(), rhs, 0);
                let gt = builder.ins().fcmp(FloatCC::GreaterThan, a, b);
                let lt = builder.ins().fcmp(FloatCC::LessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64 => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                let gt = builder.ins().icmp(IntCC::UnsignedGreaterThan, a, b);
                let lt = builder.ins().icmp(IntCC::UnsignedLessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64
            | RaskType::Char => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                Ok(Self::emit_signed_three_way(builder, a, b))
            }
            // Every arm above reads its field as an i64, because a scalar
            // narrower than a word sits in one and one comparison shape then
            // covers all of them. The 128-bit pair is the exception and has to
            // be compared at its own width. Falling through to the catch-all
            // instead meant a struct with an `i128` field had no derivable
            // `compare`, so it couldn't be sorted or ordered at all (#933).
            RaskType::I128 | RaskType::U128 => {
                let a = builder.ins().load(types::I128, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I128, MemFlags::new(), rhs, 0);
                let (gt_cc, lt_cc) = if matches!(ty, RaskType::U128) {
                    (IntCC::UnsignedGreaterThan, IntCC::UnsignedLessThan)
                } else {
                    (IntCC::SignedGreaterThan, IntCC::SignedLessThan)
                };
                let gt = builder.ins().icmp(gt_cc, a, b);
                let lt = builder.ins().icmp(lt_cc, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            RaskType::String => Self::emit_string_cmp(builder, ctx, lhs, rhs),
            RaskType::UnresolvedNamed(name) => {
                if let Some(sidx) = ctx.struct_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_struct_cmp(builder, ctx, lhs, rhs, sidx)
                } else if let Some(eidx) = ctx.enum_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_enum_cmp(builder, ctx, lhs, rhs, eidx)
                } else {
                    let _ = size;
                    Err(Self::unorderable_field(&format!("`{}`", name)))
                }
            }
            other => Err(Self::unorderable_field(&format!("`{:?}`", other))),
        }
    }

    /// Three-way compare of a value of MIR type `ty` at `lhs`/`rhs`.
    fn emit_field_cmp_mir(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::F32 | MirType::F64 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let gt = builder.ins().fcmp(FloatCC::GreaterThan, a, b);
                let lt = builder.ins().fcmp(FloatCC::LessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            MirType::String => Self::emit_string_cmp(builder, ctx, lhs, rhs),
            t if Self::is_structural_ord_type(t) => {
                Self::emit_aggregate_cmp(builder, ctx, lhs, rhs, t)
            }
            MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let a = builder.ins().uextend(types::I64, a);
                let b = builder.ins().uextend(types::I64, b);
                let gt = builder.ins().icmp(IntCC::UnsignedGreaterThan, a, b);
                let lt = builder.ins().icmp(IntCC::UnsignedLessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            // The 128-bit pair can't take the extend-to-i64 route the others
            // do — they widen so one comparison shape covers every width, and
            // there is nothing wider to widen into. Compare at the value's own
            // width instead; only the two booleans need to reach i64. Without
            // this arm a struct holding an i128 had no derivable `compare` at
            // all, so sorting one failed to build (#933).
            MirType::I128 | MirType::U128 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let (gt_cc, lt_cc) = if matches!(ty, MirType::U128) {
                    (IntCC::UnsignedGreaterThan, IntCC::UnsignedLessThan)
                } else {
                    (IntCC::SignedGreaterThan, IntCC::SignedLessThan)
                };
                let gt = builder.ins().icmp(gt_cc, a, b);
                let lt = builder.ins().icmp(lt_cc, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            MirType::Bool | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
            | MirType::Char | MirType::Handle => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let a = builder.ins().sextend(types::I64, a);
                let b = builder.ins().sextend(types::I64, b);
                Ok(Self::emit_signed_three_way(builder, a, b))
            }
            other => Err(Self::unorderable_field(&format!("`{:?}`", other))),
        }
    }

    /// Lexicographic string compare via the runtime. Returns i64 (-1/0/1).
    fn emit_string_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
    ) -> CodegenResult<Value> {
        let fr = ctx.func_refs.get("string_compare")
            .ok_or_else(|| CodegenError::FunctionNotFound("string_compare".into()))?;
        let call = builder.ins().call(*fr, &[lhs, rhs]);
        Ok(builder.inst_results(call)[0])
    }

    /// Content equality of two strings via the runtime. Returns i8 (1 = equal).
    fn emit_string_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
    ) -> CodegenResult<Value> {
        let fr = ctx.func_refs.get("string_eq")
            .ok_or_else(|| CodegenError::FunctionNotFound("string_eq".into()))?;
        let call = builder.ins().call(*fr, &[lhs, rhs]);
        let res = builder.inst_results(call)[0];
        Ok(builder.ins().icmp_imm(IntCC::NotEqual, res, 0))
    }

    /// Byte-wise equality of `size` bytes at `lhs`/`rhs`. Returns i8 (1 = equal).
    fn emit_bytes_eq(
        builder: &mut ClifFunctionBuilder,
        lhs: Value,
        rhs: Value,
        size: u32,
    ) -> Value {
        let mut acc = builder.ins().iconst(types::I8, 1);
        let size = size as i32;
        let mut off = 0i32;
        for (chunk, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
            while size - off >= chunk {
                let a = builder.ins().load(ty, MemFlags::new(), lhs, off);
                let b = builder.ins().load(ty, MemFlags::new(), rhs, off);
                let e = builder.ins().icmp(IntCC::Equal, a, b);
                acc = builder.ins().band(acc, e);
                off += chunk;
            }
        }
        acc
    }

    fn field_address_and_load(
        builder: &mut ClifFunctionBuilder,
        base: &MirOperand,
        field_index: &u32,
        byte_offset: &Option<u32>,
        access: &FieldAccess,
        expected_ty: Option<Type>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        let base_val = Self::lower_operand(builder, base, ctx)?;
        let base_ty = Self::operand_mir_type(base, ctx.locals);
        let mut load_ty = expected_ty.unwrap_or(types::I64);
        let offset = match &base_ty {
            Some(MirType::Struct(id)) => {
                if let Some(layout) = ctx.struct_layouts.get(id.id as usize) {
                    if let Some(field) = layout.fields.get(*field_index as usize) {
                        // Aggregate field: return pointer into parent struct.
                        // Covers both >8-byte structs and ≤8-byte enums/structs
                        // that use stack-slot representation in codegen.
                        //
                        // "Wider than a word" is the usual sign of an aggregate,
                        // but a 128-bit integer is sixteen bytes and still a
                        // scalar Cranelift keeps in a register pair. MIR already
                        // worked that out and said `InRegister`; re-deciding it
                        // here from the size alone handed back the address and
                        // `ledger.balance` printed a stack address (#933).
                        if !matches!(access, FieldAccess::InRegister(_))
                            && (field.size > 8 || Self::is_aggregate_field_type(&field.ty, ctx))
                        {
                            let addr = builder.ins().iadd_imm(base_val, field.offset as i64);
                            return Ok(addr);
                        }
                        // Scalar field. Layout uses 8-byte slots; load at storage
                        // width to avoid reading wrong bytes (e.g. lower f64 half).
                        // A field declared with one of the type's parameters —
                        // `value: T` — carries whatever the layout substituted, and
                        // the shared layout for a generic type substitutes `i64` for
                        // every parameter. Right size, wrong register class: reading
                        // `Box<f64>`'s field through it loaded the double's bits into
                        // an integer register, and converting them on the way into
                        // `f64_to_string` printed `wrap(3.14).value` as
                        // 4614253070214988800 (#820). The MIR local at the read knows
                        // the real type, so keep what the caller asked for.
                        // Keeping the caller's type is right for an integer and
                        // wrong for a float: the slot holds a float as an f64
                        // whatever the parameter turned out to be, so honouring
                        // an F32 request loaded the double's zero low half and
                        // `G<f32> { value: 0.5 }.value` read back as 0 (#972).
                        // Read at the slot's width and let the narrowing tail
                        // below demote — the same pair `value_to_ptr` and
                        // `load_scalar_slot` agree on, and the Option and Result
                        // payload paths already use.
                        // What width the slot holds this in is the ABI's
                        // answer, whether the field's declared type says
                        // `float` or the type parameter it was substituted
                        // from does. Deciding it here is how a `G<f32>` field
                        // came to be read four bytes wide out of a slot holding
                        // a promoted double (#972).
                        let field_is_float = matches!(
                            &field.ty,
                            RaskType::F64 | RaskType::F32
                        ) || (field.is_type_param && load_ty.is_float());
                        load_ty = if field_is_float {
                            match rask_mono::abi::slot_scalar_bytes(true, 8, field.size) {
                                8 => types::F64,
                                _ => types::F32,
                            }
                        } else if field.is_type_param {
                            load_ty
                        } else {
                            // How wide the slot holds this is the ABI's answer
                            // for an integer too, not just a float — that is
                            // where the i128 case lives.
                            match rask_mono::abi::slot_scalar_bytes(
                                false, field.size, field.size,
                            ) {
                                16 => types::I128,
                                _ => types::I64,
                            }
                        };
                        field.offset as i32
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
            Some(MirType::Enum(id)) => {
                // A float payload sits in its slot as an f64, same as anywhere
                // else a float occupies a word (#629). This arm never said so,
                // so an f32 payload was read four bytes wide and came back as
                // the double's zero low half — `Has(1.5)` printed 0. The
                // narrowing tail below demotes it. f64 was right by coincidence:
                // the width the caller asked for happened to be the storage
                // width (#973).
                if load_ty.is_float() {
                    load_ty = match rask_mono::abi::slot_scalar_bytes(
                        true, load_ty.bytes(), rask_mono::abi::PAYLOAD_SLOT_BYTES,
                    ) {
                        8 => types::F64,
                        _ => types::F32,
                    };
                }
                // Prefer the exact payload offset match_lower computed for this
                // arm's variant. Guessing "first variant with enough fields"
                // picks the wrong payload shape when variants differ at the same
                // field index (e.g. Pos(Pt) vs Scalar(i32)). An aggregate payload
                // returns a pointer via the field_size > 8 check below (#347).
                if let Some(off) = byte_offset {
                    *off as i32
                } else if let Some(layout) = ctx.enum_layouts.get(id.id as usize) {
                    let variant = layout.variants.iter()
                        .find(|v| v.fields.len() > *field_index as usize);
                    match variant {
                        Some(v) => (v.payload_offset + v.fields[*field_index as usize].offset) as i32,
                        None => layout.variants.first()
                            .map(|v| v.payload_offset as i32)
                            .unwrap_or(0),
                    }
                } else {
                    0
                }
            }
            // Tuple: compute offset from element types, using actual
            // struct/enum layout sizes instead of MirType::size() fallbacks.
            Some(MirType::Tuple(fields)) => {
                let mut off = 0u32;
                for (i, f) in fields.iter().enumerate() {
                    let (elem_size, elem_align) = Self::real_type_size_align(f, ctx);
                    off = (off + elem_align - 1) & !(elem_align - 1);
                    if i == *field_index as usize {
                        // Aggregate element: return pointer, don't load scalar.
                        // `passed_by_address` is the type's own answer and
                        // already covers the struct/enum/tuple cases; the size
                        // test only has to catch what it can't see. An i128 is
                        // sixteen bytes and still a scalar, so it has to be
                        // exempt from that test — the same exception a struct
                        // field needs, and without it `(big, n).0` came back as
                        // the tuple's address (#933).
                        if f.passed_by_address()
                            || (elem_size > 8
                                && !matches!(f, MirType::I128 | MirType::U128))
                        {
                            let addr = builder.ins().iadd_imm(base_val, off as i64);
                            return Ok(addr);
                        }
                        break;
                    }
                    off += elem_size;
                }
                off as i32
            }
            // Option/Result: payload starts after tag.
            // MIR uses EnumTag for the tag; Field indices are payload-relative.
            Some(MirType::Option(inner)) => {
                // Aggregate payload: return address, not load. A nested Option
                // counts — the payload of a `T??` is a whole `T?` slot, and
                // loading its first 8 bytes as a scalar would hand `tag` a
                // number to dereference (#493).
                if Self::is_boxed_payload(inner.as_ref()) {
                    let payload_addr = builder.ins().iadd_imm(base_val, crate::layouts::PAYLOAD_OFFSET as i64);
                    return Ok(payload_addr);
                }
                // Read at the slot's storage width, not the payload's declared
                // one: an f32 payload is stored as an f64, so a 4-byte load here
                // would take the double's zero low half. The narrowing tail below
                // demotes it back, the same way a struct's f32 field is read.
                if let Some(storage) = Self::slot_storage_type(inner.as_ref()) {
                    load_ty = storage;
                }
                crate::layouts::PAYLOAD_OFFSET + (*field_index * 8) as i32
            }
            Some(MirType::Result { ok, err }) => {
                // Same storage rule as an Option payload: a scalar float sits in
                // the slot as an f64, so read it at that width and let the
                // narrowing tail demote. Only the ok side can be a float here —
                // an error type is a nominal enum or struct.
                let payload_storage = Self::slot_storage_type(ok.as_ref())
                    .filter(|t| t.is_float());
                if let Some(storage) = payload_storage {
                    load_ty = storage;
                }
                // Use explicit byte_offset when provided (e.g., origin field reads)
                if let Some(off) = byte_offset {
                    // A payload read still hands back the address when the
                    // payload is an aggregate. MIR passes a field_size for
                    // exactly that case and leaves it None for a scalar, which
                    // is what tells the two apart when ok and err disagree
                    // (#389). Without this, unwrapping a `T? or E` loaded the
                    // T?'s first 8 bytes and dereferenced them as a pointer.
                    if *off as i32 == crate::layouts::RESULT_PAYLOAD_OFFSET
                        && matches!(access, FieldAccess::InPlace(_))
                    {
                        let payload_addr = builder
                            .ins()
                            .iadd_imm(base_val, crate::layouts::RESULT_PAYLOAD_OFFSET as i64);
                        return Ok(payload_addr);
                    }
                    *off as i32
                } else {
                    // Aggregate payload (Ok or Err): return address, not load.
                    // MIR uses field_index 0 for both Ok and Err payloads — check both.
                    let is_aggregate = Self::is_boxed_payload;
                    if *field_index == 0 && (is_aggregate(ok.as_ref()) || is_aggregate(err.as_ref())) {
                        let payload_addr = builder.ins().iadd_imm(base_val, crate::layouts::RESULT_PAYLOAD_OFFSET as i64);
                        // If the caller expects a scalar (non-I64), this is a scalar
                        // payload extraction (e.g., Ok value from Result<I32, SomeEnum>).
                        // Load from the payload address instead of returning the address.
                        // Without this, convert_value would truncate the address to I32.
                        if let Some(exp) = expected_ty {
                            if exp != types::I64 {
                                // Load at the slot's storage width, then narrow to
                                // what the caller asked for — an f32 read straight
                                // at 4 bytes would take the stored double's zero
                                // low half.
                                let width = payload_storage.unwrap_or(exp);
                                let loaded =
                                    builder.ins().load(width, MemFlags::new(), payload_addr, 0);
                                return Ok(Self::convert_value(
                                    builder, loaded, width, exp, None,
                                ));
                            }
                        }
                        return Ok(payload_addr);
                    }
                    crate::layouts::RESULT_PAYLOAD_OFFSET + (*field_index * 8) as i32
                }
            }
            // Fallback: use pre-computed byte offset from MIR when available
            _ => byte_offset.map(|o| o as i32).unwrap_or((*field_index * 8) as i32)
        };

        // A field that lives in place hands back its address, not a load.
        if access.is_address() {
            let addr = builder.ins().iadd_imm(base_val, offset as i64);
            return Ok(addr);
        }

        let flags = MemFlags::new();
        let loaded = builder.ins().load(load_ty, flags, base_val, offset);

        // Narrow from storage type to declared type when needed.
        // E.g., f32 field stored as f64 in 8-byte slot → fdemote.
        let result = if let Some(exp) = expected_ty {
            let loaded_ty = builder.func.dfg.value_type(loaded);
            if loaded_ty != exp {
                Self::convert_value(builder, loaded, loaded_ty, exp, None)
            } else {
                loaded
            }
        } else {
            loaded
        };
        Ok(result)
    }

    /// Builtin/intrinsic call dispatch (print, panic, assert_*, Ptr_*, ...).
    /// Returns Ok(true) when the call was a recognized builtin, Ok(false)
    /// otherwise so the caller falls through to extern/ordinary emission.
    fn try_lower_builtin_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<bool> {
        // Builtin print/println/eprint/eprintln — dispatch per-arg to typed
        // runtime functions. The stderr pair differs only in symbol prefix.
        if matches!(func.name.as_str(), "print" | "println" | "eprint" | "eprintln") {
            let to_stderr = func.name.starts_with('e');
            let sep_fn = if to_stderr { "rask_eprint_string" } else { "rask_print_string" };
            // One call emits several writes — a separator per extra argument,
            // one per argument, and the newline. Bracket the lot so two threads
            // can't splice mid-line ("line 0 from thread 2line 194 from
            // thread 1"). The runtime's lock is recursive, so the individual
            // writes inside just re-take it.
            let lock_fn = if to_stderr { "rask_eprint_lock" } else { "rask_print_lock" };
            let unlock_fn = if to_stderr { "rask_eprint_unlock" } else { "rask_print_unlock" };
            if let Some(fr) = ctx.func_refs.get(lock_fn) {
                builder.ins().call(*fr, &[]);
            }
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    let sp = Self::lower_operand_typed(
                        builder, &MirOperand::Constant(MirConst::String(" ".to_string())),
                        Some(types::I64), ctx,
                    )?;
                    let print_str = ctx.func_refs.get(sep_fn)
                        .ok_or_else(|| CodegenError::FunctionNotFound(sep_fn.into()))?;
                    builder.ins().call(*print_str, &[sp]);
                }
                let base_fn = Self::runtime_print_for_operand(a, ctx.locals);
                let owned_fn;
                let runtime_fn: &str = if to_stderr {
                    owned_fn = base_fn.replacen("rask_print_", "rask_eprint_", 1);
                    &owned_fn
                } else {
                    base_fn
                };
                let fr = ctx.func_refs.get(runtime_fn)
                    .ok_or_else(|| CodegenError::FunctionNotFound(runtime_fn.into()))?;
                // Get the expected param type from the runtime function's signature
                let ext_func = &builder.func.dfg.ext_funcs[*fr];
                let sig = &builder.func.dfg.signatures[ext_func.signature];
                let expected_ty = sig.params.first().map(|p| p.value_type);
                let mut val = Self::lower_operand_typed(builder, a, expected_ty, ctx)?;
                if let Some(expected) = expected_ty {
                    let actual = builder.func.dfg.value_type(val);
                    if actual != expected {
                        val = Self::convert_value(builder, val, actual, expected, None);
                    }
                }
                builder.ins().call(*fr, &[val]);
            }
            if func.name == "println" || func.name == "eprintln" {
                let nl_fn = if to_stderr { "rask_eprint_newline" } else { "rask_print_newline" };
                let nl = ctx.func_refs.get(nl_fn)
                    .ok_or_else(|| CodegenError::FunctionNotFound(nl_fn.into()))?;
                builder.ins().call(*nl, &[]);
            }
            if let Some(fr) = ctx.func_refs.get(unlock_fn) {
                builder.ins().call(*fr, &[]);
            }
            // print/println return void — define dest as zero if needed
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                }
            }
        } else if func.name == "panic" || func.name == "todo" || func.name == "unreachable" {
            // User-level diverging builtin: emit rask_panic_at with the
            // message string and trap. todo/unreachable map to fixed
            // messages when called without args.
            let msg_ptr = if let Some(arg) = args.first() {
                Self::lower_operand_as_cstr(builder, arg, ctx)?
            } else {
                let label = match func.name.as_str() {
                    "todo" => "not yet implemented",
                    "unreachable" => "entered unreachable code",
                    _ => "panic",
                };
                ctx.string_globals.get(label)
                    .map(|gv| builder.ins().global_value(types::I64, *gv))
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0))
            };
            if let Some(panic_ref) = ctx.func_refs.get("panic_at") {
                let file_ptr = ctx.source_file.and_then(|f| ctx.string_globals.get(f))
                    .map(|gv| builder.ins().global_value(types::I64, *gv))
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                builder.ins().call(*panic_ref, &[file_ptr, line_val, col_val, msg_ptr]);
            }
            builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            // After a trap, codegen needs to enter an unreachable block
            // so subsequent Cranelift instructions are still well-formed.
            let unreach_block = builder.create_block();
            builder.switch_to_block(unreach_block);
            builder.seal_block(unreach_block);
            return Ok(true);
        } else if func.name == "assert_fail" {
            // MIR already handled branching; this is the fail path.
            // If a message arg is provided, pass it as raw C string pointer.
            if !args.is_empty() {
                let msg_val = Self::lower_operand_as_cstr(builder, &args[0], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("assert_fail_msg_at"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[msg_val, file_ptr, line_val, col_val]);
                    } else {
                        let assert_fn = ctx.func_refs.get("assert_fail")
                            .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                        builder.ins().call(*assert_fn, &[]);
                    }
                } else {
                    let assert_fn = ctx.func_refs.get("assert_fail")
                        .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                    builder.ins().call(*assert_fn, &[]);
                }
            } else if let Some(file_str) = ctx.source_file {
                if let (Some(func_ref), Some(gv)) = (
                    ctx.func_refs.get("assert_fail_at"),
                    ctx.string_globals.get(file_str),
                ) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                    let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                    builder.ins().call(*func_ref, &[file_ptr, line_val, col_val]);
                } else {
                    let assert_fn = ctx.func_refs.get("assert_fail")
                        .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                    builder.ins().call(*assert_fn, &[]);
                }
            } else {
                let assert_fn = ctx.func_refs.get("assert_fail")
                    .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                builder.ins().call(*assert_fn, &[]);
            }
        } else if func.name == "assert_fail_cmp_i64" || func.name == "assert_fail_cmp_char"
            || func.name == "assert_fail_cmp_i128" || func.name == "assert_fail_cmp_u128" {
            // Comparison assert failure with scalar values: args = [left, right, op_str].
            // Same shape for all of them; the char helper formats the codepoints
            // as characters, and the 128-bit pair takes its operands at their
            // own width so the reported numbers are the real ones.
            if args.len() >= 3 {
                let arg_ty = if func.name.ends_with("128") { types::I128 } else { types::I64 };
                let left_val = Self::lower_operand_typed(builder, &args[0], Some(arg_ty), ctx)?;
                let right_val = Self::lower_operand_typed(builder, &args[1], Some(arg_ty), ctx)?;
                let op_val = Self::lower_operand_as_cstr(builder, &args[2], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get(func.name.as_str()),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[left_val, right_val, op_val, file_ptr, line_val, col_val]);
                    }
                }
            }
        } else if func.name == "assert_fail_cmp_str" {
            // Comparison assert failure with string values: args = [left, right, op_str]
            if args.len() >= 3 {
                let left_val = Self::lower_operand_as_cstr(builder, &args[0], ctx)?;
                let right_val = Self::lower_operand_as_cstr(builder, &args[1], ctx)?;
                let op_val = Self::lower_operand_as_cstr(builder, &args[2], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("assert_fail_cmp_str"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[left_val, right_val, op_val, file_ptr, line_val, col_val]);
                    }
                }
            }
        } else if func.name == "assert_fail_cmp_f64" || func.name == "assert_fail_cmp_f32" {
            // Comparison assert failure with float values: args = [left, right, op_str].
            // The operands stay at their own width — an f32 formatted as a
            // double round-trips against the wrong width and prints its exact
            // binary expansion rather than the digits `println` shows.
            if args.len() >= 3 {
                let arg_ty = if func.name.ends_with("f32") { types::F32 } else { types::F64 };
                let left_val = Self::lower_operand_typed(builder, &args[0], Some(arg_ty), ctx)?;
                let right_val = Self::lower_operand_typed(builder, &args[1], Some(arg_ty), ctx)?;
                let op_val = Self::lower_operand_as_cstr(builder, &args[2], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get(func.name.as_str()),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[left_val, right_val, op_val, file_ptr, line_val, col_val]);
                    }
                }
            }
        } else if func.name == "check_fail" {
            // check failed — record failure, don't unwind
            let msg = if !args.is_empty() {
                Self::lower_operand_as_cstr(builder, &args[0], ctx)?
            } else {
                // Create a default "check failed" message
                if let Some(gv) = ctx.string_globals.get("check failed") {
                    builder.ins().global_value(types::I64, *gv)
                } else {
                    builder.ins().iconst(types::I64, 0)
                }
            };
            let func_ref = ctx.func_refs.get("rask_check_fail")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_check_fail".into()))?;
            builder.ins().call(*func_ref, &[msg]);
        } else if func.name == "rask_test_skip" {
            // skip("reason") — pass reason as C string, calls rask_test_skip
            let reason = if !args.is_empty() {
                Self::lower_operand_as_cstr(builder, &args[0], ctx)?
            } else {
                builder.ins().iconst(types::I64, 0)
            };
            let func_ref = ctx.func_refs.get("rask_test_skip")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_test_skip".into()))?;
            builder.ins().call(*func_ref, &[reason]);
        } else if func.name == "rask_test_expect_fail" {
            // expect_fail() — set thread-local flag
            let func_ref = ctx.func_refs.get("rask_test_expect_fail")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_test_expect_fail".into()))?;
            builder.ins().call(*func_ref, &[]);
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                }
            }
        } else if func.name.starts_with("assert_eq_fail") {
            // assert_eq failure: args = [got, expected] (empty for aggregates).
            // MIR already emitted the comparison and branched here.
            let value_args: Vec<Value> = match func.name.as_str() {
                "assert_eq_fail_str" => vec![
                    Self::lower_operand_as_cstr(builder, &args[0], ctx)?,
                    Self::lower_operand_as_cstr(builder, &args[1], ctx)?,
                ],
                "assert_eq_fail_f64" => vec![
                    Self::lower_operand_typed(builder, &args[0], Some(types::F64), ctx)?,
                    Self::lower_operand_typed(builder, &args[1], Some(types::F64), ctx)?,
                ],
                // f32 stays f32: see assert_fail_cmp_f32.
                "assert_eq_fail_f32" => vec![
                    Self::lower_operand_typed(builder, &args[0], Some(types::F32), ctx)?,
                    Self::lower_operand_typed(builder, &args[1], Some(types::F32), ctx)?,
                ],
                "assert_eq_fail" => Vec::new(),
                _ => vec![
                    Self::lower_operand_typed(builder, &args[0], Some(types::I64), ctx)?,
                    Self::lower_operand_typed(builder, &args[1], Some(types::I64), ctx)?,
                ],
            };
            if let Some(file_str) = ctx.source_file {
                if let (Some(func_ref), Some(gv)) = (
                    ctx.func_refs.get(func.name.as_str()),
                    ctx.string_globals.get(file_str),
                ) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                    let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                    let mut call_args = value_args;
                    call_args.extend_from_slice(&[file_ptr, line_val, col_val]);
                    builder.ins().call(*func_ref, &call_args);
                }
            }
        } else if func.name == "panic_unwrap" {
            // MIR already handled branching; this is the panic path. The single
            // argument says which mistake it was — an absent optional or a
            // thrown-away error — since only the `!`'s operand type knows.
            let was_error = match args.first() {
                Some(op) => Self::lower_operand_typed(builder, op, Some(types::I32), ctx)?,
                None => builder.ins().iconst(types::I32, 0),
            };
            if let Some(file_str) = ctx.source_file {
                if let (Some(func_ref), Some(gv)) = (
                    ctx.func_refs.get("panic_unwrap_at"),
                    ctx.string_globals.get(file_str),
                ) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                    let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                    builder.ins().call(*func_ref, &[file_ptr, line_val, col_val, was_error]);
                } else {
                    let unwrap_fn = ctx.func_refs.get("panic_unwrap")
                        .ok_or_else(|| CodegenError::FunctionNotFound("panic_unwrap".into()))?;
                    builder.ins().call(*unwrap_fn, &[was_error]);
                }
            } else {
                let unwrap_fn = ctx.func_refs.get("panic_unwrap")
                    .ok_or_else(|| CodegenError::FunctionNotFound("panic_unwrap".into()))?;
                builder.ins().call(*unwrap_fn, &[was_error]);
            }
        } else if func.name == "Ptr_add" || func.name == "Ptr_sub" || func.name == "Ptr_offset" {
            // Pointer arithmetic: ptr.add(n) → ptr + n*elem_size
            // Element size is passed as the third arg by MIR lowering.
            let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
            let n_val = Self::lower_operand_typed(builder, &args[1], Some(types::I64), ctx)?;
            let elem_size = if args.len() > 2 {
                Self::lower_operand_typed(builder, &args[2], Some(types::I64), ctx)?
            } else {
                builder.ins().iconst(types::I64, 8)
            };
            let byte_offset = builder.ins().imul(n_val, elem_size);
            let result = if func.name == "Ptr_sub" {
                builder.ins().isub(ptr_val, byte_offset)
            } else {
                builder.ins().iadd(ptr_val, byte_offset)
            };
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    builder.def_var(*var, result);
                }
            }
        } else if func.name == "Ptr_is_null" {
            // ptr.is_null() → ptr == 0 (returns I8 boolean)
            let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
            let result = builder.ins().icmp_imm(IntCC::Equal, ptr_val, 0);
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    builder.def_var(*var, result);
                }
            }
        } else if func.name == "Ptr_cast" {
            // ptr.cast<U>() → identity (pointer is always i64)
            let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    builder.def_var(*var, ptr_val);
                }
            }
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Extern "C" call — declared signature drives arg types; handles the
    /// string out-param ABI.
    fn lower_extern_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        // Extern "C" call — use declared signature directly, no stdlib adaptation
        // EXCEPT for string-out-param functions where the C ABI uses an out-param
        // that the Rask source doesn't expose.
        let func_ref = ctx.func_refs.get(&func.name)
            .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

        // Read declared signature to get expected param types
        let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
        let sig = &builder.func.dfg.signatures[ext_func.signature];
        let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

        // A string out-param function is declared with one more parameter than
        // it's called with, and the extra one goes in *front*. Matching arg `i`
        // against param `i` therefore reads every argument's type one slot too
        // early. That was invisible while every such signature was all-`i64`;
        // `i128_to_string(out: ptr, val: i128)` made it visible by truncating
        // the value to the out pointer's width (#762).
        let injects_out_param = param_types.len() == args.len() + 1
            && ctx.adapt_table.get(func.name.as_str())
                .map(|(a, _)| *a == ArgAdapt::StringOutParam)
                .unwrap_or(false);
        let param_offset = usize::from(injects_out_param);

        let mut arg_vals = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let expected = param_types.get(i + param_offset).copied();
            let val = Self::lower_operand_typed(builder, a, expected, ctx)?;
            let actual = builder.func.dfg.value_type(val);
            if let Some(exp) = expected {
                if actual != exp {
                    arg_vals.push(Self::convert_value(builder, val, actual, exp, None));
                } else {
                    arg_vals.push(val);
                }
            } else {
                arg_vals.push(val);
            }
        }

        // Inject string out-param for extern C functions that use the
        // out-param ABI (declared with N+1 params, called with N args)
        let out_param_slot = if injects_out_param {
            let ss = dst
                .and_then(|id| ctx.stack_slot_map.get(id))
                .map(|(ss, _)| *ss)
                .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 16, 0,
                )));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            arg_vals.insert(0, addr);
            Some(ss)
        } else {
            None
        };

        let call_inst = builder.ins().call(*func_ref, &arg_vals);

        if let Some(ss) = out_param_slot {
            // String out-param: result is in the stack slot, define dst var as pointer
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    let addr = builder.ins().stack_addr(types::I64, ss, 0);
                    builder.def_var(*var, addr);
                }
            }
        } else if let Some(dst_id) = dst {
            let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
            let is_void = matches!(dst_local.map(|l| &l.ty), Some(MirType::Void));
            if !is_void {
                let var = ctx.var_map.get(dst_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Call destination variable not found".to_string()
                    ))?;
                let results = builder.inst_results(call_inst);
                let val = if !results.is_empty() {
                    let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
                    let result = results[0];
                    if let Some(local) = dst_local {
                        let dst_ty = mir_to_cranelift_type(&local.ty)?;
                        let val_ty = builder.func.dfg.value_type(result);
                        if val_ty != dst_ty {
                            Self::convert_value(builder, result, val_ty, dst_ty, None)
                        } else {
                            result
                        }
                    } else {
                        result
                    }
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                if let Some((ss, _size)) = ctx.stack_slot_map.get(dst_id) {
                    let dst_is_option = ctx.locals.iter()
                        .find(|l| l.id == *dst_id)
                        .map(|l| matches!(l.ty, MirType::Option(_)))
                        .unwrap_or(false);
                    if dst_is_option {
                        Self::build_some(builder, *ss, val);
                    } else {
                        Self::build_ok(builder, *ss, val);
                    }
                } else {
                    builder.def_var(*var, val);
                }
            }
        }
        Ok(())
    }

    /// Ordinary Rask call — lowers args, applies stdlib arg/return adaptation.
    fn lower_ordinary_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let func_ref = ctx.func_refs.get(&func.name)
            .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

        // Lower MIR args to Cranelift values
        let mut arg_vals = Vec::with_capacity(args.len());
        for (arg_idx, a) in args.iter().enumerate() {
            // string_append_cstr: second arg is raw char*, skip RaskString wrapping
            // Everything reaches a runtime call as a word unless it's 128-bit,
            // which is the one scalar width that doesn't fit one. Forcing that
            // to `i64` here truncated the value before the signature loop below
            // ever saw it (#762).
            let want = if Self::operand_is_wide(a, ctx.locals) {
                types::I128
            } else {
                types::I64
            };
            let val = if func.name == "string_append_cstr" && arg_idx == 1 {
                Self::lower_string_const_as_cstr(builder, a, ctx)?
            } else {
                Self::lower_operand_typed(builder, a, Some(want), ctx)?
            };
            let actual = builder.func.dfg.value_type(val);
            let converted = if actual != want && actual.is_int() {
                Self::convert_value(builder, val, actual, want, None)
            } else {
                val
            };
            arg_vals.push(converted);
        }

        Self::spill_scalars_for_aggregate_params(builder, &func.name, &mut arg_vals, args, ctx);

        // Adapt args for typed runtime API
        let adapt = Self::adapt_stdlib_call(builder, &func.name, &mut arg_vals, args, dst, ctx, ctx.adapt_table);

        // Re-read signature after adaptation (arg count may have changed)
        let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
        let sig = &builder.func.dfg.signatures[ext_func.signature];
        let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

        // Convert arg types to match the declared signature
        for (i, val) in arg_vals.iter_mut().enumerate() {
            if let Some(&expected) = param_types.get(i) {
                let actual = builder.func.dfg.value_type(*val);
                if actual != expected {
                    *val = Self::convert_value(builder, *val, actual, expected, None);
                }
            }
        }

        // Store source location before calling panicking functions
        if ctx.panicking_fns.contains(&func.name) {
            if let Some(file_str) = ctx.source_file {
                if let (Some(set_loc_fn), Some(gv)) = (
                    ctx.func_refs.get("set_panic_location"),
                    ctx.string_globals.get(file_str),
                ) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                    let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                    builder.ins().call(*set_loc_fn, &[file_ptr, line_val, col_val]);
                }
            }
        }

        let call_inst = builder.ins().call(*func_ref, &arg_vals);

        if let Some(dst_id) = dst {
            // Skip void-typed destinations — nothing to store
            let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
            let is_void = matches!(dst_local.map(|l| &l.ty), Some(MirType::Void));

            if !is_void {
            let var = ctx.var_map.get(dst_id)
                .ok_or_else(|| CodegenError::UnsupportedFeature(
                    "Call destination variable not found".to_string()
                ))?;

            // Lock-acquire calls return a pointer to the box's inner value.
            // For a struct payload, bind the dst straight to that pointer — a
            // pointer-alias, exactly like a pool access — so the following
            // method/field access hits the real value and a `mutate` lands in
            // the box rather than a copied slot.
            //
            // Any other payload is stored indirectly: Mutex_new/Shared_new take
            // an address to memcpy from, so a non-struct value gets spilled to a
            // slot first and the box ends up holding the value itself. Binding
            // the pointer there handed `self.counters.lock()` a pointer to the
            // map pointer, and rask_map_get crashed on it (#477) — that payload
            // needs one load. The struct test mirrors the one Mutex_new uses, so
            // the two sides agree on which payloads are indirect.
            if matches!(func.name.as_str(),
                "Mutex_acquire" | "Shared_read_acquire" | "Shared_write_acquire"
                | "Cell_acquire")
            {
                let results = builder.inst_results(call_inst);
                let ptr = if !results.is_empty() {
                    results[0]
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                let payload_by_address =
                    dst_local.is_some_and(|l| l.ty.passed_by_address());
                let bound = if payload_by_address {
                    ptr
                } else {
                    let load_ty = dst_local
                        .and_then(|l| mir_to_cranelift_type(&l.ty).ok())
                        .unwrap_or(types::I64);
                    Self::load_scalar_slot(builder, ptr, load_ty)
                };
                builder.def_var(*var, bound);
                return Ok(());
            }

            // Post-call result handling
            let mut slot_already_written = false;
            let val = match adapt {
                CallAdapt::DerefResult => {
                    // Result is void* — load the value from it.
                    // Use the destination type so f64 elements load as f64,
                    // not as i64 bit patterns that need conversion.
                    let load_ty = dst_local
                        .and_then(|l| mir_to_cranelift_type(&l.ty).ok())
                        .unwrap_or(types::I64);
                    let results = builder.inst_results(call_inst);
                    if !results.is_empty() {
                        let ptr = results[0];
                        Self::load_scalar_slot(builder, ptr, load_ty)
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    }
                }
                CallAdapt::DerefOption => {
                    // Result is void*: NULL → None, non-NULL → Some(deref).
                    // Write tag+payload into the destination stack slot.
                    let results = builder.inst_results(call_inst);
                    let ptr = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    if let Some((ss, slot_size)) = ctx.stack_slot_map.get(dst_id) {
                        slot_already_written = true;
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_null = builder.ins().icmp(IntCC::Equal, ptr, zero);
                        let then_block = builder.create_block();
                        let else_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.ins().brif(is_null, then_block, &[], else_block, &[]);

                        // NULL path: none
                        builder.switch_to_block(then_block);
                        builder.seal_block(then_block);
                        Self::build_none(builder, *ss);
                        builder.ins().jump(merge_block, &[]);

                        // non-NULL path: Some(payload copied from ptr)
                        builder.switch_to_block(else_block);
                        builder.seal_block(else_block);
                        // A plain byte copy is right for every payload now that the
                        // runtime slot and the Option payload agree on width: both
                        // hold a float as f64. This used to need a load-and-demote
                        // special case for f32, because the two sides disagreed and
                        // the copy handed an f32 read the stored double's zero low
                        // half — `m.get(k)` on a `Map<K, f32>` answered 0 for every
                        // hit (#629).
                        let payload_size = *slot_size - crate::layouts::PAYLOAD_OFFSET as u32;
                        Self::build_wrapped_aggregate(builder, *ss, false, 0, ptr, payload_size);
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        // Return dummy value — real data is in the stack slot
                        builder.ins().iconst(types::I64, 0)
                    } else {
                        // No slot means a niche `Handle?` or `Link<T>?`: the
                        // value itself is the option. A miss still comes back
                        // NULL, so answer with that type's `none` instead of
                        // loading through it — `Map<K, Handle<T>>` segfaulted
                        // on every lookup that found nothing (#561).
                        let none_word = ctx.locals.iter()
                            .find(|l| l.id == *dst_id)
                            .and_then(|l| l.ty.niche_none())
                            .unwrap_or(crate::layouts::HANDLE_NONE_SENTINEL);
                        let miss_block = builder.create_block();
                        let hit_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let is_null = builder.ins().icmp_imm(IntCC::Equal, ptr, 0);
                        builder.ins().brif(is_null, miss_block, &[], hit_block, &[]);

                        builder.switch_to_block(miss_block);
                        builder.seal_block(miss_block);
                        let sentinel = builder.ins().iconst(types::I64, none_word);
                        builder.ins().jump(merge_block, &[sentinel]);

                        builder.switch_to_block(hit_block);
                        builder.seal_block(hit_block);
                        let loaded = builder.ins().load(types::I64, MemFlags::new(), ptr, 0);
                        builder.ins().jump(merge_block, &[loaded]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    }
                }
                CallAdapt::PopOutParam(ss) => {
                    // Value was written to stack slot by callee
                    builder.ins().stack_load(types::I64, ss, 0)
                }
                CallAdapt::OptionOutParam(ss) => {
                    // Payload is already in place; 1 means it's there (tag 0),
                    // 0 means there was nothing (tag 1).
                    let results = builder.inst_results(call_inst);
                    let wrote = if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    let some_tag = builder.ins().iconst(types::I64, 0);
                    let none_tag = builder.ins().iconst(types::I64, 1);
                    let tag = builder.ins().select(wrote, some_tag, none_tag);
                    builder.ins().stack_store(tag, ss, crate::layouts::TAG_OFFSET);
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        if *dst_ss == ss {
                            slot_already_written = true;
                        }
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                CallAdapt::StringOutParam(ss) => {
                    // 16-byte RaskStr written to stack slot — return slot address.
                    // If this slot is the dst's own slot, mark as already written.
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        if *dst_ss == ss {
                            slot_already_written = true;
                        }
                    }
                    builder.ins().stack_addr(types::I64, ss, 0)
                }
                CallAdapt::DerefStringElement => {
                    // void* pointing to aggregate data in collection.
                    // Copy `slot_size` bytes into the dst's own slot.
                    let results = builder.inst_results(call_inst);
                    let ptr = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    if let Some((ss, slot_size)) = ctx.stack_slot_map.get(dst_id) {
                        Self::copy_aggregate(builder, ptr, *ss, *slot_size);
                        slot_already_written = true;
                    }
                    ptr
                }
                CallAdapt::RecvStructOk(elem_size) => {
                    // The value is in the buffer the call returns; wrap it as Ok.
                    // Storing the pointer instead left the payload holding an
                    // address, so the received struct read as garbage (#463).
                    let results = builder.inst_results(call_inst);
                    let ptr = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    if let Some((ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        let is_result = ctx.locals.iter()
                            .find(|l| l.id == *dst_id)
                            .map(|l| matches!(l.ty, MirType::Result { .. }))
                            .unwrap_or(false);
                        Self::build_wrapped_aggregate(builder, *ss, is_result, 0, ptr, elem_size);
                        slot_already_written = true;
                    }
                    ptr
                }
                CallAdapt::TryRecvResult(payload_ss, elem_size) => {
                    // Channel status → `T or E` Result. status==OK(0) →
                    // Ok(payload); anything else (EMPTY/CLOSED) → Err.
                    let results = builder.inst_results(call_inst);
                    let status = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, crate::layouts::TAG_OFFSET as i64)
                    };
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id).copied() {
                        slot_already_written = true;
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_ok = builder.ins().icmp(IntCC::Equal, status, zero);
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                        // Ok(payload) copied into the Result slot.
                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let payload_addr = builder.ins().stack_addr(types::I64, payload_ss, 0);
                        Self::build_wrapped_aggregate(
                            builder, dst_ss, true, 0, payload_addr, elem_size,
                        );
                        builder.ins().jump(merge_block, &[]);

                        // Err: tag only (recv failure carries no payload).
                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let one = builder.ins().iconst(types::I64, 1);
                        builder.ins().stack_store(one, dst_ss, crate::layouts::TAG_OFFSET);
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                CallAdapt::ParseResult(value_ss, writer_ty, ok_ty) => {
                    // parse status → `T or ParseError`. 0 → Ok(value read from
                    // the out-param slot), 1 → Err. The old entry points
                    // returned the value itself with no way to fail, so garbage
                    // input came back as Ok(0) (#472).
                    let results = builder.inst_results(call_inst);
                    let status = if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, 1)
                    };
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id).copied() {
                        slot_already_written = true;
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_ok = builder.ins().icmp(IntCC::Equal, status, zero);
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let raw = builder.ins().stack_load(writer_ty, value_ss, 0);
                        let value = if writer_ty == ok_ty {
                            raw
                        } else {
                            Self::convert_value(builder, raw, writer_ty, ok_ty, None)
                        };
                        Self::build_ok(builder, dst_ss, value);
                        builder.ins().jump(merge_block, &[]);

                        // `ParseError` is fieldless, but it still has three
                        // variants — and the payload slot is where the program
                        // reads which one. Writing only the Result's Err tag left
                        // that slot holding whatever was on the stack: usually
                        // `Empty` whatever the real failure was, and on a stack
                        // that had been used, a tag no variant has, which reached
                        // the match's `unreachable` and killed the process with
                        // SIGILL. The runtime reports the variant as `1 + tag`,
                        // so the tag is the status minus one.
                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let one = builder.ins().iconst(types::I64, 1);
                        builder.ins().stack_store(one, dst_ss, crate::layouts::TAG_OFFSET);
                        let err_tag = builder.ins().iadd_imm(status, -1);
                        builder.ins().stack_store(
                            err_tag, dst_ss, crate::layouts::RESULT_PAYLOAD_OFFSET,
                        );
                        Self::zero_result_origin(builder, dst_ss);
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                CallAdapt::StringResult(value_ss, err_ss) => {
                    // status → `string or E`. 0 → Ok(the 16-byte RaskStr the
                    // callee wrote), 1 → Err. Before this the runtime returned
                    // the string alone and nothing wrote the tag, so the caller
                    // read whatever was in that slot — `io.read_line()` failed
                    // on its first call with a bogus error.
                    let results = builder.inst_results(call_inst);
                    let status = if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, 1)
                    };
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id).copied() {
                        slot_already_written = true;
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_ok = builder.ins().icmp(IntCC::Equal, status, zero);
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let tag = builder.ins().iconst(types::I64, 0);
                        builder.ins().stack_store(tag, dst_ss, crate::layouts::TAG_OFFSET);
                        Self::zero_result_origin(builder, dst_ss);
                        let src = builder.ins().stack_addr(types::I64, value_ss, 0);
                        let dst_addr = builder.ins().stack_addr(types::I64, dst_ss, 0);
                        Self::copy_bytes(
                            builder, src, 0, dst_addr,
                            crate::layouts::RESULT_PAYLOAD_OFFSET, 16,
                        );
                        builder.ins().jump(merge_block, &[]);

                        // The failure the runtime actually reported. It says
                        // which kind, and hands back the OS text for the one
                        // that carries a message — so a read on a write-only
                        // descriptor says "Bad file descriptor (os error 9)"
                        // like the interpreter does, instead of the fixed
                        // "unexpected end of file" every failure used to get.
                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let one = builder.ins().iconst(types::I64, 1);
                        builder.ins().stack_store(one, dst_ss, crate::layouts::TAG_OFFSET);
                        Self::zero_result_origin(builder, dst_ss);
                        Self::build_io_error_payload(
                            builder, dst_ss, status, err_ss, dst_id, ctx,
                        );
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                CallAdapt::JoinOutcome(value_ss, msg_ss) => {
                    // outcome → `T or JoinError`. The call already wrote the
                    // task's value and any panic message into the two slots;
                    // all that's left is deciding which variant to build.
                    //
                    // What this replaces: the old entry point folded value and
                    // outcome into one int64_t, so `-1` meant "panicked" and the
                    // Err payload was that same `-1` — matching on it read -1 as
                    // a JoinError address and segfaulted, and every successful
                    // join reported 0 because the value was never captured.
                    let results = builder.inst_results(call_inst);
                    let outcome = if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, RASK_JOIN_PANICKED)
                    };
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id).copied() {
                        slot_already_written = true;
                        Self::build_join_result(
                            builder, dst_ss, outcome, value_ss, msg_ss, dst_id, ctx,
                        );
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                _ => {
                    let results = builder.inst_results(call_inst);
                    if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    }
                }
            };

            let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
            let final_val = if let Some(local) = dst_local {
                let dst_ty = mir_to_cranelift_type(&local.ty)?;
                let val_ty = builder.func.dfg.value_type(val);
                if val_ty != dst_ty {
                    Self::convert_value(builder, val, val_ty, dst_ty, None)
                } else {
                    val
                }
            } else {
                val
            };
            // If destination has a stack slot (aggregate type), handle differently
            // for internal Rask functions vs C stdlib functions.
            // DerefOption already wrote directly to the stack slot.
            if slot_already_written {
                // Nothing to do — DerefOption already populated the slot
            } else if let Some((ss, size)) = ctx.stack_slot_map.get(dst_id) {
                if ctx.internal_fns.contains(&func.name) {
                    // Internal function returns aggregate data loaded from its stack.
                    // Store directly into our stack slot (value, not pointer).
                    match *size {
                        8 => {
                            builder.ins().stack_store(final_val, *ss, 0);
                        }
                        // The callee's Return terminator always loads a full word
                        // for anything <= 8 bytes, so a sub-word slot still gets 8
                        // bytes of value back. Storing all 8 into a slot smaller
                        // than that runs off the end into whatever sits next to
                        // it. Park the word somewhere that can hold it and copy
                        // out just the bytes that mean anything. Same fix as the
                        // closure call path (#611/#633).
                        n if n < 8 => {
                            let scratch = builder.create_sized_stack_slot(
                                StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 0),
                            );
                            builder.ins().stack_store(final_val, scratch, 0);
                            let scratch_ptr = builder.ins().stack_addr(types::I64, scratch, 0);
                            Self::copy_aggregate(builder, scratch_ptr, *ss, n);
                        }
                        n => {
                            // Larger aggregates: copy from returned pointer
                            Self::copy_aggregate(builder, final_val, *ss, n);
                        }
                    }
                } else if ctx.adapt_table.get(&func.name)
                    .map(|(_, r)| *r == RetAdapt::NegErr)
                    .unwrap_or(false)
                {
                    // C function uses negative return = error convention
                    // (declared as RetAdapt::NegErr on the dispatch entry).
                    Self::wrap_result_into_slot(builder, final_val, *ss);
                } else if ctx.adapt_table.get(&func.name)
                    .map(|(_, r)| *r == RetAdapt::NegNone)
                    .unwrap_or(false)
                {
                    // Negative return = `none` (find/rfind's -1).
                    Self::wrap_option_into_slot(builder, final_val, *ss);
                } else {
                    // C stdlib function returns a plain value (not a pointer to an aggregate).
                    // Wrap as Some/Ok depending on destination type.
                    let dst_is_option = ctx.locals.iter()
                        .find(|l| l.id == *dst_id)
                        .map(|l| matches!(l.ty, MirType::Option(_)))
                        .unwrap_or(false);
                    if dst_is_option {
                        Self::build_some(builder, *ss, final_val);
                    } else {
                        Self::build_ok(builder, *ss, final_val);
                    }
                }
            } else {
                builder.def_var(*var, final_val);
            }
            } // !is_void
        }
        Ok(())
    }

    fn lower_terminator(
        builder: &mut ClifFunctionBuilder,
        term: &MirTerminator,
        ctx: &CodegenCtx,
        cleanup_chain_blocks: &HashMap<Vec<BlockId>, cranelift_codegen::ir::Block>,
    ) -> CodegenResult<()> {
        match &term.kind {
            MirTerminatorKind::Return { value } => {
                // Leaving an exported symbol the normal way — one level of the
                // FFI boundary comes back off. A panic never gets here; it
                // aborts at the boundary instead (ctrl.panic/A1).
                if ctx.is_extern_c {
                    if let Some(fr) = ctx.func_refs.get("rask_ffi_boundary_exit") {
                        builder.ins().call(*fr, &[]);
                    }
                }
                // main is called from C as void rask_main(void) — always return
                // void. A `void or E` main still has to report its error branch,
                // though: exit 1, not the silent 0 it used to give (#345).
                if ctx.is_main {
                    Self::emit_main_error_check(builder, value.as_ref(), ctx)?;
                    builder.ins().return_(&[]);
                } else if let Some(val) = Self::exit_value(builder, value.as_ref(), ctx)? {
                    builder.ins().return_(&[val]);
                } else {
                    builder.ins().return_(&[]);
                }
            }

            MirTerminatorKind::Goto { target } => {
                let target_block = ctx.block_map.get(target)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Target block not found".to_string()))?;
                builder.ins().jump(*target_block, &[]);
            }

            MirTerminatorKind::Branch { cond, then_block, else_block } => {
                let mut cond_val = Self::lower_operand(builder, cond, ctx)?;

                let cond_ty = builder.func.dfg.value_type(cond_val);
                if cond_ty == types::I8 {
                    cond_val = builder.ins().icmp_imm(IntCC::NotEqual, cond_val, 0);
                }

                let then_cl = ctx.block_map.get(then_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Then block not found".to_string()))?;
                let else_cl = ctx.block_map.get(else_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Else block not found".to_string()))?;
                builder.ins().brif(cond_val, *then_cl, &[], *else_cl, &[]);
            }

            MirTerminatorKind::Switch { value, cases, default } => {
                let raw_scrutinee = Self::lower_operand(builder, value, ctx)?;
                // Extend to i64 if the scrutinee is a narrower type (e.g. u8 enum tag)
                let scrutinee_val = {
                    let val_ty = builder.func.dfg.value_type(raw_scrutinee);
                    if val_ty != types::I64 && val_ty.is_int() {
                        builder.ins().uextend(types::I64, raw_scrutinee)
                    } else {
                        raw_scrutinee
                    }
                };

                // Create comparison chain: each case gets a brif, falling through to next
                // Don't seal MIR blocks here — the final seal-all loop handles them
                let mut comparison_blocks = Vec::new();

                for (value, target_id) in cases {
                    let target_block = ctx.block_map.get(target_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Switch target block not found".to_string()))?;

                    let cmp_val = builder.ins().iconst(types::I64, *value as i64);
                    let cond = builder.ins().icmp(IntCC::Equal, scrutinee_val, cmp_val);

                    let next_block = builder.create_block();
                    comparison_blocks.push(next_block);

                    builder.ins().brif(cond, *target_block, &[], next_block, &[]);
                    builder.switch_to_block(next_block);
                }

                let default_block = ctx.block_map.get(default)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Switch default block not found".to_string()))?;
                builder.ins().jump(*default_block, &[]);

                // Seal comparison chain blocks (these aren't MIR blocks)
                for block in comparison_blocks {
                    builder.seal_block(block);
                }
            }

            MirTerminatorKind::Unreachable => {
                builder.ins().trap(TrapCode::user(1).unwrap());
            }

            MirTerminatorKind::CleanupReturn { value, cleanup_chain } => {
                if !cleanup_chain.is_empty() {
                    if let Some(&shared_block) = cleanup_chain_blocks.get(cleanup_chain) {
                        // The shared cleanup block takes the return value as a
                        // block parameter, so this jump supplies exactly what a
                        // plain `return` would have returned — wrapping and all.
                        // main is void, so it passes nothing.
                        //
                        // `return ()` out of a `void or E` function arrives here
                        // as a *void-typed local*, not as `None`, so it takes the
                        // no-value path deliberately: lowered as a value it
                        // becomes a plain zero, and the caller reads a Result tag
                        // out of address 0.
                        let value = value.as_ref().filter(|op| !Self::is_void_operand(op, ctx));
                        if ctx.is_main {
                            builder.ins().jump(shared_block, &[]);
                        } else if let Some(val) = Self::exit_value(builder, value, ctx)? {
                            builder.ins().jump(shared_block, &[val]);
                        } else if matches!(ctx.ret_ty, MirType::Void) {
                            builder.ins().jump(shared_block, &[]);
                        } else {
                            // A bare `return` in a value-returning function, e.g.
                            // the success exit of a `void or E`. Several
                            // cleanup_returns share one block and the ones
                            // carrying an error do pass a value, so jumping with
                            // no argument left the block signature unsatisfied and
                            // Cranelift's verifier rejected the function (#463).
                            let placeholder = Self::empty_return_value(builder, ctx)?;
                            builder.ins().jump(shared_block, &[placeholder]);
                        }
                    } else {
                        // Fallback: inline (shouldn't happen with the setup above)
                        Self::emit_plain_return(builder, value.as_ref(), ctx)?;
                    }
                } else {
                    // Empty cleanup chain — just return directly
                    Self::emit_plain_return(builder, value.as_ref(), ctx)?;
                }
            }
        }
        Ok(())
    }

    /// Branch to a cold panic block when `overflowed` is nonzero, then continue
    /// in a fresh block. Used for the checked-arithmetic guards (type.overflow).
    fn guard_overflow(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        overflowed: Value,
        msg: &str,
    ) {
        let panic_block = builder.create_block();
        let cont_block = builder.create_block();
        builder.ins().brif(overflowed, panic_block, &[], cont_block, &[]);
        Self::emit_panic_block(builder, panic_block, msg, ctx);
        builder.switch_to_block(cont_block);
        builder.seal_block(cont_block);
    }

    /// Widen a checked-arithmetic operand to the i64 the runtime formatter takes.
    /// Sign or zero extension by the operand's own signedness, so the printed
    /// number is the value the program saw.
    fn operand_as_i64(builder: &mut ClifFunctionBuilder, val: Value, unsigned: bool) -> Value {
        let ty = builder.func.dfg.value_type(val);
        if ty == types::I64 || !ty.is_int() || ty.bits() > 64 {
            return val;
        }
        if unsigned {
            builder.ins().uextend(types::I64, val)
        } else {
            builder.ins().sextend(types::I64, val)
        }
    }

    /// F3: panic naming both operands. Falls back to the static sentence when
    /// the width has no tail registered (a width the language doesn't have, and
    /// i128, whose operands don't fit the formatter's words).
    fn guard_overflow_binary(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        overflowed: Value,
        kind: OvKind,
        op_symbol: &str,
        bits: u32,
        unsigned: bool,
        lhs: Value,
        rhs: Value,
    ) {
        // Which formatter takes these operands. The operands' own Cranelift type
        // decides, not `bits` — `bits` comes from the MIR type, and it's the call
        // that has to type-check.
        let lty = builder.func.dfg.value_type(lhs);
        let rty = builder.func.dfg.value_type(rhs);
        let wide = lty == types::I128 && rty == types::I128;
        let usable = wide || ([lty, rty].iter().all(|t| t.is_int() && t.bits() <= 64));
        let tail = if usable { overflow_range_tail(bits, unsigned) } else { None };
        let Some(tail) = tail else {
            return Self::guard_overflow(builder, ctx, overflowed, overflow_message(kind, bits, unsigned));
        };
        let (helper, lhs_arg, rhs_arg) = if wide {
            ("panic_overflow_binary_i128", lhs, rhs)
        } else {
            (
                "panic_overflow_binary",
                Self::operand_as_i64(builder, lhs, unsigned),
                Self::operand_as_i64(builder, rhs, unsigned),
            )
        };

        let panic_block = builder.create_block();
        let cont_block = builder.create_block();
        builder.ins().brif(overflowed, panic_block, &[], cont_block, &[]);

        builder.switch_to_block(panic_block);
        builder.seal_block(panic_block);
        builder.set_cold_block(panic_block);
        let emitted = (|| {
            let panic_ref = ctx.func_refs.get(helper)?;
            let tail_gv = ctx.string_globals.get(tail)?;
            let op_gv = ctx.string_globals.get(op_symbol)?;
            let (file_ptr, line_val, col_val) = Self::panic_site(builder, ctx);
            let tail_ptr = builder.ins().global_value(types::I64, *tail_gv);
            let op_ptr = builder.ins().global_value(types::I64, *op_gv);
            let uns = builder.ins().iconst(types::I32, unsigned as i64);
            builder.ins().call(
                *panic_ref,
                &[file_ptr, line_val, col_val, op_ptr, tail_ptr, lhs_arg, rhs_arg, uns],
            );
            Some(())
        })();
        if emitted.is_none() {
            Self::emit_panic_call(builder, overflow_message(kind, bits, unsigned), ctx);
        }
        builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

        builder.switch_to_block(cont_block);
        builder.seal_block(cont_block);
    }

    /// F3 for the one-operand forms: negation, and a shift amount past the width.
    fn guard_overflow_unary(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        overflowed: Value,
        kind: OvKind,
        bits: u32,
        unsigned: bool,
        operand: Value,
    ) {
        let operand_ty = builder.func.dfg.value_type(operand);
        let wide = operand_ty == types::I128;
        // A 128-bit shift amount has no formatter — the count is an i128 there,
        // and only the negation form takes one at that width.
        let usable = (wide && kind != OvKind::Shift)
            || (operand_ty.is_int() && operand_ty.bits() <= 64);
        let (helper, tail) = match (kind, wide) {
            (OvKind::Shift, _) => ("panic_shift_amount", shift_width_tail(bits, unsigned)),
            (_, true) => ("panic_overflow_neg_i128", overflow_range_tail(bits, unsigned)),
            (_, false) => ("panic_overflow_neg", overflow_range_tail(bits, unsigned)),
        };
        let tail = if usable { tail } else { None };
        let Some(tail) = tail else {
            return Self::guard_overflow(builder, ctx, overflowed, overflow_message(kind, bits, unsigned));
        };
        // A shift amount is a count, never negative in meaning; a negated value
        // is signed. Either way the printed number is the operand's own reading.
        let operand_arg = if wide {
            operand
        } else {
            Self::operand_as_i64(builder, operand, unsigned && kind == OvKind::Shift)
        };

        let panic_block = builder.create_block();
        let cont_block = builder.create_block();
        builder.ins().brif(overflowed, panic_block, &[], cont_block, &[]);

        builder.switch_to_block(panic_block);
        builder.seal_block(panic_block);
        builder.set_cold_block(panic_block);
        let emitted = (|| {
            let panic_ref = ctx.func_refs.get(helper)?;
            let tail_gv = ctx.string_globals.get(tail)?;
            let (file_ptr, line_val, col_val) = Self::panic_site(builder, ctx);
            let tail_ptr = builder.ins().global_value(types::I64, *tail_gv);
            builder.ins().call(*panic_ref, &[file_ptr, line_val, col_val, tail_ptr, operand_arg]);
            Some(())
        })();
        if emitted.is_none() {
            Self::emit_panic_call(builder, overflow_message(kind, bits, unsigned), ctx);
        }
        builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

        builder.switch_to_block(cont_block);
        builder.seal_block(cont_block);
    }

    /// The file/line/col triple every panic call site passes.
    fn panic_site(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
    ) -> (Value, Value, Value) {
        let file_ptr = match ctx.source_file.and_then(|f| ctx.string_globals.get(f)) {
            Some(gv) => builder.ins().global_value(types::I64, *gv),
            None => builder.ins().iconst(types::I64, 0),
        };
        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
        (file_ptr, line_val, col_val)
    }

    /// `rask_panic_at(file, line, col, msg)` with a static message, in the block
    /// the caller has already switched to.
    fn emit_panic_call(builder: &mut ClifFunctionBuilder, msg: &str, ctx: &CodegenCtx) {
        if let (Some(panic_ref), Some(msg_gv)) =
            (ctx.func_refs.get("panic_at"), ctx.string_globals.get(msg))
        {
            let (file_ptr, line_val, col_val) = Self::panic_site(builder, ctx);
            let msg_ptr = builder.ins().global_value(types::I64, *msg_gv);
            builder.ins().call(*panic_ref, &[file_ptr, line_val, col_val, msg_ptr]);
        }
    }

    /// OV2: panic (with a message) when the divisor is zero.
    fn guard_div_zero(builder: &mut ClifFunctionBuilder, ctx: &CodegenCtx, rhs: Value, ty: Type) {
        let zero = Self::iconst_at(builder, ty, 0);
        let is_zero = builder.ins().icmp(IntCC::Equal, rhs, zero);
        Self::guard_overflow(builder, ctx, is_zero, OV_DIV_ZERO);
    }

    /// OV3: panic when a signed division would overflow (`MIN / -1`).
    fn guard_div_overflow(builder: &mut ClifFunctionBuilder, ctx: &CodegenCtx, lhs: Value, rhs: Value, ty: Type) {
        let min = Self::emit_type_min(builder, ty);
        let neg1 = Self::iconst_at(builder, ty, -1);
        let l_is_min = builder.ins().icmp(IntCC::Equal, lhs, min);
        let r_is_neg1 = builder.ins().icmp(IntCC::Equal, rhs, neg1);
        let both = builder.ins().band(l_is_min, r_is_neg1);
        Self::guard_overflow_binary(
            builder, ctx, both, OvKind::DivMinByNegOne, "/",
            ty.bits(), false, lhs, rhs,
        );
    }

    /// Call a 128-bit runtime helper and turn its status into the usual panic.
    ///
    /// The helper writes its result through an out pointer and returns 0, 1
    /// (divide by zero) or 2 (overflow) rather than trapping, so the panic
    /// happens here where the span is — the same messages the narrower widths
    /// use (type.overflow OV1–OV4).
    fn emit_i128_helper(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        name: &str,
        lhs: Value,
        rhs: Value,
        kind: OvKind,
        symbol: &str,
        unsigned: bool,
    ) -> CodegenResult<Value> {
        let func_ref = *ctx.func_refs.get(name)
            .ok_or_else(|| CodegenError::FunctionNotFound(name.to_string()))?;
        let slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 16, 4,
        ));
        let out_ptr = builder.ins().stack_addr(types::I64, slot, 0);
        let call = builder.ins().call(func_ref, &[lhs, rhs, out_ptr]);
        let status = builder.inst_results(call)[0];

        let one = builder.ins().iconst(types::I32, 1);
        let div_zero = builder.ins().icmp(IntCC::Equal, status, one);
        Self::guard_overflow(builder, ctx, div_zero, OV_DIV_ZERO);
        let two = builder.ins().iconst(types::I32, 2);
        let overflowed = builder.ins().icmp(IntCC::Equal, status, two);
        Self::guard_overflow_binary(
            builder, ctx, overflowed, kind, symbol, 128, unsigned, lhs, rhs,
        );

        Ok(builder.ins().stack_load(types::I128, slot, 0))
    }

    /// SH1: panic when the shift amount is >= the operand's bit width.
    /// Unsigned comparison also catches negative amounts.
    fn guard_shift(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        amount: Value,
        ty: Type,
        unsigned: bool,
    ) {
        // `iconst` stops at 64 bits, so a 128-bit width is built from halves.
        // A 128-bit amount also doesn't fit the runtime formatter's word, so
        // that width keeps the static message.
        if ty == types::I128 {
            let bits = Self::iconst_i128(builder, ty.bits() as i128);
            let bad = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, amount, bits);
            Self::guard_overflow(builder, ctx, bad, overflow_message(OvKind::Shift, ty.bits(), unsigned));
            return;
        }
        let bits = builder.ins().iconst(ty, ty.bits() as i64);
        let bad = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, amount, bits);
        Self::guard_overflow_unary(builder, ctx, bad, OvKind::Shift, ty.bits(), unsigned, amount);
    }

    /// Emit a cold panic block: call rask_panic_at with the given message, then trap.
    /// The block is sealed immediately (single predecessor expected).
    fn emit_panic_block(
        builder: &mut ClifFunctionBuilder,
        block: cranelift_codegen::ir::Block,
        msg: &str,
        ctx: &CodegenCtx,
    ) {
        builder.switch_to_block(block);
        builder.seal_block(block);
        builder.set_cold_block(block);
        Self::emit_panic_call(builder, msg, ctx);
        builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
    }

    /// Emit a return instruction.
    /// The value a bare `return` yields in a function that returns something.
    ///
    /// For `T or E` that's Ok with a zero payload — the success exit of a
    /// `void or E`. For `T?` it's `none`: a bare `return` carries no value, so
    /// building a `Some` around a zero would invent a payload the source never
    /// wrote. Either way it's handed back as the address of the result slot.
    /// Anything else gets a zero of the return type.
    ///
    /// The Option side is unreachable today — the checker rejects `return` with
    /// no value in a `-> T?` function (E0308) — but the inline pass spells out
    /// the same two cases, and the two have to agree.
    /// True for an operand that carries no value — a local the lowering typed
    /// `void`. `return ()` and a bare `return` produce the same thing at the
    /// source level but different MIR, and a return path has to treat them alike.
    fn is_void_operand(op: &MirOperand, ctx: &CodegenCtx) -> bool {
        match op {
            MirOperand::Local(id) => ctx.locals.iter()
                .find(|l| l.id == *id)
                .is_some_and(|l| matches!(l.ty, MirType::Void)),
            _ => false,
        }
    }

    fn empty_return_value(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
            let slot_size = Self::resolve_type_alloc_size(
                ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts,
            ).unwrap_or(16);
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot, slot_size, 0,
            ));
            if matches!(ctx.ret_ty, MirType::Option(_)) {
                Self::build_none(builder, ss);
            } else {
                let zero = builder.ins().iconst(types::I64, 0);
                Self::build_ok(builder, ss, zero);
            }
            return Ok(builder.ins().stack_addr(types::I64, ss, 0));
        }
        let ret_cl_ty = mir_to_cranelift_type(ctx.ret_ty)?;
        Ok(if ret_cl_ty.is_float() {
            builder.ins().f64const(0.0)
        } else {
            builder.ins().iconst(ret_cl_ty, 0)
        })
    }

    /// struct.targets/EX4: `func main() -> void or E` returning its error branch
    /// exits 1. Reads the Result tag; on the error side, calls the error type's
    /// `message()` when it has one and hands the text to the runtime, which
    /// prints it and exits. The ok side falls through to the normal return.
    fn emit_main_error_check(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let MirType::Result { err, .. } = ctx.ret_ty else {
            return Ok(());
        };
        let Some(val_op) = value else { return Ok(()) };
        let Some(&exit_fr) = ctx.func_refs.get("main_error_exit") else {
            return Ok(());
        };

        let base = Self::lower_operand(builder, val_op, ctx)?;
        let msg_fr = Self::aggregate_type_name(err, ctx)
            .and_then(|name| ctx.func_refs.get(&format!("{}_message", name)).copied());

        // `return SomeError` in a `void or E` main lowers to the bare error
        // value, not a wrapped Result — that path is unconditionally an error.
        let local_ty = Self::operand_mir_type(val_op, ctx.locals);
        if Self::is_err_component(ctx.ret_ty, local_ty.as_ref()) {
            let msg = Self::call_message(builder, msg_fr, base);
            builder.ins().call(exit_fr, &[msg]);
            return Ok(());
        }

        // Wrapped Result: branch on the tag.
        if !matches!(local_ty, Some(MirType::Result { .. })) {
            return Ok(());
        }
        let tag = builder.ins().load(types::I64, MemFlags::new(), base, 0);
        let is_err = builder.ins().icmp_imm(IntCC::NotEqual, tag, 0);

        let err_block = builder.create_block();
        let ok_block = builder.create_block();
        builder.ins().brif(is_err, err_block, &[], ok_block, &[]);

        builder.switch_to_block(err_block);
        builder.seal_block(err_block);
        // `{ErrType}_message(payload) -> string` when the error type defines one.
        // Without it there's nothing to print but the fact, so pass null.
        let payload = builder
            .ins()
            .iadd_imm(base, crate::layouts::RESULT_PAYLOAD_OFFSET as i64);
        let msg = Self::call_message(builder, msg_fr, payload);
        builder.ins().call(exit_fr, &[msg]);
        // rask_main_error_exit is _Noreturn, but the block still needs a
        // terminator for the verifier.
        builder.ins().trap(TrapCode::user(1).unwrap());

        builder.switch_to_block(ok_block);
        builder.seal_block(ok_block);
        Ok(())
    }

    /// Call `{ErrType}_message(err) -> string` and copy the 16 bytes out.
    ///
    /// An aggregate return is a pointer to the callee's own storage, so the
    /// convention everywhere is: copy before doing anything else. Calls with a
    /// MIR destination get that copy for free — `stack_slot_map` gives them a
    /// caller-owned slot. This one is hand-rolled and has no destination local,
    /// so it does its own copy; the next call would otherwise reuse the frame
    /// the pointer names, and the next call here is the one that prints it.
    ///
    /// Returns a null pointer when the error type has no `message()`.
    fn call_message(
        builder: &mut ClifFunctionBuilder,
        msg_fr: Option<FuncRef>,
        err_ptr: Value,
    ) -> Value {
        let Some(fr) = msg_fr else {
            return builder.ins().iconst(types::I64, 0);
        };
        let call = builder.ins().call(fr, &[err_ptr]);
        let src = builder.inst_results(call)[0];
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 16, 0,
        ));
        for off in [0i32, 8] {
            let word = builder.ins().load(types::I64, MemFlags::new(), src, off);
            builder.ins().stack_store(word, ss, off);
        }
        builder.ins().stack_addr(types::I64, ss, 0)
    }

    /// The declared name behind a struct or enum MIR type, for mangled-name
    /// lookups like `{Type}_message`.
    fn aggregate_type_name(ty: &MirType, ctx: &CodegenCtx) -> Option<String> {
        match ty {
            MirType::Struct(id) => {
                ctx.struct_layouts.get(id.id as usize).map(|l| l.name.clone())
            }
            MirType::Enum(id) => ctx.enum_layouts.get(id.id as usize).map(|l| l.name.clone()),
            _ => None,
        }
    }

    /// The Cranelift value a non-`main` function hands back for `return v`.
    ///
    /// `None` means it returns nothing. Everything about *wrapping* lives here:
    /// a `T or E` function whose MIR returns a bare ok or err component has to
    /// build the tagged slot, and which side it is comes from the operand's MIR
    /// type. Shared by both exits — a plain `return` and a `return` that
    /// chains through `ensure` cleanup produce the same value, and only the
    /// last instruction differs (`return_` vs `jump` to the cleanup block).
    /// Split out because `CleanupReturn` used to skip all of this and pass the
    /// raw operand: `return content` from a `string or IoError` function with
    /// an `ensure` in scope handed the caller the bare string pointer, and the
    /// caller read a Result tag out of the first bytes of the text.
    fn exit_value(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Option<Value>> {
        if let Some(stack_info) = Self::return_stack_info(value, ctx.stack_slot_map) {
            // For small aggregate return values (≤8 bytes) in stack slots:
            //   - If the function return type is Result/Option, the small value
            //     is a component (ok or err), not a Result — must be wrapped.
            //   - Otherwise load the scalar and return it directly.
            // For larger aggregates, return the stack slot address. The caller
            // copies from it immediately via copy_aggregate (the callee stack
            // is still accessible at that point on x86-64).
            let (local_id, ss, size) = stack_info;
            if size <= 8 {
                let loaded = builder.ins().stack_load(types::I64, ss, 0);
                if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
                    // Small component in a Result/Option-returning function.
                    // Wrap it as Ok or Err by checking the local's type.
                    let slot_size = Self::resolve_type_alloc_size(
                        ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts
                    ).unwrap_or(16);
                    let ret_ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, slot_size, 0,
                    ));
                    let local_ty = ctx.locals.iter()
                        .find(|l| l.id == local_id)
                        .map(|l| &l.ty);
                    if Self::is_err_component(ctx.ret_ty, local_ty) {
                        Self::build_err(builder, ret_ss, loaded);
                    } else if matches!(ctx.ret_ty, MirType::Option(_)) {
                        Self::build_some(builder, ret_ss, loaded);
                    } else {
                        Self::build_ok(builder, ret_ss, loaded);
                    }
                    let addr = builder.ins().stack_addr(types::I64, ret_ss, 0);
                    return Ok(Some(addr));
                } else {
                    return Ok(Some(loaded));
                }
            } else {
                // >8-byte aggregate in a stack slot. If the function returns
                // Result/Option but this local is a bare ok/err component
                // (not an already-wrapped Result), wrap it — otherwise the
                // callee hands back the raw payload and the caller reads the
                // tag from the payload's first bytes (#347).
                let local_ty = ctx.locals.iter()
                    .find(|l| l.id == local_id)
                    .map(|l| l.ty.clone());
                let already_wrapped = local_ty.as_ref() == Some(ctx.ret_ty);
                let needs_wrap = !already_wrapped
                    && matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_));
                if needs_wrap {
                    let is_err = Self::is_err_component(ctx.ret_ty, local_ty.as_ref());
                    let is_result = matches!(ctx.ret_ty, MirType::Result { .. });
                    let slot_size = Self::resolve_type_alloc_size(
                        ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts,
                    ).unwrap_or(16);
                    let ret_ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, slot_size, 0,
                    ));
                    let src = builder.ins().stack_addr(types::I64, ss, 0);
                    Self::build_wrapped_aggregate(
                        builder, ret_ss, is_result, if is_err { 1 } else { 0 }, src, size,
                    );
                    let addr = builder.ins().stack_addr(types::I64, ret_ss, 0);
                    return Ok(Some(addr));
                } else {
                    // Pointer to the stack slot data, for copy_aggregate
                    return Self::plain_exit_value(builder, value, ctx);
                }
            }
        } else if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
            // Function returns Result/Option but value is a plain scalar
            // or non-stack-slotted local (e.g. `return 42` or
            // `return DivError {}` from a `i32 or DivError` fn).
            // Wrap as Ok/Some — or Err when the value's MIR type matches
            // the Result's err side. Without the Err detection, returning
            // a struct-typed error from a Result-returning function
            // wrapped it as Ok and caused `is X` checks at the call site
            // to silently take the wrong branch (#259 family).
            let slot_size = Self::resolve_type_alloc_size(ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts)
                .unwrap_or(16);
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size,
                0,
            ));
            let val_ty = value.and_then(|v| match v {
                MirOperand::Local(id) => ctx.locals.iter()
                    .find(|l| l.id == *id)
                    .map(|l| l.ty.clone()),
                _ => None,
            });
            let is_err_value = Self::is_err_component(ctx.ret_ty, val_ty.as_ref());

            // Aggregate payload (string/struct/...) — `val` is a pointer to
            // 16+ bytes. Copy into the payload area instead of storing the
            // pointer at offset 8 (which leaves the rest of the slot
            // uninitialized).
            let inner_branch = if is_err_value {
                match ctx.ret_ty {
                    MirType::Result { err, .. } => Some(err.as_ref()),
                    _ => None,
                }
            } else {
                match ctx.ret_ty {
                    MirType::Option(inner) => Some(inner.as_ref()),
                    MirType::Result { ok, .. } => Some(ok.as_ref()),
                    _ => None,
                }
            };
            let inner_aggregate = inner_branch.filter(|t| matches!(t,
                MirType::String | MirType::Struct(_) | MirType::Enum(_)
                | MirType::Tuple(_) | MirType::Result { .. } | MirType::Option(_)
            ));

            // A scalar payload is typed by the payload, not by the
            // container — same rule as assigning into an Option slot.
            // Lowering at I64 made `return 2.5` from a `f32 or E` build an
            // f64, and the f32 read of the payload then picked up the
            // double's zero low half (#629).
            let payload_ty = match (inner_aggregate, inner_branch) {
                (None, Some(inner)) => Self::scalar_payload_store_type(inner)?,
                _ => None,
            }
            .unwrap_or(types::I64);
            let val = if let Some(val_op) = value {
                Self::lower_operand_typed(builder, val_op, Some(payload_ty), ctx)?
            } else {
                builder.ins().iconst(types::I64, 0)
            };

            if let Some(inner) = inner_aggregate {
                let inner_size = Self::resolve_type_alloc_size(
                    inner, ctx.struct_layouts, ctx.enum_layouts,
                ).unwrap_or(inner.size());
                let is_result = matches!(ctx.ret_ty, MirType::Result { .. });
                Self::build_wrapped_aggregate(
                    builder, ss, is_result, if is_err_value { 1 } else { 0 }, val, inner_size,
                );
            } else if is_err_value {
                Self::build_err(builder, ss, val);
            } else if matches!(ctx.ret_ty, MirType::Option(_)) {
                Self::build_some(builder, ss, val);
            } else {
                Self::build_ok(builder, ss, val);
            }
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            return Ok(Some(addr));
        } else {
            return Self::plain_exit_value(builder, value, ctx);
        }
    }

    /// No wrapping to do — convert the operand to the return type, or nothing.
    fn plain_exit_value(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Option<Value>> {
        let Some(val_op) = value else { return Ok(None) };
        let expected_ty = mir_to_cranelift_type(ctx.ret_ty)?;
        let val = Self::lower_operand_typed(builder, val_op, Some(expected_ty), ctx)?;
        let actual_ty = builder.func.dfg.value_type(val);
        Ok(Some(if actual_ty != expected_ty {
            Self::convert_value(builder, val, actual_ty, expected_ty, None)
        } else {
            val
        }))
    }

    /// Emit the return instruction for a CleanupReturn that has no cleanup
    /// block to chain through. `main` returns void to C whatever its Rask
    /// signature says.
    fn emit_plain_return(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        if ctx.is_main {
            builder.ins().return_(&[]);
            return Ok(());
        }
        match Self::exit_value(builder, value, ctx)? {
            Some(val) => builder.ins().return_(&[val]),
            None => builder.ins().return_(&[]),
        };
        Ok(())
    }

    /// Check if a return value comes from a stack-allocated aggregate local.
    /// Returns the (stack_slot, size) if so.
    fn return_stack_info(
        value: Option<&MirOperand>,
        stack_slot_map: &HashMap<LocalId, (StackSlot, u32)>,
    ) -> Option<(LocalId, StackSlot, u32)> {
        if let Some(MirOperand::Local(id)) = value {
            if let Some((ss, size)) = stack_slot_map.get(id) {
                return Some((*id, *ss, *size));
            }
        }
        None
    }

    /// Compute the actual allocation size for a MirType, resolving struct/enum
    /// sizes from layouts. Unlike MirType::size() which returns 8 for Struct/Enum
    /// (pointer size), this returns the true layout size. Needed for stack slots
    /// that store aggregate values inline (Result<Struct, Enum>, Option<Struct>, etc.).
    fn resolve_type_alloc_size(
        ty: &MirType,
        struct_layouts: &[StructLayout],
        enum_layouts: &[EnumLayout],
    ) -> Option<u32> {
        match ty {
            MirType::Struct(id) => struct_layouts.get(id.id as usize).map(|sl| sl.size),
            MirType::Enum(id) => enum_layouts.get(id.id as usize).map(|el| el.size),
            MirType::Array { elem, len } => Some(elem.size() * len),
            MirType::Result { ok, err } => {
                let ok_size = Self::resolve_type_alloc_size(ok, struct_layouts, enum_layouts)
                    .unwrap_or(ok.size());
                let err_size = Self::resolve_type_alloc_size(err, struct_layouts, enum_layouts)
                    .unwrap_or(err.size());
                // tag (8) + origin_file (8) + origin_line (8) + payload.
                // Scalars are stored as 8-byte values in codegen; .max(8) prevents OOB writes.
                Some(crate::layouts::RESULT_PAYLOAD_OFFSET as u32 + ok_size.max(8).max(err_size.max(8)))
            }
            // A niche option — `Handle<T>?` or `Link<T>?` — is one word with no
            // tag and needs no slot. Giving it the tagged layout made the local
            // hold a slot address, and comparing that against the sentinel was
            // always false (#438).
            MirType::Option(inner) if inner.is_niche_payload() => None,
            MirType::Option(inner) => {
                let inner_size = Self::resolve_type_alloc_size(inner, struct_layouts, enum_layouts)
                    .unwrap_or(inner.size());
                // Scalars are stored as 8-byte values in codegen; .max(8) prevents OOB writes.
                Some(8 + inner_size.max(8))
            }
            MirType::Tuple(fields) => {
                let mut offset = 0u32;
                for f in fields {
                    let f_size = Self::resolve_type_alloc_size(f, struct_layouts, enum_layouts)
                        .unwrap_or(f.size());
                    let align = f.align();
                    offset = (offset + align - 1) & !(align - 1);
                    offset += f_size;
                }
                let max_align = fields.iter().map(|f| f.align()).max().unwrap_or(1);
                Some((offset + max_align - 1) & !(max_align - 1))
            }
            MirType::String => Some(16),
            MirType::Slice(_) | MirType::TraitObject { .. } => Some(ty.size()),
            // `[member:8][member bytes]` — the index word counts, or the slot
            // comes up 8 bytes short and the widest member's tail lands past its
            // end (#776).
            MirType::Union(variants) => {
                let max = variants.iter()
                    .map(|v| Self::resolve_type_alloc_size(v, struct_layouts, enum_layouts)
                        .unwrap_or(v.size()))
                    .max()
                    .unwrap_or(0);
                Some(rask_mono::abi::UNION_PAYLOAD_OFFSET + max)
            }
            _ => None,
        }
    }

    /// Copy `size` bytes from `src_ptr + src_off` to `dst_ptr + dst_off`.
    /// Delegates to the crate-wide [`copy_bytes`] — see it for why there's
    /// exactly one of these.
    fn copy_bytes(
        builder: &mut ClifFunctionBuilder,
        src_ptr: Value,
        src_off: i32,
        dst_ptr: Value,
        dst_off: i32,
        size: u32,
    ) {
        copy_bytes(builder, src_ptr, src_off, dst_ptr, dst_off, size);
    }

    /// Copy aggregate data from a source pointer into a caller-owned stack slot.
    /// Used after calls that return aggregate types (struct, enum, Result, etc.)
    /// to avoid dangling pointers to callee stack frames.
    fn copy_aggregate(builder: &mut ClifFunctionBuilder, src_ptr: Value, dst_slot: StackSlot, size: u32) {
        Self::copy_aggregate_at(builder, src_ptr, dst_slot, 0, size);
    }

    /// Copy `size` bytes from `src_ptr` into `dst_slot` starting at `dst_off`.
    fn copy_aggregate_at(
        builder: &mut ClifFunctionBuilder,
        src_ptr: Value,
        dst_slot: StackSlot,
        dst_off: i32,
        size: u32,
    ) {
        let dst_ptr = builder.ins().stack_addr(types::I64, dst_slot, 0);
        Self::copy_bytes(builder, src_ptr, 0, dst_ptr, dst_off, size);
    }

    /// Copy `size` bytes from `src_ptr` to `dst_ptr`. Mirror of `copy_aggregate`
    /// but for through-pointer destinations (mutate-params, where the dst's
    /// variable holds an external pointer instead of a stack-slot address).
    fn copy_aggregate_to_ptr(builder: &mut ClifFunctionBuilder, src_ptr: Value, dst_ptr: Value, size: u32) {
        Self::copy_bytes(builder, src_ptr, 0, dst_ptr, 0, size);
    }

    // ─── Option/Result slot constructors ────────────────────────
    // One place that knows the tag + origin-fields + payload layout, keyed off
    // the rask_mono::abi offsets. Every wrapping site routes through these
    // instead of re-emitting the store sequence inline.
    //
    //   Result slot: [tag:8][origin_file:8][origin_line:8][payload]
    //   Option slot: [tag:8][payload]     (no origin fields)
    // Tag 0 = Ok/Some, tag 1 = Err/none.

    /// Zero the Result origin-file/line fields. Ok and C-FFI errors have no
    /// Rask origin; real Err origins are filled by the error-construction path.
    fn zero_result_origin(builder: &mut ClifFunctionBuilder, slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(zero, slot, crate::layouts::ORIGIN_FILE_OFFSET);
        builder.ins().stack_store(zero, slot, crate::layouts::ORIGIN_LINE_OFFSET);
    }

    /// Ok(scalar) into a Result slot.
    fn build_ok(builder: &mut ClifFunctionBuilder, slot: StackSlot, payload: Value) {
        let tag = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, slot);
        builder.ins().stack_store(payload, slot, crate::layouts::RESULT_PAYLOAD_OFFSET);
    }

    /// Err(scalar) into a Result slot (origin zeroed — no source location here).
    fn build_err(builder: &mut ClifFunctionBuilder, slot: StackSlot, payload: Value) {
        let tag = builder.ins().iconst(types::I64, 1);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, slot);
        builder.ins().stack_store(payload, slot, crate::layouts::RESULT_PAYLOAD_OFFSET);
    }

    /// Some(scalar) into an Option slot. Option layout has no origin fields —
    /// using the Result constructor here would write past the end of the slot.
    fn build_some(builder: &mut ClifFunctionBuilder, slot: StackSlot, payload: Value) {
        let tag = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
        builder.ins().stack_store(payload, slot, crate::layouts::PAYLOAD_OFFSET);
    }

    /// none into an Option slot (tag only — payload is dead).
    fn build_none(builder: &mut ClifFunctionBuilder, slot: StackSlot) {
        let tag = builder.ins().iconst(types::I64, 1);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
    }

    /// Write the `IoError` payload for a failed string-out-param call.
    ///
    /// `status` is the runtime's `RASK_STROUT_*`: 2 means the input ran out,
    /// anything else non-zero means a real error whose message the runtime put
    /// in `err_ss`. Those become `IoError.UnexpectedEof` and
    /// `IoError.Other(msg)` — the two shapes the interpreter produces.
    ///
    /// Variant tags and the message field's offset come from the destination's
    /// own error layout, so reordering `enum IoError` in stdlib/io.rk can't
    /// silently change what gets built.
    fn build_io_error_payload(
        builder: &mut ClifFunctionBuilder,
        dst_ss: StackSlot,
        status: Value,
        err_ss: StackSlot,
        dst_id: &LocalId,
        ctx: &CodegenCtx,
    ) {
        let err_layout = ctx.locals.iter()
            .find(|l| l.id == *dst_id)
            .and_then(|l| match &l.ty {
                MirType::Result { err, .. } => match err.as_ref() {
                    MirType::Enum(id) => ctx.enum_layouts.get(id.id as usize),
                    _ => None,
                },
                _ => None,
            });
        let variant = |name: &str, fallback: i64| -> (i64, i32) {
            err_layout
                .and_then(|l| l.variants.iter().find(|v| v.name == name))
                .map(|v| {
                    let field_off = v.fields.first().map(|f| f.offset).unwrap_or(0);
                    (v.tag as i64, (v.payload_offset + field_off) as i32)
                })
                .unwrap_or((fallback, 8))
        };
        let (other_tag, msg_offset) = variant("Other", 7);
        let (eof_tag, _) = variant("UnexpectedEof", IO_ERROR_UNEXPECTED_EOF);

        let eof_block = builder.create_block();
        let other_block = builder.create_block();
        let done_block = builder.create_block();

        let eof_code = builder.ins().iconst(types::I64, RASK_STROUT_EOF);
        let is_eof = builder.ins().icmp(IntCC::Equal, status, eof_code);
        builder.ins().brif(is_eof, eof_block, &[], other_block, &[]);

        builder.switch_to_block(eof_block);
        builder.seal_block(eof_block);
        let v = builder.ins().iconst(types::I64, eof_tag);
        builder.ins().stack_store(v, dst_ss, crate::layouts::RESULT_PAYLOAD_OFFSET);
        builder.ins().jump(done_block, &[]);

        builder.switch_to_block(other_block);
        builder.seal_block(other_block);
        let v = builder.ins().iconst(types::I64, other_tag);
        builder.ins().stack_store(v, dst_ss, crate::layouts::RESULT_PAYLOAD_OFFSET);
        let src = builder.ins().stack_addr(types::I64, err_ss, 0);
        let dst_addr = builder.ins().stack_addr(types::I64, dst_ss, 0);
        Self::copy_bytes(
            builder, src, 0, dst_addr,
            crate::layouts::RESULT_PAYLOAD_OFFSET + msg_offset,
            crate::layouts::STRING_SIZE as u32,
        );
        builder.ins().jump(done_block, &[]);

        builder.switch_to_block(done_block);
        builder.seal_block(done_block);
    }

    /// Assemble a `T or JoinError` from what the runtime reported.
    ///
    /// `outcome` is RASK_JOIN_OK / _PANICKED / _CANCELLED; `value_ss` holds the
    /// task's return value and `msg_ss` a 16-byte RaskStr (empty unless it
    /// panicked). The JoinError variant tags and its message field's offset come
    /// from the destination's own error layout, so renaming or reordering the
    /// enum in stdlib/async.rk doesn't silently change what gets built.
    fn build_join_result(
        builder: &mut ClifFunctionBuilder,
        dst_ss: StackSlot,
        outcome: Value,
        value_ss: StackSlot,
        msg_ss: StackSlot,
        dst_id: &LocalId,
        ctx: &CodegenCtx,
    ) {
        let (ok_ty, err_layout) = ctx.locals.iter()
            .find(|l| l.id == *dst_id)
            .and_then(|l| match &l.ty {
                MirType::Result { ok, err } => {
                    let err_layout = match err.as_ref() {
                        MirType::Enum(id) => ctx.enum_layouts.get(id.id as usize),
                        _ => None,
                    };
                    Some((ok.as_ref().clone(), err_layout))
                }
                _ => None,
            })
            .unwrap_or((MirType::I64, None));

        let variant = |name: &str, fallback: i64| -> (i64, i32) {
            err_layout
                .and_then(|l| l.variants.iter().find(|v| v.name == name))
                .map(|v| {
                    let field_off = v.fields.first().map(|f| f.offset).unwrap_or(0);
                    (v.tag as i64, (v.payload_offset + field_off) as i32)
                })
                .unwrap_or((fallback, 8))
        };
        let (panicked_tag, msg_offset) = variant("Panicked", 0);
        let (cancelled_tag, _) = variant("Cancelled", 1);

        let ok_block = builder.create_block();
        let fail_block = builder.create_block();
        let panicked_block = builder.create_block();
        let cancelled_block = builder.create_block();
        let merge_block = builder.create_block();

        let ok_code = builder.ins().iconst(types::I64, RASK_JOIN_OK);
        let is_ok = builder.ins().icmp(IntCC::Equal, outcome, ok_code);
        builder.ins().brif(is_ok, ok_block, &[], fail_block, &[]);

        builder.switch_to_block(ok_block);
        builder.seal_block(ok_block);
        let ok_size = Self::resolve_type_alloc_size(
            &ok_ty, ctx.struct_layouts, ctx.enum_layouts,
        ).unwrap_or(8);
        let tag = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(tag, dst_ss, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, dst_ss);
        if ok_size > 8 {
            // The task handed back an address. Copy through it — nothing in the
            // slot survives the callee otherwise.
            let src = builder.ins().stack_load(types::I64, value_ss, 0);
            let dst_addr = builder.ins().stack_addr(types::I64, dst_ss, 0);
            Self::copy_bytes(
                builder, src, 0, dst_addr, crate::layouts::RESULT_PAYLOAD_OFFSET, ok_size,
            );
        } else {
            let load_ty = mir_to_cranelift_type(&ok_ty).unwrap_or(types::I64);
            let value = builder.ins().stack_load(load_ty, value_ss, 0);
            builder.ins().stack_store(value, dst_ss, crate::layouts::RESULT_PAYLOAD_OFFSET);
        }
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(fail_block);
        builder.seal_block(fail_block);
        let err_tag = builder.ins().iconst(types::I64, 1);
        builder.ins().stack_store(err_tag, dst_ss, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, dst_ss);
        // The message slot is a valid string either way — empty for Cancelled —
        // so copy it before the split. A Cancelled left with an uninitialized
        // 16 bytes there would be freed as if it were a heap string.
        let src = builder.ins().stack_addr(types::I64, msg_ss, 0);
        let dst_addr = builder.ins().stack_addr(types::I64, dst_ss, 0);
        Self::copy_bytes(
            builder, src, 0, dst_addr,
            crate::layouts::RESULT_PAYLOAD_OFFSET + msg_offset,
            crate::layouts::STRING_SIZE as u32,
        );
        let panicked_code = builder.ins().iconst(types::I64, RASK_JOIN_PANICKED);
        let is_panicked = builder.ins().icmp(IntCC::Equal, outcome, panicked_code);
        builder.ins().brif(is_panicked, panicked_block, &[], cancelled_block, &[]);

        builder.switch_to_block(panicked_block);
        builder.seal_block(panicked_block);
        let v = builder.ins().iconst(types::I64, panicked_tag);
        builder.ins().stack_store(v, dst_ss, crate::layouts::RESULT_PAYLOAD_OFFSET);
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(cancelled_block);
        builder.seal_block(cancelled_block);
        let v = builder.ins().iconst(types::I64, cancelled_tag);
        builder.ins().stack_store(v, dst_ss, crate::layouts::RESULT_PAYLOAD_OFFSET);
        builder.ins().jump(merge_block, &[]);

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);
    }

    /// Payload types that live in their own storage, so extracting one yields
    /// an address rather than a loaded scalar. Nested `Option`/`Result` belong
    /// here: a `T??` payload is a whole 16-byte `T?` slot (#493).
    fn is_boxed_payload(ty: &MirType) -> bool {
        // A niche option is one word — the value itself, with one reserved word
        // meaning `none` — so it loads like a scalar even though it is spelled
        // `Option`. Handing back its address instead made every slot of a
        // `Vec<Handle<T>?>` read as present: an address is never the sentinel,
        // and the address was then used as the handle (#959).
        if matches!(ty, MirType::Option(inner) if inner.is_niche_payload()) {
            return false;
        }
        matches!(
            ty,
            MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::String
                | MirType::Option(_)
                | MirType::Result { .. }
                // A trait object is two words, so the payload read has to hand
                // back its address like any other aggregate. Loading the first
                // 8 bytes as a scalar kept the data pointer and dropped the
                // vtable (#552).
                | MirType::TraitObject { .. }
                // An error union is `[member:8][member bytes]` in the payload
                // area. Loaded as a word, the member index came back as if it
                // were the union's address (#776).
                | MirType::Union(_)
        )
    }

    /// The Cranelift type a bare scalar takes on once it becomes an Option's
    /// payload. `None` for a boxed payload — those arrive as an address and keep
    /// the container's pointer type.
    ///
    /// A float keeps its own type: the payload read is a plain float load, so
    /// anything that renumbers the value on the way in (rather than moving its
    /// bits) is lost. Integers still widen to the full 8-byte slot, which is
    /// value-preserving and leaves no undefined bytes beside them.
    fn scalar_payload_store_type(inner: &MirType) -> CodegenResult<Option<Type>> {
        let ty = mir_to_cranelift_type(inner)?;
        Ok(
            match rask_mono::abi::payload_repr(ty.is_float(), Self::is_boxed_payload(inner)) {
                rask_mono::abi::PayloadRepr::InPlace => None,
                rask_mono::abi::PayloadRepr::Float64 => Some(types::F64),
                rask_mono::abi::PayloadRepr::IntFullWidth => Some(types::I64),
            },
        )
    }

    /// The Cranelift type an 8-byte storage slot holds a scalar as.
    ///
    /// One rule for every slot in the language: **a float lives in an 8-byte slot
    /// as an f64**, integers as a full-width integer. Struct fields already did
    /// this, collections were made to (#621/#629), and Option/Result payloads
    /// joined them rather than staying a third convention. `None` for anything
    /// that isn't a scalar — those live at their own address.
    ///
    /// Reads go through here too, so the write width and the read width can't
    /// drift: that drift is exactly what made `v.push(2.5)` on a `Vec<f32>` store
    /// 8 bytes and read back 4.
    fn slot_storage_type(inner: &MirType) -> Option<Type> {
        let ty = mir_to_cranelift_type(inner).ok()?;
        match rask_mono::abi::payload_repr(ty.is_float(), Self::is_boxed_payload(inner)) {
            rask_mono::abi::PayloadRepr::InPlace => None,
            rask_mono::abi::PayloadRepr::Float64 => Some(types::F64),
            // Read at the value's own width — the write was full-width and
            // little-endian put the meaningful bytes first.
            rask_mono::abi::PayloadRepr::IntFullWidth => Some(ty),
        }
    }

    /// Wrap an aggregate payload (copied from `src_ptr`) into an Option/Result
    /// slot. `is_result` picks the payload offset and whether origin is zeroed;
    /// `tag` is 0 (Ok/Some) or 1 (Err/none).
    fn build_wrapped_aggregate(
        builder: &mut ClifFunctionBuilder,
        slot: StackSlot,
        is_result: bool,
        tag: i64,
        src_ptr: Value,
        size: u32,
    ) {
        let tag_v = builder.ins().iconst(types::I64, tag);
        builder.ins().stack_store(tag_v, slot, crate::layouts::TAG_OFFSET);
        let payload_off = if is_result {
            Self::zero_result_origin(builder, slot);
            crate::layouts::RESULT_PAYLOAD_OFFSET
        } else {
            crate::layouts::PAYLOAD_OFFSET
        };
        Self::copy_aggregate_at(builder, src_ptr, slot, payload_off, size);
    }

    /// True when `local_ty` is the err side of `ret_ty` (a Result). Drives the
    /// Ok-vs-Err tag choice when a bare component is returned/assigned into a
    /// Result — the type identity, not name capitalization, decides (#259).
    fn is_err_component(ret_ty: &MirType, local_ty: Option<&MirType>) -> bool {
        match ret_ty {
            MirType::Result { err, .. } => local_ty == Some(err.as_ref()),
            _ => false,
        }
    }

    /// C functions that use "negative return = error" convention.
    /// If value < 0: tag=1 (Err), payload=value. Otherwise: tag=0 (Ok), payload=value.
    /// Note: fs_open/fs_create return NULL (0) for errors, not -1 — handled separately.
    /// Negative scalar → `none`, otherwise `some(value)`. The Option twin of
    /// `wrap_result_into_slot`; Option's payload sits at its own offset.
    fn wrap_option_into_slot(builder: &mut ClifFunctionBuilder, value: Value, dst_slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        let is_none = builder.ins().icmp(IntCC::SignedLessThan, value, zero);
        let tag = builder.ins().uextend(types::I64, is_none);
        builder.ins().stack_store(tag, dst_slot, crate::layouts::TAG_OFFSET);
        builder.ins().stack_store(value, dst_slot, crate::layouts::PAYLOAD_OFFSET);
    }

    fn wrap_result_into_slot(builder: &mut ClifFunctionBuilder, value: Value, dst_slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        let is_err = builder.ins().icmp(IntCC::SignedLessThan, value, zero);
        let tag = builder.ins().uextend(types::I64, is_err);
        builder.ins().stack_store(tag, dst_slot, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, dst_slot);
        builder.ins().stack_store(value, dst_slot, crate::layouts::RESULT_PAYLOAD_OFFSET);
    }

    /// Store a value to a stack slot and return its address.
    /// Used for pointer-based calling convention (typed runtime API).
    ///
    /// An f32 is widened to f64 first, so a float always occupies the whole
    /// 8-byte slot — the same convention struct fields use. `load_scalar_slot`
    /// is the matching read. Without agreeing on one width, `v.push(1.5)` on a
    /// `Vec<f32>` wrote 8 bytes of double (a float literal materializes as f64
    /// when the callee's declared parameter isn't a float type) while
    /// `v.push(a)` on an `f32` local wrote 4, and the 4-byte read back got the
    /// double's zero low half — printing 0 for the literal and the right value
    /// for the local (#629).
    fn value_to_ptr(builder: &mut ClifFunctionBuilder, val: Value) -> Value {
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 8, 0,
        ));
        let stored = if builder.func.dfg.value_type(val) == types::F32 {
            builder.ins().fpromote(types::F64, val)
        } else {
            val
        };
        builder.ins().stack_store(stored, ss, 0);
        builder.ins().stack_addr(types::I64, ss, 0)
    }

    /// Load a scalar the runtime stored in an 8-byte slot, as `want`.
    /// The twin of `value_to_ptr`: a float lives there as f64, so an f32
    /// destination loads the double and demotes.
    fn load_scalar_slot(builder: &mut ClifFunctionBuilder, ptr: Value, want: Type) -> Value {
        if want == types::F32 {
            let wide = builder.ins().load(types::F64, MemFlags::new(), ptr, 0);
            return builder.ins().fdemote(types::F32, wide);
        }
        builder.ins().load(want, MemFlags::new(), ptr, 0)
    }

    /// Check if MIR arg at `index` is a string type.
    fn is_string_arg(mir_args: &[MirOperand], index: usize, locals: &[rask_mir::MirLocal]) -> bool {
        match mir_args.get(index) {
            Some(MirOperand::Local(id)) => locals
                .iter()
                .find(|l| l.id == *id)
                .map(|l| l.ty == MirType::String)
                .unwrap_or(false),
            Some(MirOperand::Constant(rask_mir::MirConst::String(_))) => true,
            _ => false,
        }
    }

    /// MIR arg already lives behind a pointer — its i64 value is a pointer to
    /// the data, not the data itself. Strings, structs, enums, tuples, options,
    /// results, slices, and trait objects all qualify.
    fn is_by_ptr_arg(mir_args: &[MirOperand], index: usize, locals: &[rask_mir::MirLocal]) -> bool {
        match mir_args.get(index) {
            Some(MirOperand::Local(id)) => locals
                .iter()
                .find(|l| l.id == *id)
                .map(|l| matches!(l.ty,
                    MirType::String
                    | MirType::Struct(_)
                    | MirType::Enum(_)
                    | MirType::Tuple(_)
                    | MirType::Option(_)
                    | MirType::Result { .. }
                    | MirType::Slice(_)
                    | MirType::Union(_)
                    | MirType::TraitObject { .. }
                ))
                .unwrap_or(false),
            Some(MirOperand::Constant(rask_mir::MirConst::String(_))) => true,
            _ => false,
        }
    }

    /// Check if destination local is a string type.
    fn is_string_dst(dst: Option<&LocalId>, ctx: &CodegenCtx) -> bool {
        dst.and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| l.ty == MirType::String)
            .unwrap_or(false)
    }

    /// Destination that owns storage the callee should write into directly —
    /// anything wider than a word, or with its own layout.
    fn is_aggregate_dst(dst: Option<&LocalId>, ctx: &CodegenCtx) -> bool {
        dst.and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| matches!(l.ty,
                MirType::String
                | MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Option(_)
                | MirType::Result { .. }
                | MirType::Union(_)
                | MirType::TraitObject { .. }))
            .unwrap_or(false)
    }

    /// Wrap args[index] as a pointer unless it's already a pointer to data
    /// (string, struct, enum, tuple, option, result, etc.).
    fn wrap_arg_as_ptr(
        builder: &mut ClifFunctionBuilder,
        args: &mut Vec<Value>,
        mir_args: &[MirOperand],
        index: usize,
        locals: &[rask_mir::MirLocal],
    ) {
        if args.len() > index && !Self::is_by_ptr_arg(mir_args, index, locals) {
            let val = args[index];
            args[index] = Self::value_to_ptr(builder, val);
        }
    }

    /// Add out-param for pop/remove-style calls. Returns StringOutParam for string
    /// destinations, PopOutParam otherwise.
    fn append_out_param(
        builder: &mut ClifFunctionBuilder,
        args: &mut Vec<Value>,
        dst: Option<&LocalId>,
        ctx: &CodegenCtx,
    ) -> CallAdapt {
        // Any aggregate destination, not just a string: `Vec.remove` on a
        // `Vec<Pair>` was handed an 8-byte scratch slot, so the struct came back
        // with only its first word and every field after that read as zero.
        // The destination's own slot is the right target — it's already the
        // right size, and writing into it directly skips a copy.
        if Self::is_aggregate_dst(dst, ctx) {
            let fallback_size = dst
                .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
                .map(|l| l.ty.size().max(16))
                .unwrap_or(16);
            let ss = dst
                .and_then(|id| ctx.stack_slot_map.get(id))
                .map(|(ss, _)| *ss)
                .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, fallback_size, 0,
                )));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            args.push(addr);
            CallAdapt::StringOutParam(ss)
        } else {
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot, 8, 0,
            ));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            args.push(addr);
            CallAdapt::PopOutParam(ss)
        }
    }

    /// Deref result, but copy through dst's slot for aggregate destinations
    /// (string, struct, tuple, ...). The collection holds the data inline; the
    /// runtime returns a pointer into that storage which we must copy into the
    /// caller's slot before the collection moves.
    fn deref_or_string(dst: Option<&LocalId>, ctx: &CodegenCtx) -> CallAdapt {
        let is_aggregate = dst
            .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| matches!(l.ty,
                MirType::String
                | MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Option(_)
                | MirType::Result { .. }
                | MirType::Slice(_)
                | MirType::Union(_)
                | MirType::TraitObject { .. }
            ))
            .unwrap_or(false);
        if is_aggregate { CallAdapt::DerefStringElement } else { CallAdapt::DerefResult }
    }

    /// The node type's link-bearing fields, as (kind, byte offset) pairs.
    ///
    /// This is the whole descriptor the rack needs: `insert` walks it to record
    /// the edges a node literal already carries, `delete` walks it to drop the
    /// edges the dying node holds, and `snapshot` walks it to re-point them at
    /// the copy. A link is a leaf — no walk follows one — so nested aggregates
    /// aren't descended into.
    fn link_field_descriptor(
        mir_args: &[MirOperand],
        arg_index: usize,
        ctx: &CodegenCtx,
    ) -> Vec<(i32, u32)> {
        let Some(MirOperand::Local(arg_id)) = mir_args.get(arg_index) else { return Vec::new() };
        let Some(local) = ctx.locals.iter().find(|l| l.id == *arg_id) else { return Vec::new() };
        let MirType::Struct(layout_id) = &local.ty else { return Vec::new() };
        let Some(layout) = ctx.struct_layouts.get(layout_id.id as usize) else { return Vec::new() };
        layout
            .fields
            .iter()
            .filter_map(|f| Self::link_field_kind(&f.ty).map(|k| (k, f.offset)))
            .collect()
    }

    /// `Link<T>` / `Link<T>?` → 0, `Vec<Link<T>>` → 1, `Map<K, Link<T>>` → 2.
    /// Must agree with the `RASK_RACK_FIELD_*` defines in rask_runtime.h.
    fn link_field_kind(ty: &rask_types::Type) -> Option<i32> {
        use rask_types::{GenericArg, Type};
        let bare = ty.as_option().unwrap_or(ty);
        let (name, args) = match bare {
            Type::UnresolvedGeneric { name, args } => (name.as_str(), args.as_slice()),
            _ => return None,
        };
        if name == "Link" {
            return Some(0);
        }
        // The value type is the last type argument: `Vec<T>`'s only one, and
        // `Map<K, V>`'s second.
        let value = args.iter().rev().find_map(|a| match a {
            GenericArg::Type(t) => Some(t),
            _ => None,
        })?;
        let holds_link = matches!(
            value.as_option().unwrap_or(value),
            Type::UnresolvedGeneric { name, .. } if name == "Link"
        );
        if !holds_link {
            return None;
        }
        match name {
            "Vec" => Some(1),
            "Map" => Some(2),
            _ => None,
        }
    }

    /// Look up struct layout size for a MIR arg, returning (elem_size, is_struct).
    /// (byte size, already-a-pointer) for an aggregate argument.
    ///
    /// Every aggregate is passed as an address, so a caller that spills
    /// "scalars" through `value_to_ptr` must not touch these — doing so stores
    /// the address itself and hands the runtime a pointer to a pointer. Only
    /// structs used to count, so sending an enum over a channel copied the
    /// pointer's bytes as if they were the value (#360).
    fn struct_elem_size(mir_args: &[MirOperand], arg_index: usize, ctx: &CodegenCtx) -> (i64, bool) {
        match mir_args.get(arg_index) {
            Some(MirOperand::Local(arg_id)) => {
                if let Some(local) = ctx.locals.iter().find(|l| l.id == *arg_id) {
                    match &local.ty {
                        MirType::Struct(layout_id) => {
                            if let Some(layout) = ctx.struct_layouts.get(layout_id.id as usize) {
                                return (layout.size as i64, true);
                            }
                        }
                        MirType::Enum(layout_id) => {
                            if let Some(layout) = ctx.enum_layouts.get(layout_id.id as usize) {
                                return (layout.size as i64, true);
                            }
                        }
                        // Same question the lock side asks: does the address
                        // name the value? The two used to spell out different
                        // lists, and the short one made a `Mutex<string>` look
                        // word-sized.
                        ty if ty.passed_by_address() => return (ty.size() as i64, true),
                        _ => {}
                    }
                }
                (8, false)
            }
            // A string constant is already the address of its 16 bytes — same
            // shape as a string local. Reading only the operand kind, it looked
            // like a scalar and `Mutex.new("hello")` copied 8 of them.
            Some(MirOperand::Constant(MirConst::String(_))) => (16, true),
            _ => (8, false),
        }
    }

    /// Adapt stdlib call args for the typed runtime API.
    /// Looks up adaptation from the dispatch table, applies mechanically.
    /// Custom entries fall through to hand-written code.
    fn adapt_stdlib_call(
        builder: &mut ClifFunctionBuilder,
        func_name: &str,
        args: &mut Vec<Value>,
        mir_args: &[MirOperand],
        dst: Option<&LocalId>,
        ctx: &CodegenCtx,
        adapt_table: &HashMap<String, (ArgAdapt, RetAdapt)>,
    ) -> CallAdapt {
        let (arg_adapt, ret_adapt) = adapt_table
            .get(func_name)
            .copied()
            .unwrap_or((ArgAdapt::None, RetAdapt::None));

        // Apply arg adaptation
        let call_adapt = match arg_adapt {
            ArgAdapt::None => CallAdapt::None,

            ArgAdapt::InjectOneSize => {
                if args.is_empty() {
                    args.insert(0, builder.ins().iconst(types::I64, 8));
                }
                CallAdapt::None
            }

            ArgAdapt::InjectTwoSizes => {
                if args.is_empty() {
                    args.insert(0, builder.ins().iconst(types::I64, 8));
                    args.insert(1, builder.ins().iconst(types::I64, 8));
                }
                CallAdapt::None
            }

            ArgAdapt::WrapArg1 => {
                Self::wrap_arg_as_ptr(builder, args, mir_args, 1, ctx.locals);
                CallAdapt::None
            }

            ArgAdapt::WrapArg2 => {
                Self::wrap_arg_as_ptr(builder, args, mir_args, 2, ctx.locals);
                CallAdapt::None
            }

            ArgAdapt::WrapArg1And2 => {
                Self::wrap_arg_as_ptr(builder, args, mir_args, 1, ctx.locals);
                Self::wrap_arg_as_ptr(builder, args, mir_args, 2, ctx.locals);
                CallAdapt::None
            }

            ArgAdapt::StringOutParam => {
                let ss = dst
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.insert(0, addr);
                CallAdapt::StringOutParam(ss)
            }

            // Same shape as StringOutParam — 16 bytes into the destination's
            // own slot — but the bytes are a `(usize, usize)` pair rather than
            // a RaskStr.
            ArgAdapt::PairOutParam => {
                let ss = dst
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.insert(0, addr);
                CallAdapt::StringOutParam(ss)
            }

            ArgAdapt::StringResultOutParam => {
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, crate::layouts::STRING_SIZE as u32, 3,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.insert(0, addr);
                // A second out-param for *why* it failed. Without it the call
                // reported only that it failed, and codegen had to invent a
                // variant — every failure became `IoError.UnexpectedEof`,
                // including a read on a write-only descriptor (#682).
                let err_ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, crate::layouts::STRING_SIZE as u32, 3,
                ));
                args.push(builder.ins().stack_addr(types::I64, err_ss, 0));
                CallAdapt::StringResult(ss, err_ss)
            }

            ArgAdapt::StringClone => {
                if let Some(dst_id) = dst {
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        if !args.is_empty() {
                            let src_ptr = args[0];
                            Self::copy_aggregate(builder, src_ptr, *dst_ss, 16);
                        }
                        let dst_addr = builder.ins().stack_addr(types::I64, *dst_ss, 0);
                        args[0] = dst_addr;
                        CallAdapt::StringOutParam(*dst_ss)
                    } else {
                        CallAdapt::None
                    }
                } else {
                    CallAdapt::None
                }
            }

            ArgAdapt::InPlaceStringMut => {
                let ss = mir_args.first()
                    .and_then(|op| if let MirOperand::Local(id) = op { Some(id) } else { None })
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                // C signature: push_str(out, s, other) — prepend out-param address
                let out_addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.insert(0, out_addr);
                CallAdapt::StringOutParam(ss)
            }

            ArgAdapt::AppendOutParam => {
                return Self::append_out_param(builder, args, dst, ctx);
            }

            ArgAdapt::AppendZero => {
                args.push(builder.ins().iconst(types::I64, 0));
                CallAdapt::None
            }

            ArgAdapt::AppendElemSize => {
                args.push(builder.ins().iconst(types::I64, 8));
                CallAdapt::None
            }

            ArgAdapt::AtomicCas => {
                // compare-exchange writes the observed value through an out_ok
                // pointer; the call returns success as its scalar result.
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                args.push(builder.ins().stack_addr(types::I64, ss, 0));
                CallAdapt::PopOutParam(ss)
            }

            ArgAdapt::OptionOutParam => {
                // Hand the callee the destination's payload address so it can
                // copy the element out while it's still live. The old shape —
                // return a pointer and have codegen copy afterwards — meant the
                // pointer named storage the pool had already put on its free
                // list by the time anyone read it.
                let ss = dst
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                let addr = builder.ins().stack_addr(types::I64, ss, crate::layouts::PAYLOAD_OFFSET);
                args.push(addr);
                CallAdapt::OptionOutParam(ss)
            }

            ArgAdapt::ParseOutParam => {
                // The value comes back through an out-param so the return value
                // can carry the 0/1 status.
                //
                // Load it with the type the runtime actually wrote — the float
                // entry points store a double, the integer ones an i64 — then
                // convert to what the destination wants. Loading with the
                // destination type instead read a double's bits as an integer,
                // so "42".parse() came out as 4631107791820423168.
                let writer_ty = if matches!(func_name,
                    "string_parse_float" | "string_parse_f32" | "string_parse_f64")
                {
                    types::F64
                } else {
                    types::I64
                };
                // The payload has to go in at the width the *reader* takes it
                // out at, which is the slot's storage type, not the declared
                // one: a float payload lives in the slot as an f64 and the read
                // demotes. Converting to the declared f32 here wrote four bytes
                // where eight were read, so `let a: f32 = "2.25".parse() ?? -1.0`
                // came back 0 while the f64 spelling was fine.
                let ok_ty = dst
                    .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
                    .and_then(|l| match &l.ty {
                        MirType::Result { ok, .. } => Self::slot_storage_type(ok)
                            .or_else(|| mir_to_cranelift_type(ok).ok()),
                        other => Self::slot_storage_type(other)
                            .or_else(|| mir_to_cranelift_type(other).ok()),
                    })
                    .unwrap_or(writer_ty);
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                args.push(builder.ins().stack_addr(types::I64, ss, 0));
                CallAdapt::ParseResult(ss, writer_ty, ok_ty)
            }

            ArgAdapt::JoinOutcomeOutParams => {
                let value_ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                let msg_ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, crate::layouts::STRING_SIZE as u32, 3,
                ));
                args.push(builder.ins().stack_addr(types::I64, value_ss, 0));
                args.push(builder.ins().stack_addr(types::I64, msg_ss, 0));
                CallAdapt::JoinOutcome(value_ss, msg_ss)
            }

            ArgAdapt::Custom => {
                return Self::adapt_stdlib_custom(builder, func_name, args, mir_args, dst, ctx);
            }
        };

        // Apply return adaptation (override if ret_adapt specifies something)
        match ret_adapt {
            RetAdapt::None => call_adapt,
            RetAdapt::DerefOrString => Self::deref_or_string(dst, ctx),
            RetAdapt::DerefOption => CallAdapt::DerefOption,
            RetAdapt::FromArgAdapt => call_adapt,
            // Negative-return=Err wrapping happens in the result-store path,
            // keyed off the entry's RetAdapt::NegErr — arg handling is untouched.
            RetAdapt::NegErr | RetAdapt::NegNone => call_adapt,
        }
    }

    /// Hand-written adaptation for complex cases that need runtime type inspection.
    fn adapt_stdlib_custom(
        builder: &mut ClifFunctionBuilder,
        func_name: &str,
        args: &mut Vec<Value>,
        mir_args: &[MirOperand],
        dst: Option<&LocalId>,
        ctx: &CodegenCtx,
    ) -> CallAdapt {
        match func_name {
            // Vec.contains: the runtime compares elem_size bytes through a
            // pointer. An aggregate argument is already an address; a scalar
            // has to be spilled so there's something to point at.
            "Vec_contains" => {
                if args.len() >= 2 {
                    let is_aggregate = matches!(
                        mir_args.get(1),
                        Some(MirOperand::Local(id)) if ctx.locals.iter()
                            .find(|l| l.id == *id)
                            .map(|l| Self::resolve_type_alloc_size(
                                &l.ty, ctx.struct_layouts, ctx.enum_layouts,
                            ).is_some())
                            .unwrap_or(false)
                    ) || Self::is_string_arg(mir_args, 1, ctx.locals);
                    if !is_aggregate {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                CallAdapt::None
            }

            // The descriptor for a struct that already sits in its storage —
            // same (kind, offset) pairs `Rack_insert` passes, read off the same
            // layout. Emitting the pairs at the call site keeps the runtime from
            // needing to know anything about Rask types.
            "Link_register_struct" => {
                let fields = Self::link_field_descriptor(mir_args, 0, ctx);
                args.push(builder.ins().iconst(types::I64, fields.len() as i64));
                if fields.is_empty() {
                    args.push(builder.ins().iconst(types::I64, 0));
                } else {
                    let ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, (fields.len() * 8) as u32, 0,
                    ));
                    for (i, (kind, off)) in fields.iter().enumerate() {
                        let k = builder.ins().iconst(types::I32, *kind as i64);
                        let o = builder.ins().iconst(types::I32, *off as i64);
                        builder.ins().stack_store(k, ss, (i * 8) as i32);
                        builder.ins().stack_store(o, ss, (i * 8 + 4) as i32);
                    }
                    args.push(builder.ins().stack_addr(types::I64, ss, 0));
                }
                CallAdapt::None
            }

            // Rack insert: the node's bytes by address, then the shape of `T` —
            // its size, and the byte offsets of its link fields. `Rack.new()`
            // had no argument to read `T` off, so this is where the runtime
            // learns what it is holding (mem.racks).
            "Rack_insert" => {
                let (elem_size, is_struct) = Self::struct_elem_size(mir_args, 1, ctx);
                if args.len() >= 2 && !is_struct {
                    let val = args[1];
                    args[1] = Self::value_to_ptr(builder, val);
                }
                let fields = Self::link_field_descriptor(mir_args, 1, ctx);
                args.push(builder.ins().iconst(types::I64, elem_size));
                args.push(builder.ins().iconst(types::I64, fields.len() as i64));
                if fields.is_empty() {
                    args.push(builder.ins().iconst(types::I64, 0));
                } else {
                    let ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, (fields.len() * 8) as u32, 0,
                    ));
                    for (i, (kind, off)) in fields.iter().enumerate() {
                        let k = builder.ins().iconst(types::I32, *kind as i64);
                        let o = builder.ins().iconst(types::I32, *off as i64);
                        builder.ins().stack_store(k, ss, (i * 8) as i32);
                        builder.ins().stack_store(o, ss, (i * 8 + 4) as i32);
                    }
                    args.push(builder.ins().stack_addr(types::I64, ss, 0));
                }
                CallAdapt::None
            }

            // Pool insert: wrap value as pointer, append elem_size
            "Pool_insert" | "Pool_try_insert" => {
                let (elem_size, is_struct) = Self::struct_elem_size(mir_args, 1, ctx);
                if args.len() >= 2 && !is_struct {
                    let val = args[1];
                    args[1] = Self::value_to_ptr(builder, val);
                }
                args.push(builder.ins().iconst(types::I64, elem_size));
                CallAdapt::None
            }

            // Cell_new(value, size) / Cell_set(cell, value): both take the
            // value by pointer, so a scalar has to be spilled to a slot first.
            "Cell_new" => {
                if !args.is_empty() {
                    let (data_size, is_aggregate) = Self::struct_elem_size(mir_args, 0, ctx);
                    if !is_aggregate {
                        let val = args[0];
                        args[0] = Self::value_to_ptr(builder, val);
                    }
                    let size = builder.ins().iconst(types::I64, data_size);
                    if args.len() >= 2 { args[1] = size; } else { args.push(size); }
                }
                CallAdapt::None
            }
            // Both take the new value by pointer, so a scalar spills to a slot
            // first. `replace` additionally hands back the old value's address —
            // returning CallAdapt::None here would leave that pointer as the
            // result and `let old = c.replace(0)` would print an address.
            "Cell_set" | "Cell_replace"
            | "Shared_set" | "Shared_replace"
            | "Mutex_set" | "Mutex_replace" => {
                if args.len() >= 2 {
                    let (_, is_aggregate) = Self::struct_elem_size(mir_args, 1, ctx);
                    if !is_aggregate {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                if func_name.ends_with("_replace") {
                    CallAdapt::DerefResult
                } else {
                    CallAdapt::None
                }
            }

            // Shared_new / Mutex_new: ensure data is pointer, compute actual
            // data_size. The size arg may or may not already be there —
            // `Shared.new(v)` and `Shared.mutex(v)` reach this under a
            // signature with one parameter, and the old `Shared.new(v)` under
            // one with two.
            "Shared_new" | "Mutex_new" => {
                if !args.is_empty() {
                    let (data_size, is_struct) = Self::struct_elem_size(mir_args, 0, ctx);
                    if !is_struct {
                        let val = args[0];
                        args[0] = Self::value_to_ptr(builder, val);
                    }
                    let size = builder.ins().iconst(types::I64, data_size);
                    if args.len() >= 2 { args[1] = size; } else { args.push(size); }
                }
                CallAdapt::None
            }

            // Sender_send: wrap value as pointer (structs already are)
            "Sender_send" | "send" => {
                if args.len() >= 2 {
                    let (_, is_struct) = Self::struct_elem_size(mir_args, 1, ctx);
                    if !is_struct {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                CallAdapt::None
            }

            // Receiver_receive_struct: replace elem_size arg with stack buffer address
            "Receiver_receive_struct" => {
                let elem_size = match mir_args.get(1) {
                    Some(MirOperand::Constant(MirConst::Int(size))) => *size as u32,
                    _ => 8,
                };
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, elem_size, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                if args.len() >= 2 { args[1] = addr; } else { args.push(addr); }
                CallAdapt::RecvStructOk(elem_size)
            }

            // Receiver_try_receive: recv into a buffer of the element's real size;
            // the C call returns the channel status. Args in: [rx, elem_size].
            // Args out: [rx, out_ptr]. Post-call builds the `T or E` Result.
            "Receiver_try_receive" => {
                let elem_size = match mir_args.get(1) {
                    Some(MirOperand::Constant(MirConst::Int(size))) => *size as u32,
                    _ => 8,
                };
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, elem_size.max(8), 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                if args.len() >= 2 { args[1] = addr; } else { args.push(addr); }
                CallAdapt::TryRecvResult(ss, elem_size)
            }

            _ => CallAdapt::None,
        }
    }

    fn lower_operand(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        Self::lower_operand_typed(builder, op, None, ctx)
    }

    /// Lower a string constant as a raw `const char*` pointer (no RaskString wrapping).
    /// Used by `string_append_cstr` to avoid allocating a temporary RaskString.
    fn lower_string_const_as_cstr(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        if let MirOperand::Constant(MirConst::String(s)) = op {
            if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                return Ok(builder.ins().global_value(types::I64, *gv));
            }
        }
        // Shouldn't reach here — transform only emits cstr variant for constants
        Ok(builder.ins().iconst(types::I64, 0))
    }

    /// Lower a MIR operand to a raw C string pointer (for runtime functions
    /// that take `const char*`). For MirConst::String, returns the data section
    /// pointer directly instead of constructing a Rask string object.
    fn lower_operand_as_cstr(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        if let MirOperand::Constant(MirConst::String(s)) = op {
            if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                return Ok(builder.ins().global_value(types::I64, *gv));
            }
        }
        // Fallback: treat as i64 (pointer)
        Self::lower_operand_typed(builder, op, Some(types::I64), ctx)
    }

    /// Materialize a 128-bit constant.
    ///
    /// Cranelift's `iconst` only takes up to 64 bits, so a 128-bit value is
    /// built as two halves joined with `iconcat` — low word first, which is
    /// the order `iconcat` takes them in.
    fn iconst_i128(builder: &mut ClifFunctionBuilder, n: i128) -> Value {
        let lo = builder.ins().iconst(types::I64, n as u64 as i64);
        let hi = builder.ins().iconst(types::I64, (n >> 64) as u64 as i64);
        builder.ins().iconcat(lo, hi)
    }

    /// An integer constant at any width, including 128 bits.
    ///
    /// `iconst` has no I128 rule — Cranelift builds a 128-bit constant by
    /// concatenating two 64-bit halves. Emitting `iconst.i128` doesn't fail at
    /// the builder; it fails in the *verifier*, as a bare `unreachable!()` with
    /// no message, so the compiler panics rather than reporting anything. Every
    /// guard that builds a constant at the operand's own type has to come
    /// through here (#832).
    fn iconst_at(builder: &mut ClifFunctionBuilder, ty: Type, n: i128) -> Value {
        if ty == types::I128 {
            return Self::iconst_i128(builder, n);
        }
        builder.ins().iconst(ty, n as i64)
    }

    /// SH2: mask a shift amount to the receiver's width, so every amount is
    /// meaningful instead of a trap. `u8` masks to 0-7, `i64` to 0-63.
    fn mask_shift_amount(builder: &mut ClifFunctionBuilder, amount: Value, ty: Type) -> Value {
        let mask = Self::iconst_at(builder, ty, (ty.bits() as i128) - 1);
        builder.ins().band(amount, mask)
    }

    /// OV5 saturating arithmetic: compute it wrapping, ask whether it
    /// overflowed, and on overflow answer the limit it ran into rather than
    /// the wrapped bits.
    ///
    /// Which limit depends on where the true answer went. Unsigned add and mul
    /// can only run off the top, unsigned sub only off the bottom. Signed add
    /// and sub overflow in the direction of the left operand's sign — the two
    /// operands have to agree in sign for add to overflow at all, and disagree
    /// for sub — and a signed product runs off the end its own sign points to.
    fn emit_saturating(
        builder: &mut ClifFunctionBuilder,
        op: BinOp,
        lhs: Value,
        rhs: Value,
        ty: Type,
        is_unsigned: bool,
    ) -> Value {
        let (wrapped, overflowed) = match (op, is_unsigned) {
            (BinOp::SaturatingAdd, true) => builder.ins().uadd_overflow(lhs, rhs),
            (BinOp::SaturatingAdd, false) => builder.ins().sadd_overflow(lhs, rhs),
            (BinOp::SaturatingSub, true) => builder.ins().usub_overflow(lhs, rhs),
            (BinOp::SaturatingSub, false) => builder.ins().ssub_overflow(lhs, rhs),
            (_, true) => builder.ins().umul_overflow(lhs, rhs),
            (_, false) => builder.ins().smul_overflow(lhs, rhs),
        };

        let limit = if is_unsigned {
            match op {
                BinOp::SaturatingSub => Self::iconst_at(builder, ty, 0),
                _ => Self::emit_type_max(builder, ty, true),
            }
        } else {
            let max = Self::emit_type_max(builder, ty, false);
            let min = Self::emit_type_min(builder, ty);
            let zero = Self::iconst_at(builder, ty, 0);
            let negative = match op {
                BinOp::SaturatingMul => {
                    // The product's sign is the operands' signs XORed.
                    let l = builder.ins().icmp(IntCC::SignedLessThan, lhs, zero);
                    let r = builder.ins().icmp(IntCC::SignedLessThan, rhs, zero);
                    builder.ins().bxor(l, r)
                }
                // Add overflows away from zero, sub away from the right
                // operand — either way the left operand's sign says which end.
                _ => builder.ins().icmp(IntCC::SignedLessThan, lhs, zero),
            };
            builder.ins().select(negative, min, max)
        };

        builder.ins().select(overflowed, limit, wrapped)
    }

    /// The maximum of an integer type, at that type's own width. Unsigned
    /// maxima are all-ones, which `iconst` takes as the same bit pattern as -1
    /// at that width.
    fn emit_type_max(builder: &mut ClifFunctionBuilder, ty: Type, unsigned: bool) -> Value {
        if unsigned {
            return Self::iconst_at(builder, ty, -1);
        }
        let n: i128 = match ty.bits() {
            8 => i8::MAX as i128,
            16 => i16::MAX as i128,
            32 => i32::MAX as i128,
            64 => i64::MAX as i128,
            _ => i128::MAX,
        };
        Self::iconst_at(builder, ty, n)
    }

    /// The signed minimum of an integer type, at that type's own width.
    ///
    /// The old version capped at `i64::MIN`, which is both the wrong number for
    /// a 128-bit type and — through `iconst` — an instruction Cranelift has no
    /// rule for. `let a: i128 = 5` and then `-a` was enough to panic the
    /// compiler (#832).
    fn emit_type_min(builder: &mut ClifFunctionBuilder, ty: Type) -> Value {
        let n: i128 = match ty.bits() {
            8 => i8::MIN as i128,
            16 => i16::MIN as i128,
            32 => i32::MIN as i128,
            64 => i64::MIN as i128,
            _ => i128::MIN,
        };
        Self::iconst_at(builder, ty, n)
    }

    fn lower_operand_typed(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        expected_ty: Option<Type>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        match op {
            MirOperand::Local(local_id) => {
                let var = ctx.var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Local not found".to_string()))?;
                let val = builder.use_var(*var);
                // Widen to expected type if needed (e.g., i32 local used where i64 expected)
                if let Some(exp_ty) = expected_ty {
                    let actual_ty = builder.func.dfg.value_type(val);
                    if actual_ty != exp_ty && actual_ty.is_int() && exp_ty.is_int() {
                        // Cranelift's integer types carry no signedness, so
                        // `convert_value` widens by sign-extending. An unsigned
                        // value has to zero-extend or its top bit reads as a
                        // sign — `u8` 200 arrived as -56 (#326).
                        let is_unsigned = ctx.locals.iter()
                            .find(|l| l.id == *local_id)
                            .map_or(false, |l| matches!(l.ty,
                                MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64));
                        if is_unsigned && exp_ty.bits() > actual_ty.bits() {
                            return Ok(builder.ins().uextend(exp_ty, val));
                        }
                        return Ok(Self::convert_value(builder, val, actual_ty, exp_ty, None));
                    }
                }
                Ok(val)
            }

            MirOperand::Constant(const_val) => {
                match const_val {
                    MirConst::Int(n) => {
                        let ty = expected_ty.unwrap_or(types::I64);
                        if ty == types::I128 {
                            // `iconst` has no I128 form. A constant that got
                            // here as a 64-bit one is in a 128-bit slot because
                            // something widened it, and only a signed operand
                            // reaches this path — MIR emits `Int128` where the
                            // signedness mattered.
                            return Ok(Self::iconst_i128(builder, *n as i128));
                        }
                        Ok(builder.ins().iconst(ty, *n))
                    }
                    MirConst::Int128(n) => Ok(Self::iconst_i128(builder, *n)),
                    MirConst::Float(f) => {
                        // Only use expected_ty if it's a float type; ignore int expected types
                        let ty = match expected_ty {
                            Some(t) if t.is_float() => t,
                            _ => types::F64,
                        };
                        if ty == types::F32 {
                            Ok(builder.ins().f32const(*f as f32))
                        } else {
                            Ok(builder.ins().f64const(*f))
                        }
                    }
                    MirConst::Bool(b) => {
                        let ty = if matches!(expected_ty, Some(t) if t.is_int() && t != types::I8) {
                            expected_ty.unwrap()
                        } else {
                            types::I8
                        };
                        Ok(builder.ins().iconst(ty, if *b { 1 } else { 0 }))
                    }
                    MirConst::Char(c) => {
                        Ok(builder.ins().iconst(types::I32, *c as i64))
                    }
                    MirConst::String(s) => {
                        // String constants: allocate a 16-byte stack slot,
                        // get raw char* from data section, call rask_string_from(out, cstr).
                        if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                            let raw_ptr = builder.ins().global_value(types::I64, *gv);
                            let tmp_slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot, 16, 0,
                            ));
                            let out_ptr = builder.ins().stack_addr(types::I64, tmp_slot, 0);
                            if let Some(string_from_ref) = ctx.func_refs.get("string_from") {
                                builder.ins().call(*string_from_ref, &[out_ptr, raw_ptr]);
                                Ok(out_ptr)
                            } else {
                                return Err(CodegenError::FunctionNotFound("string_from".to_string()))
                            }
                        } else {
                            // Empty string: SSO with remaining=15
                            let tmp_slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot, 16, 0,
                            ));
                            let lo = builder.ins().iconst(types::I64, crate::layouts::EMPTY_STRING_LO);
                            builder.ins().stack_store(lo, tmp_slot, 0);
                            let hi = builder.ins().iconst(types::I64, crate::layouts::EMPTY_STRING_HI);
                            builder.ins().stack_store(hi, tmp_slot, 8);
                            let out_ptr = builder.ins().stack_addr(types::I64, tmp_slot, 0);
                            Ok(out_ptr)
                        }
                    }
                }
            }
        }
    }
}

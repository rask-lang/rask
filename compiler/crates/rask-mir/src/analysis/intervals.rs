// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Interval analysis — demand-driven value range propagation (comp.advanced IV1-IV7).
//!
//! Forward dataflow analysis that tracks integer value ranges [lo, hi] per local.
//! Used by bounds check elimination to prove indices are in-bounds.
//!
//! Design: lazy evaluation (IV1) — only runs on functions with indexing ops.
//! Per-function, no interprocedural (IV7). Widen at loop headers (IV5).

use std::collections::{HashMap, HashSet};

use crate::analysis::dataflow::{self, DataflowAnalysis, DataflowResults, Direction};
use crate::analysis::dominators::DominatorTree;
use crate::analysis::loops;
use crate::{
    BlockId, LocalId, MirBlock, MirFunction, MirOperand, MirConst,
    MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use crate::operand::BinOp;

/// An integer interval [lo, hi] (inclusive on both ends).
/// Unbounded represented by i64::MIN / i64::MAX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub lo: i64,
    pub hi: i64,
}

impl Interval {
    pub const TOP: Self = Self { lo: i64::MIN, hi: i64::MAX };
    pub const BOTTOM: Self = Self { lo: i64::MAX, hi: i64::MIN };

    pub fn constant(v: i64) -> Self {
        Self { lo: v, hi: v }
    }

    pub fn new(lo: i64, hi: i64) -> Self {
        Self { lo, hi }
    }

    /// True if this interval is empty (bottom).
    pub fn is_bottom(&self) -> bool {
        self.lo > self.hi
    }

    /// True if this interval is the full range.
    pub fn is_top(&self) -> bool {
        self.lo == i64::MIN && self.hi == i64::MAX
    }

    /// Union of two intervals (smallest interval containing both).
    pub fn union(self, other: Self) -> Self {
        if self.is_bottom() { return other; }
        if other.is_bottom() { return self; }
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// Intersection of two intervals.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            lo: self.lo.max(other.lo),
            hi: self.hi.min(other.hi),
        }
    }

    /// Does this interval prove 0 <= x < bound?
    pub fn provably_in_bounds(&self, bound: &Interval) -> bool {
        if self.is_bottom() || bound.is_bottom() {
            return false;
        }
        // Need: lo >= 0 AND hi < bound.lo (the minimum possible length)
        self.lo >= 0 && bound.lo > 0 && self.hi < bound.lo
    }

    /// Saturating add of two intervals.
    pub fn add(self, other: Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Self::BOTTOM;
        }
        Self {
            lo: self.lo.saturating_add(other.lo),
            hi: self.hi.saturating_add(other.hi),
        }
    }

    /// Saturating subtract.
    pub fn sub(self, other: Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Self::BOTTOM;
        }
        Self {
            lo: self.lo.saturating_sub(other.hi),
            hi: self.hi.saturating_sub(other.lo),
        }
    }

    /// Multiply (conservative for mixed-sign).
    pub fn mul(self, other: Self) -> Self {
        if self.is_bottom() || other.is_bottom() {
            return Self::BOTTOM;
        }
        let products = [
            self.lo.saturating_mul(other.lo),
            self.lo.saturating_mul(other.hi),
            self.hi.saturating_mul(other.lo),
            self.hi.saturating_mul(other.hi),
        ];
        Self {
            lo: *products.iter().min().unwrap(),
            hi: *products.iter().max().unwrap(),
        }
    }
}

/// Metadata about a comparison that produced a boolean value.
#[derive(Debug, Clone)]
struct ComparisonInfo {
    op: BinOp,
    left: LocalId,
    right: LocalId,
}

impl PartialEq for ComparisonInfo {
    fn eq(&self, other: &Self) -> bool {
        // BinOp doesn't derive Eq — compare via discriminant
        std::mem::discriminant(&self.op) == std::mem::discriminant(&other.op)
            && self.left == other.left
            && self.right == other.right
    }
}
impl Eq for ComparisonInfo {}

/// Per-function interval map: local → interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntervalDomain {
    pub ranges: HashMap<LocalId, Interval>,
    /// Tracks which comparison produced each boolean local (for branch narrowing).
    comparisons: HashMap<LocalId, ComparisonInfo>,
    /// Relational constraints: (a, b) means a < b is known to hold.
    /// Survives widening better than absolute intervals.
    pub known_lt: HashSet<(LocalId, LocalId)>,
}

impl IntervalDomain {
    pub fn new() -> Self {
        Self { ranges: HashMap::new(), comparisons: HashMap::new(), known_lt: HashSet::new() }
    }

    pub fn get(&self, local: LocalId) -> Interval {
        self.ranges.get(&local).copied().unwrap_or(Interval::BOTTOM)
    }

    pub fn set(&mut self, local: LocalId, interval: Interval) {
        self.ranges.insert(local, interval);
    }
}

/// The interval analysis implementation.
pub struct IntervalAnalysis {
    /// Integer locals we care about.
    int_locals: HashSet<LocalId>,
    /// Loop header blocks (for targeted widening — future refinement).
    _loop_headers: HashSet<BlockId>,
    /// Locals that are known Vec/array lengths (from Vec_len calls).
    pub len_locals: HashMap<LocalId, LenSource>,
    /// Initial state: integer parameters set to TOP.
    init_state: IntervalDomain,
}

/// Where a length value came from.
#[derive(Debug, Clone)]
pub struct LenSource {
    /// The collection local whose length this is.
    pub collection: LocalId,
}

/// An indexing operation found in MIR.
#[derive(Debug, Clone)]
pub struct IndexOp {
    /// Block containing this index.
    pub block: BlockId,
    /// Statement index within the block.
    pub stmt_idx: usize,
    /// The index operand local.
    pub index: LocalId,
    /// The collection being indexed.
    pub collection: LocalId,
    /// The length local for this collection (if found).
    pub len_local: Option<LocalId>,
    /// Span of the indexing operation.
    pub span: crate::Span,
    /// Whether this is a Vec_get (vs ArrayIndex).
    pub is_vec_get: bool,
}

impl IntervalAnalysis {
    /// Build the analysis from a function. Returns None if no indexing ops exist (IV1: lazy).
    pub fn from_function(func: &MirFunction) -> Option<(Self, Vec<IndexOp>)> {
        let mut int_locals = HashSet::new();
        let mut len_locals = HashMap::new();
        let mut index_ops = Vec::new();

        // Identify integer locals
        for local in &func.locals {
            if is_integer_type(&local.ty) {
                int_locals.insert(local.id);
            }
        }

        // Scan for Vec_len calls and indexing operations
        for block in &func.blocks {
            for (stmt_idx, stmt) in block.statements.iter().enumerate() {
                match &stmt.kind {
                    MirStmtKind::Call { dst: Some(dst), func: fref, args } => {
                        if fref.name == "Vec_len" || fref.name == "Array_len" {
                            if let Some(MirOperand::Local(collection)) = args.first() {
                                len_locals.insert(*dst, LenSource { collection: *collection });
                            }
                        }
                        // Vec_get(collection, index) — the indexing operation
                        if fref.name == "Vec_get" || fref.name == "Vec_index" {
                            if args.len() >= 2 {
                                if let (Some(MirOperand::Local(collection)), Some(idx_op)) =
                                    (args.first(), args.get(1))
                                {
                                    if let MirOperand::Local(idx_local) = idx_op {
                                        // Find length local for this collection
                                        let len_local = len_locals.iter()
                                            .find(|(_, src)| src.collection == *collection)
                                            .map(|(id, _)| *id);
                                        index_ops.push(IndexOp {
                                            block: block.id,
                                            stmt_idx,
                                            index: *idx_local,
                                            collection: *collection,
                                            len_local,
                                            span: stmt.span,
                                            is_vec_get: true,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    MirStmtKind::Assign { dst: _, rvalue: MirRValue::ArrayIndex { base, index, .. } } => {
                        if let MirOperand::Local(idx_local) = index {
                            let collection = match base {
                                MirOperand::Local(l) => *l,
                                _ => continue,
                            };
                            index_ops.push(IndexOp {
                                block: block.id,
                                stmt_idx,
                                index: *idx_local,
                                collection,
                                len_local: None,
                                span: stmt.span,
                                is_vec_get: false,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        // IV1: Only run analysis if there are indexing operations
        if index_ops.is_empty() {
            return None;
        }

        // Detect loop headers for widening (IV5)
        let dom_tree = DominatorTree::build(func);
        let natural_loops = loops::detect_loops(func, &dom_tree);
        let loop_headers: HashSet<BlockId> = natural_loops.iter()
            .map(|l| l.header)
            .collect();

        // Parameters start as TOP (unknown range)
        let mut init_state = IntervalDomain::new();
        for local in &func.locals {
            if local.is_param && int_locals.contains(&local.id) {
                init_state.set(local.id, Interval::TOP);
            }
        }

        Some((
            Self { int_locals, _loop_headers: loop_headers, len_locals, init_state },
            index_ops,
        ))
    }
}

impl DataflowAnalysis for IntervalAnalysis {
    type Domain = IntervalDomain;

    fn direction(&self) -> Direction {
        Direction::Forward
    }

    fn bottom(&self) -> IntervalDomain {
        // Seed with parameter intervals (TOP for integer params).
        // For unreachable blocks, missing locals default to BOTTOM via get().
        self.init_state.clone()
    }

    fn join(&self, a: &IntervalDomain, b: &IntervalDomain) -> IntervalDomain {
        let mut result = IntervalDomain::new();
        let all_keys: HashSet<_> = a.ranges.keys().chain(b.ranges.keys()).collect();
        for &key in &all_keys {
            let ia = a.get(*key);
            let ib = b.get(*key);
            result.set(*key, ia.union(ib));
        }
        // Relational constraints: only keep what both branches agree on
        result.known_lt = a.known_lt.intersection(&b.known_lt).copied().collect();
        result
    }

    fn transfer_block(&self, block: &MirBlock, in_state: &IntervalDomain) -> IntervalDomain {
        let mut state = in_state.clone();

        for stmt in &block.statements {
            transfer_stmt(stmt, &mut state, &self.int_locals, &self.len_locals);
        }

        state
    }

    fn widen(&self, old: &IntervalDomain, new: &IntervalDomain) -> IntervalDomain {
        // Only widen at loop headers — checked by the framework at block granularity.
        // For non-loop blocks, return new unchanged.
        // The framework calls widen(old_exit, new_exit) for every block.
        // We widen conservatively: if a bound is moving, push to infinity.
        let mut result = new.clone();
        for (&local, new_iv) in &new.ranges {
            if let Some(old_iv) = old.ranges.get(&local) {
                if old_iv != new_iv && !old_iv.is_bottom() {
                    let lo = if new_iv.lo < old_iv.lo { i64::MIN } else { new_iv.lo };
                    let hi = if new_iv.hi > old_iv.hi { i64::MAX } else { new_iv.hi };
                    result.set(local, Interval::new(lo, hi));
                }
            }
        }
        result
    }

    fn transfer_edge(
        &self,
        _from: BlockId,
        to: BlockId,
        terminator: &MirTerminator,
        exit_state: &IntervalDomain,
    ) -> IntervalDomain {
        // IV4: Conditional narrowing after branch.
        let MirTerminatorKind::Branch { cond: MirOperand::Local(cond_local), then_block, else_block } = &terminator.kind else {
            return exit_state.clone();
        };

        let Some(cmp) = exit_state.comparisons.get(cond_local) else {
            return exit_state.clone();
        };

        let left_iv = exit_state.get(cmp.left);
        let right_iv = exit_state.get(cmp.right);
        let is_true_branch = to == *then_block;
        let is_false_branch = to == *else_block;

        if !is_true_branch && !is_false_branch {
            return exit_state.clone();
        }

        let mut narrowed = exit_state.clone();

        // Narrow based on comparison op and branch direction.
        // true branch of (left < right)  → left in [left.lo, min(left.hi, right.hi - 1)]
        //                                   right in [max(right.lo, left.lo + 1), right.hi]
        // false branch of (left < right) → left >= right
        //                                   left in [max(left.lo, right.lo), left.hi]
        match (cmp.op, is_true_branch) {
            (BinOp::Lt, true) => {
                // left < right is true
                narrowed.known_lt.insert((cmp.left, cmp.right));
                if !right_iv.is_bottom() && right_iv.hi != i64::MIN {
                    let new_left = left_iv.intersect(Interval::new(i64::MIN, right_iv.hi.saturating_sub(1)));
                    if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
                }
                if !left_iv.is_bottom() {
                    let new_right = right_iv.intersect(Interval::new(left_iv.lo.saturating_add(1), i64::MAX));
                    if !new_right.is_bottom() { narrowed.set(cmp.right, new_right); }
                }
            }
            (BinOp::Lt, false) => {
                // left >= right
                let new_left = left_iv.intersect(Interval::new(right_iv.lo, i64::MAX));
                if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
            }
            (BinOp::Le, true) => {
                // left <= right
                let new_left = left_iv.intersect(Interval::new(i64::MIN, right_iv.hi));
                if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
            }
            (BinOp::Le, false) => {
                // left > right → right < left
                narrowed.known_lt.insert((cmp.right, cmp.left));
                if !right_iv.is_bottom() {
                    let new_left = left_iv.intersect(Interval::new(right_iv.lo.saturating_add(1), i64::MAX));
                    if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
                }
            }
            (BinOp::Gt, true) => {
                // left > right → same as right < left
                narrowed.known_lt.insert((cmp.right, cmp.left));
                if !right_iv.is_bottom() {
                    let new_left = left_iv.intersect(Interval::new(right_iv.lo.saturating_add(1), i64::MAX));
                    if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
                }
            }
            (BinOp::Gt, false) => {
                // left <= right
                let new_left = left_iv.intersect(Interval::new(i64::MIN, right_iv.hi));
                if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
            }
            (BinOp::Ge, true) => {
                // left >= right
                let new_left = left_iv.intersect(Interval::new(right_iv.lo, i64::MAX));
                if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
            }
            (BinOp::Ge, false) => {
                // left < right
                narrowed.known_lt.insert((cmp.left, cmp.right));
                if !right_iv.is_bottom() && right_iv.hi != i64::MIN {
                    let new_left = left_iv.intersect(Interval::new(i64::MIN, right_iv.hi.saturating_sub(1)));
                    if !new_left.is_bottom() { narrowed.set(cmp.left, new_left); }
                }
            }
            _ => {}
        }

        narrowed
    }
}

/// Apply a single statement's effect on intervals.
fn transfer_stmt(
    stmt: &MirStmt,
    state: &mut IntervalDomain,
    int_locals: &HashSet<LocalId>,
    len_locals: &HashMap<LocalId, LenSource>,
) {
    match &stmt.kind {
        MirStmtKind::Assign { dst, rvalue } => {
            // Track comparisons for branch narrowing (IV4)
            if let MirRValue::BinaryOp { op, left: MirOperand::Local(l), right: MirOperand::Local(r) } = rvalue {
                if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
                    state.comparisons.insert(*dst, ComparisonInfo {
                        op: *op,
                        left: *l,
                        right: *r,
                    });
                }
            }
            // Invalidate relational constraints involving the reassigned local
            state.known_lt.retain(|&(a, b)| a != *dst && b != *dst);
            if !int_locals.contains(dst) {
                return;
            }
            let interval = eval_rvalue(rvalue, state, len_locals);
            state.set(*dst, interval);
        }
        MirStmtKind::Call { dst: Some(dst), func, args: _ } => {
            state.known_lt.retain(|&(a, b)| a != *dst && b != *dst);
            if !int_locals.contains(dst) {
                return;
            }
            // Vec_len returns a non-negative value
            if func.name == "Vec_len" || func.name == "Array_len" || func.name == "Pool_len" {
                state.set(*dst, Interval::new(0, i64::MAX));
            } else {
                // Unknown call — result is TOP
                state.set(*dst, Interval::TOP);
            }
        }
        MirStmtKind::Phi { dst, args } => {
            state.known_lt.retain(|&(a, b)| a != *dst && b != *dst);
            if !int_locals.contains(dst) {
                return;
            }
            let mut result = Interval::BOTTOM;
            for (_, op) in args {
                let iv = operand_interval(op, state);
                result = result.union(iv);
            }
            state.set(*dst, result);
        }
        _ => {}
    }
}

/// Evaluate an rvalue to an interval.
fn eval_rvalue(
    rvalue: &MirRValue,
    state: &IntervalDomain,
    _len_locals: &HashMap<LocalId, LenSource>,
) -> Interval {
    match rvalue {
        MirRValue::Use(op) => operand_interval(op, state),
        MirRValue::BinaryOp { op, left, right } => {
            let l = operand_interval(left, state);
            let r = operand_interval(right, state);
            match op {
                BinOp::Add => l.add(r),
                BinOp::Sub => l.sub(r),
                BinOp::Mul => l.mul(r),
                // Comparison results are boolean (0 or 1)
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                | BinOp::Eq | BinOp::Ne => Interval::new(0, 1),
                // Bitwise and shift — conservatively TOP
                _ => Interval::TOP,
            }
        }
        MirRValue::Cast { value, target_ty } => {
            let iv = operand_interval(value, state);
            // Narrowing casts can change bounds
            match target_ty {
                MirType::U8 => Interval::new(iv.lo.max(0), iv.hi.min(255)),
                MirType::U16 => Interval::new(iv.lo.max(0), iv.hi.min(65535)),
                MirType::U32 => Interval::new(iv.lo.max(0), iv.hi.min(u32::MAX as i64)),
                MirType::I8 => Interval::new(iv.lo.max(-128), iv.hi.min(127)),
                MirType::I16 => Interval::new(iv.lo.max(-32768), iv.hi.min(32767)),
                MirType::I32 => Interval::new(iv.lo.max(i32::MIN as i64), iv.hi.min(i32::MAX as i64)),
                _ => iv,
            }
        }
        MirRValue::UnaryOp { op: crate::operand::UnaryOp::Neg, operand } => {
            let iv = operand_interval(operand, state);
            if iv.is_bottom() { return Interval::BOTTOM; }
            Interval::new(iv.hi.saturating_neg(), iv.lo.saturating_neg())
        }
        _ => Interval::TOP,
    }
}

/// Get the interval for an operand.
fn operand_interval(op: &MirOperand, state: &IntervalDomain) -> Interval {
    match op {
        MirOperand::Local(id) => state.get(*id),
        MirOperand::Constant(c) => match c {
            MirConst::Int(v) => Interval::constant(*v),
            MirConst::Bool(b) => Interval::constant(*b as i64),
            _ => Interval::TOP,
        },
    }
}

fn is_integer_type(ty: &MirType) -> bool {
    matches!(ty,
        MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
        | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
    )
}

/// Run interval analysis on a function.
/// Returns None if no indexing operations exist (IV1: demand-driven).
pub fn analyze(func: &MirFunction) -> Option<(IntervalAnalysis, Vec<IndexOp>, DataflowResults<IntervalDomain>)> {
    let (analysis, index_ops) = IntervalAnalysis::from_function(func)?;

    let dom_tree = DominatorTree::build(func);

    // The solver initializes entry[entry_block] to bottom(). We need parameters
    // to start as TOP (unknown). We solve, then patch the entry block's initial
    // state and re-solve. Instead, we set it up by making from_function produce
    // an init_state that gets used as the entry block's seed.
    //
    // The dataflow framework seeds entry[entry_block] = bottom(). But for a
    // forward analysis the entry block is never joined from predecessors —
    // it keeps its initial state. We work around this by running the solver
    // and then checking results, since the entry block's transfer will produce
    // correct intervals for assignments in block 0.
    //
    // Note: parameters remain BOTTOM (unreachable/unknown) in the entry state,
    // but transfer_stmt sets them to TOP when they first appear in operations.
    // This is conservative and correct.
    let results = dataflow::solve(func, &analysis, &dom_tree);

    Some((analysis, index_ops, results))
}

/// Check if an index operation is provably in-bounds.
pub fn is_in_bounds(
    func: &MirFunction,
    analysis: &IntervalAnalysis,
    results: &DataflowResults<IntervalDomain>,
    op: &IndexOp,
) -> bool {
    // Get the block containing this index operation
    let block = func.blocks.iter().find(|b| b.id == op.block).unwrap();

    // Get interval state at the point of indexing
    let state = results.state_at_statement(analysis, block, op.stmt_idx);

    let index_interval = state.get(op.index);

    // For Vec_get: check against the length local's interval
    if let Some(len_local) = op.len_local {
        let len_interval = state.get(len_local);
        // Absolute interval proof: index.hi < len.lo
        if index_interval.provably_in_bounds(&len_interval) {
            return true;
        }
        // Relational proof: known index < len AND index >= 0
        if index_interval.lo >= 0 && state.known_lt.contains(&(op.index, len_local)) {
            return true;
        }
        return false;
    }

    // For fixed-size arrays, we'd need the array length from the type.
    // Conservative: can't prove in-bounds without length info.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::{MirBlock, MirLocal};
    use crate::{MirTerminator, MirTerminatorKind, MirType, FunctionRef};

    fn block(n: u32) -> BlockId { BlockId(n) }
    fn local(n: u32) -> LocalId { LocalId(n) }
    fn sp() -> crate::Span { crate::Span { start: 0, end: 0 } }

    fn int_local(id: u32, name: &str) -> MirLocal {
        MirLocal { id: local(id), name: Some(name.into()), ty: MirType::I64, is_param: false }
    }

    fn assign_const(dst: u32, val: i64) -> MirStmt {
        MirStmt {
            kind: MirStmtKind::Assign {
                dst: local(dst),
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(val))),
            },
            span: sp(),
        }
    }

    fn assign_add(dst: u32, left: u32, right_val: i64) -> MirStmt {
        MirStmt {
            kind: MirStmtKind::Assign {
                dst: local(dst),
                rvalue: MirRValue::BinaryOp {
                    op: BinOp::Add,
                    left: MirOperand::Local(local(left)),
                    right: MirOperand::Constant(MirConst::Int(right_val)),
                },
            },
            span: sp(),
        }
    }

    fn call_vec_len(dst: u32, vec_local: u32) -> MirStmt {
        MirStmt {
            kind: MirStmtKind::Call {
                dst: Some(local(dst)),
                func: FunctionRef::internal("Vec_len".into()),
                args: vec![MirOperand::Local(local(vec_local))],
            },
            span: sp(),
        }
    }

    fn call_vec_get(dst: u32, vec_local: u32, idx_local: u32) -> MirStmt {
        MirStmt {
            kind: MirStmtKind::Call {
                dst: Some(local(dst)),
                func: FunctionRef::internal("Vec_get".into()),
                args: vec![
                    MirOperand::Local(local(vec_local)),
                    MirOperand::Local(local(idx_local)),
                ],
            },
            span: sp(),
        }
    }

    fn cmp_lt(dst: u32, left: u32, right: u32) -> MirStmt {
        MirStmt {
            kind: MirStmtKind::Assign {
                dst: local(dst),
                rvalue: MirRValue::BinaryOp {
                    op: BinOp::Lt,
                    left: MirOperand::Local(local(left)),
                    right: MirOperand::Local(local(right)),
                },
            },
            span: sp(),
        }
    }

    fn term_goto(target: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Goto { target: block(target) })
    }

    fn term_branch(cond: u32, then_b: u32, else_b: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(local(cond)),
            then_block: block(then_b),
            else_block: block(else_b),
        })
    }

    fn term_ret() -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: None })
    }

    #[test]
    fn interval_arithmetic() {
        let a = Interval::new(1, 5);
        let b = Interval::new(2, 3);
        assert_eq!(a.add(b), Interval::new(3, 8));
        assert_eq!(a.sub(b), Interval::new(-2, 3));
        assert_eq!(a.mul(b), Interval::new(2, 15));
    }

    #[test]
    fn interval_union_and_intersect() {
        let a = Interval::new(0, 5);
        let b = Interval::new(3, 10);
        assert_eq!(a.union(b), Interval::new(0, 10));
        assert_eq!(a.intersect(b), Interval::new(3, 5));
    }

    #[test]
    fn provably_in_bounds_check() {
        // i in [0, 4], len in [5, 100] → provably in bounds
        assert!(Interval::new(0, 4).provably_in_bounds(&Interval::new(5, 100)));
        // i in [0, 5], len in [5, 100] → NOT provably in bounds (i could be 5)
        assert!(!Interval::new(0, 5).provably_in_bounds(&Interval::new(5, 100)));
        // i in [-1, 4], len in [5, 100] → NOT in bounds (negative)
        assert!(!Interval::new(-1, 4).provably_in_bounds(&Interval::new(5, 100)));
    }

    #[test]
    fn constant_index_in_bounds() {
        // Simulates: len = vec.len(); x = vec[0]
        // _0: vec (param), _1: len, _2: result, _3: idx
        let func = MirFunction {
            name: "test".into(),
            params: vec![
                MirLocal { id: local(0), name: Some("vec".into()), ty: MirType::Ptr, is_param: true },
            ],
            ret_ty: MirType::I64,
            locals: vec![
                MirLocal { id: local(0), name: Some("vec".into()), ty: MirType::Ptr, is_param: true },
                int_local(1, "len"),
                int_local(2, "result"),
                int_local(3, "idx"),
            ],
            blocks: vec![
                MirBlock {
                    id: block(0),
                    statements: vec![
                        call_vec_len(1, 0),
                        assign_const(3, 0),
                        call_vec_get(2, 0, 3),
                    ],
                    terminator: term_ret(),
                },
            ],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };

        let Some((analysis, ops, results)) = analyze(&func) else {
            panic!("expected analysis to run");
        };
        assert_eq!(ops.len(), 1);

        // idx=0, len from Vec_len is [0, MAX]. idx [0,0] < len [0, MAX]?
        // provably_in_bounds needs len.lo > 0, but Vec_len returns [0, MAX].
        // So a constant index 0 is NOT provably in bounds (vec could be empty).
        assert!(!is_in_bounds(&func, &analysis, &results, &ops[0]));
    }

    /// The canonical loop pattern: for i in 0..len { vec[i] }
    /// After the branch `i < len` into the body, i is in [0, len-1].
    #[test]
    fn loop_index_pattern() {
        // _0: vec (param)
        // _1: len = Vec_len(vec)
        // _2: i (counter, starts at 0)
        // _3: cond = i < len
        // _4: result = Vec_get(vec, i)
        // _5: i_next = i + 1

        let func = MirFunction {
            name: "loop_test".into(),
            params: vec![
                MirLocal { id: local(0), name: Some("vec".into()), ty: MirType::Ptr, is_param: true },
            ],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("vec".into()), ty: MirType::Ptr, is_param: true },
                int_local(1, "len"),
                int_local(2, "i"),
                MirLocal { id: local(3), name: Some("cond".into()), ty: MirType::Bool, is_param: false },
                int_local(4, "result"),
                int_local(5, "i_next"),
            ],
            blocks: vec![
                // Block 0: init
                MirBlock {
                    id: block(0),
                    statements: vec![
                        call_vec_len(1, 0),  // len = Vec_len(vec)
                        assign_const(2, 0),  // i = 0
                    ],
                    terminator: term_goto(1),
                },
                // Block 1: loop header — check i < len
                MirBlock {
                    id: block(1),
                    statements: vec![
                        cmp_lt(3, 2, 1),  // cond = i < len
                    ],
                    terminator: term_branch(3, 2, 3), // if cond goto body else exit
                },
                // Block 2: loop body — vec[i], i += 1
                MirBlock {
                    id: block(2),
                    statements: vec![
                        call_vec_get(4, 0, 2),     // result = Vec_get(vec, i)
                        assign_add(5, 2, 1),       // i_next = i + 1
                        MirStmt {                   // i = i_next (copy)
                            kind: MirStmtKind::Assign {
                                dst: local(2),
                                rvalue: MirRValue::Use(MirOperand::Local(local(5))),
                            },
                            span: sp(),
                        },
                    ],
                    terminator: term_goto(1), // back to header
                },
                // Block 3: exit
                MirBlock {
                    id: block(3),
                    statements: vec![],
                    terminator: term_ret(),
                },
            ],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };

        let Some((analysis, ops, results)) = analyze(&func) else {
            panic!("expected analysis to run");
        };

        // Should find one indexing op (Vec_get in block 2)
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].block, block(2));
        assert_eq!(ops[0].index, local(2));

        // With transfer_edge narrowing (IV4):
        // Block 1 has `cond = i < len`, then branches to block 2 (true) / block 3 (false).
        // In block 2 (true branch), i is narrowed to [i.lo, len.hi - 1].
        // After widening, i is [0, MAX] in block 1. len is [0, MAX].
        // In the true branch: i narrowed to [0, MAX-1].
        // len narrowed to [1, MAX] (since i < len and i >= 0).
        let block2 = func.blocks.iter().find(|b| b.id == block(2)).unwrap();
        let state = results.state_at_statement(&analysis, block2, 0);
        let i_interval = state.get(local(2));
        let len_interval = state.get(local(1));

        // i starts at 0, widened at loop header, narrowed by i < len in true branch
        assert!(i_interval.lo >= 0, "i should be non-negative: {:?}", i_interval);
        // len narrowed by i < len: len.lo >= i.lo + 1 = 1
        assert!(len_interval.lo >= 1, "len in true branch should be >= 1: {:?}", len_interval);
        // Absolute intervals are too wide after widening (i.hi = MAX-1 vs len.lo = 1).
        // But relational constraint (i < len) survives widening.
        assert!(
            state.known_lt.contains(&(local(2), local(1))),
            "should have relational constraint i < len",
        );
        // Full is_in_bounds check uses both absolute + relational
        assert!(
            is_in_bounds(&func, &analysis, &results, &ops[0]),
            "loop index should be provably in bounds via relational constraint",
        );
    }

    #[test]
    fn no_analysis_without_indexing() {
        let func = MirFunction {
            name: "pure".into(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![int_local(0, "x")],
            blocks: vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 42)],
                    terminator: term_ret(),
                },
            ],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };
        assert!(analyze(&func).is_none(), "IV1: no analysis without indexing");
    }

    #[test]
    fn vec_len_is_non_negative() {
        let mut state = IntervalDomain::new();
        let len_locals = {
            let mut m = HashMap::new();
            m.insert(local(1), LenSource { collection: local(0) });
            m
        };
        let int_locals: HashSet<_> = [local(1)].into();

        let stmt = call_vec_len(1, 0);
        transfer_stmt(&stmt, &mut state, &int_locals, &len_locals);

        let len_iv = state.get(local(1));
        assert_eq!(len_iv.lo, 0);
        assert_eq!(len_iv.hi, i64::MAX);
    }
}

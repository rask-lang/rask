// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Handle typestate analysis (comp.advanced TS1-TS8, MA1-MA5, FN1-FN4).
//!
//! Tracks handle validity states through control flow to catch stale handle
//! access at compile time. Forward dataflow using the generic framework.
//!
//! States: Fresh > Valid > Unknown > Invalid (TS1)
//! Join: conservative minimum (TS2)
//! Alias tracking: copies share state transitions (MA1-MA5)

use std::collections::{HashMap, HashSet};

use crate::analysis::dataflow::{self, DataflowAnalysis, DataflowResults, Direction};
use crate::analysis::dominators::DominatorTree;
use crate::analysis::pool_ops;
use crate::analysis::uses;
use crate::{
    BlockId, LocalId, MirBlock, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind, MirType, Span,
};

// ── Handle State ────────────────────────────────────────────────────────

/// Handle validity state (TS1). Ordered for lattice join: lower = less information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HandleState {
    /// Provably removed — accessing is a compile error (TS8).
    Invalid = 0,
    /// Unknown validity — parameter default, or widened by structural mutation.
    Unknown = 1,
    /// Proven valid — successfully accessed or narrowed by check.
    Valid = 2,
    /// Just created by pool.insert() — valid and not yet aliased.
    Fresh = 3,
}

/// Per-handle tracking info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleInfo {
    pub state: HandleState,
    /// Which pool local this handle belongs to.
    pub pool: LocalId,
    /// Source span where this state was last set (for error messages).
    pub state_span: Span,
}

// ── Alias Tracking ──────────────────────────────────────────────────────

pub type AliasGroupId = u32;

/// Manages alias groups for must-alias tracking (MA1-MA5).
#[derive(Debug, Clone, PartialEq, Eq)]
struct AliasTracker {
    /// Local → group membership.
    local_to_group: HashMap<LocalId, AliasGroupId>,
    /// Group → member set.
    group_members: HashMap<AliasGroupId, HashSet<LocalId>>,
    next_id: AliasGroupId,
}

impl AliasTracker {
    fn new() -> Self {
        Self {
            local_to_group: HashMap::new(),
            group_members: HashMap::new(),
            next_id: 0,
        }
    }

    /// Create a fresh alias group for a single local (MA2).
    fn fresh_group(&mut self, local: LocalId) {
        self.remove(local);
        let gid = self.next_id;
        self.next_id += 1;
        self.local_to_group.insert(local, gid);
        self.group_members.insert(gid, HashSet::from([local]));
    }

    /// Make `dst` a must-alias of `src` (MA1: copy creates alias).
    fn add_alias(&mut self, dst: LocalId, src: LocalId) {
        self.remove(dst);
        if let Some(&gid) = self.local_to_group.get(&src) {
            self.local_to_group.insert(dst, gid);
            if let Some(members) = self.group_members.get_mut(&gid) {
                members.insert(dst);
            }
        }
    }

    /// Remove a local from its alias group (MA4: reassignment breaks alias).
    fn remove(&mut self, local: LocalId) {
        if let Some(gid) = self.local_to_group.remove(&local) {
            if let Some(members) = self.group_members.get_mut(&gid) {
                members.remove(&local);
                if members.is_empty() {
                    self.group_members.remove(&gid);
                }
            }
        }
    }

    /// Break all aliases for a local (MA3: function call breaks aliases).
    fn break_aliases(&mut self, local: LocalId) {
        self.remove(local);
    }

    /// Get all must-aliases of a local (including itself).
    fn aliases_of(&self, local: LocalId) -> Vec<LocalId> {
        if let Some(&gid) = self.local_to_group.get(&local) {
            if let Some(members) = self.group_members.get(&gid) {
                return members.iter().copied().collect();
            }
        }
        vec![local]
    }

    /// Conservative join: intersect alias groups from two states.
    /// Only keep aliases that exist in both.
    fn join(&self, other: &Self) -> Self {
        let mut result = Self::new();
        result.next_id = self.next_id.max(other.next_id);

        // For each group in self, check if the same alias relationship
        // exists in other. Only keep relationships present in both.
        for (&local, &gid) in &self.local_to_group {
            if let (Some(self_members), Some(&other_gid)) =
                (self.group_members.get(&gid), other.local_to_group.get(&local))
            {
                if let Some(other_members) = other.group_members.get(&other_gid) {
                    // Keep members that are in the same group in both states
                    let common: HashSet<LocalId> = self_members
                        .intersection(other_members)
                        .copied()
                        .collect();
                    if common.len() > 1 {
                        let new_gid = result.next_id;
                        result.next_id += 1;
                        for &member in &common {
                            result.local_to_group.insert(member, new_gid);
                        }
                        result.group_members.insert(new_gid, common);
                    }
                }
            }
        }

        result
    }
}

// ── Typestate Domain ────────────────────────────────────────────────────

/// Per-function typestate: handle states + alias tracking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypestateDomain {
    /// Per-handle state. Only locals with MirType::Handle are tracked.
    pub handles: HashMap<LocalId, HandleInfo>,
    /// Must-alias tracking.
    aliases: AliasTracker,
}

impl TypestateDomain {
    fn empty() -> Self {
        Self {
            handles: HashMap::new(),
            aliases: AliasTracker::new(),
        }
    }

    /// Set state for a handle and all its must-aliases (TS3, TS4).
    fn set_state(&mut self, local: LocalId, state: HandleState, span: Span) {
        let aliases = self.aliases.aliases_of(local);
        for alias in aliases {
            if let Some(info) = self.handles.get_mut(&alias) {
                info.state = state;
                info.state_span = span;
            }
        }
    }

    /// Widen all handles for a specific pool to Unknown (TS5).
    fn widen_pool(&mut self, pool: LocalId, span: Span) {
        for info in self.handles.values_mut() {
            if info.pool == pool && info.state > HandleState::Unknown {
                info.state = HandleState::Unknown;
                info.state_span = span;
            }
        }
    }

    /// Conservative join (TS2): per-handle minimum state.
    fn join(&self, other: &Self) -> Self {
        let mut result = TypestateDomain::empty();
        result.aliases = self.aliases.join(&other.aliases);

        // Union of all tracked handles; min state where both exist
        let all_locals: HashSet<LocalId> = self
            .handles
            .keys()
            .chain(other.handles.keys())
            .copied()
            .collect();

        for local in all_locals {
            match (self.handles.get(&local), other.handles.get(&local)) {
                (Some(a), Some(b)) => {
                    // TS2: take the lower bound
                    let (state, span) = if a.state <= b.state {
                        (a.state, a.state_span)
                    } else {
                        (b.state, b.state_span)
                    };
                    result.handles.insert(
                        local,
                        HandleInfo {
                            state,
                            pool: a.pool,
                            state_span: span,
                        },
                    );
                }
                (Some(info), None) | (None, Some(info)) => {
                    // Handle only exists in one branch — treat as Unknown
                    result.handles.insert(
                        local,
                        HandleInfo {
                            state: HandleState::Unknown.min(info.state),
                            pool: info.pool,
                            state_span: info.state_span,
                        },
                    );
                }
                (None, None) => unreachable!(),
            }
        }

        result
    }
}

// ── Analysis Context ────────────────────────────────────────────────────

/// Context for running typestate analysis on a single function.
pub struct TypestateAnalysis {
    /// Locals with MirType::Handle that we track.
    pub handle_locals: HashSet<LocalId>,
    /// Locals used as pool references.
    pub pool_locals: HashSet<LocalId>,
    /// Initial state for the entry block (parameters → Unknown).
    pub init_state: TypestateDomain,
    /// Interprocedural summaries for called functions.
    pub summaries: HashMap<String, FunctionSummary>,
}

impl TypestateAnalysis {
    /// Build analysis context from a function.
    pub fn from_function(func: &MirFunction) -> Option<Self> {
        let all_locals: Vec<&MirLocal> =
            func.locals.iter().collect();

        let handle_locals: HashSet<LocalId> = all_locals
            .iter()
            .filter(|l| l.ty == MirType::Handle)
            .map(|l| l.id)
            .collect();

        if handle_locals.is_empty() {
            return None;
        }

        // Identify pool locals from PoolCheckedAccess statements
        let pool_locals: HashSet<LocalId> = func
            .blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|stmt| {
                if let MirStmtKind::PoolCheckedAccess { pool, .. } = &stmt.kind {
                    Some(*pool)
                } else if let MirStmtKind::Call { func: f, args, .. } = &stmt.kind {
                    if pool_ops::is_pool_mutator(&f.name) || pool_ops::is_safe_pool_call(&f.name) {
                        args.first().and_then(|a| {
                            if let MirOperand::Local(id) = a {
                                Some(*id)
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Build initial state: parameters that are handles start as Unknown (TS7)
        let mut init_state = TypestateDomain::empty();
        for local in &all_locals {
            if handle_locals.contains(&local.id) && local.is_param {
                // TS7: function parameters default to Unknown
                // Pool association inferred from usage (conservative: use first pool)
                let pool = pool_locals.iter().next().copied().unwrap_or(LocalId(0));
                init_state.handles.insert(
                    local.id,
                    HandleInfo {
                        state: HandleState::Unknown,
                        pool,
                        state_span: Span::new(0, 0),
                    },
                );
            }
        }

        Some(Self {
            handle_locals,
            pool_locals,
            init_state,
            summaries: HashMap::new(),
        })
    }
}

// ── Transfer Functions ──────────────────────────────────────────────────

/// Apply a single statement's effect on typestate.
pub fn transfer_stmt(
    stmt: &MirStmt,
    state: &mut TypestateDomain,
    handle_locals: &HashSet<LocalId>,
    pool_locals: &HashSet<LocalId>,
    summaries: &HashMap<String, FunctionSummary>,
) {
    let span = stmt.span;

    match &stmt.kind {
        // Pool_insert returns a fresh handle
        MirStmtKind::Call {
            dst: Some(dst),
            func,
            args,
        } if pool_ops::is_pool_grower(&func.name) => {
            if handle_locals.contains(dst) {
                let pool = args
                    .first()
                    .and_then(|a| match a {
                        MirOperand::Local(id) => Some(*id),
                        _ => None,
                    })
                    .unwrap_or(LocalId(0));

                state.handles.insert(
                    *dst,
                    HandleInfo {
                        state: HandleState::Fresh,
                        pool,
                        state_span: span,
                    },
                );
                state.aliases.fresh_group(*dst); // MA2
            }

            // Structural growth widens other handles on same pool (TS5)
            if let Some(MirOperand::Local(pool)) = args.first() {
                widen_pool_except(state, *pool, Some(*dst), span);
            }
        }

        // Pool_remove: invalidate handle + all aliases (TS4)
        MirStmtKind::Call { func, args, .. } if func.name == "Pool_remove" => {
            if let Some((_pool, handle)) = pool_ops::pool_remove_target(stmt) {
                state.set_state(handle, HandleState::Invalid, span);
            }
            // Also widen other handles on same pool (TS5)
            if let Some(MirOperand::Local(pool)) = args.first() {
                // Removed handle is already Invalid; widen others to Unknown
                let removed = args.get(1).and_then(|a| match a {
                    MirOperand::Local(id) => Some(*id),
                    _ => None,
                });
                widen_pool_except(state, *pool, removed, span);
            }
        }

        // Pool_clear/Pool_drain: invalidate ALL handles on that pool
        MirStmtKind::Call { func, args, .. }
            if func.name == "Pool_clear" || func.name == "Pool_drain" =>
        {
            if let Some(MirOperand::Local(pool)) = args.first() {
                for info in state.handles.values_mut() {
                    if info.pool == *pool {
                        info.state = HandleState::Invalid;
                        info.state_span = span;
                    }
                }
            }
        }

        // PoolCheckedAccess: narrows handle to Valid (FN2)
        MirStmtKind::PoolCheckedAccess { handle, pool, .. } => {
            if let Some(info) = state.handles.get(handle) {
                // Only narrow if not already Invalid (don't hide the error)
                if info.state != HandleState::Invalid {
                    let pool_id = *pool;
                    state.handles.insert(
                        *handle,
                        HandleInfo {
                            state: HandleState::Valid,
                            pool: pool_id,
                            state_span: span,
                        },
                    );
                }
            } else if handle_locals.contains(handle) {
                // First time seeing this handle — register it
                state.handles.insert(
                    *handle,
                    HandleInfo {
                        state: HandleState::Valid,
                        pool: *pool,
                        state_span: span,
                    },
                );
            }
        }

        // Handle copy: dst joins src's alias group, copies state (MA1)
        MirStmtKind::Assign {
            dst,
            rvalue: MirRValue::Use(MirOperand::Local(src)),
        } if handle_locals.contains(dst) && handle_locals.contains(src) => {
            if let Some(src_info) = state.handles.get(src).cloned() {
                state.handles.insert(*dst, src_info);
                state.aliases.add_alias(*dst, *src);
            }
        }

        // Phi node: joins states from multiple predecessors
        MirStmtKind::Phi { dst, args } if handle_locals.contains(dst) => {
            // Take the minimum state from all phi args
            let mut min_state = HandleState::Fresh;
            let mut min_span = Span::new(0, 0);
            let mut pool = LocalId(0);
            for (_, operand) in args {
                if let MirOperand::Local(src) = operand {
                    if let Some(info) = state.handles.get(src) {
                        if info.state < min_state {
                            min_state = info.state;
                            min_span = info.state_span;
                        }
                        pool = info.pool;
                    }
                }
            }
            state.handles.insert(
                *dst,
                HandleInfo {
                    state: min_state,
                    pool,
                    state_span: min_span,
                },
            );
            state.aliases.fresh_group(*dst);
        }

        // Handle reassignment from non-handle source (MA4)
        MirStmtKind::Assign { dst, .. } if handle_locals.contains(dst) => {
            state.aliases.remove(*dst);
            // Unknown until proven otherwise
            if let Some(info) = state.handles.get_mut(dst) {
                info.state = HandleState::Unknown;
                info.state_span = span;
            }
        }

        // Function call with handle args: break aliases (MA3) + apply summaries
        MirStmtKind::Call { func, args, .. }
            if !pool_ops::is_pool_mutator(&func.name)
                && !pool_ops::is_safe_pool_call(&func.name) =>
        {
            // Apply interprocedural summaries: if the callee invalidates a param,
            // mark the corresponding argument handle as Invalid.
            if let Some(summary) = summaries.get(&func.name) {
                for &param_idx in &summary.invalidated_params {
                    if let Some(MirOperand::Local(id)) = args.get(param_idx) {
                        if handle_locals.contains(id) {
                            state.set_state(*id, HandleState::Invalid, span);
                        }
                    }
                }
                for &param_idx in &summary.widened_params {
                    if let Some(MirOperand::Local(id)) = args.get(param_idx) {
                        if handle_locals.contains(id) {
                            state.set_state(*id, HandleState::Unknown, span);
                        }
                    }
                }
            }

            for arg in args {
                if let MirOperand::Local(id) = arg {
                    if handle_locals.contains(id) {
                        state.aliases.break_aliases(*id); // MA3
                    }
                }
            }
            // Unknown calls passing pool args widen that pool's handles
            for arg in args {
                if let MirOperand::Local(id) = arg {
                    if pool_locals.contains(id) {
                        state.widen_pool(*id, span);
                    }
                }
            }
        }

        // Closure calls: conservative, widen everything (can't know captures)
        MirStmtKind::ClosureCall { .. } => {
            for info in state.handles.values_mut() {
                if info.state > HandleState::Unknown {
                    info.state = HandleState::Unknown;
                    info.state_span = span;
                }
            }
        }

        _ => {}
    }
}

/// Widen all handles on a pool to Unknown, except one (TS5).
fn widen_pool_except(
    state: &mut TypestateDomain,
    pool: LocalId,
    except: Option<LocalId>,
    span: Span,
) {
    for (local, info) in state.handles.iter_mut() {
        if info.pool == pool && info.state > HandleState::Unknown {
            if except.is_none() || except != Some(*local) {
                info.state = HandleState::Unknown;
                info.state_span = span;
            }
        }
    }
}

// ── DataflowAnalysis impl ───────────────────────────────────────────────

impl DataflowAnalysis for TypestateAnalysis {
    type Domain = TypestateDomain;

    fn direction(&self) -> Direction {
        Direction::Forward
    }

    fn bottom(&self) -> TypestateDomain {
        self.init_state.clone()
    }

    fn join(&self, a: &TypestateDomain, b: &TypestateDomain) -> TypestateDomain {
        a.join(b)
    }

    fn transfer_block(&self, block: &MirBlock, in_state: &TypestateDomain) -> TypestateDomain {
        let mut state = in_state.clone();
        for stmt in &block.statements {
            transfer_stmt(stmt, &mut state, &self.handle_locals, &self.pool_locals, &self.summaries);
        }
        state
    }

    fn widen(&self, old: &TypestateDomain, new: &TypestateDomain) -> TypestateDomain {
        // FN4: At loop headers, widen Fresh/Valid back to Unknown to ensure convergence.
        // Detect widening need: if a handle went from Unknown→Valid or Unknown→Fresh,
        // that could cycle. Conservatively: if new differs from old and old was Unknown,
        // keep Unknown.
        let mut result = new.clone();
        for (local, new_info) in result.handles.iter_mut() {
            if let Some(old_info) = old.handles.get(local) {
                if old_info.state == HandleState::Unknown && new_info.state > HandleState::Unknown {
                    // FN4: Don't let loop iterations promote Unknown → Valid/Fresh
                    new_info.state = HandleState::Unknown;
                    new_info.state_span = old_info.state_span;
                }
            }
        }
        result
    }
}

/// Run typestate analysis on a function. Returns dataflow results if the
/// function uses handles, None otherwise.
pub fn analyze(func: &MirFunction) -> Option<(TypestateAnalysis, DataflowResults<TypestateDomain>)> {
    let analysis = TypestateAnalysis::from_function(func)?;
    let dom = DominatorTree::build(func);
    let results = dataflow::solve(func, &analysis, &dom);
    Some((analysis, results))
}

// ── Error Detection ─────────────────────────────────────────────────────

/// A detected typestate violation.
#[derive(Debug)]
pub struct TypestateError {
    /// Span of the stale access.
    pub access_span: Span,
    /// Span where the handle was invalidated.
    pub invalidation_span: Span,
    /// Name of the handle local (for error messages).
    pub handle_name: Option<String>,
    /// Whether this was detected via must-alias (h2 aliased h1).
    pub via_alias: bool,
}

/// Check all PoolCheckedAccess sites for Invalid handle state.
pub fn check_errors(
    func: &MirFunction,
    analysis: &TypestateAnalysis,
    results: &DataflowResults<TypestateDomain>,
) -> Vec<TypestateError> {
    let mut errors = Vec::new();

    // Build local name lookup
    let local_names: HashMap<LocalId, &str> = func
        .locals
        .iter()
        .filter_map(|l| l.name.as_deref().map(|n| (l.id, n)))
        .collect();

    for block in &func.blocks {
        let mut state = results.entry[&block.id].clone();

        for stmt in &block.statements {
            // Check before applying this statement's effect
            if let MirStmtKind::PoolCheckedAccess { handle, .. } = &stmt.kind {
                if let Some(info) = state.handles.get(handle) {
                    if info.state == HandleState::Invalid {
                        errors.push(TypestateError {
                            access_span: stmt.span,
                            invalidation_span: info.state_span,
                            handle_name: local_names.get(handle).map(|s| s.to_string()),
                            via_alias: false, // TODO: detect alias chains
                        });
                    }
                }
            }

            transfer_stmt(stmt, &mut state, &analysis.handle_locals, &analysis.pool_locals, &analysis.summaries);
        }
    }

    errors
}

// ── Interprocedural Summaries ────────────────────────────────────────────

/// Summary of a function's effect on handle parameters.
/// Computed by lightweight scanning — no dataflow needed.
#[derive(Debug, Clone, Default)]
pub struct FunctionSummary {
    /// Parameter indices (0-based) that may be invalidated by the function.
    /// "Invalidated" = the function calls Pool_remove/Pool_clear on a handle param.
    pub invalidated_params: HashSet<usize>,
    /// Parameter indices that may be widened (passed to pool mutators).
    pub widened_params: HashSet<usize>,
}

/// Compute summaries for all functions by scanning their bodies.
/// No dataflow — just checks if handle parameters flow into Pool_remove/Pool_clear.
pub fn compute_summaries(functions: &[MirFunction]) -> HashMap<String, FunctionSummary> {
    let mut summaries = HashMap::new();

    for func in functions {
        if func.params.is_empty() {
            continue;
        }

        // Map param LocalId → argument position (matches call site arg order)
        let param_index: HashMap<LocalId, usize> = func
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();

        let handle_params: HashSet<LocalId> = func
            .params
            .iter()
            .filter(|p| p.ty == MirType::Handle)
            .map(|p| p.id)
            .collect();

        if handle_params.is_empty() {
            continue;
        }

        let mut summary = FunctionSummary::default();

        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    // Pool_remove(pool, handle) — if handle is a param, it's invalidated
                    MirStmtKind::Call { func: fref, args, .. }
                        if fref.name == "Pool_remove" || fref.name == "Pool_clear"
                            || fref.name == "Pool_drain" =>
                    {
                        for arg in args {
                            if let MirOperand::Local(id) = arg {
                                if handle_params.contains(id) {
                                    if let Some(&idx) = param_index.get(id) {
                                        summary.invalidated_params.insert(idx);
                                    }
                                }
                            }
                        }
                    }
                    // Pool mutators widen handle params
                    MirStmtKind::Call { func: fref, args, .. }
                        if pool_ops::is_pool_mutator(&fref.name) =>
                    {
                        for arg in args {
                            if let MirOperand::Local(id) = arg {
                                if handle_params.contains(id) {
                                    if let Some(&idx) = param_index.get(id) {
                                        summary.widened_params.insert(idx);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if !summary.invalidated_params.is_empty() || !summary.widened_params.is_empty() {
            summaries.insert(func.name.clone(), summary);
        }
    }

    summaries
}

/// Run typestate analysis with interprocedural summaries.
/// Phase 1: compute summaries. Phase 2: analyze with summary-aware transfer.
pub fn analyze_with_summaries(
    func: &MirFunction,
    summaries: &HashMap<String, FunctionSummary>,
) -> Option<(TypestateAnalysis, DataflowResults<TypestateDomain>)> {
    let mut analysis = TypestateAnalysis::from_function(func)?;
    analysis.summaries = summaries.clone();
    let dom = DominatorTree::build(func);
    let results = dataflow::solve(func, &analysis, &dom);
    Some((analysis, results))
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::{MirBlock, MirLocal};
    use crate::{FunctionRef, MirConst, MirTerminator, MirTerminatorKind};

    fn local(id: u32) -> LocalId {
        LocalId(id)
    }

    fn block(n: u32) -> BlockId {
        BlockId(n)
    }

    fn handle_local(id: u32, name: &str, is_param: bool) -> MirLocal {
        MirLocal {
            id: local(id),
            name: Some(name.into()),
            ty: MirType::Handle,
            is_param,
        }
    }

    fn pool_local(id: u32) -> MirLocal {
        MirLocal {
            id: local(id),
            name: Some("pool".into()),
            ty: MirType::I64,
            is_param: false,
        }
    }

    fn pool_access(dst: u32, pool: u32, handle: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
            dst: local(dst),
            pool: local(pool),
            handle: local(handle),
        })
    }

    fn pool_insert(dst: u32, pool: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Call {
            dst: Some(local(dst)),
            func: FunctionRef::internal("Pool_insert".to_string()),
            args: vec![MirOperand::Local(local(pool))],
        })
    }

    fn pool_remove(pool: u32, handle: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal("Pool_remove".to_string()),
            args: vec![
                MirOperand::Local(local(pool)),
                MirOperand::Local(local(handle)),
            ],
        })
    }

    fn assign_local(dst: u32, src: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: local(dst),
            rvalue: MirRValue::Use(MirOperand::Local(local(src))),
        })
    }

    fn term_ret() -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: None })
    }

    fn term_goto(target: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Goto {
            target: block(target),
        })
    }

    fn term_branch(then: u32, else_: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Constant(MirConst::Bool(true)),
            then_block: block(then),
            else_block: block(else_),
        })
    }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    /// TS8: Remove then access → compile error
    #[test]
    fn remove_then_access_is_error() {
        let func = make_fn(
            vec![pool_local(0), handle_local(1, "h", false), pool_local(10)],
            vec![MirBlock {
                id: block(0),
                statements: vec![
                    pool_insert(1, 0),
                    pool_remove(0, 1),
                    pool_access(10, 0, 1),
                ],
                terminator: term_ret(),
            }],
        );
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert_eq!(errors.len(), 1, "should detect stale handle access");
    }

    /// Fresh handle access → no error
    #[test]
    fn fresh_access_is_ok() {
        let func = make_fn(
            vec![pool_local(0), handle_local(1, "h", false), pool_local(10)],
            vec![MirBlock {
                id: block(0),
                statements: vec![pool_insert(1, 0), pool_access(10, 0, 1)],
                terminator: term_ret(),
            }],
        );
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert!(errors.is_empty(), "fresh handle access should be ok");
    }

    /// MA1 + TS4 + TS8: Aliased handle after remove → error
    #[test]
    fn aliased_remove_is_error() {
        let func = make_fn(
            vec![
                pool_local(0),
                handle_local(1, "h1", false),
                handle_local(2, "h2", false),
                pool_local(10),
            ],
            vec![MirBlock {
                id: block(0),
                statements: vec![
                    pool_insert(1, 0),       // h1 = pool.insert() → Fresh
                    assign_local(2, 1),       // h2 = h1 → alias (MA1)
                    pool_remove(0, 1),        // pool.remove(h1) → h1+h2 Invalid (TS4)
                    pool_access(10, 0, 2),    // pool[h2] → ERROR (TS8 via alias)
                ],
                terminator: term_ret(),
            }],
        );
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert_eq!(
            errors.len(),
            1,
            "should detect stale access via aliased handle"
        );
    }

    /// TS2: Conditional remove — one branch removes, the other doesn't.
    /// At join point, state is min(Invalid, Fresh) = Invalid.
    /// Access after the join → error.
    #[test]
    fn conditional_remove_is_error_after_join() {
        let func = make_fn(
            vec![
                pool_local(0),
                handle_local(1, "h", false),
                pool_local(10),
            ],
            vec![
                // Block 0: insert, then branch
                MirBlock {
                    id: block(0),
                    statements: vec![pool_insert(1, 0)],
                    terminator: term_branch(1, 2),
                },
                // Block 1: remove h
                MirBlock {
                    id: block(1),
                    statements: vec![pool_remove(0, 1)],
                    terminator: term_goto(3),
                },
                // Block 2: nothing (h stays Fresh)
                MirBlock {
                    id: block(2),
                    statements: vec![],
                    terminator: term_goto(3),
                },
                // Block 3: access h — h is Invalid from one path
                MirBlock {
                    id: block(3),
                    statements: vec![pool_access(10, 0, 1)],
                    terminator: term_ret(),
                },
            ],
        );
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert_eq!(
            errors.len(),
            1,
            "access after conditional remove should be an error"
        );
    }

    /// TS7: Parameter handles start as Unknown — access is ok (runtime check).
    #[test]
    fn parameter_handle_access_is_ok() {
        let func = MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![pool_local(0), handle_local(1, "h", true), pool_local(10)],
            blocks: vec![MirBlock {
                id: block(0),
                statements: vec![pool_access(10, 0, 1)],
                terminator: term_ret(),
            }],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert!(
            errors.is_empty(),
            "parameter handle access should be ok (Unknown, not Invalid)"
        );
    }

    /// Different pools: remove from pool_a doesn't invalidate pool_b handles.
    #[test]
    fn different_pool_isolation() {
        let func = make_fn(
            vec![
                pool_local(0),           // pool_a
                MirLocal {               // pool_b
                    id: local(3),
                    name: Some("pool_b".into()),
                    ty: MirType::I64,
                    is_param: false,
                },
                handle_local(1, "h_a", false),
                handle_local(2, "h_b", false),
                pool_local(10),
            ],
            vec![MirBlock {
                id: block(0),
                statements: vec![
                    pool_insert(1, 0),        // h_a from pool_a
                    pool_insert(2, 3),        // h_b from pool_b
                    pool_remove(0, 1),        // remove h_a from pool_a
                    pool_access(10, 3, 2),    // access h_b from pool_b → should be ok
                ],
                terminator: term_ret(),
            }],
        );
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert!(
            errors.is_empty(),
            "removing from pool_a should not invalidate pool_b handles"
        );
    }

    /// No handles → analysis returns None (early bail).
    #[test]
    fn no_handles_returns_none() {
        let func = make_fn(
            vec![MirLocal {
                id: local(0),
                name: Some("x".into()),
                ty: MirType::I32,
                is_param: false,
            }],
            vec![MirBlock {
                id: block(0),
                statements: vec![],
                terminator: term_ret(),
            }],
        );
        assert!(analyze(&func).is_none());
    }

    /// MA4: Reassignment breaks alias
    #[test]
    fn reassignment_breaks_alias() {
        let func = make_fn(
            vec![
                pool_local(0),
                handle_local(1, "h1", false),
                handle_local(2, "h2", false),
                pool_local(10),
            ],
            vec![MirBlock {
                id: block(0),
                statements: vec![
                    pool_insert(1, 0),        // h1 = pool.insert() → Fresh
                    assign_local(2, 1),       // h2 = h1 → alias
                    pool_insert(2, 0),        // h2 = pool.insert() → reassignment breaks alias
                    pool_remove(0, 1),        // remove h1 → only h1 Invalid (alias broken)
                    pool_access(10, 0, 2),    // pool[h2] → should be ok (h2 is Fresh, not aliased)
                ],
                terminator: term_ret(),
            }],
        );
        let (analysis, results) = analyze(&func).unwrap();
        let errors = check_errors(&func, &analysis, &results);
        assert!(
            errors.is_empty(),
            "h2 was reassigned so alias with h1 is broken"
        );
    }

    /// Interprocedural: callee that removes a handle parameter invalidates it at call site.
    #[test]
    fn interprocedural_invalidation() {
        let pool_param = MirLocal {
            id: local(0), name: Some("pool".into()), ty: MirType::I64, is_param: true,
        };
        let handle_param = MirLocal {
            id: local(1), name: Some("h".into()), ty: MirType::Handle, is_param: true,
        };

        // Callee: func destroy(pool, handle) { Pool_remove(pool, handle) }
        let callee = MirFunction {
            name: "destroy".to_string(),
            params: vec![pool_param.clone(), handle_param.clone()],
            ret_ty: MirType::Void,
            locals: vec![pool_param, handle_param],
            blocks: vec![MirBlock {
                id: block(0),
                statements: vec![pool_remove(0, 1)],
                terminator: term_ret(),
            }],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };

        // Caller: insert h, call destroy(pool, h), access pool[h] → error
        let caller = MirFunction {
            name: "caller".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                pool_local(0),
                handle_local(1, "h", false),
                pool_local(10),
            ],
            blocks: vec![MirBlock {
                id: block(0),
                statements: vec![
                    pool_insert(1, 0),
                    // Call destroy(pool, h)
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("destroy".to_string()),
                        args: vec![
                            MirOperand::Local(local(0)),
                            MirOperand::Local(local(1)),
                        ],
                    }),
                    pool_access(10, 0, 1), // access after destroy → should be error
                ],
                terminator: term_ret(),
            }],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };

        let all_fns = vec![callee, caller];
        let summaries = compute_summaries(&all_fns);

        // Verify summary: param 1 (handle, the second parameter) is invalidated
        assert!(summaries.contains_key("destroy"), "should have summary for destroy");
        assert!(
            summaries["destroy"].invalidated_params.contains(&1),
            "destroy should invalidate param 1 (handle)"
        );

        // Analyze caller with summaries
        let (analysis, results) = analyze_with_summaries(&all_fns[1], &summaries).unwrap();
        let errors = check_errors(&all_fns[1], &analysis, &results);
        assert_eq!(
            errors.len(), 1,
            "should detect stale handle after interprocedural invalidation"
        );
    }
}

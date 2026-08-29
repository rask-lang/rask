// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Container drop insertion.
//!
//! `Vec.new()` allocates a `RaskVec` and, on the first push, a data array.
//! Nothing in the pipeline ever freed either (#1027) — a vector built in a
//! loop leaked the handle, the array, and every heap string in it, once per
//! turn. `Map`, `Rack` and `Pool` are the same.
//!
//! Modelled on `trait_drop.rs`, which solves the same problem for trait
//! objects, and shares its rules: track only *fresh* allocations — the
//! destinations of the constructors below, plus whatever a chain of moves or
//! phis carries forward — and drop only what never hands ownership to anything
//! else. A container read back out of a struct field, another container, or a
//! call's return value is somebody else's, and freeing it double-frees.
//!
//! One rule differs, and it has to. A container's own methods take it as their
//! first argument, so the "any call argument escapes" rule of the trait pass
//! would mark every vector escaping the moment anything was pushed onto it, and
//! nothing would ever be dropped. A container-prefixed call borrows its
//! *receiver* — argument zero — and escapes the rest: `v.push(inner)` hands
//! `inner` over and keeps `v`.

use std::collections::{HashMap, HashSet};

use crate::{
    BlockId, FunctionRef, LocalId, MirBlock, MirConst, MirFunction, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminatorKind, MirType,
};

/// What an element owns. Mirrors `RASK_ELEM_*` in `rask_runtime.h`.
const ELEM_NONE: i64 = 0;
const ELEM_STRING: i64 = 1;

/// Constructors that hand back a fresh container, and the function that frees
/// what each one made.
///
/// Only containers whose `free` the runtime actually exposes. A `Rack` or a
/// `Pool` is usually held for a scope by design rather than built and dropped,
/// but the rule is the same and costs nothing extra.
const CONTAINERS: &[(&str, &str)] = &[
    ("Vec_new", "Vec_free_elems"),
    ("Vec_with_capacity", "Vec_free_elems"),
    ("Vec_fixed", "Vec_free_elems"),
    ("Map_new", "Map_free_elems"),
    ("Map_new_string_keys", "Map_free_elems"),
];

fn free_for(ctor: &str) -> Option<&'static str> {
    CONTAINERS.iter().find(|(c, _)| *c == ctor).map(|(_, f)| *f)
}

/// Type prefixes whose methods borrow their receiver. Mirrors the lists in
/// `rc_insert` and `rc_elide`.
fn is_container_method(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "Vec_", "Map_", "Set_", "Deque_", "Rack_", "Pool_", "Link_",
    ];
    let head = name.rsplit("::").next().unwrap_or(name);
    PREFIXES.iter().any(|p| head.starts_with(p))
}

pub fn insert_container_drops(fns: &mut [MirFunction]) {
    for func in fns.iter_mut() {
        insert_for_function(func);
    }
}

fn insert_for_function(func: &mut MirFunction) {
    let fresh = collect_fresh_containers(func);
    if fresh.is_empty() {
        return;
    }
    let escaping = find_escaping(func, &fresh);
    let moved_away = find_moved_away(func, &fresh);
    let already_freed = find_already_freed(func, &fresh);
    let fresh: HashMap<LocalId, &'static str> = fresh
        .into_iter()
        .filter(|(id, _)| !already_freed.contains(id))
        .collect();
    if fresh.is_empty() {
        return;
    }

    let mut droppable: HashMap<LocalId, &'static str> = fresh
        .iter()
        .filter(|(id, _)| !escaping.contains(id) && !moved_away.contains(id))
        .map(|(id, f)| (*id, *f))
        .collect();

    // The moved-away rule assumes a chain: `a` into `b` into `c`, where only
    // the last name still holds the value. Inlining breaks that. `v.min()` and
    // `v.max()` both copy the same vector into their own parameter local, so
    // one `Vec.new()` reaches two surviving names — and each one freed it.
    //
    // A value that fans out like that is left alone. Leaking it is the wrong
    // answer; freeing it twice is a worse one.
    for group in value_groups(func, &fresh) {
        let survivors = group.iter().filter(|id| droppable.contains_key(id)).count();
        if survivors > 1 {
            for id in &group {
                droppable.remove(id);
            }
        }
    }

    if droppable.is_empty() {
        return;
    }
    let kinds = element_kinds(func, &droppable);
    insert_drops(func, &droppable, &kinds);
}

/// What each container's elements own, read off what gets put into it.
///
/// The container carries only its element *size*, which can't tell a
/// sixteen-byte string from a sixteen-byte struct — and the element type isn't
/// on the local either, which is an opaque handle by the time it reaches MIR.
/// But the pushes and inserts are right here, and their arguments are typed.
///
/// A container filled some other way — decoded, built from a static — reads as
/// owning nothing and leaks its elements as before. Narrower than the general
/// case, not wronger: too few releases is a leak, too many is a double free.
/// The general answer is drop glue per element type, which is the rest of
/// #1027.
fn element_kinds(
    func: &MirFunction,
    droppable: &HashMap<LocalId, &'static str>,
) -> HashMap<LocalId, (i64, i64)> {
    let ty_of: HashMap<LocalId, MirType> =
        func.locals.iter().map(|l| (l.id, l.ty.clone())).collect();
    let is_string = |op: &MirOperand| match op {
        MirOperand::Local(id) => ty_of.get(id) == Some(&MirType::String),
        MirOperand::Constant(MirConst::String(_)) => true,
        _ => false,
    };

    let mut kinds: HashMap<LocalId, (i64, i64)> =
        droppable.keys().map(|id| (*id, (ELEM_NONE, ELEM_NONE))).collect();

    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::Call { func: fref, args, .. } = &stmt.kind else { continue };
            let Some(MirOperand::Local(receiver)) = args.first() else { continue };
            let Some(entry) = kinds.get_mut(receiver) else { continue };
            let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
            let base = head.split('$').next().unwrap_or(head);
            match base {
                // `push(v, x)` / `insert(v, i, x)` — the element is the last arg.
                "Vec_push" | "Vec_set" | "Vec_insert" => {
                    if args.last().is_some_and(is_string) {
                        entry.0 = ELEM_STRING;
                    }
                }
                // `insert(m, k, v)`.
                "Map_insert" | "Map_set" => {
                    if args.len() >= 3 {
                        if is_string(&args[1]) {
                            entry.0 = ELEM_STRING;
                        }
                        if is_string(&args[2]) {
                            entry.1 = ELEM_STRING;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    kinds
}

/// Locals holding a container this function made, mapped to how to free it.
fn collect_fresh_containers(func: &MirFunction) -> HashMap<LocalId, &'static str> {
    let mut fresh: HashMap<LocalId, &'static str> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::Call { dst: Some(dst), func: fref, .. } = &stmt.kind {
                let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
                // A monomorphized name carries a `$` suffix.
                let base = head.split('$').next().unwrap_or(head);
                if let Some(free) = free_for(base) {
                    fresh.insert(*dst, free);
                }
            }
        }
    }
    if fresh.is_empty() {
        return fresh;
    }

    // The same value under a new name: follow it, so the name still holding it
    // at its death is the one that gets freed.
    loop {
        let mut added = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                        if let Some(free) = fresh.get(src).copied() {
                            if fresh.insert(*dst, free).is_none() {
                                added = true;
                            }
                        }
                    }
                    MirStmtKind::Phi { dst, args } => {
                        for (_, op) in args {
                            if let MirOperand::Local(src) = op {
                                if let Some(free) = fresh.get(src).copied() {
                                    if fresh.insert(*dst, free).is_none() {
                                        added = true;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !added {
            break;
        }
    }
    fresh
}

/// Containers the lowering already frees itself.
///
/// The iterator and sort lowerings build a scratch vector and free it when
/// they're done. Adding a second free there is a double free — `sort_by_key`
/// segfaulted on the way out of `main`. Anything with an explicit free on it
/// is somebody else's business.
fn find_already_freed(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let mut freed = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::Call { func: fref, args, .. } = &stmt.kind else { continue };
            let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
            let base = head.split('$').next().unwrap_or(head);
            if !base.ends_with("_free") && !base.ends_with("_free_elems") {
                continue;
            }
            if let Some(MirOperand::Local(id)) = args.first() {
                if containers.contains_key(id) {
                    freed.insert(*id);
                }
            }
        }
    }
    freed
}

/// The names that hold one value: copies and phi merges.
fn value_groups(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> Vec<HashSet<LocalId>> {
    let mut parent: HashMap<LocalId, LocalId> = HashMap::new();

    fn find(parent: &mut HashMap<LocalId, LocalId>, x: LocalId) -> LocalId {
        let p = *parent.get(&x).unwrap_or(&x);
        if p == x {
            return x;
        }
        let root = find(parent, p);
        parent.insert(x, root);
        root
    }

    fn union(parent: &mut HashMap<LocalId, LocalId>, a: LocalId, b: LocalId) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if containers.contains_key(dst) && containers.contains_key(src) =>
                {
                    union(&mut parent, *dst, *src);
                }
                MirStmtKind::Phi { dst, args } if containers.contains_key(dst) => {
                    for (_, op) in args {
                        if let MirOperand::Local(src) = op {
                            if containers.contains_key(src) {
                                union(&mut parent, *dst, *src);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut groups: HashMap<LocalId, HashSet<LocalId>> = HashMap::new();
    for id in containers.keys() {
        let root = find(&mut parent, *id);
        groups.entry(root).or_default().insert(*id);
    }
    groups.into_values().collect()
}

/// Returned, stored, captured, or handed to something that isn't one of its own
/// methods. A container method borrows its receiver and escapes its other
/// arguments.
fn find_escaping(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let mut escaping = HashSet::new();
    let mut mark = |op: &MirOperand, escaping: &mut HashSet<LocalId>| {
        if let MirOperand::Local(id) = op {
            if containers.contains_key(id) {
                escaping.insert(*id);
            }
        }
    };

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { func: fref, args, .. } => {
                    let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
                    let skip_receiver = is_container_method(head) && !args.is_empty();
                    let start = if skip_receiver { 1 } else { 0 };
                    for arg in &args[start..] {
                        mark(arg, &mut escaping);
                    }
                }
                MirStmtKind::ClosureCall { args, .. } | MirStmtKind::TraitCall { args, .. } => {
                    for arg in args {
                        mark(arg, &mut escaping);
                    }
                }
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::TraitBox { value, .. } => {
                    mark(value, &mut escaping);
                }
                MirStmtKind::ClosureCreate { captures, .. } => {
                    for cap in captures {
                        if containers.contains_key(&cap.local_id) {
                            escaping.insert(cap.local_id);
                        }
                    }
                }
                MirStmtKind::Assign { rvalue: MirRValue::Ref(src), .. } => {
                    if containers.contains_key(src) {
                        escaping.insert(*src);
                    }
                }
                _ => {}
            }
        }

        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(op) }
            | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
                mark(op, &mut escaping);
            }
            _ => {}
        }
    }
    escaping
}

/// Copied into another local, or merged through a phi: the new name owns it.
fn find_moved_away(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let mut moved = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if containers.contains_key(src) && src != dst =>
                {
                    moved.insert(*src);
                }
                MirStmtKind::Phi { args, .. } => {
                    for (_, op) in args {
                        if let MirOperand::Local(id) = op {
                            if containers.contains_key(id) {
                                moved.insert(*id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    moved
}

/// Free before every return the container's definition dominates, and before
/// every loop back-edge it was built inside. Same placement rules as
/// `trait_drop.rs`, and for the same reasons — see the comments there on why
/// dominance is what decides it rather than block order.
fn insert_drops(
    func: &mut MirFunction,
    droppable: &HashMap<LocalId, &'static str>,
    kinds: &HashMap<LocalId, (i64, i64)>,
) {
    let dom = crate::analysis::dominators::DominatorTree::build(func);

    let mut defined_in_block: HashMap<LocalId, usize> = HashMap::new();
    for (idx, block) in func.blocks.iter().enumerate() {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                if droppable.contains_key(&dst) {
                    defined_in_block.insert(dst, idx);
                }
            }
        }
    }

    let mut to_insert: Vec<(usize, Vec<LocalId>)> = Vec::new();

    for (block_idx, block) in func.blocks.iter().enumerate() {
        match &block.terminator.kind {
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. } => {
                let drops: Vec<LocalId> = droppable
                    .keys()
                    .copied()
                    .filter(|id| {
                        defined_in_block.get(id).is_some_and(|&def_idx| {
                            dom.dominates(func.blocks[def_idx].id, block.id)
                        })
                    })
                    .collect();
                if !drops.is_empty() {
                    to_insert.push((block_idx, drops));
                }
            }
            MirTerminatorKind::Goto { target } => backedge_drops(
                &mut to_insert, block_idx, block.id, *target, &func.blocks, &dom, &defined_in_block,
            ),
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                backedge_drops(
                    &mut to_insert, block_idx, block.id, *then_block, &func.blocks, &dom,
                    &defined_in_block,
                );
                backedge_drops(
                    &mut to_insert, block_idx, block.id, *else_block, &func.blocks, &dom,
                    &defined_in_block,
                );
            }
            _ => {}
        }
    }

    for (block_idx, locals) in to_insert {
        for local in locals {
            let free = droppable[&local];
            let (a, b) = kinds.get(&local).copied().unwrap_or((ELEM_NONE, ELEM_NONE));
            let mut args = vec![MirOperand::Local(local), MirOperand::Constant(MirConst::Int(a))];
            if free.starts_with("Map_") {
                args.push(MirOperand::Constant(MirConst::Int(b)));
            }
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(free.to_string()),
                args,
            }));
        }
    }
}

fn backedge_drops(
    out: &mut Vec<(usize, Vec<LocalId>)>,
    block_idx: usize,
    source: BlockId,
    target: BlockId,
    blocks: &[MirBlock],
    dom: &crate::analysis::dominators::DominatorTree,
    defined_in_block: &HashMap<LocalId, usize>,
) {
    if !dom.dominates(target, source) {
        return;
    }
    let drops: Vec<LocalId> = defined_in_block
        .iter()
        .filter(|(_, &def_idx)| {
            let def = blocks[def_idx].id;
            dom.dominates(target, def) && dom.dominates(def, source)
        })
        .map(|(&id, _)| id)
        .collect();
    if !drops.is_empty() {
        out.push((block_idx, drops));
    }
}

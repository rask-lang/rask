// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `Rack<T>` + `Link<T>` — the delete-time edge fixup (analysis.fourth-option).
//!
//! The model in one line: deleting a node walks every edge pointing at it and
//! sets it to `none`, so a dead link never exists and following a live one needs
//! no check.
//!
//! Each node carries the list of places pointing at it (`RackData::incoming`),
//! so a delete visits exactly those places — O(in-degree), not a scan. Edges are
//! recorded wherever a link is stored: into a node on `insert`, into a field on
//! assignment, into an edge list on `push`, into an index on `insert`.
//!
//! A node field and a root field are the same thing to this code. That falls out
//! of keying backlinks on the holder rather than on "is it inside the rack":
//! `world.player` and `entity.target` are both a struct field holding a link, so
//! root edges need no separate machinery.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::value::{node_key, Backlink, BacklinkKey, MapKey, RackData, StructData, Value};

/// Fixup work, for the delete-cost question the analysis flags as the model's
/// one real regression. `RASK_RACK_STATS=1` prints the totals at exit.
///
/// `HOLDERS_VISITED` is the in-degree actually walked; `EDGES_FIXED` is the
/// edges rewritten. A struct field unlinks exactly on overwrite, so the two
/// agree for scalar edges.
///
/// They can differ for a container holder, whose backlink names the container
/// rather than a position: pop the last element pointing at T and the entry
/// stays until T is deleted, at which point the visit finds nothing. That costs
/// one check, once, because the entry is deduped per (container, target) and the
/// list is discarded by the delete that read it — so there is nothing here that
/// grows.
pub static EDGES_FIXED: AtomicUsize = AtomicUsize::new(0);
pub static HOLDERS_VISITED: AtomicUsize = AtomicUsize::new(0);
pub static DELETES: AtomicUsize = AtomicUsize::new(0);

/// Has this program made a rack at all?
///
/// Native only arms its `atexit` printer when one is created, so a program with
/// no rack says nothing there while the interpreter printed `0/0/0`. Same
/// question, same answer.
pub static RACKS_MADE: AtomicUsize = AtomicUsize::new(0);

pub fn stats_enabled() -> bool {
    std::env::var("RASK_RACK_STATS").is_ok()
}

pub fn print_stats() {
    if !stats_enabled() || RACKS_MADE.load(Ordering::Relaxed) == 0 {
        return;
    }
    eprintln!(
        "rack stats: deletes={} edges_fixed={} holders_visited={}",
        DELETES.load(Ordering::Relaxed),
        EDGES_FIXED.load(Ordering::Relaxed),
        HOLDERS_VISITED.load(Ordering::Relaxed),
    );
}

/// How deep registration and fixup descend through owned values inside one
/// holder. Links are leaves — neither walk ever follows one — so this only
/// bounds nesting of plain aggregates.
const MAX_DEPTH: usize = 32;

/// The node a value names, seeing through the `Link<T>?` optional every edge
/// field carries.
fn link_target(v: &Value, rack_id: u32) -> Option<Arc<Mutex<StructData>>> {
    match v {
        Value::Link { rack_id: sid, node } if *sid == rack_id => Some(Arc::clone(node)),
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            fields.first().and_then(|f| link_target(f, rack_id))
        }
        _ => None,
    }
}

/// Any link, whichever rack it belongs to.
fn any_link(v: &Value) -> Option<(u32, Arc<Mutex<StructData>>)> {
    match v {
        Value::Link { rack_id, node } => Some((*rack_id, Arc::clone(node))),
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            fields.first().and_then(any_link)
        }
        _ => None,
    }
}

fn is_link_to(v: &Value, rack_id: u32, dead: &Arc<Mutex<StructData>>) -> bool {
    link_target(v, rack_id).is_some_and(|n| Arc::ptr_eq(&n, dead))
}

fn option_none() -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "None".to_string(),
        fields: vec![],
        variant_index: 1,
        origin: None,
    }
}

// ---------------------------------------------------------------------------
// Registration — recording who points at whom, as links are stored
// ---------------------------------------------------------------------------

/// Record the edge `value` puts in `holder.field`, and forget whatever edge the
/// field held before.
///
/// This is the exact case: a struct field names its slot, so an overwrite
/// unlinks the old target precisely and nothing accumulates however many times
/// the field is written. `old` is the value being replaced — pass `None` when
/// the field is newly created and held nothing.
pub fn register_field(
    holder: &Arc<Mutex<StructData>>,
    field: &str,
    old: Option<&Value>,
    value: &Value,
) {
    let slot = BacklinkKey { holder: Arc::as_ptr(holder) as usize, field: Some(field.to_string()) };

    // Unlink first: `a.target = a.target` must not drop the backlink it re-adds.
    if let Some((rack_id, previous)) = old.and_then(any_link) {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            let same = any_link(value)
                .is_some_and(|(sid, next)| sid == rack_id && Arc::ptr_eq(&next, &previous));
            if !same {
                rack.lock().unwrap().unregister_backlink(&previous, &slot);
            }
        }
    }

    // A container replaced wholesale takes its records with it. The old `Vec`
    // named *itself* in every target's incoming list, and nothing dropped those
    // when the field stopped holding it — so `old.children =
    // old.children.filter(…)` left a record for a vector nobody holds any more.
    // The next delete of one of those targets walked it, found a dead weak
    // reference, and counted a visit that fixed nothing (#983).
    //
    // Only containers: a struct value is shared by `Arc` here, so the one being
    // replaced may still be reachable elsewhere and its edges are not ours to
    // drop.
    if let Some(old) = old {
        if !std::ptr::eq(old as *const Value, value as *const Value) {
            forget_container_edges(old, 0);
        }
    }

    if let Some((rack_id, target)) = any_link(value) {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack.lock().unwrap().register_backlink(
                &target,
                Backlink::Field(Arc::downgrade(holder), field.to_string()),
            );
        }
        return;
    }
    register_nested(value, 0);
}

/// Drop the records a container held, when the container itself is being
/// replaced. Descends through options and nested containers; stops at a struct,
/// whose data may still be reachable through another name.
fn forget_container_edges(value: &Value, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match value {
        Value::Vec(vec) => {
            let slot = BacklinkKey { holder: Arc::as_ptr(vec) as usize, field: None };
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                match any_link(it) {
                    Some((rack_id, target)) => {
                        if let Some(rack) = crate::value::rack_by_id(rack_id) {
                            rack.lock().unwrap().unregister_backlink(&target, &slot);
                        }
                    }
                    None => forget_container_edges(it, depth + 1),
                }
            }
        }
        Value::Map(map) => {
            let slot = BacklinkKey { holder: Arc::as_ptr(map) as usize, field: None };
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                match any_link(it) {
                    Some((rack_id, target)) => {
                        if let Some(rack) = crate::value::rack_by_id(rack_id) {
                            rack.lock().unwrap().unregister_backlink(&target, &slot);
                        }
                    }
                    None => forget_container_edges(it, depth + 1),
                }
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                forget_container_edges(f, depth + 1);
            }
        }
        _ => {}
    }
}

/// Record an edge pushed onto or written into an edge list.
///
/// A list backlink names the list, not a position, so it is one entry per
/// (list, target) pair however many elements match. Nothing accumulates from
/// repeated pushes of the same target, and an entry left behind after the last
/// matching element goes away is dropped by `fix_elements` on the visit that
/// finds nothing.
pub fn register_element(vec: &Arc<Mutex<crate::value::VecData>>, value: &Value) {
    if let Some((rack_id, target)) = any_link(value) {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack
                .lock()
                .unwrap()
                .register_backlink(&target, Backlink::Element(Arc::downgrade(vec)));
        }
    }
}

/// Record an edge inserted into an index. Same one-per-(map, target) shape as
/// `register_element`; `old` is the displaced value when a key is overwritten.
pub fn register_entry(
    map: &Arc<Mutex<crate::value::MapData>>,
    old: Option<&Value>,
    value: &Value,
) {
    let slot = BacklinkKey { holder: Arc::as_ptr(map) as usize, field: None };
    if let Some((rack_id, previous)) = old.and_then(any_link) {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            // Only if no other entry still points there — the backlink covers
            // the whole map, not this one key.
            let still_held = map
                .lock()
                .unwrap()
                .values()
                .any(|v| link_target(v, rack_id).is_some_and(|n| Arc::ptr_eq(&n, &previous)));
            if !still_held {
                rack.lock().unwrap().unregister_backlink(&previous, &slot);
            }
        }
    }
    if let Some((rack_id, target)) = any_link(value) {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack
                .lock()
                .unwrap()
                .register_backlink(&target, Backlink::Entry(Arc::downgrade(map)));
        }
    }
}

/// Record every edge reachable inside `value` without crossing a link.
///
/// Used where a whole aggregate arrives at once and its interior hasn't been
/// registered piecewise: `rack.insert(node)`, a struct literal, and a field
/// assignment whose value is a container (`old.children = old.children.filter(…)`
/// builds a fresh `Vec`, whose entries nothing has recorded yet).
pub fn register_nested(value: &Value, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match value {
        Value::Struct(s) => {
            let fields: Vec<(String, Value)> = {
                let guard = s.lock().unwrap();
                guard.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            };
            for (name, v) in &fields {
                register_field(s, name, None, v);
            }
        }
        Value::Vec(vec) => {
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                register_element(vec, it);
                register_nested(it, depth + 1);
            }
        }
        Value::Map(map) => {
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                register_entry(map, None, it);
                register_nested(it, depth + 1);
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                register_nested(f, depth + 1);
            }
        }
        _ => {}
    }
}

/// Drop every edge the dying node itself holds, so its targets stop naming it.
///
/// The mirror of `register_nested`, and the counterpart of native's
/// `forget_own_edges`. A node's own fields point *out*; those records live on
/// the targets' incoming lists, keyed by this node. Leaving them there means a
/// later delete of one of those targets walks a record naming a node that no
/// longer exists — one wasted visit per stale record, and they accumulate for
/// the life of the rack.
///
/// It showed up as a counter disagreement first: `l1_list_links.rk` reported
/// `edges_fixed=1 holders_visited=1` on the interpreter and `0/0` natively,
/// because a removed node's `prev` was still recorded on the node it had
/// pointed at (#983).
fn forget_own_edges(value: &Value, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match value {
        Value::Struct(s) => {
            let fields: Vec<(String, Value)> = {
                let guard = s.lock().unwrap();
                guard.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            };
            for (name, v) in &fields {
                if let Some((rack_id, target)) = any_link(v) {
                    let slot = BacklinkKey {
                        holder: Arc::as_ptr(s) as usize,
                        field: Some(name.clone()),
                    };
                    if let Some(rack) = crate::value::rack_by_id(rack_id) {
                        rack.lock().unwrap().unregister_backlink(&target, &slot);
                    }
                    continue;
                }
                forget_own_edges(v, depth + 1);
            }
        }
        Value::Vec(vec) => {
            let slot = BacklinkKey { holder: Arc::as_ptr(vec) as usize, field: None };
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                if let Some((rack_id, target)) = any_link(it) {
                    if let Some(rack) = crate::value::rack_by_id(rack_id) {
                        rack.lock().unwrap().unregister_backlink(&target, &slot);
                    }
                    continue;
                }
                forget_own_edges(it, depth + 1);
            }
        }
        Value::Map(map) => {
            let slot = BacklinkKey { holder: Arc::as_ptr(map) as usize, field: None };
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                if let Some((rack_id, target)) = any_link(it) {
                    if let Some(rack) = crate::value::rack_by_id(rack_id) {
                        rack.lock().unwrap().unregister_backlink(&target, &slot);
                    }
                    continue;
                }
                forget_own_edges(it, depth + 1);
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                forget_own_edges(f, depth + 1);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Fixup — visiting the recorded holders and nulling their edges
// ---------------------------------------------------------------------------

/// Null one field if it points at the dying node.
fn fix_field(holder: &Arc<Mutex<StructData>>, field: &str, rack_id: u32, dead: &Arc<Mutex<StructData>>) {
    let current = {
        let guard = holder.lock().unwrap();
        match guard.fields.get(field) {
            Some(v) => v.clone(),
            None => return,
        }
    };
    if is_link_to(&current, rack_id, dead) {
        EDGES_FIXED.fetch_add(1, Ordering::Relaxed);
        holder.lock().unwrap().fields.insert(field.to_string(), option_none());
        return;
    }
    // The backlink may name a field whose edge now sits inside a container the
    // field holds, rather than in the field itself.
    fix_container(&current, rack_id, dead, 0);
}

/// Drop entries pointing at the dying node from an edge list. A list of live
/// things loses the entry rather than holding a `none`.
fn fix_elements(vec: &Arc<Mutex<crate::value::VecData>>, rack_id: u32, dead: &Arc<Mutex<StructData>>) {
    let mut guard = vec.lock().unwrap();
    let before = guard.len();
    let kept: Vec<Value> = guard
        .iter()
        .filter(|el| !is_link_to(el, rack_id, dead))
        .cloned()
        .collect();
    let removed = before - kept.len();
    if removed > 0 {
        EDGES_FIXED.fetch_add(removed, Ordering::Relaxed);
        **guard = kept;
    }
}


/// Drop index entries pointing at the dying node — the database's
/// index-maintenance move.
fn fix_entries(map: &Arc<Mutex<crate::value::MapData>>, rack_id: u32, dead: &Arc<Mutex<StructData>>) {
    let mut guard = map.lock().unwrap();
    let doomed: Vec<MapKey> = guard
        .iter()
        .filter(|(_, v)| is_link_to(v, rack_id, dead))
        .map(|(k, _)| k.clone())
        .collect();
    if !doomed.is_empty() {
        EDGES_FIXED.fetch_add(doomed.len(), Ordering::Relaxed);
        for k in doomed {
            guard.shift_remove(&k);
        }
    }
}

/// Fix edges nested inside a value a backlink pointed at. Bounded by the value's
/// own size, and never follows a link.
fn fix_container(v: &Value, rack_id: u32, dead: &Arc<Mutex<StructData>>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match v {
        Value::Vec(vec) => {
            fix_elements(vec, rack_id, dead);
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                if link_target(it, rack_id).is_none() {
                    fix_container(it, rack_id, dead, depth + 1);
                }
            }
        }
        Value::Map(map) => {
            fix_entries(map, rack_id, dead);
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                if link_target(it, rack_id).is_none() {
                    fix_container(it, rack_id, dead, depth + 1);
                }
            }
        }
        Value::Struct(s) => {
            let names: Vec<String> = {
                let guard = s.lock().unwrap();
                guard.fields.keys().cloned().collect()
            };
            for name in names {
                fix_field(s, &name, rack_id, dead);
            }
        }
        _ => {}
    }
}

/// Delete a node: unlink every edge pointing at it, then free the slot.
///
/// Returns the node's value, owned — `@resource` fields inside it stay linear,
/// same as `pool.remove`. Returns `None` if the node is not in this rack.
pub fn delete_node(
    rack: &Arc<Mutex<RackData>>,
    node: &Arc<Mutex<StructData>>,
) -> Option<Value> {
    let (rack_id, holders) = {
        let mut guard = rack.lock().unwrap();
        let idx = guard.index_of(node)?;
        // Free the slot first, so nothing the fixup touches can reach the dying
        // node through the rack.
        guard.slots[idx] = None;
        guard.free_list.push(idx as u32);
        guard.slot_of.remove(&node_key(node));
        guard.len -= 1;
        let holders = guard.take_incoming(node);
        (guard.rack_id, holders)
    };

    // The dying node's own edges go before its incoming ones are walked: its
    // targets must stop naming it, or a later delete of one of them walks a
    // record for a node that no longer exists.
    forget_own_edges(&Value::Struct(Arc::clone(node)), 0);

    DELETES.fetch_add(1, Ordering::Relaxed);
    HOLDERS_VISITED.fetch_add(holders.len(), Ordering::Relaxed);

    for holder in &holders {
        match holder {
            Backlink::Field(w, field) => {
                if let Some(s) = w.upgrade() {
                    fix_field(&s, field, rack_id, node);
                }
            }
            Backlink::Element(w) => {
                if let Some(v) = w.upgrade() {
                    fix_elements(&v, rack_id, node);
                }
            }
            Backlink::Entry(w) => {
                if let Some(m) = w.upgrade() {
                    fix_entries(&m, rack_id, node);
                }
            }
        }
    }

    Some(Value::Struct(Arc::clone(node)))
}

/// Deep-copy a rack, rewriting every edge inside the copy to point at the copy.
///
/// This is the delete-time fixup's machinery pointed at a different job. Delete
/// walks the edges into one node and nulls them; a snapshot walks the edges out of
/// every node and re-points them. Both work because the rack knows its own graph.
///
/// Edges *out* of the snapshot are left alone: a link held in a caller's field
/// still names the original node, which is what the caller asked for. Use
/// `corresponding` to translate one across.
///
/// Root edges into the original are untouched for the same reason — nothing about
/// the original changes.
pub fn snapshot_rack(rack: &Arc<Mutex<RackData>>) -> Value {
    let (old_id, nodes) = {
        let guard = rack.lock().unwrap();
        (guard.rack_id, guard.live_nodes())
    };

    // Copy the nodes first, so every target exists before any edge is rewritten.
    let mut map: HashMap<usize, Arc<Mutex<StructData>>> = HashMap::new();
    let mut copies: Vec<Arc<Mutex<StructData>>> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let cloned = node.lock().unwrap().clone();
        let copy = Arc::new(Mutex::new(cloned));
        map.insert(node_key(node), Arc::clone(&copy));
        copies.push(copy);
    }

    let new_rack = Arc::new(Mutex::new(RackData::with_type_param(
        rack.lock().unwrap().type_param.clone(),
    )));
    crate::value::register_rack(&new_rack);
    let new_id = new_rack.lock().unwrap().rack_id;

    for copy in &copies {
        new_rack.lock().unwrap().insert(Arc::clone(copy));
    }

    // Now the edges. A link is rewritten only if it names a node of *this* rack;
    // a cross-rack edge points somewhere this snapshot has no copy of, and
    // rewriting it would invent one.
    for copy in &copies {
        let fields: Vec<(String, Value)> = {
            let guard = copy.lock().unwrap();
            guard.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        for (name, value) in fields {
            if let Some(rewritten) = rewrite_links(&value, old_id, new_id, &map, 0) {
                copy.lock().unwrap().fields.insert(name.clone(), rewritten);
            }
        }
        crate::rack::register_nested(&Value::Struct(Arc::clone(copy)), 0);
    }

    {
        let mut guard = new_rack.lock().unwrap();
        guard.origin_id = Some(old_id);
        guard.origin = map;
    }
    Value::Rack(new_rack)
}

/// Rebuild `value` with links into `old_id` re-pointed at their copies. Returns
/// `None` when nothing inside it changed, so untouched fields aren't rewritten.
fn rewrite_links(
    value: &Value,
    old_id: u32,
    new_id: u32,
    map: &HashMap<usize, Arc<Mutex<StructData>>>,
    depth: usize,
) -> Option<Value> {
    if depth >= MAX_DEPTH {
        return None;
    }
    match value {
        Value::Link { rack_id, node } if *rack_id == old_id => map
            .get(&node_key(node))
            .map(|copy| Value::Link { rack_id: new_id, node: Arc::clone(copy) }),
        Value::Vec(items) => {
            let current: Vec<Value> = items.lock().unwrap().iter().cloned().collect();
            let mut changed = false;
            let next: Vec<Value> = current
                .iter()
                .map(|v| match rewrite_links(v, old_id, new_id, map, depth + 1) {
                    Some(nv) => {
                        changed = true;
                        nv
                    }
                    None => v.clone(),
                })
                .collect();
            changed.then(|| Value::vec(next))
        }
        // An edge field is `Link<T>?`, so the common case arrives wrapped in an
        // Option rather than bare. Enum payloads generally: rewrite each field.
        Value::Enum { name, variant, fields, variant_index, origin } => {
            let mut changed = false;
            let next: Vec<Value> = fields
                .iter()
                .map(|f| match rewrite_links(f, old_id, new_id, map, depth + 1) {
                    Some(nv) => {
                        changed = true;
                        nv
                    }
                    None => f.clone(),
                })
                .collect();
            changed.then(|| Value::Enum {
                name: name.clone(),
                variant: variant.clone(),
                fields: next,
                variant_index: *variant_index,
                origin: origin.clone(),
            })
        }
        // A struct value inside a node field. `StructData::clone` copies the field
        // map but not the values behind it, so this both rewrites the links and
        // gives the copy its own struct at this level.
        Value::Struct(inner) => {
            let fields: Vec<(String, Value)> = {
                let guard = inner.lock().unwrap();
                guard.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            };
            let mut changed = false;
            let mut next = inner.lock().unwrap().clone();
            for (name, v) in fields {
                if let Some(nv) = rewrite_links(&v, old_id, new_id, map, depth + 1) {
                    changed = true;
                    next.fields.insert(name, nv);
                }
            }
            changed.then(|| Value::Struct(Arc::new(Mutex::new(next))))
        }
        Value::Map(entries) => {
            let current: Vec<(MapKey, Value)> = entries
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let mut changed = false;
            let mut next = indexmap::IndexMap::new();
            for (k, v) in current {
                match rewrite_links(&v, old_id, new_id, map, depth + 1) {
                    Some(nv) => {
                        changed = true;
                        next.insert(k, nv);
                    }
                    None => {
                        next.insert(k, v);
                    }
                }
            }
            changed.then(|| Value::Map(Arc::new(Mutex::new(next))))
        }
        _ => None,
    }
}

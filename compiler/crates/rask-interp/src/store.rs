// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `Store<T>` + `Link<T>` — the delete-time edge fixup (analysis.fourth-option).
//!
//! The model in one line: deleting a node walks every edge pointing at it and
//! sets it to `none`, so a dead link never exists and following a live one needs
//! no check.
//!
//! Each node carries the list of places pointing at it (`StoreData::incoming`),
//! so a delete visits exactly those places — O(in-degree), not a scan. Edges are
//! recorded wherever a link is stored: into a node on `insert`, into a field on
//! assignment, into an edge list on `push`, into an index on `insert`.
//!
//! A node field and a root field are the same thing to this code. That falls out
//! of keying backlinks on the holder rather than on "is it inside the store":
//! `world.player` and `entity.target` are both a struct field holding a link, so
//! root edges need no separate machinery.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::value::{node_key, Backlink, BacklinkKey, MapKey, StoreData, StructData, Value};

/// Fixup work, for the delete-cost question the analysis flags as the model's
/// one real regression. `RASK_STORE_STATS=1` prints the totals at exit.
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

pub fn stats_enabled() -> bool {
    std::env::var("RASK_STORE_STATS").is_ok()
}

pub fn print_stats() {
    if !stats_enabled() {
        return;
    }
    eprintln!(
        "store stats: deletes={} edges_fixed={} holders_visited={}",
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
fn link_target(v: &Value, store_id: u32) -> Option<Arc<Mutex<StructData>>> {
    match v {
        Value::Link { store_id: sid, node } if *sid == store_id => Some(Arc::clone(node)),
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            fields.first().and_then(|f| link_target(f, store_id))
        }
        _ => None,
    }
}

/// Any link, whichever store it belongs to.
fn any_link(v: &Value) -> Option<(u32, Arc<Mutex<StructData>>)> {
    match v {
        Value::Link { store_id, node } => Some((*store_id, Arc::clone(node))),
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            fields.first().and_then(any_link)
        }
        _ => None,
    }
}

fn is_link_to(v: &Value, store_id: u32, dead: &Arc<Mutex<StructData>>) -> bool {
    link_target(v, store_id).is_some_and(|n| Arc::ptr_eq(&n, dead))
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
    if let Some((store_id, previous)) = old.and_then(any_link) {
        if let Some(store) = crate::value::store_by_id(store_id) {
            let same = any_link(value)
                .is_some_and(|(sid, next)| sid == store_id && Arc::ptr_eq(&next, &previous));
            if !same {
                store.lock().unwrap().unregister_backlink(&previous, &slot);
            }
        }
    }

    if let Some((store_id, target)) = any_link(value) {
        if let Some(store) = crate::value::store_by_id(store_id) {
            store.lock().unwrap().register_backlink(
                &target,
                Backlink::Field(Arc::downgrade(holder), field.to_string()),
            );
        }
        return;
    }
    register_nested(value, 0);
}

/// Record an edge pushed onto or written into an edge list.
///
/// A list backlink names the list, not a position, so it is one entry per
/// (list, target) pair however many elements match. Nothing accumulates from
/// repeated pushes of the same target, and an entry left behind after the last
/// matching element goes away is dropped by `fix_elements` on the visit that
/// finds nothing.
pub fn register_element(vec: &Arc<Mutex<crate::value::VecData>>, value: &Value) {
    if let Some((store_id, target)) = any_link(value) {
        if let Some(store) = crate::value::store_by_id(store_id) {
            store
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
    if let Some((store_id, previous)) = old.and_then(any_link) {
        if let Some(store) = crate::value::store_by_id(store_id) {
            // Only if no other entry still points there — the backlink covers
            // the whole map, not this one key.
            let still_held = map
                .lock()
                .unwrap()
                .values()
                .any(|v| link_target(v, store_id).is_some_and(|n| Arc::ptr_eq(&n, &previous)));
            if !still_held {
                store.lock().unwrap().unregister_backlink(&previous, &slot);
            }
        }
    }
    if let Some((store_id, target)) = any_link(value) {
        if let Some(store) = crate::value::store_by_id(store_id) {
            store
                .lock()
                .unwrap()
                .register_backlink(&target, Backlink::Entry(Arc::downgrade(map)));
        }
    }
}

/// Record every edge reachable inside `value` without crossing a link.
///
/// Used where a whole aggregate arrives at once and its interior hasn't been
/// registered piecewise: `store.insert(node)`, a struct literal, and a field
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

// ---------------------------------------------------------------------------
// Fixup — visiting the recorded holders and nulling their edges
// ---------------------------------------------------------------------------

/// Null one field if it points at the dying node.
fn fix_field(holder: &Arc<Mutex<StructData>>, field: &str, store_id: u32, dead: &Arc<Mutex<StructData>>) {
    let current = {
        let guard = holder.lock().unwrap();
        match guard.fields.get(field) {
            Some(v) => v.clone(),
            None => return,
        }
    };
    if is_link_to(&current, store_id, dead) {
        EDGES_FIXED.fetch_add(1, Ordering::Relaxed);
        holder.lock().unwrap().fields.insert(field.to_string(), option_none());
        return;
    }
    // The backlink may name a field whose edge now sits inside a container the
    // field holds, rather than in the field itself.
    fix_container(&current, store_id, dead, 0);
}

/// Drop entries pointing at the dying node from an edge list. A list of live
/// things loses the entry rather than holding a `none`.
fn fix_elements(vec: &Arc<Mutex<crate::value::VecData>>, store_id: u32, dead: &Arc<Mutex<StructData>>) {
    let mut guard = vec.lock().unwrap();
    let before = guard.len();
    let kept: Vec<Value> = guard
        .iter()
        .filter(|el| !is_link_to(el, store_id, dead))
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
fn fix_entries(map: &Arc<Mutex<crate::value::MapData>>, store_id: u32, dead: &Arc<Mutex<StructData>>) {
    let mut guard = map.lock().unwrap();
    let doomed: Vec<MapKey> = guard
        .iter()
        .filter(|(_, v)| is_link_to(v, store_id, dead))
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
fn fix_container(v: &Value, store_id: u32, dead: &Arc<Mutex<StructData>>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match v {
        Value::Vec(vec) => {
            fix_elements(vec, store_id, dead);
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                if link_target(it, store_id).is_none() {
                    fix_container(it, store_id, dead, depth + 1);
                }
            }
        }
        Value::Map(map) => {
            fix_entries(map, store_id, dead);
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                if link_target(it, store_id).is_none() {
                    fix_container(it, store_id, dead, depth + 1);
                }
            }
        }
        Value::Struct(s) => {
            let names: Vec<String> = {
                let guard = s.lock().unwrap();
                guard.fields.keys().cloned().collect()
            };
            for name in names {
                fix_field(s, &name, store_id, dead);
            }
        }
        _ => {}
    }
}

/// Delete a node: unlink every edge pointing at it, then free the slot.
///
/// Returns the node's value, owned — `@resource` fields inside it stay linear,
/// same as `pool.remove`. Returns `None` if the node is not in this store.
pub fn delete_node(
    store: &Arc<Mutex<StoreData>>,
    node: &Arc<Mutex<StructData>>,
) -> Option<Value> {
    let (store_id, holders) = {
        let mut guard = store.lock().unwrap();
        let idx = guard.index_of(node)?;
        // Free the slot first, so nothing the fixup touches can reach the dying
        // node through the store.
        guard.slots[idx] = None;
        guard.free_list.push(idx as u32);
        guard.slot_of.remove(&node_key(node));
        guard.len -= 1;
        let holders = guard.take_incoming(node);
        (guard.store_id, holders)
    };

    DELETES.fetch_add(1, Ordering::Relaxed);
    HOLDERS_VISITED.fetch_add(holders.len(), Ordering::Relaxed);

    for holder in &holders {
        match holder {
            Backlink::Field(w, field) => {
                if let Some(s) = w.upgrade() {
                    fix_field(&s, field, store_id, node);
                }
            }
            Backlink::Element(w) => {
                if let Some(v) = w.upgrade() {
                    fix_elements(&v, store_id, node);
                }
            }
            Backlink::Entry(w) => {
                if let Some(m) = w.upgrade() {
                    fix_entries(&m, store_id, node);
                }
            }
        }
    }

    Some(Value::Struct(Arc::clone(node)))
}

/// Deep-copy a store, rewriting every edge inside the copy to point at the copy.
///
/// This is the delete-time fixup's machinery pointed at a different job. Delete
/// walks the edges into one node and nulls them; a snapshot walks the edges out of
/// every node and re-points them. Both work because the store knows its own graph.
///
/// Edges *out* of the snapshot are left alone: a link held in a caller's field
/// still names the original node, which is what the caller asked for. Use
/// `corresponding` to translate one across.
///
/// Root edges into the original are untouched for the same reason — nothing about
/// the original changes.
pub fn snapshot_store(store: &Arc<Mutex<StoreData>>) -> Value {
    let (old_id, nodes) = {
        let guard = store.lock().unwrap();
        (guard.store_id, guard.live_nodes())
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

    let new_store = Arc::new(Mutex::new(StoreData::with_type_param(
        store.lock().unwrap().type_param.clone(),
    )));
    crate::value::register_store(&new_store);
    let new_id = new_store.lock().unwrap().store_id;

    for copy in &copies {
        new_store.lock().unwrap().insert(Arc::clone(copy));
    }

    // Now the edges. A link is rewritten only if it names a node of *this* store;
    // a cross-store edge points somewhere this snapshot has no copy of, and
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
        crate::store::register_nested(&Value::Struct(Arc::clone(copy)), 0);
    }

    {
        let mut guard = new_store.lock().unwrap();
        guard.origin_id = Some(old_id);
        guard.origin = map;
    }
    Value::Store(new_store)
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
        Value::Link { store_id, node } if *store_id == old_id => map
            .get(&node_key(node))
            .map(|copy| Value::Link { store_id: new_id, node: Arc::clone(copy) }),
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

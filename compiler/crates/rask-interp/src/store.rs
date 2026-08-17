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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::value::{node_key, Backlink, MapKey, StoreData, StructData, Value};

/// Fixup work, for the delete-cost question the analysis flags as the model's
/// one real regression. `RASK_STORE_STATS=1` prints the totals at exit.
///
/// `HOLDERS_VISITED` is the in-degree actually walked; `EDGES_FIXED` is the
/// edges rewritten. They differ only by backlinks left behind after an edge was
/// overwritten — visits that find nothing and cost one check.
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

/// Record the edge in `value` held by `holder.field`, and any edges nested
/// inside it. O(1) for the common case of a scalar link.
pub fn register_field(holder: &Arc<Mutex<StructData>>, field: &str, value: &Value) {
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

/// Record an edge pushed onto an edge list.
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

/// Record an edge inserted into an index.
pub fn register_entry(map: &Arc<Mutex<crate::value::MapData>>, value: &Value) {
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
                register_field(s, name, v);
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
                register_entry(map, it);
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

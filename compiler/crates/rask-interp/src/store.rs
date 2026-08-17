// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `Store<T>` + `Link<T>` — the delete-time edge fixup (analysis.fourth-option).
//!
//! The model in one line: deleting a node walks every edge pointing at it and
//! sets that edge to `none`, so a dead link never exists and following a live
//! one needs no check.
//!
//! Where a real implementation keeps an intrusive backlink list per node and
//! pays O(in-degree) at delete, this prototype **finds incoming edges by
//! scanning** the store's live nodes plus its registered root holders. Same
//! observable semantics, O(n) instead of O(degree). The scan is a deliberate
//! prototype shortcut: it exists to test what the model *does*, not what it
//! costs. See the delete-cost note in the comparison write-up.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::value::{MapKey, RootRef, StoreData, StructData, Value};

/// Edges actually rewritten, and nodes walked to find them. With
/// `RASK_STORE_STATS=1` the totals print at exit.
///
/// These separate the model's cost from the prototype's: EDGES_FIXED is what a
/// real backlinked implementation would pay (O(in-degree)); NODES_SCANNED is
/// what this prototype pays instead, because it finds incoming edges by
/// scanning rather than by following backlinks.
pub static EDGES_FIXED: AtomicUsize = AtomicUsize::new(0);
pub static NODES_SCANNED: AtomicUsize = AtomicUsize::new(0);
pub static DELETES: AtomicUsize = AtomicUsize::new(0);

pub fn stats_enabled() -> bool {
    std::env::var("RASK_STORE_STATS").is_ok()
}

pub fn print_stats() {
    if !stats_enabled() {
        return;
    }
    eprintln!(
        "store stats: deletes={} edges_fixed={} nodes_scanned={}",
        DELETES.load(Ordering::Relaxed),
        EDGES_FIXED.load(Ordering::Relaxed),
        NODES_SCANNED.load(Ordering::Relaxed),
    );
}

/// How deep the fixup walk descends through owned values inside one node.
/// Links are leaves — the walk never follows one — so this only bounds
/// nesting of plain aggregates.
const MAX_DEPTH: usize = 32;

/// Does this value hold a link to `dead`?
fn is_link_to(v: &Value, store_id: u32, dead: &Arc<Mutex<StructData>>) -> bool {
    match v {
        Value::Link { store_id: sid, node } => *sid == store_id && Arc::ptr_eq(node, dead),
        // An edge field is `Link<T>?`, so the common shape is `Some(link)`.
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            fields.first().map_or(false, |f| is_link_to(f, store_id, dead))
        }
        _ => false,
    }
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

/// Rewrite one field/element slot in place. Returns the replacement value, or
/// `None` when the slot itself should be dropped (an element of an edge list).
///
/// A scalar edge (`target: Link<Entity>?`) becomes `none`. An entry in an edge
/// list (`children: Vec<Link<Entity>>`) or an index (`by_id: Map<K, Link<T>>`)
/// drops out — the database's index-maintenance move.
fn fix_slot(v: &Value, store_id: u32, dead: &Arc<Mutex<StructData>>, depth: usize) -> Option<Value> {
    if is_link_to(v, store_id, dead) {
        EDGES_FIXED.fetch_add(1, Ordering::Relaxed);
        return match v {
            // `Some(link)` → `none`: the edge survives, its target doesn't.
            Value::Enum { .. } => Some(option_none()),
            // A bare link, only reachable as a container element — drop it.
            _ => None,
        };
    }
    Some(fix_in_place(v, store_id, dead, depth))
}

/// Walk an owned value, fixing any edges nested inside it. Containers are
/// behind `Arc<Mutex<..>>`, so this mutates through the shared cell and hands
/// the same value back.
fn fix_in_place(
    v: &Value,
    store_id: u32,
    dead: &Arc<Mutex<StructData>>,
    depth: usize,
) -> Value {
    if depth >= MAX_DEPTH {
        return v.clone();
    }
    match v {
        Value::Vec(vec) => {
            let mut guard = vec.lock().unwrap();
            let fixed: Vec<Value> = guard
                .iter()
                .filter_map(|el| fix_slot(el, store_id, dead, depth + 1))
                .collect();
            **guard = fixed;
            drop(guard);
            v.clone()
        }
        Value::Map(map) => {
            let mut guard = map.lock().unwrap();
            let fixed: Vec<(MapKey, Value)> = guard
                .iter()
                .filter_map(|(k, val)| {
                    fix_slot(val, store_id, dead, depth + 1).map(|nv| (k.clone(), nv))
                })
                .collect();
            guard.clear();
            for (k, val) in fixed {
                guard.insert(k, val);
            }
            drop(guard);
            v.clone()
        }
        Value::Struct(s) => {
            fix_struct(s, store_id, dead, depth + 1);
            v.clone()
        }
        Value::Enum { name, variant, fields, variant_index, origin } => Value::Enum {
            name: name.clone(),
            variant: variant.clone(),
            fields: fields
                .iter()
                .filter_map(|f| fix_slot(f, store_id, dead, depth + 1))
                .collect(),
            variant_index: *variant_index,
            origin: origin.clone(),
        },
        other => other.clone(),
    }
}

/// Fix every edge held by one struct — a node, or a root holder like `World`.
fn fix_struct(s: &Arc<Mutex<StructData>>, store_id: u32, dead: &Arc<Mutex<StructData>>, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    // Snapshot names first: the walk can re-enter this struct through a nested
    // container, and holding the lock across that would deadlock.
    let names: Vec<String> = {
        let guard = s.lock().unwrap();
        guard.fields.keys().cloned().collect()
    };
    for name in names {
        let current = {
            let guard = s.lock().unwrap();
            match guard.fields.get(&name) {
                Some(v) => v.clone(),
                None => continue,
            }
        };
        let replacement = fix_slot(&current, store_id, dead, depth);
        let mut guard = s.lock().unwrap();
        match replacement {
            // A scalar field can't vanish; `fix_slot` only returns `None` for
            // container elements, so this is a defensive no-op.
            None => {
                guard.fields.insert(name, option_none());
            }
            Some(v) => {
                guard.fields.insert(name, v);
            }
        }
    }
}

/// Delete a node: unlink every edge pointing at it, then free the slot.
///
/// Returns the node's value, owned — `@resource` fields inside it stay linear,
/// same as `pool.remove`. Returns `None` if the link's target is not (or is no
/// longer) in this store.
pub fn delete_node(
    store: &Arc<Mutex<StoreData>>,
    node: &Arc<Mutex<StructData>>,
) -> Option<Value> {
    let (store_id, live, roots) = {
        let mut guard = store.lock().unwrap();
        let idx = guard.index_of(node)?;
        // Free the slot first so the fixup walk doesn't visit the dying node.
        guard.slots[idx] = None;
        guard.free_list.push(idx as u32);
        guard.len -= 1;
        guard.roots.retain(|r| r.is_live());
        (guard.store_id, guard.live_nodes(), guard.roots.clone())
    };

    NODES_SCANNED.fetch_add(live.len(), Ordering::Relaxed);
    DELETES.fetch_add(1, Ordering::Relaxed);
    for n in &live {
        fix_struct(n, store_id, node, 0);
    }

    // Root edges — `world.player`, `editor.line_order`. These live beside the
    // store, so the walk reaches them through the registered weak references.
    for root in &roots {
        match root {
            RootRef::Struct(w) => {
                if let Some(s) = w.upgrade() {
                    fix_struct(&s, store_id, node, 0);
                }
            }
            RootRef::Vec(w) => {
                if let Some(v) = w.upgrade() {
                    fix_in_place(&Value::Vec(v), store_id, node, 0);
                }
            }
            RootRef::Map(w) => {
                if let Some(m) = w.upgrade() {
                    fix_in_place(&Value::Map(m), store_id, node, 0);
                }
            }
        }
    }

    Some(Value::Struct(Arc::clone(node)))
}

/// Register every place `v` might hold links into `store` as a root edge.
///
/// Called where a value that could contain links is stored outside the store:
/// a struct literal field, a field assignment, a `Vec.push`. Root edges are a
/// static property in the real design (the compiler knows which fields target
/// which store); the prototype discovers them at the write instead.
pub fn register_roots(v: &Value, depth: usize) {
    if depth >= MAX_DEPTH {
        return;
    }
    match v {
        Value::Struct(s) => {
            let fields: Vec<Value> = {
                let guard = s.lock().unwrap();
                guard.fields.values().cloned().collect()
            };
            for f in &fields {
                if let Some(store) = store_of(f) {
                    store
                        .lock()
                        .unwrap()
                        .register_root(RootRef::Struct(Arc::downgrade(s)));
                }
                register_roots(f, depth + 1);
            }
        }
        Value::Vec(vec) => {
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                if let Some(store) = store_of(it) {
                    store
                        .lock()
                        .unwrap()
                        .register_root(RootRef::Vec(Arc::downgrade(vec)));
                }
                register_roots(it, depth + 1);
            }
        }
        Value::Map(map) => {
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                if let Some(store) = store_of(it) {
                    store
                        .lock()
                        .unwrap()
                        .register_root(RootRef::Map(Arc::downgrade(map)));
                }
                register_roots(it, depth + 1);
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                register_roots(f, depth + 1);
            }
        }
        _ => {}
    }
}

/// The store a link belongs to, looked up in the process-wide registry.
/// Only used for root registration — following a link never needs this.
fn store_of(v: &Value) -> Option<Arc<Mutex<StoreData>>> {
    let store_id = match v {
        Value::Link { store_id, .. } => *store_id,
        Value::Enum { name, variant, fields, .. } if name == "Option" && variant == "Some" => {
            match fields.first() {
                Some(Value::Link { store_id, .. }) => *store_id,
                _ => return None,
            }
        }
        _ => return None,
    };
    crate::value::store_by_id(store_id)
}

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

/// Whether any value can hold a link yet. A link comes from a rack, so until
/// the program makes one there is nothing to record and the registration walks
/// return at once. Without this, pushing a value walked all of it, so a tree
/// built by pushing each level into the next was quadratic to build (#2084).
pub fn links_possible() -> bool {
    RACKS_MADE.load(Ordering::Relaxed) > 0
}

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

// The walks below recurse as deep as the value they are given. They never
// follow a link, and a value can't reach itself without going through one, so
// each walk ends with the value, the way `deep_clone` does. A depth cap here
// used to leave a link nested more than about 16 levels down unrecorded, and
// deleting its target then left it pointing at a dead node (#1837).

/// The node a value names, seeing through the `Link<T>?` optional every edge
/// field carries.
fn link_target(v: &Value, rack_id: u32) -> Option<Arc<Mutex<StructData>>> {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| link_target(v, rack_id));
    }
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

/// `v` without the optionals around a payload that isn't a link: `Cursor?`
/// holds its links where `Cursor` does, `Target?` where `Target` does.
fn strip_some(mut v: &Value) -> &Value {
    while let Value::Enum { name, variant, fields, .. } = v {
        if name != "Option" || variant != "Some" || any_link(v).is_some() {
            break;
        }
        match fields.first() {
            Some(inner) => v = inner,
            None => break,
        }
    }
    v
}

/// Whether `v`'s own fields can hold links behind it: a tuple, an enum, or an
/// optional around either. An optional around a link is the link.
fn has_value_fields(v: &Value) -> bool {
    match v {
        Value::Tuple(_) => true,
        Value::Enum { name, .. } => name != "Option" || any_link(v).is_none(),
        _ => false,
    }
}

fn is_link_to(v: &Value, rack_id: u32, dead: &Arc<Mutex<StructData>>) -> bool {
    link_target(v, rack_id).is_some_and(|n| Arc::ptr_eq(&n, dead))
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
    if !links_possible() {
        return;
    }
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
            forget_container_edges(old);
            // A loop still walking the vector that was just replaced is
            // walking one its owner dropped; native frees it and the loop
            // ends there.
            if let Value::Vec(vec) = old {
                if !matches!(value, Value::Vec(new) if Arc::ptr_eq(vec, new)) {
                    vec.lock().unwrap().detach_cursors();
                }
            }
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
    // A tuple field holds its links in the field itself: the tuple is
    // rebuilt with `none` in their place when a target dies.
    let mut in_tuple = Vec::new();
    tuple_links(value, &mut in_tuple);
    for (rack_id, target) in in_tuple {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack.lock().unwrap().register_backlink(
                &target,
                Backlink::Field(Arc::downgrade(holder), field.to_string()),
            );
        }
    }
    register_nested(value);
}

/// A loop's temporary vector of links is done with: drop its records.
pub fn forget_vec_links(value: &Value) {
    if !links_possible() {
        return;
    }
    forget_container_edges(value);
}

/// Drop the records a container held, when the container itself is being
/// replaced. Descends through options and nested containers; stops at a struct,
/// whose data may still be reachable through another name.
fn forget_container_edges(value: &Value) {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| forget_container_edges(value));
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
                    None => forget_container_edges(it),
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
                    None => forget_container_edges(it),
                }
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                forget_container_edges(f);
            }
        }
        Value::Tuple(items) => {
            for f in items.iter() {
                forget_container_edges(f);
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
    if !links_possible() {
        return;
    }
    // A struct or enum element is walked here and only here, so nested
    // vectors of structs cost one pass, not two per level.
    // An element of struct type holds its links in fields of its own: those
    // are edges the element's fields record, and delete nulls them in place
    // (#1804).
    if let s @ Value::Struct(_) = strip_some(value) {
        register_nested(s);
        return;
    }
    // A struct an enum, a tuple or an inner container carries records its own
    // edges the same way (#1844): a `Vec<Link<T>>` pushed into a grid holds
    // its links itself, whether a push recorded them or it arrived fresh from
    // a `clone`.
    if matches!(value, Value::Enum { .. } | Value::Tuple(_) | Value::Vec(_) | Value::Map(_)) {
        register_nested(value);
    }
    // A tuple or an enum element is a value: its links are recorded against
    // the vector, and delete rebuilds the element in place.
    let mut links = Vec::new();
    match any_link(value) {
        Some(l) => links.push(l),
        None => tuple_links(value, &mut links),
    }
    for (rack_id, target) in links {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack
                .lock()
                .unwrap()
                .register_backlink(&target, Backlink::Element(Arc::downgrade(vec)));
        }
    }
}

/// Whether a loop entry names a deleted node: a link, or the link in an
/// `enumerate` pair.
pub fn entry_is_dead(v: &Value) -> bool {
    match v {
        Value::Tuple(items) => items.last().is_some_and(link_is_dead),
        _ => link_is_dead(v),
    }
}

/// Whether `v` is a link whose node has been deleted. A loop over a vector of
/// links walks a snapshot, and a delete in its body takes the entry out of the
/// vector itself; the loop skips what is no longer there (mem.racks/RK15).
pub fn link_is_dead(v: &Value) -> bool {
    let Some((rack_id, node)) = any_link(v) else { return false };
    match crate::value::rack_by_id(rack_id) {
        Some(rack) => rack.lock().unwrap().index_of(&node).is_none(),
        None => true,
    }
}

/// The links a tuple holds, at any depth of tuple nesting. A struct inside one
/// is a cell with fields of its own and records itself.
fn tuple_links(v: &Value, out: &mut Vec<(u32, Arc<Mutex<StructData>>)>) {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| tuple_links(v, out));
    }
    let items: &[Value] = match v {
        Value::Tuple(items) => items,
        // An enum's variant fields are values the same way (#1801), and so
        // is an optional's payload unless it is the link itself, which
        // `any_link` already sees through.
        Value::Enum { fields, .. } if has_value_fields(v) => fields,
        _ => return,
    };
    for item in items {
        match item {
            Value::Tuple(_) | Value::Enum { .. } if any_link(item).is_none() => tuple_links(item, out),
            _ => {
                if let Some(l) = any_link(item) {
                    out.push(l);
                }
            }
        }
    }
}

/// `v` with every link to `dead` inside its tuples set to `none`, or `None`
/// when nothing in it named `dead`.
fn tuple_nulled(v: &Value, rack_id: u32, dead: &Arc<Mutex<StructData>>) -> Option<Value> {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| tuple_nulled(v, rack_id, dead));
    }
    let items: &[Value] = match v {
        Value::Tuple(items) => items,
        Value::Enum { fields, .. } if has_value_fields(v) => fields,
        _ => return None,
    };
    let mut changed = false;
    let rebuilt: Vec<Value> = items
        .iter()
        .map(|item| {
            if is_link_to(item, rack_id, dead) {
                changed = true;
                EDGES_FIXED.fetch_add(1, Ordering::Relaxed);
                return Value::none();
            }
            match tuple_nulled(item, rack_id, dead) {
                Some(inner) => {
                    changed = true;
                    inner
                }
                None => item.clone(),
            }
        })
        .collect();
    if !changed {
        return None;
    }
    Some(match v {
        Value::Enum { name, variant, variant_index, origin, .. } => Value::Enum {
            name: name.clone(),
            variant: variant.clone(),
            fields: rebuilt,
            variant_index: *variant_index,
            origin: origin.clone(),
        },
        _ => Value::tuple(rebuilt),
    })
}

/// Whether a value is a tuple or an enum (not an optional): the values whose
/// links a local's own storage has to hold the records for.
pub fn may_hold_value_links(v: &Value) -> bool {
    has_value_fields(strip_some(v))
}

/// A local's value changed. A tuple holding links records each against the
/// local's storage, and the one it replaced gives its records up. Nothing else
/// is recorded here: a bare link in a local is a root, and a struct records
/// itself.
pub fn register_var(cell: &Arc<Mutex<Value>>, old: Option<&Value>, value: &Value) {
    if !links_possible() {
        return;
    }
    let key = BacklinkKey { holder: Arc::as_ptr(cell) as usize, field: None };
    let mut before = Vec::new();
    if let Some(old) = old {
        tuple_links(old, &mut before);
    }
    for (rack_id, target) in before {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack.lock().unwrap().unregister_backlink(&target, &key);
        }
    }
    let mut now = Vec::new();
    tuple_links(value, &mut now);
    for (rack_id, target) in now {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack.lock().unwrap().register_backlink(&target, Backlink::Var(Arc::downgrade(cell)));
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
    if !links_possible() {
        return;
    }
    if let s @ Value::Struct(_) = strip_some(value) {
        register_nested(s);
        return;
    }
    if matches!(value, Value::Enum { .. } | Value::Tuple(_) | Value::Vec(_) | Value::Map(_)) {
        register_nested(value);
    }
    let mut in_value = Vec::new();
    tuple_links(value, &mut in_value);
    for (rack_id, target) in in_value {
        if let Some(rack) = crate::value::rack_by_id(rack_id) {
            rack.lock().unwrap().register_backlink(&target, Backlink::Entry(Arc::downgrade(map)));
        }
    }
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
/// builds a fresh `Vec`, whose entries nothing has recorded yet), and a value
/// bound whole (`let w = rows.clone()`, a `filter` result), whose links are
/// edges of the new binding's own (mem.racks/RK15). An inner vector records
/// its elements too, so a clone of a `Vec<Vec<Link<T>>>` loses its links on a
/// delete like the source does. Records are one per (holder, target), so a
/// vector whose pushes already did it changes nothing.
pub fn register_nested(value: &Value) {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| register_nested(value));
    }
    if !links_possible() {
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
            }
        }
        Value::Map(map) => {
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                register_entry(map, None, it);
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                register_nested(f);
            }
        }
        Value::Tuple(items) => {
            for f in items.iter() {
                register_nested(f);
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
/// The vectors a dying node holds by value are freed with it natively. Mark
/// them, so a loop still walking one stops as the native one does.
fn mark_owned_vecs_freed(value: &Value) {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| mark_owned_vecs_freed(value));
    }
    match value {
        Value::Struct(s) => {
            let fields: Vec<Value> = s.lock().unwrap().fields.values().cloned().collect();
            for v in &fields {
                mark_owned_vecs_freed(v);
            }
        }
        Value::Tuple(items) => {
            for v in items.iter() {
                mark_owned_vecs_freed(v);
            }
        }
        Value::Vec(v) => v.lock().unwrap().freed = true,
        _ => {}
    }
}

fn forget_own_edges(value: &Value) {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| forget_own_edges(value));
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
                forget_own_edges(v);
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
                forget_own_edges(it);
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
                forget_own_edges(it);
            }
        }
        Value::Enum { fields, .. } => {
            for f in fields {
                forget_own_edges(f);
            }
        }
        Value::Tuple(items) => {
            for f in items.iter() {
                forget_own_edges(f);
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
    if let Some(fixed) = tuple_nulled(&current, rack_id, dead) {
        holder.lock().unwrap().fields.insert(field.to_string(), fixed);
        return;
    }
    if is_link_to(&current, rack_id, dead) {
        EDGES_FIXED.fetch_add(1, Ordering::Relaxed);
        holder.lock().unwrap().fields.insert(field.to_string(), Value::none());
        return;
    }
    // The backlink may name a field whose edge now sits inside a container the
    // field holds, rather than in the field itself.
    fix_container(&current, rack_id, dead);
}

/// Drop entries pointing at the dying node from an edge list. A list of live
/// things loses the entry rather than holding a `none`.
fn fix_elements(vec: &Arc<Mutex<crate::value::VecData>>, rack_id: u32, dead: &Arc<Mutex<StructData>>) {
    let mut guard = vec.lock().unwrap();
    // A tuple or enum element keeps its place and loses the link inside it.
    for el in guard.items.iter_mut() {
        if let Some(fixed) = tuple_nulled(el, rack_id, dead) {
            *el = fixed;
        }
    }
    let doomed: Vec<usize> = guard
        .iter()
        .enumerate()
        .filter(|(_, el)| is_link_to(el, rack_id, dead))
        .map(|(i, _)| i)
        .collect();
    if !doomed.is_empty() {
        EDGES_FIXED.fetch_add(doomed.len(), Ordering::Relaxed);
        // Back to front, so each index is still where the loops last saw it.
        for &i in doomed.iter().rev() {
            guard.removed_at(i);
            guard.items.remove(i);
        }
    }
}


/// Drop index entries pointing at the dying node — the database's
/// index-maintenance move.
fn fix_entries(map: &Arc<Mutex<crate::value::MapData>>, rack_id: u32, dead: &Arc<Mutex<StructData>>) {
    let mut guard = map.lock().unwrap();
    for (_, v) in guard.iter_mut() {
        if let Some(fixed) = tuple_nulled(v, rack_id, dead) {
            *v = fixed;
        }
    }
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
fn fix_container(v: &Value, rack_id: u32, dead: &Arc<Mutex<StructData>>) {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| fix_container(v, rack_id, dead));
    }
    match v {
        Value::Vec(vec) => {
            fix_elements(vec, rack_id, dead);
            let items: Vec<Value> = vec.lock().unwrap().iter().cloned().collect();
            for it in &items {
                if link_target(it, rack_id).is_none() {
                    fix_container(it, rack_id, dead);
                }
            }
        }
        Value::Map(map) => {
            fix_entries(map, rack_id, dead);
            let items: Vec<Value> = map.lock().unwrap().values().cloned().collect();
            for it in &items {
                if link_target(it, rack_id).is_none() {
                    fix_container(it, rack_id, dead);
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
        guard.epoch += 1;
        guard.slot_of.remove(&node_key(node));
        guard.len -= 1;
        let holders = guard.take_incoming(node);
        (guard.rack_id, holders)
    };

    // The dying node's own edges go before its incoming ones are walked: its
    // targets must stop naming it, or a later delete of one of them walks a
    // record for a node that no longer exists.
    forget_own_edges(&Value::Struct(Arc::clone(node)));
    mark_owned_vecs_freed(&Value::Struct(Arc::clone(node)));

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
            Backlink::Var(w) => {
                if let Some(cell) = w.upgrade() {
                    let current = cell.lock().unwrap().clone();
                    if let Some(fixed) = tuple_nulled(&current, rack_id, node) {
                        *cell.lock().unwrap() = fixed;
                    }
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

    let new_rack = Arc::new(Mutex::new(RackData::new()));
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
            if let Some(rewritten) = rewrite_links(&value, old_id, new_id, &map) {
                copy.lock().unwrap().fields.insert(name.clone(), rewritten);
            }
        }
        crate::rack::register_nested(&Value::Struct(Arc::clone(copy)));
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
) -> Option<Value> {
    if crate::stack_nearly_exhausted() {
        return crate::grow_interp_stack(|| rewrite_links(value, old_id, new_id, map));
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
                .map(|v| match rewrite_links(v, old_id, new_id, map) {
                    Some(nv) => {
                        changed = true;
                        nv
                    }
                    None => v.clone(),
                })
                .collect();
            changed.then(|| Value::vec(next))
        }
        Value::Tuple(items) => {
            let mut changed = false;
            let next: Vec<Value> = items
                .iter()
                .map(|v| match rewrite_links(v, old_id, new_id, map) {
                    Some(nv) => {
                        changed = true;
                        nv
                    }
                    None => v.clone(),
                })
                .collect();
            changed.then(|| Value::tuple(next))
        }
        // An edge field is `Link<T>?`, so the common case arrives wrapped in an
        // Option rather than bare. Enum payloads generally: rewrite each field.
        Value::Enum { name, variant, fields, variant_index, origin } => {
            let mut changed = false;
            let next: Vec<Value> = fields
                .iter()
                .map(|f| match rewrite_links(f, old_id, new_id, map) {
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
                if let Some(nv) = rewrite_links(&v, old_id, new_id, map) {
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
                match rewrite_links(&v, old_id, new_id, map) {
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

<!-- id: ctrl.loops -->
<!-- status: decided -->
<!-- summary: Index/handle iteration with no collection borrow, copy-out for values -->
<!-- depends: memory/borrowing.md, control/control-flow.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-interp/ -->

# Loops

Loops yield indices/handles (never borrowed values). Access uses existing collection borrow rules. Value extraction follows the 16-byte Copy threshold.

## Loop Syntax

| Rule | Description |
|------|-------------|
| **LP1: Index iteration** | `for binding in collection` yields indices (Vec) or handles (Pool) or keys (Map) |
| **LP2: No collection borrow** | Loop does not borrow the collection — only captures length at start |
| **LP3: Copy bindings** | Loop variable is a Copy value (index/handle/key), not a reference |

```rask
for <binding> in <collection> { ... }
```

| Collection Type | Binding Type | Semantics |
|----------------|--------------|-----------|
| `Vec<T>` | `usize` | Index into vec |
| `Pool<T>` | `Handle<T>` | Generational handle |
| `Map<K,V>` | `K` (requires K: Copy) | Key (copied) |
| `Range` (`0..n`) | Integer | Range value |

<!-- test: skip -->
```rask
const items = Vec.new()
for i in items {              // i: usize
    items[i].process()        // Expression-scoped access
    items.push(new_item)      // OK: no borrow held
}

const entities = Pool.new()
for h in entities {           // h: Handle<Entity>
    entities[h].update()      // Expression-scoped access
    entities.remove(h)        // OK: no borrow held
}
```

## Value Extraction

| Rule | Description |
|------|-------------|
| **LP4: Access via index** | Inside loop, access elements with `collection[binding]` |
| **LP5: Copy-out small values** | Types ≤16 bytes auto-copy on extraction |
| **LP6: Clone large values** | Types >16 bytes require explicit `.clone()` |
| **LP7: Take ownership** | Use `collection.take_all()` for consuming iteration |

<!-- test: skip -->
```rask
// Copy-out (≤16 bytes)
for i in positions {
    const pos = positions[i]  // Vec3: 12 bytes, auto-copied
    process(pos)
}

// Clone required (>16 bytes)
for i in entities {
    const e = entities[i].clone()  // Entity: large, explicit clone
    archive(e)
}

// Take ownership (consuming)
for item in vec.take_all() {  // vec now empty
    process(item)             // owns item
}
```

## Collection Access During Iteration

| Rule | Description |
|------|-------------|
| **LP8: Mutation allowed** | Collection can be mutated during iteration (indices may become stale) |
| **LP9: Length captured** | Loop length captured at start — new items not visited |
| **LP10: Stale index risk** | Removals during iteration may cause index out-of-bounds (programmer responsibility) |

<!-- test: skip -->
```rask
for i in vec {
    vec[i].field = x          // OK: mutate element
    vec.push(new_item)        // OK: not visited (length captured at start)
}

for h in pool {
    if pool[h].should_remove {
        pool.remove(h)        // OK: handle becomes invalid, no future access
    }
}
```

## Desugaring

Index iteration does not create a borrow — it captures length and generates indices.

<!-- test: skip -->
```rask
// Index iteration (Vec, Pool, Map):
for i in vec { body }

// Equivalent to:
{
    const _len = vec.len()
    let _pos = 0
    while _pos < _len {
        const i = _pos
        body
        _pos += 1
    }
}
```

<!-- test: skip -->
```rask
// Consume iteration (take_all):
for item in vec.take_all() { body }

// Equivalent to:
{
    const _iter = vec.take_all()  // Takes ownership, vec now empty
    while _iter.next() is Some(item) {
        body
    }
    // _iter drops here, dropping any remaining items
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Grow during iteration | LP9 | New items not visited (length captured at start) |
| Shrink during iteration | LP10 | Stale index may panic — programmer responsibility |
| Remove current handle (Pool) | LP8 | OK if no further access to that handle |
| Nested loops same collection | LP2 | Allowed (no borrow conflict) |
| Break/continue | — | Standard semantics (exit or skip iteration) |
| Empty collection | — | Zero iterations |

---

## Appendix (non-normative)

### Rationale

**LP1/LP2 (index iteration, no borrow):** I wanted to eliminate stored references while keeping loop syntax simple. Indices are Copy values, not borrows, so no lifetime tracking needed. This matches Go, C, and Zig semantics.

**LP2 (no collection borrow):** Enables mutation during iteration without borrow conflicts. Each `collection[i]` access is expression-scoped (instant view per `mem.borrowing/V1`), so no persistent borrow exists. The cost is stale index risk, which I consider acceptable — same tradeoff as Go.

**LP4/LP5/LP6 (value extraction):** Follows value semantics (`mem.value-semantics`). Small types copy out automatically, large types require explicit `.clone()`. This makes copy costs visible without ceremony for common cases (loop over positions, indices, etc.).

**LP7 (take_all):** Ownership transfer is explicit. `vec.take_all()` empties the collection and yields owned items. This prevents accidental consumption while keeping consuming iteration ergonomic.

**LP9/LP10 (length capture, stale risk):** I chose to capture length at loop start rather than recheck on each iteration. This makes iteration cost predictable (no hidden checks) and allows growth without infinite loops. The downside is stale indices if you remove during iteration — same risk as manual `for (int i = 0; i < len; i++)` in C.

### Patterns & Guidance

**When to use which pattern:**

| Goal | Pattern | Example |
|------|---------|---------|
| Read all elements | Index iteration | `for i in vec { process(vec[i]) }` |
| Mutate elements in place | Index iteration | `for i in vec { vec[i].field = x }` |
| Copy small values out | Index iteration + copy | `for i in vec { const v = vec[i]; use(v) }` |
| Clone large values | Index iteration + clone | `for i in vec { const v = vec[i].clone(); use(v) }` |
| Take ownership (consume) | `take_all()` | `for item in vec.take_all() { own(item) }` |
| Multi-statement mutation | `with...as` or `modify()` | See `mem.borrowing/W1` |

**Multi-statement element mutation:**

<!-- test: skip -->
```rask
// Single-statement: direct access
for i in vec {
    vec[i].field = value
}

// Multi-statement: use with...as
for i in vec {
    with vec[i] as item {
        item.health -= damage
        item.last_hit = now()
    }
}

// Or collect + modify
const indices = (0..vec.len()).collect()
for i in indices {
    vec.modify(i, |item| {
        item.health -= damage
        item.last_hit = now()
    })
}
```

**Removing during iteration (Pool):**

<!-- test: skip -->
```rask
// Safe: collect handles first
const to_remove = pool.handles().filter(|h| pool[h].dead).collect()
for h in to_remove {
    pool.remove(h)
}

// Risky: remove during iteration (OK if no further access)
for h in pool {
    if pool[h].should_remove {
        pool.remove(h)  // OK: don't use h again
    }
}
```

**Nested iteration:**

<!-- test: skip -->
```rask
// No borrow conflict — each access is instant
for i in vec {
    for j in vec {
        if vec[i].collides_with(vec[j]) {
            handle_collision(i, j)
        }
    }
}
```

### IDE Integration

The IDE annotates loop bindings with their type and source semantics.

| Annotation | Meaning |
|------------|---------|
| `i: usize [index into vec]` | Loop variable is an index |
| `h: Handle<Entity> [handle into pool]` | Loop variable is a handle |
| `vec[i] [instant view]` | Collection access is expression-scoped |

<!-- test: skip -->
```rask
for i in vec {              // i: usize [index into vec]
    vec[i].process()        // [instant view: released at ;]
    vec.push(item)          // [OK: no borrow held]
}
```

Hover on `for i in collection` shows:
- "Index iteration: loop captures `collection.len()` at start, yields indices 0..len"
- "Collection remains accessible inside loop"
- "Length changes during iteration not reflected"

### See Also

- [Borrowing](../memory/borrowing.md) — Collection view semantics (`mem.borrowing`)
- [Value Semantics](../memory/value-semantics.md) — Copy threshold (`mem.value-semantics`)
- [Collections](../stdlib/collections.md) — Vec, Pool, Map APIs (`std.collections`)
- [Iterator Protocol](iterator-protocol.md) — Iterator trait and adapters (`ctrl.iterator`)

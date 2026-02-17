<!-- id: ctrl.loops -->
<!-- status: decided -->
<!-- summary: Value-first iteration (borrowed elements by default), index mode for mutation -->
<!-- depends: memory/borrowing.md, control/control-flow.md, stdlib/iteration.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-interp/ -->

# Loops

Loops yield borrowed values by default (read-only). Index/handle mode requires explicit syntax for mutation.

## Loop Syntax

| Rule | Description |
|------|-------------|
| **LP1: Value iteration** | `for binding in collection` yields borrowed elements (read-only) |
| **LP2: Index mode explicit** | Use `for i in 0..vec.len()` or `for h in pool.handles()` for mutation |
| **LP3: Collection accessible** | Loop does not prevent collection access (expression-scoped borrows) |

```rask
for <binding> in <collection> { ... }
```

| Collection Type | Value Mode (Default) | Index Mode (Explicit) |
|----------------|---------------------|----------------------|
| `Vec<T>` | Borrowed `T` | `0..vec.len()` yields `usize` |
| `Pool<T>` | Borrowed `T` | `.handles()` yields `Handle<T>` |
| `Map<K,V>` | `(K, borrowed V)` | `.keys()` yields `K` |
| `Range` (`0..n`) | Integer | (direct iteration) |

<!-- test: skip -->
```rask
const items = Vec.new()
for item in items {           // item: borrowed T
    item.process()            // Read-only access
    print(item.name)          // Natural iteration
}

// Index mode for mutation
for i in 0..items.len() {
    items[i].health -= 10     // Mutate in place
    items.push(new_item)      // OK: no borrow held
}

const entities = Pool.new()
for entity in entities {      // entity: borrowed Entity
    entity.update()           // Read-only access
}

// Handle mode for mutation
for h in entities.handles() {
    entities[h].health -= 10  // Mutate via handle
    entities.remove(h)        // OK if no further access
}
```

## Value Mode Constraints

| Rule | Description |
|------|-------------|
| **LP4: Read-only** | Value mode yields borrowed elements (cannot mutate) |
| **LP5: Copy-out small fields** | Access fields: `item.field` copies if field ≤16 bytes |
| **LP6: No take parameters** | Cannot pass borrowed items to `take` parameters |
| **LP7: Take ownership** | Use `collection.take_all()` for consuming iteration |

<!-- test: skip -->
```rask
// Read-only value iteration
for item in items {
    print(item.name)          // Copy small field
    item.display()            // Call methods (expression-scoped borrow)
    // item.health -= 10      // ERROR: cannot mutate borrowed value
}

// Index mode for mutation
for i in 0..items.len() {
    items[i].health -= 10     // Mutate via index
}

// Take ownership (consuming)
for item in vec.take_all() {  // vec now empty
    process(item)             // owns item
}
```

## Collection Mutation

| Rule | Description |
|------|-------------|
| **LP8: Value mode read-only** | Value iteration prevents structural mutation (insert/remove) |
| **LP9: Index mode allows mutation** | Index/handle mode allows mutation (programmer responsibility) |
| **LP10: Length captured** | Index loops capture length at start — new items not visited |

<!-- test: skip -->
```rask
// Value mode: read-only
for item in vec {
    print(item.name)          // OK: read-only
    // vec.push(x)            // ERROR: cannot mutate during value iteration
}

// Index mode: mutation allowed
for i in 0..vec.len() {
    vec[i].field = x          // OK: mutate element
    vec.push(new_item)        // OK: not visited (length captured at start)
}

for h in pool.handles() {
    if pool[h].should_remove {
        pool.remove(h)        // OK: handle becomes invalid, no future access
    }
}
```

## Desugaring

Value iteration creates expression-scoped borrows per element access.

<!-- test: skip -->
```rask
// Value iteration (Vec):
for item in vec { body }

// Equivalent to:
{
    const _len = vec.len()
    let _pos = 0
    while _pos < _len {
        const item = vec[_pos]  // Expression-scoped borrow
        body
        _pos += 1
    }
}
```

<!-- test: skip -->
```rask
// Index iteration (explicit):
for i in 0..vec.len() { body }

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
| Mutate in value loop | LP8 | Compile error (value mode is read-only) |
| Grow during index iteration | LP10 | New items not visited (length captured at start) |
| Shrink during index iteration | LP10 | Stale index may panic — programmer responsibility |
| Remove current handle (Pool) | LP9 | OK in handle mode if no further access |
| Nested loops same collection | LP3 | Allowed (expression-scoped borrows) |
| Break/continue | — | Standard semantics (exit or skip iteration) |
| Empty collection | — | Zero iterations |

---

## Appendix (non-normative)

### Rationale

**LP1 (value iteration default):** I chose borrowed values as the default because it matches Python, Rust, Go, JavaScript expectations. Most iteration is read-only (80%+ of real code), so optimizing for the common case reduces ceremony. This decision was validated against METRICS.md targets — natural iteration should not require index manipulation.

**LP2 (index mode explicit):** Mutation requires explicit syntax (`0..vec.len()` or `.handles()`). This makes the mutation intent clear and matches the "transparency of cost" principle — you see you're doing index-based access, not value iteration.

**LP3 (collection accessible):** Each element access in value mode is statement-scoped (per `mem.borrowing/V1`), so the collection remains accessible. No block-scoped borrow exists. This enables natural patterns without borrow conflicts.

**LP7 (take_all):** Ownership transfer is explicit. `vec.take_all()` empties the collection and yields owned items. This prevents accidental consumption while keeping consuming iteration ergonomic.

**LP10 (length capture):** I chose to capture length at loop start rather than recheck on each iteration. This makes iteration cost predictable (no hidden checks) and allows growth without infinite loops. The downside is stale indices if you remove during iteration — same risk as manual `for (int i = 0; i < len; i++)` in C.

### Patterns & Guidance

**When to use which pattern:**

| Goal | Pattern | Example |
|------|---------|---------|
| Read all elements | Value iteration | `for item in vec { process(item) }` |
| Mutate elements in place | Index iteration | `for i in 0..vec.len() { vec[i].field = x }` |
| Clone values | Value iteration + clone | `for item in vec { const v = item.clone(); use(v) }` |
| Take ownership (consume) | `take_all()` | `for item in vec.take_all() { own(item) }` |
| Iterate Pool by handle | `.handles()` | `for h in pool.handles() { pool[h].update() }` |

**Value iteration (read-only):**

<!-- test: skip -->
```rask
// Natural, readable iteration
for item in vec {
    print(item.name)
    item.display()
}

for entity in entities {
    if entity.health > 0 {
        entity.render()
    }
}
```

**Index mode for mutation:**

<!-- test: skip -->
```rask
// Explicit index mode for mutation
for i in 0..vec.len() {
    vec[i].health -= damage
    vec[i].last_hit = now()
}

// Handle mode for Pool
for h in pool.handles() {
    pool[h].health -= damage
    if pool[h].health <= 0 {
        pool.remove(h)
    }
}
```

**Removing during iteration (Pool):**

<!-- test: skip -->
```rask
// Safe: collect handles first
const to_remove = Vec.new()
for entity in pool {
    if entity.dead {
        to_remove.push(/* need handle */)  // Note: value mode doesn't provide handle
    }
}

// Better: use handle mode
const to_remove = pool.handles().filter(|h| pool[h].dead).collect()
for h in to_remove {
    pool.remove(h)
}
```

**Nested iteration:**

<!-- test: skip -->
```rask
// No borrow conflict — each access is expression-scoped
for a in entities {
    for b in entities {
        if a.collides_with(b) {
            handle_collision(a, b)
        }
    }
}
```

### IDE Integration

The IDE annotates loop bindings with their type and source semantics.

| Annotation | Meaning |
|------------|---------|
| `item: borrowed T` | Loop variable is a borrowed value (read-only) |
| `i: usize [index into vec]` | Loop variable is an index |
| `h: Handle<Entity> [handle into pool]` | Loop variable is a handle |

<!-- test: skip -->
```rask
for item in vec {           // item: borrowed T [read-only]
    item.process()          // [expression-scoped borrow]
    // vec.push(x)          // [ERROR: cannot mutate during value iteration]
}

for i in 0..vec.len() {     // i: usize [index into vec]
    vec[i].process()        // [expression-scoped view]
    vec.push(item)          // [OK: no borrow held]
}
```

Hover on `for item in collection` shows:
- "Value iteration: yields borrowed elements (read-only)"
- "Cannot mutate collection structure during iteration"
- "Use index mode (0..collection.len()) for mutation"

Hover on `for i in 0..collection.len()` shows:
- "Index iteration: loop captures length at start, yields indices 0..len"
- "Collection remains accessible inside loop"
- "Length changes during iteration not reflected"

### See Also

- [Borrowing](../memory/borrowing.md) — Collection view semantics (`mem.borrowing`)
- [Value Semantics](../memory/value-semantics.md) — Copy threshold (`mem.value-semantics`)
- [Collections](../stdlib/collections.md) — Vec, Pool, Map APIs (`std.collections`)
- [Iterator Protocol](iterator-protocol.md) — Iterator trait and adapters (`ctrl.iterator`)

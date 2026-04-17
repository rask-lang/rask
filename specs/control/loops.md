<!-- id: ctrl.loops -->
<!-- status: decided -->
<!-- summary: Value-first iteration (borrowed elements by default), mutable iteration for in-place mutation, index mode for structural mutation -->
<!-- depends: memory/borrowing.md, control/control-flow.md, stdlib/iteration.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-interp/ -->

# Loops

Loops yield borrowed values by default (read-only). `for mutate` provides in-place element mutation. Index/handle mode for structural mutation.

## Loop Syntax

| Rule | Description |
|------|-------------|
| **LP1: Value iteration** | `for binding in collection` yields borrowed elements (read-only) |
| **LP2: Index mode explicit** | Use `for i in 0..vec.len()` or `for h in pool.handles()` for structural mutation |
| **LP3: Collection accessible** | Loop does not prevent collection access (inline expression access) |
| **LP11: Mutable iteration** | `for mutate binding in collection` yields mutable access to each element |
| **LP12: Mutable binding** | The `mutate` keyword applies to the loop binding. Each use of the binding desugars to mutable inline access on the current element |

```rask
for <binding> in <collection> { ... }
for mutate <binding> in <collection> { ... }
```

| Collection Type | Value Mode (Default) | Mutable Mode | Index Mode (Explicit) |
|----------------|---------------------|--------------|----------------------|
| `Vec<T>` | Borrowed `T` | Mutable `T` | `0..vec.len()` yields `usize` |
| `Pool<T>` | Borrowed `T` | Mutable `T` | `.handles()` yields `Handle<T>` |
| `Map<K,V>` | `(K, borrowed V)` | `(K, mutable V)` | `.keys()` yields `K` |
| `Range` (`0..n`) | Integer | N/A | (direct iteration) |

<!-- test: skip -->
```rask
const items = Vec.new()
for item in items {           // item: borrowed T
    item.process()            // Read-only access
    print(item.name)          // Natural iteration
}

// Mutable mode for in-place mutation
for mutate item in items {
    item.health -= 10         // Mutate in place
    item.last_updated = now()
}

// Index mode for structural mutation
for i in 0..items.len() {
    items[i].health -= 10     // Mutate in place
    items.push(new_item)      // OK: structural mutation allowed in index mode
}

const entities = Pool.new()
for mutate entity in entities {
    entity.health -= 10       // Mutate via mutable iteration
    entity.velocity *= 0.9
}

// Handle mode for structural mutation (removal)
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
    item.display()            // Call methods (inline access)
    // item.health -= 10      // ERROR: cannot mutate borrowed value
}

// Use mutable iteration instead
for mutate item in items {
    item.health -= 10         // OK: mutable access
}

// Take ownership (consuming)
for item in vec.take_all() {  // vec now empty
    process(item)             // owns item
}
```

## Mutable Mode Constraints

| Rule | Description |
|------|-------------|
| **LP13: In-place mutation** | `item.field = x` mutates the element in place. `item = x` replaces the entire element |
| **LP14: No structural mutation** | Cannot insert, remove, or clear during mutable iteration |
| **LP15: Collection readable** | Other elements accessible via inline expression access (same as LP3) |
| **LP16: No take parameters** | Cannot pass `item` to `take` parameters (same as LP6) |

<!-- test: skip -->
```rask
// Mutable iteration
for mutate item in items {
    item.health -= damage          // LP13: in-place mutation
    item.name = "updated"          // LP13: field replacement
    const other = items[0].health  // LP15: read other elements
    // items.push(new_item)        // ERROR LP14: structural mutation
}

// Map: keys are always copies, values are mutable
for mutate (key, value) in config {
    value.count += 1
    value.last_access = now()
    // key = "new_key"             // ERROR: keys are copies, not mutable bindings
}
```

## Collection Mutation

| Rule | Description |
|------|-------------|
| **LP8: Value mode read-only** | Value iteration prevents all mutation (element and structural) |
| **LP8a: Mutable mode in-place only** | Mutable iteration allows element mutation, prevents structural mutation |
| **LP9: Index mode allows mutation** | Index/handle mode allows all mutation (programmer responsibility) |
| **LP10: Length captured** | Index and mutable loops capture length at start — new items not visited |

<!-- test: skip -->
```rask
// Value mode: read-only
for item in vec {
    print(item.name)          // OK: read-only
    // vec.push(x)            // ERROR: cannot mutate during value iteration
}

// Mutable mode: element mutation, no structural changes
for mutate item in vec {
    item.field = x            // OK: in-place mutation
    // vec.push(new_item)     // ERROR: structural mutation forbidden
}

// Index mode: all mutation allowed
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

## Loop Binding Scope

| Rule | Description |
|------|-------------|
| **LP17: Binding is alias** | Loop bindings are inline aliases — each use of `item` desugars to `collection[_pos]` at point of use. Not a regular `const` binding; no copy or move at binding creation. `mem.borrowing/E4` and `std.iteration/A4` do not apply to loop bindings |

In value mode, `item` is a read-only alias. In mutable mode, `item` is a mutable alias. The binding doesn't exist as a separate variable — it's syntactic shorthand for indexed access into the collection.

## Desugaring

Value iteration creates inline access per element. Mutable iteration creates mutable inline access.

<!-- test: parse -->
```rask
// Value iteration (Vec):
for item in vec { body }

// Equivalent to:
{
    const _len = vec.len()
    mut _pos = 0
    while _pos < _len {
        // `item` is a read-only alias for vec[_pos]  (LP17)
        // item.field  → vec[_pos].field  (inline access, E1)
        // item.method() → vec[_pos].method()  (expression borrow, E2)
        body
        _pos += 1
    }
}
```

<!-- test: skip -->
```rask
// Mutable iteration (Vec):
for mutate item in vec { body }

// Equivalent to:
{
    const _len = vec.len()
    mut _pos = 0
    while _pos < _len {
        // `item` is a mutable alias for vec[_pos]
        // item.field  → vec[_pos].field  (inline access)
        // item.field = x → vec[_pos].field = x  (in-place mutation, E3)
        // item = x → vec[_pos] = x  (element replacement)
        body
        _pos += 1
    }
}
```

<!-- test: skip -->
```rask
// Mutable iteration (Pool):
for mutate item in pool { body }

// Equivalent to:
{
    const _handles = pool._snapshot_handles()  // Handle snapshot
    mut _idx = 0
    while _idx < _handles.len() {
        const _h = _handles[_idx]
        // `item` is a mutable alias for pool[_h]
        body
        _idx += 1
    }
}
```

<!-- test: skip -->
```rask
// Mutable iteration (Map):
for mutate (k, v) in map { body }

// Equivalent to:
{
    // Internal key snapshot
    for _k in map._snapshot_keys() {
        const k = _k                    // Copy of key (immutable)
        // `v` is a mutable alias for map[_k]
        body
    }
}
```

<!-- test: parse -->
```rask
// Index iteration (explicit):
for i in 0..vec.len() { body }

// Equivalent to:
{
    const _len = vec.len()
    mut _pos = 0
    while _pos < _len {
        const i = _pos
        body
        _pos += 1
    }
}
```

<!-- test: parse -->
```rask
// Consume iteration (take_all returns a Sequence<T>):
for item in vec.take_all() { body }

// Equivalent to the Sequence desugar below (LP18).
```

## Custom Sequence Iteration

For-loops over a `Sequence<T>` or `SequenceMut<T>` desugar to a yield-closure call. See `type.sequence` for the full protocol.

| Rule | Description |
|------|-------------|
| **LP18: Sequence desugar** | `for x in seq_expr { body }` where `seq_expr: Sequence<T>` desugars to `seq_expr(\|x\| { body_with_break_continue_translated; return true })` |
| **LP19: SequenceMut desugar** | `for mutate x in seq_expr { body }` where `seq_expr: SequenceMut<T>` desugars to `seq_expr(\|mutate x: T\| { body_with_break_continue_translated; return true })` |
| **LP20: Break translation** | Inside the desugared closure body, `break` becomes `return false` |
| **LP21: Continue translation** | Inside the desugared closure body, `continue` becomes `return true` |
| **LP22: Return propagation** | `return` in the loop body exits the enclosing function (not the closure). Compiler translates via a non-local exit flag |
| **LP23: No break with value** | `break value` is not supported inside Sequence for-loops — desugaring to a closure return cannot carry a value out. Use `find` or an outer `let` binding |

<!-- test: parse -->
```rask
// Custom sequence (value):
for node in tree.in_order() { body }

// Equivalent to:
tree.in_order()(|node| {
    body   // break → return false, continue → return true
    return true
})
```

<!-- test: skip -->
```rask
// Custom sequence (mutable):
for mutate node in tree.in_order_mut() { body }

// Equivalent to:
tree.in_order_mut()(|mutate node: Node<T>| {
    body
    return true
})
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Mutate in value loop | LP8 | Compile error (value mode is read-only) |
| Structural mutation in mutable loop | LP8a/LP14 | Compile error |
| Grow during index iteration | LP10 | New items not visited (length captured at start) |
| Shrink during index iteration | LP10 | Stale index may panic — programmer responsibility |
| Remove current handle (Pool) | LP9 | OK in handle mode if no further access |
| Nested loops same collection | LP3 | Allowed (inline access) |
| Nested mutable + value loops | LP3/LP15 | Allowed — mutable loop allows inline reads of collection |
| `for mutate` on Range | — | Compile error (ranges are not collections) |
| Break/continue | — | Standard semantics (exit or skip iteration) |
| Empty collection | — | Zero iterations |

---

## Appendix (non-normative)

### Rationale

**LP1 (value iteration default):** I chose borrowed values as the default because it matches Python, Rust, Go, JavaScript expectations. Most iteration is read-only (80%+ of real code), so optimizing for the common case reduces ceremony. This decision was validated against METRICS.md targets — natural iteration should not require index manipulation.

**LP2 (index mode explicit):** Structural mutation requires explicit syntax (`0..vec.len()` or `.handles()`). This makes the structural mutation intent clear and matches the "transparency of cost" principle — you see you're doing index-based access, not value iteration.

**LP11 (mutable iteration):** In-place element mutation is the second most common iteration pattern after read-only. Index mode works but forces you to repeat `collection[i]` for every access — noisy and error-prone. `for mutate` provides a named binding that desugars to mutable inline access, keeping the code clean while making mutation intent visible via the `mutate` keyword. The keyword is consistent with function parameter annotations (`func f(mutate x: T)`). Structural mutation (insert/remove) still requires index mode because it can invalidate iteration state.

**LP3 (collection accessible):** Each element access in value mode is inline (per `mem.borrowing/E1`), so the collection remains accessible. No block-scoped borrow exists. This enables natural patterns without borrow conflicts.

**LP7 (take_all):** Ownership transfer is explicit. `vec.take_all()` empties the collection and yields owned items. This prevents accidental consumption while keeping consuming iteration ergonomic.

**LP10 (length capture):** I chose to capture length at loop start rather than recheck on each iteration. This makes iteration cost predictable (no hidden checks) and allows growth without infinite loops. The downside is stale indices if you remove during iteration — same risk as manual `for (int i = 0; i < len; i++)` in C.

### Patterns & Guidance

**When to use which pattern:**

| Goal | Pattern | Example |
|------|---------|---------|
| Read all elements | Value iteration | `for item in vec { process(item) }` |
| Mutate elements in place | Mutable iteration | `for mutate item in vec { item.field = x }` |
| Structural mutation (insert/remove) | Index iteration | `for i in 0..vec.len() { vec.swap_remove(i) }` |
| Clone values | Value iteration + clone | `for item in vec { const v = item.clone(); use(v) }` |
| Take ownership (consume) | `take_all()` | `for item in vec.take_all() { own(item) }` |
| Iterate Pool by handle | `.handles()` | `for h in pool.handles() { pool[h].update() }` |

**Value iteration (read-only):**

<!-- test: parse -->
```rask
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

**Mutable iteration (in-place mutation):**

<!-- test: skip -->
```rask
for mutate item in vec {
    item.health -= damage
    item.last_hit = now()
}

for mutate entity in pool {
    entity.velocity += entity.acceleration * dt
    entity.position += entity.velocity * dt
}

for mutate (key, value) in scores {
    value.total += value.round_score
    value.round_score = 0
}
```

**Index mode for structural mutation:**

<!-- test: parse -->
```rask
for h in pool.handles() {
    pool[h].health -= damage
    if pool[h].health <= 0 {
        pool.remove(h)
    }
}
```

**Removing during iteration (Pool):**

<!-- test: parse -->
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

<!-- test: parse -->
```rask
// No borrow conflict — each access is inline
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
| `item: mutable T` | Loop variable has mutable access to element |
| `i: usize [index into vec]` | Loop variable is an index |
| `h: Handle<Entity> [handle into pool]` | Loop variable is a handle |

<!-- test: skip -->
```rask
for item in vec {           // item: borrowed T [read-only]
    item.process()          // [inline access]
    // vec.push(x)          // [ERROR: cannot mutate during value iteration]
}

for mutate item in vec {    // item: mutable T [in-place mutation]
    item.health -= 10       // [mutable inline access]
    // vec.push(x)          // [ERROR: structural mutation forbidden]
}

for i in 0..vec.len() {     // i: usize [index into vec]
    vec[i].process()        // [inline access]
    vec.push(item)          // [OK: no borrow held]
}
```

Hover on `for item in collection` shows:
- "Value iteration: yields borrowed elements (read-only)"
- "Cannot mutate elements or collection structure"
- "Use `for mutate` for element mutation, index mode for structural mutation"

Hover on `for mutate item in collection` shows:
- "Mutable iteration: yields mutable access to each element"
- "Element mutation allowed, structural mutation forbidden"
- "Use index mode for insert/remove during iteration"

Hover on `for i in 0..collection.len()` shows:
- "Index iteration: loop captures length at start, yields indices 0..len"
- "Collection remains accessible inside loop"
- "Length changes during iteration not reflected"

### See Also

- [Borrowing](../memory/borrowing.md) — Value-based access, `with` blocks (`mem.borrowing`)
- [Value Semantics](../memory/value-semantics.md) — Copy threshold (`mem.value-semantics`)
- [Collections](../stdlib/collections.md) — Vec, Pool, Map APIs (`std.collections`)
- [Sequence Protocol](../types/sequence-protocol.md) — Function-valued iteration, adapters, terminals (`type.sequence`)

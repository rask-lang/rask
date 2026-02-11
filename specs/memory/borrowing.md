<!-- id: mem.borrowing -->
<!-- status: decided -->
<!-- summary: Block-scoped views for fixed sources, expression-scoped for growable -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Borrowing

Views last as long as the source is stable. Collections (Vec, Pool, Map) release views instantly. Fixed sources (strings, struct fields) keep views until block end.

| Rule | Source | View duration | Why |
|------|--------|---------------|-----|
| **B1: Growable = instant** | Vec, Pool, Map | Released at semicolon | Growing/shrinking could invalidate the view |
| **B2: Fixed = persistent** | string, struct fields, arrays | Valid until block ends | Source can't change, so view stays valid |

## Parameter and Receiver Borrows

| Rule | Description |
|------|-------------|
| **B3: Call duration** | Function parameters and method receivers create persistent borrows for the call duration |
| **B4: Element access follows source** | Indexing into a borrowed collection follows the collection's own rules (instant for Vec/Pool/Map) |

| Annotation | Borrow Mode | Determined By |
|------------|-------------|---------------|
| (none) | Shared | Default — read-only, enforced |
| `mutate` | Exclusive | Mutable access, enforced |
| `take` | N/A | Ownership transfer, not a borrow |

<!-- test: skip -->
```rask
func process(items: Vec<Item>) {
    // items: persistent borrow (valid for entire function)
    // items[0]: instant view (Vec can grow inside process)

    const first = items[0].name   // Copy out - instant view released
    items.push(new_item)          // OK: no view held
}
```

## Persistent Views

Views into fixed sources persist until block end.

| Rule | Description |
|------|-------------|
| **S1: Block duration** | Stable borrow valid from creation until end of enclosing block |
| **S2: Source outlives borrow** | Source must be valid for borrow's entire duration |
| **S3: No escape** | Cannot store in struct, return, or send cross-task |
| **S4: Duration extension** | Borrowing a temporary extends its duration to match borrow |
| **S5: Aliasing XOR mutation** | Source cannot be mutated while borrowed; mutable borrow excludes all other access |

<!-- test: skip -->
```rask
const line = read_line()
const key = line[0..eq]        // Borrow, valid until block ends
const value = line[eq+1..]     // Another borrow
validate(key)                // OK: key still valid
process(key, value)          // OK: both valid
```

**Lifetime extension (S4):**
<!-- test: skip -->
```rask
const slice = get_string()[0..n]  // OK: temporary extended

// Equivalent to:
const _temp = get_string()
const slice = _temp[0..n]
// _temp lives as long as slice
```

Every temporary in the chain that the borrow transitively depends on is extended. Temporaries in inner blocks are NOT extended to outer blocks.

<!-- test: compile-fail -->
```rask
const slice = {
    const s = get_string()
    s[0..n]  // ERROR: s dies at block end
}
// slice would outlive s
```

**Mutation blocked (S5):**
<!-- test: compile-fail -->
```rask
const s = string.new()
const slice = s[0..3]      // Read borrow active
s.push('!')              // ERROR: cannot mutate while borrowed
process(slice)
// Block ends, borrow released
s.push('!')              // OK: no active borrow
```

## Instant Views

Views into growable sources (Pool, Vec, Map) are released at the semicolon.

| Rule | Description |
|------|-------------|
| **V1: Expression duration** | Access valid only within the expression |
| **V2: Released at semicolon** | Access ends when statement completes |
| **V3: Chain calls OK** | `pool[h].field.method()` is one expression |
| **V4: Same aliasing rules** | Aliasing XOR mutation still applies within expression |

<!-- test: skip -->
```rask
pool[h].health -= damage     // View released at semicolon
if pool[h].health <= 0 {     // New view
    pool.remove(h)           // No active view - OK
}
```

## Multi-Statement Collection Access

Volatile access prevents multi-statement operations on collection elements. Use closure-based access.

| Method | Signature | Use Case |
|--------|-----------|----------|
| `read(key, f)` | `func(T) -> R -> Option<R>` | Multi-statement read access |
| `modify(key, f)` | `func(T) -> R -> Option<R>` | Multi-statement mutation |

| Lines of code | Pattern | Example |
|---------------|---------|---------|
| 1 statement | Direct `collection[key]` | `pool[h].field = value` |
| Method chain | Direct `collection[key]` | `pool[h].pos.normalize().scale(2)` |
| 2+ statements | Closure `modify()` | See below |
| Needs `try` inside | Closure with Result | See below |

<!-- test: skip -->
```rask
try pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
})
```

The closure borrows the collection exclusively — no other collection access inside it:
<!-- test: compile-fail -->
```rask
pool.modify(h, |e| {
    e.health -= 10
    pool.remove(other_h)     // ERROR: pool borrowed by closure
})
```

For iteration + mutation, collect handles first:
<!-- test: skip -->
```rask
const handles = pool.handles().collect()
for h in handles {
    try pool.modify(h, |e| e.update())
}
```

## Block-Scoped Element Binding (`with...as`)

Alternative to closures for multi-statement access.

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}

// Multiple elements
with pool[h1] as e1, pool[h2] as e2 {
    e1.health -= damage
    e2.health += heal
}
```

| Rule | Description |
|------|-------------|
| **W1: Sugar for modify** | Same borrowing rules as closure-based `modify()` |
| **W2: Exclusive borrow** | Collection exclusively borrowed for block duration |
| **W3: Aliasing check** | Multiple bindings from same collection: runtime panic if same key/handle |
| **W4: Error semantics** | Match direct indexing — panics on invalid handle/OOB |
| **W5: Mutable bindings** | Bindings are mutable (can assign to fields) |
| **W6: Value production** | Block can produce a value (last expression) |

## Field Projections for Partial Borrowing

Borrowing a struct borrows all of it. Field projections (`Type.{field1, field2}`) borrow only specific fields.

| Rule | Description |
|------|-------------|
| **P1: Syntax** | `value.{field1, field2}` creates a projection of the named fields |
| **P2: Type syntax** | `Type.{field1}` in function params accepts a projection |
| **P3: Non-overlapping** | Projections with disjoint fields can be borrowed simultaneously |
| **P4: Parallel safe** | Non-overlapping mutable projections can be sent to different threads |

<!-- test: skip -->
```rask
struct GameState {
    entities: Pool<Entity>
    score: i32
}

// Only borrows `entities` - other fields remain available
func movement_system(state: GameState.{entities}, dt: f32) {
    for h in state.entities {
        state.entities[h].position.x += state.entities[h].velocity.dx * dt
    }
}

// Only borrows `score` - can run alongside movement_system
func update_score(state: GameState.{score}, points: i32) {
    state.score += points
}
```

See [types/structs.md](../types/structs.md) for projection type syntax (`type.structs/P1`).

## Aliasing Rules

| Rule | Read borrow | Mutable borrow |
|------|-------------|----------------|
| **A1: Other reads** | Allowed | Forbidden |
| **A2: Mutations** | Forbidden | Forbidden |
| **A3: Count** | Unlimited | Exactly one |

Aliasing XOR Mutation: multiple immutable borrows OK, one mutable borrow OK, mixed is an error.

## Borrow Checking

All checks are performed **locally** within the function. No cross-function analysis.

| Check | When | Error |
|-------|------|-------|
| Lifetime validity | At borrow creation | "source doesn't live long enough" |
| Aliasing violation | At conflicting access | "cannot mutate while borrowed" |
| Escape attempt | At assignment/return | "borrow cannot escape scope" |

## Error Messages

Error messages teach B1/B2 (growable vs fixed) and provide concrete fixes.

**Holding a view from a growable source [V2]:**
```
ERROR [mem.borrowing/V2]: cannot hold view from growable source
   |
5  |  let entity = pool[h]
   |               ^^^^^^^ Pool can grow/shrink - view must be instant
6  |  entity.update()
   |  ^^^^^^ view already released

WHY: Pool, Vec, and Map can grow or shrink, which would invalidate
     any persistent view. Views are released at the semicolon.

FIX: Copy the value out, or use a closure:

  // Option 1: Copy out the fields you need
  const health = pool[h].health
  if health <= 0 { pool.remove(h) }

  // Option 2: Closure for multi-statement mutation
  pool.modify(h, |entity| {
      entity.health -= damage
      entity.last_hit = now()
  })
```

**Mutation during persistent view [S5]:**
```
ERROR [mem.borrowing/S5]: cannot mutate source while viewed
   |
3  |  let slice = line[0..5]
   |              ^^^^^^^^^ view created here
4  |  line.push('!')
   |  ^^^^^^^^^^^^^ cannot mutate - would invalidate view
5  |  process(slice)
   |          ^^^^^ view still active

WHY: Mutating a string might reallocate, invalidating the view.

FIX: Finish using the view first, or copy:

  // Finish using view first
  const slice = line[0..5]
  process(slice)
  line.push('!')  // OK - view ended

  // Or work with a copy
  const copy = line[0..5].to_string()
  line.push('!')  // OK - copy is independent
  process(copy)
```

**Mutation during closure [W2]:**
```
ERROR [mem.borrowing/W2]: cannot mutate collection inside its own closure
   |
5  |  pool.modify(h, |entity| {
   |  ---- pool borrowed here
6  |      entity.health -= 10
7  |      pool.remove(other)
   |      ^^^^^^^^^^^^^^^^^ cannot mutate pool here

FIX: Collect handles first, then mutate:

  const to_remove = pool.handles().filter(...).collect()
  for h in to_remove {
      pool.remove(h)
  }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Borrow from temporary | S4 | Temporary duration extended to match borrow |
| Chained temporaries | S4 | ALL temporaries in chain extended |
| Temporary in inner block | S3 | NOT extended to outer block |
| Nested borrows | S1 | Inner borrow must not outlive outer |
| Borrow across match arms | S1 | All arms see same borrow mode |
| Clone of borrowed | — | Allowed (creates independent copy) |
| Borrow of clone | — | Borrows the new copy, not original |
| Volatile access in method chain | V3 | Access spans entire chain |
| Mixed stable/volatile | B1, B2 | Each follows its source's rules |

## Quick Reference

| Aspect | Fixed Sources | Growable Sources |
|--------|---------------|------------------|
| Types | string, struct fields, arrays | Pool, Vec, Map |
| View duration | Until block ends | Until semicolon |
| **Parameter borrows** | Persistent (call duration) | Persistent (call duration) |
| **Indexing into param** | Persistent (fixed source) | Instant (growable source) |
| Can store in `const`? | Yes | No (use inline or copy out) |
| Multi-statement use? | Direct | Closure (`read`/`modify`) or copy out |
| The test | Can't grow or shrink | Can grow or shrink |

## Examples

### String Parsing (Persistent Borrow)
<!-- test: parse -->
```rask
func parse_header(line: string) -> Option<(string, string)> {
    const colon = try line.find(':')
    const key = line[0..colon].trim()      // Persistent borrow (S1)
    const value = line[colon+1..].trim()   // Another persistent borrow
    Some((key.to_string(), value.to_string()))
}
```

### Entity Update (Instant Access)
<!-- test: parse -->
```rask
func update_combat(pool: Pool<Entity>) {
    let targets: Vec<Handle<Entity>> = find_targets(pool)

    for h in targets {
        pool[h].health -= 10             // Instant access (V1)
        if pool[h].health <= 0 {         // New instant access
            pool.remove(h)               // No active borrow - OK
        }
    }
}
```

---

## Appendix (non-normative)

### Rationale

**B1/B2 (instant vs persistent):** I wanted to avoid "borrow checker wrestling" — code that looks fine then explodes 20 lines later. Collections release views instantly so you'll never write code that silently holds a dangling view. The error is immediate, the fix is obvious.

**S3 (no escape):** The cost is more `.to_string()` calls. I think that's better than lifetime annotations leaking into function signatures.

**Why collections have instant views:** Collections can change structurally — `Vec` reallocates, `Pool` compacts, `Map` rehashes. Persistent views would dangle. Instant views kill this bug class.

**Why strings have persistent views:** Strings don't change structure once created. Can't insert/remove chars without making a new string. Source can't change, so views stay valid. This enables multi-statement string parsing without copying.

### Patterns & Guidance

**The pattern for collections:** Since collection views are instant (B1), multi-statement access uses one of two patterns:

<!-- test: skip -->
```rask
// Pattern 1: Copy out the value
const health = pool[h].health    // Value copied, view released
if health <= 0 { ... }

// Pattern 2: Closure for multi-statement mutation
pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
})
```

**Multi-Statement Mutation Example:**
<!-- test: parse -->
```rask
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> () or Error {
    try pool.modify(h, |entity| {
        entity.strength += 10
        entity.defense += 5
        entity.buff_expiry = now() + Duration.seconds(30)
        try log_buff_applied(entity.id)
        Ok(())
    })
}
```

### IDE Integration

The IDE makes view durations visible through ghost annotations.

| Context | Annotation |
|---------|------------|
| Persistent view | `[view: until line N]` |
| Instant view | `[instant: released at ;]` |
| Conflict site | `[conflict: viewed on line N]` |

<!-- test: skip -->
```rask
// Instant view (collection)
const health = pool[h].health  // [instant: released at ;]
if health <= 0 {             // view already released
    pool.remove(h)           // OK - no conflict
}
```

<!-- test: skip -->
```rask
// Persistent view (string)
const key = line[0..eq]        // [view: until line 8]
const value = line[eq+1..]     // [view: until line 8]
validate(key)                // [uses view from line 3]
process(key, value)          // [uses views from lines 3-4]
}                            // line 8: views released
```

Hover information shows the view type, duration, and suggested patterns for that source type.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value-semantics`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
- [Collections](../stdlib/collections.md) — Vec, Map APIs (`std.collections`)

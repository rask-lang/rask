<!-- id: mem.borrowing -->
<!-- status: decided -->
<!-- summary: Block-scoped views for fixed-layout sources (struct fields, arrays), statement-scoped for heap-buffered (Vec, Pool, Map, string) -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Borrowing

Views last as long as the source is stable. Sources with heap buffers (collections, strings) release views at the end of the statement. Sources with fixed layout (struct fields, arrays) keep views until the block ends.

| Rule | Source | View duration | Why |
|------|--------|---------------|-----|
| **B1: Growable = statement-scoped** | Vec, Pool, Map, string | Released at semicolon | Heap buffer could reallocate, invalidating the view |
| **B2: Fixed = block-scoped** | Struct fields, arrays | Valid until block ends | Fixed layout, no reallocation possible |

The dividing line is **"has a heap buffer"** vs **"doesn't."** Strings own heap-allocated UTF-8 buffers — they go in B1 regardless of `const`/`let`. Struct fields and arrays have fixed in-place layout — they go in B2.

## Parameter and Receiver Borrows

| Rule | Description |
|------|-------------|
| **B3: Call duration** | Function parameters and method receivers are borrowed for the call duration |
| **B4: Element access follows source** | Indexing into a borrowed collection follows the collection's own rules (statement-scoped for Vec/Pool/Map) |

| Annotation | Borrow Mode | Determined By |
|------------|-------------|---------------|
| (none) | Shared | Default — read-only, enforced |
| `mutate` | Exclusive | Mutable access, enforced |
| `take` | N/A | Ownership transfer, not a borrow |

<!-- test: skip -->
```rask
func process(items: Vec<Item>) {
    // items: borrowed for entire function
    // items[0]: statement-scoped view (Vec can grow inside process)

    const first = items[0].name   // Copy out - view released at semicolon
    items.push(new_item)          // OK: no view held
}
```

## Block-Scoped Views

Views into fixed sources (struct fields, arrays) persist until the block ends.

| Rule | Description |
|------|-------------|
| **S1: Block duration** | View valid from creation until end of enclosing block |
| **S2: Source outlives borrow** | Source must be valid for borrow's entire duration |
| **S3: No escape** | Cannot store in struct, return, or send cross-task |
| **S4: Duration extension** | Borrowing a temporary extends its duration to match borrow |
| **S5: Exclusive access** | Source cannot be mutated while borrowed; mutable borrow excludes all other access |

<!-- test: skip -->
```rask
const point = get_point()
const x = point.x               // View, valid until block ends
const y = point.y               // Another view
validate(x)                    // OK: x still valid
process(x, y)                  // OK: both valid
```

**Duration extension (S4):**
<!-- test: skip -->
```rask
const x = get_point().x         // OK: temporary extended

// Equivalent to:
const _temp = get_point()
const x = _temp.x
// _temp lives as long as x
```

Every temporary in the chain that the borrow transitively depends on is extended. Temporaries in inner blocks are NOT extended to outer blocks.

<!-- test: compile-fail -->
```rask
const x = {
    const p = get_point()
    p.x  // ERROR: p dies at block end
}
// x would outlive p
```

**Strings are statement-scoped (B1), not block-scoped:**
<!-- test: compile-fail -->
```rask
const s = "hello world"
const slice = s[0..5]    // ERROR: string slices are statement-scoped
```

Strings own heap buffers — same category as Vec. Use `.to_string()` or `string_view` indices:
<!-- test: skip -->
```rask
const s = "hello world"
const owned = s[0..5].to_string()  // copy to owned string
process(owned)                     // OK: independent value

const view = string_view(0, 5)     // or store indices
process(s[view])                   // resolve inline
```

## Statement-Scoped Views

Views into growable sources (Pool, Vec, Map) are released at the semicolon.

| Rule | Description |
|------|-------------|
| **V1: Expression duration** | Access valid only within the expression |
| **V2: Released at semicolon** | Access ends when statement completes |
| **V3: Chain calls OK** | `pool[h].field.method()` is one expression |
| **V4: Same access rules** | Exclusive access rule still applies within expression |

<!-- test: skip -->
```rask
pool[h].health -= damage     // View released at semicolon
if pool[h].health <= 0 {     // New view
    pool.remove(h)           // No active view - OK
}
```

## Multi-Statement Collection Access

Statement-scoped access prevents multi-statement operations on collection elements. Use closure-based access.

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

## Access Rules

Many read-only borrows can coexist, OR one mutable borrow can exist — but not both. This prevents one piece of code from modifying data while another is reading it.

| Rule | Read borrow | Mutable borrow |
|------|-------------|----------------|
| **A1: Other reads** | Allowed | Forbidden |
| **A2: Mutations** | Forbidden | Forbidden |
| **A3: Count** | Unlimited | Exactly one |

This is sometimes called "aliasing XOR mutation" — you can alias (have multiple references) or mutate, but not both at the same time.

## Borrow Checking

All checks are performed **locally** within the function. No cross-function analysis.

| Check | When | Error |
|-------|------|-------|
| Duration validity | At borrow creation | "source doesn't live long enough" |
| Aliasing violation | At conflicting access | "cannot mutate while borrowed" |
| Escape attempt | At assignment/return | "borrow cannot escape scope" |

## Error Messages

Error messages explain growable vs fixed sources (B1/B2) and provide concrete fixes.

**Holding a view from a growable source [V2]:**
```
ERROR [mem.borrowing/V2]: cannot hold view from growable source
   |
5  |  let entity = pool[h]
   |               ^^^^^^^ Pool can grow/shrink - view released at semicolon
6  |  entity.update()
   |  ^^^^^^ view already released

WHY: Pool, Vec, and Map can grow or shrink, which would invalidate
     any held view. Views are released at the semicolon.

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

**Storing view from string [B1]:**
```
ERROR [mem.borrowing/B1]: cannot store view from growable source
   |
3  |  const slice = line[0..5]
   |                ^^^^^^^^^^ string has heap buffer - view released at semicolon

WHY: Strings own heap buffers that can reallocate. Views into
     heap buffers are statement-scoped (B1).

FIX 1: Copy to owned string:

  const copy = line[0..5].to_string()

FIX 2: Store indices:

  const view = string_view(0, 5)
  process(line[view])
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
| Statement-scoped access in method chain | V3 | Access spans entire chain |
| Mixed fixed/growable | B1, B2 | Each follows its source's rules |

## Quick Reference

| Aspect | Fixed Sources | Growable Sources |
|--------|---------------|------------------|
| Types | Struct fields, arrays | Pool, Vec, Map, string |
| View duration | Until block ends (block-scoped) | Until semicolon (statement-scoped) |
| **Parameter borrows** | Block-scoped (call duration) | Block-scoped (call duration) |
| **Indexing into param** | Block-scoped (fixed source) | Statement-scoped (growable source) |
| Can store in `const`? | Yes | No (use inline or copy out) |
| Multi-statement use? | Direct | Closure (`read`/`modify`) or copy out |
| The test | Can't grow or shrink | Can grow or shrink |

## Examples

### String Parsing (Statement-Scoped)
<!-- test: parse -->
```rask
func parse_header(line: string) -> Option<(string, string)> {
    const colon = try line.find(':')
    const key = line[0..colon].trim().to_string()      // Copy out (B1)
    const value = line[colon+1..].trim().to_string()   // Copy out (B1)
    Some((key, value))
}
```

### Entity Update (Statement-Scoped Access)
<!-- test: parse -->
```rask
func update_combat(pool: Pool<Entity>) {
    let targets: Vec<Handle<Entity>> = find_targets(pool)

    for h in targets {
        pool[h].health -= 10             // Statement-scoped access (V1)
        if pool[h].health <= 0 {         // New statement-scoped access
            pool.remove(h)               // No active borrow - OK
        }
    }
}
```

---

## Appendix (non-normative)

### Rationale

**B1/B2 (statement-scoped vs block-scoped):** I wanted to avoid "borrow checker wrestling" — code that looks fine then explodes 20 lines later. Collections release views at the semicolon so you'll never write code that silently holds a dangling view. The error is immediate, the fix is obvious.

**S3 (no escape):** The cost is more `.to_string()` calls. I think that's better than scope annotations leaking into function signatures.

**Why collections use statement-scoped views:** Collections can change structurally — `Vec` reallocates, `Pool` compacts, `Map` rehashes. Block-scoped views would dangle. Statement-scoped views kill this bug class.

**Why strings are statement-scoped, not block-scoped:** Strings own heap buffers — structurally the same as Vec. Block-scoped string views would require a hidden view type distinct from `string` and borrow-of-borrow tracking when views are passed to functions. This contradicts the "no storable references" principle. The cost is `.to_string()` calls or `string_view` indices — visible, simple, no borrow tracking needed.

### Patterns & Guidance

**The pattern for collections:** Since collection views are statement-scoped (B1), multi-statement access uses one of two patterns:

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
| Block-scoped view | `[view: until line N]` |
| Statement-scoped view | `[view: released at ;]` |
| Conflict site | `[conflict: viewed on line N]` |

<!-- test: skip -->
```rask
// Statement-scoped view (collection)
const health = pool[h].health  // [view: released at ;]
if health <= 0 {             // view already released
    pool.remove(h)           // OK - no conflict
}
```

<!-- test: skip -->
```rask
// Block-scoped view (struct field)
const pos = entity.position    // [view: until line 8]
const vel = entity.velocity    // [view: until line 8]
normalize(pos)               // [uses view from line 3]
apply(pos, vel)              // [uses views from lines 3-4]
}                            // line 8: views released
```

Hover information shows the view type, duration, and suggested patterns for that source type.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value-semantics`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
- [Collections](../stdlib/collections.md) — Vec, Map APIs (`std.collections`)

# Solution: Generation Check Coalescing

## The Question
How do we eliminate redundant generation checks when the same handle is accessed multiple times?

## Decision
The compiler performs local dataflow analysis to coalesce multiple generation checks on the same handle into a single check, when no intervening pool mutations could invalidate the handle.

## Rationale
Expression-scoped borrowing means each `pool[h]` access performs a generation check. Sequential accesses to the same handle repeat this check unnecessarily:

```
pool[h].health -= damage        // Check 1
if pool[h].health <= 0 {        // Check 2 (redundant)
    pool[h].status = Dead       // Check 3 (redundant)
}
```

This affects RO ≤ 1.10 (runtime overhead). Generation coalescing is a pure compiler optimization that eliminates redundant checks without changing semantics.

## Specification

### Basic Coalescing

```
// Source code
pool[h].x = 1
pool[h].y = 2
pool[h].z = 3

// Compiler transformation
let _checked = &mut pool.slots[h.index]  // One generation check
assert!(_checked.generation == h.generation)
_checked.value.x = 1
_checked.value.y = 2
_checked.value.z = 3
```

### Coalescing Rules

| Rule | Description |
|------|-------------|
| **GC1: Same handle** | Only coalesce accesses to the same handle variable |
| **GC2: No intervening mutation** | No `pool.insert()`, `pool.remove()`, or `pool.clear()` between accesses |
| **GC3: No reassignment** | Handle variable not reassigned between accesses |
| **GC4: Local analysis** | Coalescing within function scope only |
| **GC5: Conservative** | When in doubt, keep the check |

### What Breaks Coalescing

```
// Coalesced (no mutation)
pool[h].a = 1
pool[h].b = 2    // Same check as above

// NOT coalesced (intervening mutation)
pool[h].a = 1
pool.remove(other_h)  // Mutation invalidates assumption
pool[h].b = 2    // Fresh check required

// NOT coalesced (function call may mutate)
pool[h].a = 1
unknown_function(&mut pool)  // May mutate pool
pool[h].b = 2    // Fresh check required

// NOT coalesced (handle reassignment)
pool[h].a = 1
h = get_new_handle()
pool[h].b = 2    // Different handle, fresh check
```

### Control Flow

Coalescing works across simple control flow:

```
// Coalesced across if-else
pool[h].x = 1        // Check here
if condition {
    pool[h].y = 2    // No check (same path)
} else {
    pool[h].z = 3    // No check (same path)
}

// NOT coalesced across loop iterations
for i in 0..n {
    pool[h].x = i    // Check on EACH iteration (loop may have mutations)
}
```

### Ambient Pools

`with` blocks enable more aggressive coalescing:

```
with pool {
    h.x = 1         // Check once at first access
    h.y = 2         // Coalesced
    h.z = 3         // Coalesced
    compute(h.w)    // Coalesced (if compute doesn't mutate pool)
}
```

### Analysis Scope

| Scope | Coalescing |
|-------|-----------|
| Within basic block | Full coalescing |
| Across branches (if/else) | Coalesced if both paths check same handle |
| Across loops | Fresh check per iteration |
| Across function calls | Fresh check after calls that may mutate |
| Across await points | Fresh check after await |

### Known Mutations

The compiler tracks which operations may invalidate handles:

| Operation | Invalidates Coalescing? |
|-----------|------------------------|
| `pool[h].field = x` | No (read/write, not structural) |
| `pool.insert(x)` | Yes (may reallocate) |
| `pool.remove(h2)` | Yes (changes generation) |
| `pool.clear()` | Yes (invalidates all) |
| `pool.modify(h, f)` | No (f cannot remove) |
| Function call with `&Pool` | No (immutable borrow) |
| Function call with `&mut Pool` | Yes (may mutate) |
| `pool.cursor()` iteration | Per-iteration checks |

### Debug vs Release

| Mode | Behavior |
|------|----------|
| Debug | Coalescing still applies (optimization is semantics-preserving) |
| Release | More aggressive coalescing with inlining |

Coalescing is always safe because it only removes checks that would have succeeded anyway.

### Interaction with Frozen Pools

Frozen pools skip ALL generation checks (not just coalescing):

```
let frozen = pool.freeze()
frozen[h].x = 1    // No check (frozen)
frozen[h].y = 2    // No check (frozen)
```

Coalescing is irrelevant for frozen pools—there are no checks to coalesce.

### Compiler IR

The compiler represents coalesced access as a single checked dereference:

```
// IR (simplified)
%slot = pool_checked_access(pool, handle)  // Generation check
store %slot.x, 1
store %slot.y, 2
store %slot.z, 3
```

vs non-coalesced:

```
// IR (simplified)
%slot1 = pool_checked_access(pool, handle)  // Check
store %slot1.x, 1
%slot2 = pool_checked_access(pool, handle)  // Check (redundant)
store %slot2.y, 2
```

### Benchmarks (Expected)

| Pattern | Without Coalescing | With Coalescing |
|---------|-------------------|-----------------|
| 3 field updates | 3 checks | 1 check |
| Loop body, 5 accesses | 5 checks/iteration | 1 check/iteration |
| `with` block, 10 accesses | 10 checks | 1 check |

**Expected improvement:** 5-15% for access-heavy code paths.

### Limitations

1. **Cross-function:** Coalescing doesn't cross function boundaries (local analysis)
2. **Indirect handles:** `pool[get_handle()]` checks on each call
3. **Collections of handles:** `for h in handles { pool[h] }` checks per handle
4. **Async:** Checks required after await points

### Debugging

Compiler flag to disable coalescing for debugging:

```
rask build --no-generation-coalescing
```

This ensures every access performs its check, useful for debugging stale handle issues.

### IDE Support

IDE SHOULD show coalesced regions:

```
pool[h].x = 1    // IDE: [generation check]
pool[h].y = 2    // IDE: [coalesced]
pool[h].z = 3    // IDE: [coalesced]
pool.remove(other)
pool[h].w = 4    // IDE: [generation check]
```

## Examples

### Entity Update

```
// Source
fn update_entity(pool: &mut Pool<Entity>, h: Handle<Entity>, dt: f32) {
    pool[h].velocity += pool[h].acceleration * dt
    pool[h].position += pool[h].velocity * dt
    pool[h].age += dt
}

// After coalescing (conceptual)
fn update_entity(pool: &mut Pool<Entity>, h: Handle<Entity>, dt: f32) {
    let e = pool.checked_get_mut(h)  // Single check
    e.velocity += e.acceleration * dt
    e.position += e.velocity * dt
    e.age += dt
}
```

### With Ambient Pool

```
with pool {
    // All these coalesce to one check
    h.health = 100
    h.mana = 50
    h.stamina = 75
    h.position = spawn_point
    h.rotation = default_rotation
}
```

### Non-Coalesced (Mutation Between)

```
pool[h1].x = 1        // Check for h1
pool.remove(h2)       // Invalidates coalescing
pool[h1].y = 2        // Fresh check for h1 (h1 might be h2!)
```

## Integration Notes

- **Memory Model:** No semantic change. Optimization is invisible to user.
- **Type System:** No impact.
- **Generics:** Works for all `Pool<T>`.
- **Concurrency:** Each task optimizes independently.
- **Compiler:** Implemented as MIR optimization pass.

## Remaining Issues

### Low Priority
1. **Inter-procedural coalescing** — With inlining, could coalesce across function boundaries.

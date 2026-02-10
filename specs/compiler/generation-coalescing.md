# Generation Check Coalescing

## The Question
How to eliminate redundant generation checks when same handle gets accessed multiple times?

## Decision
Compiler performs local dataflow analysis to coalesce multiple generation checks on same handle into single check, when no intervening pool mutations invalidate the handle.

## Rationale
Expression-scoped borrowing means each `pool[h]` access performs generation check. Sequential accesses to same handle repeat unnecessarily:

```rask
pool[h].health -= damage        // Check 1
if pool[h].health <= 0 {        // Check 2 (redundant)
    pool[h].status = Dead       // Check 3 (redundant)
}
```

Affects RO ≤ 1.10 (runtime overhead). Generation coalescing is pure compiler optimization—eliminates redundant checks without changing semantics.

## Specification

### Performance Guarantees

Generation check coalescing is **best-effort**. Compiler applies strong heuristics (GC1-GC4) but may conservatively retain checks when analysis is uncertain (GC5).

| Guarantee Level | Mechanism | Checks | Use Case |
|-----------------|-----------|--------|----------|
| **Guaranteed zero** | Frozen pool | 0 | Read-only hot paths |
| **Guaranteed 1** | `with_valid(h, f)` | 1 | Write hot paths (safe) |
| **Guaranteed zero** | `get_unchecked(h)` (unsafe) | 0 | Caller-validated handles |
| **Expected ≤1/handle** | Coalescing | ≤1 | General code |
| **Worst case** | No coalescing | 1/access | Compiler can't prove safety |

**Why best-effort?** Guaranteeing coalescing requires proving no aliasing between handles and no intervening mutations. Analysis may require inter-procedural reasoning (violates local analysis). Corner cases need conservative choice.

**Escape hatches:** Hot paths where coalescing insufficient: use `with_valid` (safe, 1 check) or `get_unchecked` (unsafe, 0 checks). See [pools.md](../memory/pools.md).

### Basic Coalescing

```rask
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

```rask
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

```rask
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

```rask
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

Compiler tracks which operations may invalidate handles:

| Operation | Invalidates Coalescing? |
|-----------|------------------------|
| `pool[h].field = x` | No (read/write, not structural) |
| `pool.insert(x)` | Yes (may reallocate) |
| `pool.remove(h2)` | Yes (changes generation) |
| `pool.clear()` | Yes (invalidates all) |
| `pool.modify(h, f)` | No (f cannot remove) |
| Function call with `Pool` (read) | No (immutable borrow) |
| Function call with `Pool` (mutate) | Yes (may mutate) |
| `pool.cursor()` iteration | Per-iteration checks |

### Debug vs Release

| Mode | Behavior |
|------|----------|
| Debug | Coalescing still applies (optimization is semantics-preserving) |
| Release | More aggressive coalescing with inlining |

Coalescing always safe—only removes checks that would have succeeded anyway.

### Interaction with Frozen Pools

Frozen pools skip ALL generation checks (not just coalescing):

```rask
let frozen = pool.freeze()
frozen[h].x = 1    // No check (frozen)
frozen[h].y = 2    // No check (frozen)
```

Coalescing irrelevant for frozen pools—no checks to coalesce.

### Compiler IR

Compiler represents coalesced access as single checked dereference:

```rask
// IR (simplified)
%slot = pool_checked_access(pool, handle)  // Generation check
store %slot.x, 1
store %slot.y, 2
store %slot.z, 3
```

vs non-coalesced:

```rask
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

Compiler flag to disable coalescing:

```
rask build --no-generation-coalescing
```

Every access performs its check. Useful for debugging stale handle issues.

### IDE Support

IDE should show coalesced regions:

```rask
pool[h].x = 1    // IDE: [generation check]
pool[h].y = 2    // IDE: [coalesced]
pool[h].z = 3    // IDE: [coalesced]
pool.remove(other)
pool[h].w = 4    // IDE: [generation check]
```

## Examples

### Entity Update

```rask
// Source
func update_entity(pool: Pool<Entity>, h: Handle<Entity>, dt: f32) {
    pool[h].velocity += pool[h].acceleration * dt
    pool[h].position += pool[h].velocity * dt
    pool[h].age += dt
}

// After coalescing (conceptual)
func update_entity(pool: Pool<Entity>, h: Handle<Entity>, dt: f32) {
    let e = pool.checked_get_mut(h)  // Single check
    e.velocity += e.acceleration * dt
    e.position += e.velocity * dt
    e.age += dt
}
```

### With Ambient Pool

```rask
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

```rask
pool[h1].x = 1        // Check for h1
pool.remove(h2)       // Invalidates coalescing
pool[h1].y = 2        // Fresh check for h1 (h1 might be h2!)
```

## Integration Notes

- **Memory Model:** No semantic change. Optimization invisible to user.
- **Type System:** No impact.
- **Generics:** Works for all `Pool<T>`.
- **Concurrency:** Each task optimizes independently.
- **Compiler:** Implemented as MIR optimization pass.

## Remaining Issues

### Low Priority
1. **Inter-procedural coalescing** — With inlining, could coalesce across function boundaries.
2. **Guaranteed coalescing for `with` blocks** — Could strengthen guarantee within ambient pool scope.

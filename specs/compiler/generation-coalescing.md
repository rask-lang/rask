<!-- id: comp.gen-coalesce -->
<!-- status: decided -->
<!-- summary: Eliminate redundant generation checks on same handle within basic blocks -->
<!-- depends: memory/pools.md, compiler/codegen.md -->

# Generation Check Coalescing

Compiler performs local dataflow analysis to coalesce multiple generation checks on same handle into a single check, when no intervening pool mutations invalidate the handle.

## Coalescing Rules

| Rule | Description |
|------|-------------|
| **GC1: Same handle** | Only coalesce accesses to the same handle variable |
| **GC2: No intervening mutation** | No `pool.insert()`, `pool.remove()`, or `pool.clear()` between accesses |
| **GC3: No reassignment** | Handle variable not reassigned between accesses |
| **GC4: Local analysis** | Coalescing within function scope only; no inter-procedural reasoning |
| **GC5: Conservative default** | When analysis is uncertain, keep the check |

<!-- test: skip -->
```rask
// Three accesses, one check
pool[h].x = 1
pool[h].y = 2
pool[h].z = 3
// Compiler transforms to single checked dereference + three stores
```

## Performance Guarantees

| Rule | Description |
|------|-------------|
| **PG1: Best-effort** | Coalescing is best-effort; may conservatively retain checks |
| **PG2: Semantics-preserving** | Only removes checks that would have succeeded; safe in debug and release |
| **PG3: Escape hatches** | `with_valid(h, f)` guarantees 1 check; `get_unchecked(h)` (unsafe) guarantees 0 |

| Guarantee Level | Mechanism | Checks | Use Case |
|-----------------|-----------|--------|----------|
| Guaranteed zero | Frozen pool | 0 | Read-only hot paths |
| Guaranteed 1 | `with_valid(h, f)` | 1 | Write hot paths (safe) |
| Guaranteed zero | `get_unchecked(h)` (unsafe) | 0 | Caller-validated handles |
| Expected 1 or fewer per handle | Coalescing | 1 or fewer | General code |
| Worst case | No coalescing | 1/access | Compiler can't prove safety |

## Mutation Tracking

Compiler tracks which operations invalidate coalescing.

| Rule | Description |
|------|-------------|
| **MT1: Structural mutation breaks** | `pool.insert()`, `pool.remove()`, `pool.clear()` invalidate all coalesced checks |
| **MT2: Field writes safe** | `pool[h].field = x` does not break coalescing |
| **MT3: Mutable borrows break** | Function calls receiving `Pool` as mutable break coalescing |
| **MT4: Immutable borrows safe** | Function calls receiving `Pool` as immutable do not break coalescing |
| **MT5: modify safe** | `pool.modify(h, f)` does not break (f cannot remove) |

| Operation | Invalidates Coalescing? |
|-----------|------------------------|
| `pool[h].field = x` | No (field write, not structural) |
| `pool.insert(x)` | Yes (may reallocate) |
| `pool.remove(h2)` | Yes (changes generation) |
| `pool.clear()` | Yes (invalidates all) |
| `pool.modify(h, f)` | No (f cannot remove) |
| Function call with `Pool` (read) | No (immutable borrow) |
| Function call with `Pool` (mutate) | Yes (may mutate) |
| `pool.cursor()` iteration | Per-iteration checks |

## Control Flow

| Rule | Description |
|------|-------------|
| **CF1: Basic block** | Full coalescing within a basic block |
| **CF2: Branches** | Coalesced across if/else if both paths check same handle |
| **CF3: Loop boundary** | Fresh check per loop iteration |
| **CF4: Function call boundary** | Fresh check after calls that may mutate pool |
| **CF5: Await boundary** | Fresh check after await points |

<!-- test: skip -->
```rask
// Coalesced across if-else
pool[h].x = 1        // Check here
if condition {
    pool[h].y = 2    // No check (same path)
} else {
    pool[h].z = 3    // No check (same path)
}

// NOT coalesced across loop
for i in 0..n {
    pool[h].x = i    // Check on EACH iteration
}
```

## Ambient Pools

| Rule | Description |
|------|-------------|
| **AP1: With blocks** | `with pool { }` enables more aggressive coalescing within scope |

<!-- test: skip -->
```rask
with pool {
    h.x = 1         // Check once at first access
    h.y = 2         // Coalesced
    h.z = 3         // Coalesced
    compute(h.w)    // Coalesced (if compute doesn't mutate pool)
}
```

## Frozen Pools

| Rule | Description |
|------|-------------|
| **FP1: No checks** | Frozen pools skip all generation checks; coalescing irrelevant |

## Debug vs Release

| Rule | Description |
|------|-------------|
| **DR1: Always applied** | Coalescing applies in both debug and release (semantics-preserving) |
| **DR2: Release more aggressive** | Release mode may inline functions, enabling wider coalescing scope |
| **DR3: Disable flag** | `rask build --no-generation-coalescing` disables for debugging stale handle issues |

## Compiler IR

Coalesced access represented as single `pool_checked_access` in MIR.

```
// Coalesced (1 check, 3 stores)
%slot = pool_checked_access(pool, handle)
store %slot.x, 1
store %slot.y, 2
store %slot.z, 3

// Non-coalesced (2 checks)
%slot1 = pool_checked_access(pool, handle)
store %slot1.x, 1
%slot2 = pool_checked_access(pool, handle)
store %slot2.y, 2
```

## Edge Cases

| Case | Handling | Rule |
|------|---------|------|
| Indirect handles (`pool[get_handle()]`) | Check on each call (can't prove same handle) | GC1 |
| Collections of handles (`for h in handles { pool[h] }`) | Check per handle (different handles) | GC1, CF3 |
| Handle reassignment mid-block | Fresh check after reassignment | GC3 |
| Mutation between accesses | Fresh check after mutation | GC2, MT1 |
| Unknown function with `&mut pool` | Fresh check after call | MT3 |
| Async: after await point | Fresh check | CF5 |
| Frozen pool accesses | No checks at all (coalescing irrelevant) | FP1 |

---

## Appendix (non-normative)

### Rationale

**GC1-GC5 (coalescing strategy):** Expression-scoped borrowing means each `pool[h]` access performs a generation check. Sequential accesses to the same handle repeat unnecessarily. Coalescing eliminates redundant checks without changing semantics. Affects runtime overhead metric (RO <= 1.10).

**PG1 (best-effort):** Guaranteeing coalescing requires proving no aliasing between handles and no intervening mutations. That may require inter-procedural reasoning, which violates local analysis (GC4). Corner cases need the conservative choice.

**PG3 (escape hatches):** Hot paths where coalescing is insufficient can use `with_valid` (safe, 1 check) or `get_unchecked` (unsafe, 0 checks). See `mem.pools`.

### Patterns & Guidance

**Entity update pattern:**

<!-- test: skip -->
```rask
// Source
func update_entity(pool: Pool<Entity>, h: Handle<Entity>, dt: f32) {
    pool[h].velocity += pool[h].acceleration * dt
    pool[h].position += pool[h].velocity * dt
    pool[h].age += dt
}
// After coalescing: single check, then direct field access
```

**Expected improvements:**

| Pattern | Without Coalescing | With Coalescing |
|---------|-------------------|-----------------|
| 3 field updates | 3 checks | 1 check |
| Loop body, 5 accesses | 5 checks/iteration | 1 check/iteration |
| `with` block, 10 accesses | 10 checks | 1 check |

Expected 5-15% improvement for access-heavy code paths.

### IDE Integration

IDE should show coalesced regions:

<!-- test: skip -->
```rask
pool[h].x = 1    // IDE: [generation check]
pool[h].y = 2    // IDE: [coalesced]
pool[h].z = 3    // IDE: [coalesced]
pool.remove(other)
pool[h].w = 4    // IDE: [generation check]
```

### Open Issues

1. **Inter-procedural coalescing** — With inlining, could coalesce across function boundaries.
2. **Guaranteed coalescing for `with` blocks** — Could strengthen guarantee within ambient pool scope.

### See Also

- `comp.codegen` — MIR optimization pipeline where coalescing runs
- `mem.pools` — Pool and Handle design, `with_valid`, `get_unchecked`
- `mem.borrowing` — Expression-scoped borrowing that motivates per-access checks

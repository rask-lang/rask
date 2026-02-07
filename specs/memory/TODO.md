# Memory Model: Open Issues

Stress points that need specification or resolution.

---

## 1. Expression-Scoped Aliasing Detection

Rule EC4 (aliasing rules apply to expression-scoped closures) needs local analysis to catch aliasing violations.

**Problem:** `pool.modify(h, |e| pool.remove(h))`—closure accesses `pool` while `modify()` holds a mutable borrow. Must catch this without whole-program analysis.

### Algorithm

**Phase 1: Build Borrow Graph**

During expression evaluation, track active borrows as a stack:

| Event | Action |
|-------|--------|
| Method call `x.method(args)` | Push borrow of `x` with mode from method signature |
| Index expression `x[i]` | Push borrow of `x` (read for read context, mut for assignment) |
| Argument evaluation | Push borrows as arguments are evaluated left-to-right |
| Expression completion (`;`) | Pop all borrows from that expression |

**Borrow modes from method signatures:**

| Signature | Borrow Mode |
|-----------|-------------|
| `self` | Borrow (compiler infers read vs mutate) |
| `take self` | Move (consumes, no conflict after) |
| `func(x: T)` | Borrow of argument (compiler infers read vs mutate) |
| `func(take x: T)` | Move of argument |

**Phase 2: Closure Body Analysis**

When an expression-scoped closure is encountered:

1. **Collect accesses**: Scan closure body for all variable references
2. **Classify each access**: Read, mutate, or call (infer from usage context)
3. **Check conflicts**: For each access, check against the active borrow stack

**Conflict rules:**

| Active Borrow | Closure Access | Result |
|---------------|----------------|--------|
| Shared(x) | Read(x) | ✅ OK |
| Shared(x) | Mutate(x) | ❌ ERROR |
| Shared(x) | Call(x.mut_method) | ❌ ERROR |
| Exclusive(x) | Read(x) | ❌ ERROR |
| Exclusive(x) | Mutate(x) | ❌ ERROR |
| Exclusive(x) | Call(x.any_method) | ❌ ERROR |
| Any(x) | Access(y) where y ≠ x | ✅ OK (disjoint) |

**Phase 3: Disjoint Access Refinement**

Field-level tracking enables more patterns:

```rask
// pool.modify(h, |e| other_pool.remove(h2))
// ✅ OK: pool and other_pool are disjoint

// entity.pos.modify(|p| entity.vel.read())
// ✅ OK: pos and vel are disjoint fields
```

Track borrows at field granularity when possible:

| Expression | Tracked Borrow |
|------------|----------------|
| `x.f` | Borrow of `x.f` (not all of `x`) |
| `x[i]` | Borrow of `x` (index not statically known) |
| `x.method()` | Borrow of `x` (method may access any field) |

### Examples

**Example 1: Basic conflict**
```rask
pool.modify(h, |e| {
    pool.remove(h)    // ❌ ERROR: pool exclusively borrowed by modify()
})

// Borrow stack when closure executes:
//   [Exclusive(pool)]  ← from modify()'s `self` (mutating)
// Closure accesses:
//   [Call(pool.remove)]  ← conflicts with Exclusive(pool)
```

**Example 2: Disjoint OK**
```rask
pool.modify(h, |e| {
    other_pool.remove(h2)    // ✅ OK: different variable
})

// Borrow stack: [Exclusive(pool)]
// Closure accesses: [Call(other_pool.remove)]
// No conflict: pool ≠ other_pool
```

**Example 3: Read during read**
```rask
pool.read(h, |e| {
    const x = pool.get(h2)    // ✅ OK: shared borrows compatible
})

// Borrow stack: [Shared(pool)]  ← from read()'s `self` (non-mutating)
// Closure accesses: [Call(pool.get)]  ← Shared + Shared = OK
```

**Example 4: Nested expression chains**
```rask
entities[h].weapons[w].fire(|bullet| {
    entities.spawn(bullet)    // ❌ ERROR: entities borrowed
})

// Borrow stack when closure executes:
//   [Shared(entities), Shared(entities[h].weapons)]
// Closure accesses:
//   [Call(entities.spawn)]  ← conflicts if spawn takes `self` (mutating)
```

**Example 5: Chained methods**
```rask
pool.get(h)?.transform().apply(|v| {
    pool.insert(v)    // Depends on return type ownership
})

// If transform() returns owned value:
//   Borrow stack: []  ← pool borrow ended after get()
//   ✅ OK: no conflict

// If transform() returns reference into pool:
//   Borrow stack: [Shared(pool)]
//   ❌ ERROR if insert() needs exclusive access
```

### Complexity

| Phase | Complexity | Notes |
|-------|------------|-------|
| Borrow tracking | O(expression depth) | Stack operations |
| Closure scanning | O(closure body size) | Single pass |
| Conflict checking | O(accesses × borrows) | Usually small constants |
| **Total** | O(function size) | Same as existing borrow checking |

No cross-function analysis required. Method signatures provide borrow requirements without examining method bodies.

### Implementation Notes

1. **Method signatures are trusted**: Compiler infers from method body whether `self` is read or mutated. No cross-function analysis—just check method body.

2. **Expression-scoped only**: Analysis only applies to closures that execute immediately. Storable closures use capture-by-value.

3. **Conservative for dynamic indices**: `pool[computed_index]` borrows the entire pool, not a specific slot. Sound but may reject valid programs.

4. **Error messages**: Report borrow source ("pool is exclusively borrowed by modify() at line 5") and conflicting access ("cannot call pool.remove() while pool is borrowed").

### Status

✅ **Specified.** Ready to implement.

---

## Summary

| Issue | Primary Metric Risk | Status |
|-------|---------------------|--------|
| 1. Expression-Scoped Aliasing | Local analysis complexity | ✅ Specified |

---

## Resolved Issues

The following issues from the original list have been addressed:

| Original # | Issue | Resolution |
|------------|-------|------------|
| 2 | Context Passing Tax | Ambient Pool Scoping (`with pool { }`) in pools.md |
| 3 | Handle Lifecycle Zombies | Weak Handles in pools.md |
| 4 | Self-Referential Structures | Self-Referential Patterns section in pools.md |
| 5 | Lifetime Extension Edge Cases | Chained temporaries section in borrowing.md |
| 10 | Multi-Pool Operations | Multi-pool `with (a, b) { }` in pools.md |
| 11 | Iterator + Mutation Allocation | Cursor iteration in pools.md |
| 13 | No Thread-Local Pattern | Ambient pools establish task-local context |
| 14 | Expression-Scoped Double Access | Frozen pools + generation check coalescing in pools.md |
| 15 | Linear Resources in Errors | Linear Resources in Error Types section in linear-types.md |
| 16 | Pool Partitioning for Parallelism | Scoped API (`with_partition`) + mutable chunks + snapshot isolation in pools.md. Pool::merge removed (generation conflicts). |
| 17 | Storable Slices | SliceDescriptor<T> (Handle + Range) in collections.md |
| 18 | Handle Exhaustion & Fragmentation | Pool Growth & Memory Management section in pools.md |

The following are documented design tradeoffs (not bugs):

| Original # | Issue | Documentation |
|------------|-------|---------------|
| 4 | 16-Byte Threshold | value-semantics.md (deliberate, with rationale) |
| 6 | Dual Borrowing Semantics | borrowing.md (load-bearing; mitigated via "Containers Might Change" framing + IDE ghost annotations + improved error messages) |
| 7 | Scope-Constrained Closures | closures.md (BC1-BC5 rules) |

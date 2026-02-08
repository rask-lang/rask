# Solution: Expression-Scoped Aliasing Detection

## The Question

How do we prevent closures from mutating collections that are already borrowed by the method calling them?

## Decision

Local borrow analysis during type checking. Track active borrows in a stack, scan closure bodies for conflicts. No whole-program analysis needed.

## Rationale

Expression-scoped closures (EC4 rule) can access outer scope directly. This creates a problem:

```rask
pool.modify(h, |entity| {
    pool.remove(h)    // pool borrowed by modify(), can't mutate here
})
```

Runtime checks would panic. Better to catch at compile time—clear error, no testing needed.

The algorithm respects local analysis: method signatures declare borrow modes, closure bodies are scanned locally. Same O(function size) as existing type checking.

## Specification

### The Problem

**Rule EC4** states that aliasing rules apply to expression-scoped closures. The dangerous pattern:

```rask
pool.modify(h, |entity| {
    entity.health -= 10        // entity borrowed from pool
    pool.remove(h)             // ERROR: tries to mutate pool while modify() has it borrowed
})
```

**Without detection:** Pool's internal state could be modified while the closure reads from it, causing:
- Handles becoming invalid while in use
- Generation counters out of sync
- Potential panics or data corruption

**With detection:** Compile-time error prevents the pattern entirely.

### Algorithm

The algorithm has three phases. Phase 1-2 are mandatory, Phase 3 is optional refinement.

### Phase 1: Build Borrow Stack

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

### Phase 2: Closure Body Analysis

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

### Phase 3: Disjoint Access Refinement (Optional)

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

**Note:** Phase 3 can be deferred. Phases 1-2 provide safety; Phase 3 reduces false positives.

## Examples

### Example 1: Basic Conflict

```rask
pool.modify(h, |e| {
    pool.remove(h)    // ❌ ERROR: pool exclusively borrowed by modify()
})

// Borrow stack when closure executes:
//   [Exclusive(pool)]  ← from modify()'s `self` (mutating)
// Closure accesses:
//   [Call(pool.remove)]  ← conflicts with Exclusive(pool)
```

**Error message:**
```
error: cannot mutate `pool` while borrowed
  --> example.rask:2:5
   |
 1 | pool.modify(h, |e| {
   |      ------ `pool` is exclusively borrowed here
 2 |     pool.remove(h)
   |     ^^^^^^^^^^^^^^ cannot mutate while borrowed
```

### Example 2: Disjoint Variables

```rask
pool.modify(h, |e| {
    other_pool.remove(h2)    // ✅ OK: different variable
})

// Borrow stack: [Exclusive(pool)]
// Closure accesses: [Call(other_pool.remove)]
// No conflict: pool ≠ other_pool
```

### Example 3: Read During Read

```rask
pool.read(h, |e| {
    const x = pool.get(h2)    // ✅ OK: shared borrows compatible
})

// Borrow stack: [Shared(pool)]  ← from read()'s `self` (non-mutating)
// Closure accesses: [Call(pool.get)]  ← Shared + Shared = OK
```

### Example 4: Nested Expression Chains

```rask
entities[h].weapons[w].fire(|bullet| {
    entities.spawn(bullet)    // ❌ ERROR: entities borrowed
})

// Borrow stack when closure executes:
//   [Shared(entities), Shared(entities[h].weapons)]
// Closure accesses:
//   [Call(entities.spawn)]  ← conflicts if spawn takes `self` (mutating)
```

**Without Phase 3:** This fails (conservative).
**With Phase 3:** Could succeed if `fire()` only borrows `weapons[w]`, not all of `entities`.

### Example 5: Chained Methods

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

## Complexity

| Phase | Complexity | Notes |
|-------|------------|-------|
| Borrow tracking | O(expression depth) | Stack operations |
| Closure scanning | O(closure body size) | Single pass |
| Conflict checking | O(accesses × borrows) | Usually small constants |
| **Total** | O(function size) | Same as existing borrow checking |

No cross-function analysis required. Method signatures provide borrow requirements without examining method bodies.

## Implementation Notes

1. **Method signatures are trusted**: Compiler infers from method body whether `self` is read or mutated. No cross-function analysis—just check method body locally.

2. **Expression-scoped only**: Analysis only applies to closures that execute immediately. Storable closures use capture-by-value (different safety mechanism).

3. **Conservative for dynamic indices**: `pool[computed_index]` borrows the entire pool, not a specific slot. Sound but may reject valid programs.

4. **Error messages**: Report borrow source ("pool is exclusively borrowed by modify() at line 5") and conflicting access ("cannot call pool.remove() while pool is borrowed").

5. **Phase 3 is optional**: Implement Phases 1-2 first for safety. Add Phase 3 later if false positives become problematic.

## Metrics Impact

| Metric | Impact | Notes |
|--------|--------|-------|
| MC (Mechanical Safety) | ✅ +0.05 | Prevents data races in closures at compile time |
| TC (Transparency of Cost) | ✅ Neutral | Borrow stack operations are O(1) per call (implicit) |
| ED (Ergonomic Simplicity) | ⚠️ -0.1 to +0.1 | Phase 1-2: conservative (may reject valid patterns). Phase 3: natural |
| SN (Syntactic Noise) | ✅ Neutral | Zero new syntax, uses existing closure patterns |

**Recommendation:** Implement Phases 1-2 first. Measure false positive rate. Add Phase 3 if ED regression is significant.

## References

- [borrowing.md](borrowing.md) — Expression-scoped vs block-scoped semantics
- [closures.md](closures.md) — EC1-EC5 rules for expression-scoped closures
- [pools.md](pools.md) — Pool methods that use closures (`modify()`, `read()`, etc.)

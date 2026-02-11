<!-- id: mem.aliasing -->
<!-- status: decided -->
<!-- summary: Local borrow analysis prevents aliasing conflicts in expression-scoped closures -->
<!-- depends: memory/borrowing.md, memory/closures.md -->
<!-- implemented-by: compiler/crates/rask-ownership/ -->

# Aliasing Detection

Compile-time analysis that prevents closures from mutating collections already borrowed by the calling method. Local to each function, O(function size).

## Borrow Stack

| Rule | Description |
|------|-------------|
| **AL1: Borrow stack tracking** | Method calls push borrows onto a stack; expression completion pops them |

| Event | Action |
|-------|--------|
| Method call `x.method(args)` | Push borrow of `x` with mode from method signature |
| Index expression `x[i]` | Push borrow of `x` (read for read context, mut for assignment) |
| Argument evaluation | Push borrows as arguments are evaluated left-to-right |
| Expression completion | Pop all borrows from that expression |

## Closure Body Scan

| Rule | Description |
|------|-------------|
| **AL2: Closure body scan** | Closure body checked against active borrows for conflicts |

When an expression-scoped closure is encountered: collect all variable references in the body, classify each as read/mutate/call, check against the borrow stack.

## Conflict Rules

| Rule | Description |
|------|-------------|
| **AL3: Shared-shared OK** | Shared borrow + shared access is allowed |
| **AL4: Shared-mutate conflict** | Shared borrow + mutation is a compile error |
| **AL5: Exclusive-any conflict** | Exclusive borrow + any access is a compile error |
| **AL6: Disjoint OK** | Different variables or fields never conflict |

| Active Borrow | Closure Access | Result | Rule |
|---------------|----------------|--------|------|
| Shared(x) | Read(x) | OK | AL3 |
| Shared(x) | Mutate(x) | Error | AL4 |
| Shared(x) | Call(x.mut_method) | Error | AL4 |
| Exclusive(x) | Read(x) | Error | AL5 |
| Exclusive(x) | Mutate(x) | Error | AL5 |
| Exclusive(x) | Call(x.any_method) | Error | AL5 |
| Any(x) | Access(y) where y != x | OK | AL6 |

## Analysis Scope

| Rule | Description |
|------|-------------|
| **AL7: Local analysis** | O(function size), no cross-function analysis needed |

Method signatures declare borrow modes. The compiler infers from each method body whether `self` is read or mutated, then uses that information locally at call sites. No whole-program analysis.

## Error Messages

**Mutation during exclusive borrow [AL5]:**
```
ERROR [mem.aliasing/AL5]: cannot mutate `pool` while borrowed
   |
1  |  pool.modify(h, |e| {
   |       ------ `pool` is exclusively borrowed here
2  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ cannot mutate while borrowed

WHY: modify() holds an exclusive borrow on pool. The closure
     cannot access pool again until modify() completes.

FIX: Collect operations, apply after:

  const to_remove = find_removable(pool)
  for h in to_remove {
      pool.remove(h)
  }
```

**Mutation during shared borrow [AL4]:**
```
ERROR [mem.aliasing/AL4]: cannot mutate `pool` while shared borrow active
   |
1  |  pool.read(h, |e| {
   |       ---- `pool` is shared-borrowed here
2  |      pool.remove(other_h)
   |      ^^^^^^^^^^^^^^^^^^^^ cannot mutate while borrowed

WHY: read() holds a shared borrow. Mutation would invalidate it.

FIX: Copy what you need, then mutate:

  const data = pool[h].clone()
  pool.remove(other_h)
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Disjoint variables | AL6 | `pool.modify(h, \|e\| other_pool.remove(h2))` is OK |
| Shared + shared | AL3 | `pool.read(h, \|e\| pool.get(h2))` is OK |
| Chained methods returning owned | AL1 | Borrow released when ownership transfers |
| Dynamic indices | AL1 | `pool[computed]` borrows entire pool (conservative) |
| Nested expression chains | AL1 | All borrows accumulate on stack |
| Field-level disjointness | AL6 | Optional refinement; phases 1-2 are conservative |

---

## Appendix (non-normative)

### Rationale

**AL1-AL2 (borrow stack + closure scan):** Expression-scoped closures (`mem.closures/EC4`) access outer scope directly. Without detection, a closure could mutate a collection while the calling method holds a borrow — causing handle invalidation, generation counter desync, or panics. Compile-time detection kills this bug class with zero runtime cost.

**AL7 (local analysis):** Method signatures provide borrow requirements without examining method bodies. Same cost as existing type checking.

**Phased implementation:** AL3-AL6 cover phases 1-2 (mandatory). Field-level disjoint tracking (phase 3) reduces false positives but can be deferred — phases 1-2 provide safety.

### Patterns & Guidance

**Basic conflict — exclusive borrow blocks all access:**
<!-- test: skip -->
```rask
pool.modify(h, |e| {
    pool.remove(h)    // ERROR: pool exclusively borrowed by modify()
})
// Borrow stack: [Exclusive(pool)]
// Closure accesses: [Call(pool.remove)] — conflicts with Exclusive(pool)
```

**Disjoint variables — different collections never conflict:**
<!-- test: skip -->
```rask
pool.modify(h, |e| {
    other_pool.remove(h2)    // OK: different variable
})
// Borrow stack: [Exclusive(pool)]
// Closure accesses: [Call(other_pool.remove)] — pool != other_pool
```

**Shared reads are compatible:**
<!-- test: skip -->
```rask
pool.read(h, |e| {
    const x = pool.get(h2)    // OK: shared borrows compatible
})
// Borrow stack: [Shared(pool)]
// Closure accesses: [Call(pool.get)] — Shared + Shared = OK
```

**Chained methods — borrow depends on return type:**
<!-- test: skip -->
```rask
pool.get(h)?.transform().apply(|v| {
    pool.insert(v)
})
// If transform() returns owned value: borrow stack empty, OK
// If transform() returns reference into pool: Shared(pool) active, ERROR
```

### See Also

- [Borrowing](borrowing.md) — Expression-scoped vs block-scoped semantics (`mem.borrowing`)
- [Closures](closures.md) — EC1-EC4 rules for expression-scoped closures (`mem.closures`)
- [Pools](pools.md) — Pool methods that use closures (`mem.pools`)

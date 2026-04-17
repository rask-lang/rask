<!-- id: mem.aliasing -->
<!-- status: decided -->
<!-- summary: Local borrow analysis prevents aliasing conflicts in expression-scoped closures -->
<!-- depends: memory/borrowing.md, memory/closures.md -->
<!-- implemented-by: compiler/crates/rask-ownership/ -->

# Aliasing Detection

Compile-time analysis that prevents structural mutations on collections with active element borrows, and prevents closures from violating borrow invariants. Local to each function, O(function size).

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

**Structural mutation during element borrow [AL5]:**
```
ERROR [mem.aliasing/AL5]: cannot structurally mutate `pool` inside with block
   |
1  |  with pool[h] as e {
   |  ---- element borrowed here
2  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ structural mutation not allowed

WHY: insert, remove, and clear can invalidate the borrowed element.
     Reading and writing other elements is fine.

FIX: Separate the check from the mutation:

  const should_remove = pool[h].health <= 0
  if should_remove {
      pool.remove(h)
  }
```

**Structural mutation during element borrow (different handle) [AL4]:**
```
ERROR [mem.aliasing/AL4]: cannot structurally mutate `pool` inside with block
   |
1  |  with pool[h] as e {
   |  ---- element borrowed here
2  |      pool.remove(other_h)
   |      ^^^^^^^^^^^^^^^^^^^^ structural mutation not allowed

WHY: remove can trigger reallocation, invalidating the borrowed element.

FIX: Move the mutation outside the with block:

  const should_remove = with pool[h] as e { e.health <= 0 }
  if should_remove {
      pool.remove(other_h)
  }
```

Non-structural access to other elements is allowed:
```rask
with pool[h] as e {
    e.health -= pool[other_h].bonus    // OK: inline read of other element
    pool[other_h].hit_count += 1       // OK: inline write to other element
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Disjoint variables | AL6 | `with pool[h] as e { other_pool.remove(h2) }` is OK |
| Multi-element access | AL3 | `with pool[h1] as e1, pool[h2] as e2 { ... }` is OK |
| Chained methods returning owned | AL1 | Borrow released when ownership transfers |
| Dynamic indices | AL1 | `pool[computed]` borrows entire pool (conservative) |
| Nested expression chains | AL1 | All borrows accumulate on stack |
| Field-level disjointness | AL6 | Optional refinement; phases 1-2 are conservative |

---

## Appendix (non-normative)

### Rationale

**AL1-AL2 (borrow stack + closure scan):** Expression-scoped closures (`mem.closures/EC4`) access outer scope directly. Without detection, a closure could structurally mutate a collection while the calling method holds an element borrow — causing reallocation, handle invalidation, or panics. Compile-time detection kills this bug class with zero runtime cost. Non-structural access (reading/writing other elements) is safe because element borrows don't conflict with access to different slots.

**AL7 (local analysis):** Method signatures provide borrow requirements without examining method bodies. Same cost as existing type checking.

**Phased implementation:** AL3-AL6 cover phases 1-2 (mandatory). Field-level disjoint tracking (phase 3) reduces false positives but can be deferred — phases 1-2 provide safety.

### Patterns & Guidance

**Basic conflict — structural mutations are forbidden:**
<!-- test: skip -->
```rask
with pool[h] as e {
    pool.remove(h)    // ERROR: structural mutation inside with block
}
// Borrow stack: [ElementBorrow(pool, h)]
// with body accesses: [Call(pool.remove)] — structural mutation conflicts with ElementBorrow
```

**Non-structural access — reading/writing other elements is fine:**
<!-- test: skip -->
```rask
with pool[h] as e {
    e.health -= pool[other_h].attack    // OK: inline read of different element
}
// Borrow stack: [ElementBorrow(pool, h)]
// with body accesses: [Read(pool[other_h])] — non-structural, different element, OK
```

**Disjoint variables — different collections never conflict:**
<!-- test: skip -->
```rask
with pool[h] as e {
    other_pool.remove(h2)    // OK: different variable
}
// Borrow stack: [Exclusive(pool)]
// with body accesses: [Call(other_pool.remove)] — pool != other_pool
```

**Multi-element access is compatible:**
<!-- test: skip -->
```rask
with pool[h1] as e1, pool[h2] as e2 {
    // OK: compiler verifies disjoint elements
    // Runtime panic if h1 == h2
}
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

- [Borrowing](borrowing.md) — Value-based access, `with` blocks, block-scoped views (`mem.borrowing`)
- [Boxes](boxes.md) — The container family whose `with` access this analysis secures (`mem.boxes`)
- [Closures](closures.md) — EC1-EC4 rules for expression-scoped closures (`mem.closures`)
- [Pools](pools.md) — Pool `with`-based access (`mem.pools`)

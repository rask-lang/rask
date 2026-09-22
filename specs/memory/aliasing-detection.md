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
ERROR [mem.aliasing/AL5]: cannot structurally mutate `units` inside with block
   |
1  |  with units[i] as e {
   |  ----- element borrowed here
2  |      units.remove(i)
   |      ^^^^^^^^^^^^^^^ structural mutation not allowed

WHY: push, remove, and clear can move the buffer, and the binding points into it.
     Reading and writing other elements is fine.

FIX: Separate the check from the mutation:

  let should_remove = units[i].health <= 0
  if should_remove {
      units.remove(i)
  }
```

**Structural mutation during element borrow (different index) [AL4]:**
```
ERROR [mem.aliasing/AL4]: cannot structurally mutate `units` inside with block
   |
1  |  with units[i] as e {
   |  ----- element borrowed here
2  |      units.remove(j)
   |      ^^^^^^^^^^^^^^^ structural mutation not allowed

WHY: remove shifts the rest of the buffer, so the binding may no longer name
     the element it was taken from — a different index is no defence.

FIX: Move the mutation outside the with block:

  let should_remove = with units[i] as e { e.health <= 0 }
  if should_remove {
      units.remove(j)
  }
```

Non-structural access to other elements is allowed:
```rask
with units[i] as e {
    e.health -= units[j].bonus    // OK: inline read of another element
    units[j].hit_count += 1       // OK: inline write to another element
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Disjoint variables | AL6 | `with units[i] as e { others.remove(k) }` is OK |
| Multi-element access | AL3 | `with units[i] as a, units[j] as b { ... }` is OK |
| Chained methods returning owned | AL1 | Borrow released when ownership transfers |
| Dynamic indices | AL1 | `units[computed]` borrows the whole collection (conservative) |
| Nested expression chains | AL1 | All borrows accumulate on stack |
| Field-level disjointness | AL6 | Optional refinement; phases 1-2 are conservative |

---

## Appendix (non-normative)

### Rationale

**AL1-AL2 (borrow stack + closure scan):** Expression-scoped closures (`mem.closures/MC1`) access outer scope directly. Without detection, a closure could structurally mutate a collection while the calling method holds an element borrow — moving the buffer out from under the binding. Compile-time detection kills this bug class with zero runtime cost. Non-structural access (reading/writing other elements) is safe because element borrows don't conflict with access to different slots.

**AL7 (local analysis):** Method signatures provide borrow requirements without examining method bodies. Same cost as existing type checking.

**Phased implementation:** AL3-AL6 cover phases 1-2 (mandatory). Field-level disjoint tracking (phase 3) reduces false positives but can be deferred — phases 1-2 provide safety.

### Patterns & Guidance

**Basic conflict — structural mutations are forbidden:**
<!-- test: skip -->
```rask
with units[i] as e {
    units.remove(i)    // ERROR: structural mutation inside with block
}
// Borrow stack: [ElementBorrow(units, i)]
// with body accesses: [Call(units.remove)] — structural mutation conflicts with ElementBorrow
```

**Non-structural access — reading/writing other elements is fine:**
<!-- test: skip -->
```rask
with units[i] as e {
    e.health -= units[j].attack    // OK: inline read of a different element
}
// Borrow stack: [ElementBorrow(units, i)]
// with body accesses: [Read(units[j])] — non-structural, different element, OK
```

**Disjoint variables — different collections never conflict:**
<!-- test: skip -->
```rask
with units[i] as e {
    others.remove(k)    // OK: different variable
}
// Borrow stack: [Exclusive(units)]
// with body accesses: [Call(others.remove)] — units != others
```

**Multi-element access is compatible:**
<!-- test: skip -->
```rask
with units[i] as a, units[j] as b {
    // OK: compiler verifies disjoint elements
    // Runtime panic if i == j
}
```

**Chained methods — borrow depends on return type:**
<!-- test: skip -->
```rask
units.first()?.transform().apply(|v| {
    units.push(v)
})
// If transform() returns an owned value: borrow stack empty, OK
// If transform() returns a reference into units: Shared(units) active, ERROR
```

### See Also

- [Borrowing](borrowing.md) — Value-based access, `with` blocks, block-scoped views (`mem.borrowing`)
- [Shared, Rack and Heap](shared-rack-heap.md) — The types whose `with` access this analysis secures (`mem.shared-rack-heap`)
- [Cell](cell.md) — Retired: one value, exclusive access (`mem.cell`)
- [Closures](closures.md) — EC1-EC4 rules for expression-scoped closures (`mem.closures`)
- [Racks and Links](racks.md) — a link is a reference, not a borrow (`mem.racks`)
- [Heap Values](heap.md) — Single-consumer semantics remove aliasing entirely (`mem.heap`)
- [Synchronization](../concurrency/sync.md) — `Shared<T, S>` and its lock strategies (`conc.sync`)

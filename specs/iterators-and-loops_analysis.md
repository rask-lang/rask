# Iterator Specification Gap Analysis

## Summary
Analyzed `specs/iterators-and-loops.md` against `CORE_DESIGN.md` and related specs (`memory-model.md`, `dynamic-data-structures.md`).

**Total gaps identified:** 10
- HIGH priority: 3
- MEDIUM priority: 5
- LOW priority: 2

## HIGH Priority Gaps

### Gap 1: Collection Borrowing During Iteration
**Type:** Underspecified + Potential Conflict
**Priority:** HIGH

**Question:** How can code iterate a collection and simultaneously access it?

The spec shows:
```
for i in vec {
    process(&vec[i]);
}
```

**Unclear:**
- Does `for i in vec` borrow `vec`?
- If yes, how can `vec[i]` work (accessing borrowed collection)?
- If no, what prevents mutation of `vec` during iteration (like `vec.clear()`)?
- This is the CORE ergonomic question for the entire design

**Related:** Memory model specifies expression-scoped borrows for collections, but loop semantics aren't fully integrated.

---

### Gap 2: Iterator Adapter Implementation & Closure Capture
**Type:** Underspecified + Semantic Conflict
**Priority:** HIGH

**Question:** How do iterator adapters work without violating "no storable references"?

The spec shows:
```
for i in vec.indices().filter(|i| vec[*i].active).take(10) {
    process(&vec[i]);
}
```

**Unclear:**
- The closure `|i| vec[*i].active` accesses `vec` from outer scope
- Spec says: "expression-scoped capture: closure is fully evaluated before next iteration"
- But `memory-model.md` says closures capture by value (copy or move), never by reference
- **CONFLICT:** How does the closure access `vec` without capturing it?
- What is the type of `.filter(...)`? It must hold the closure somehow.
- How does lazy evaluation work without stored references to the collection?

**Related:** This is fundamental to adapter ergonomics and may require special compiler treatment.

---

### Gap 3: Pool Iteration Semantics
**Type:** Specification Conflict
**Priority:** HIGH

**Question:** What does `for h in pool` yield?

**Conflict between specs:**

`iterators-and-loops.md` line 149-154:
```
for h in pool {
    pool.remove(h)?.close()?;
}
```
Implies `h` is `Handle<T>`.

`dynamic-data-structures.md` line 187-189:
```
for (handle, item) in &pool { }  // handle: Handle<T>, item: &T
pool.handles() -> Iterator<Handle<T>>
```
Implies iteration yields tuples.

**Unclear:**
- Is `for h in pool` syntax sugar for `for h in pool.handles()`?
- Or does `for h in pool` iterate differently than `for (h, item) in &pool`?
- What are ALL the iteration modes for Pool<T>?

---

## MEDIUM Priority Gaps

### Gap 4: Iterator Adapter Type System
**Type:** Underspecified
**Priority:** MEDIUM

**Question:** What are the types and composition rules for adapters?

The spec shows adapters (`filter`, `take`, `skip`, `rev`) but doesn't specify:
- Return types (generic iterator types?)
- How chaining works (trait-based composition?)
- Lazy vs eager evaluation guarantees
- Whether adapters can be stored in variables

**Example:**
```
let filtered = vec.indices().filter(|i| vec[*i].active);  // What type?
for i in filtered { ... }  // Can this work?
```

**Impact:** Needed for implementation and understanding composition limits.

---

### Gap 5: Map Iteration Ergonomics
**Type:** Underspecified Alternative
**Priority:** MEDIUM

**Question:** What's the recommended pattern for iterating Map<String, V>?

The spec shows:
```
// ERROR: string is not Copy
for key in config { ... }

// Required:
for (key, value) in config.drain() { ... }
```

And mentions:
```
for key in config.keys_cloned() {
    print(config[key.clone()]);  // Clone twice
}
```

**Unclear:**
- Is double-cloning really the intended pattern?
- What does `map.drain()` yield exactly? `(K, V)` tuple?
- Are there other patterns (closure-based access like `map.modify_all(...)`)?
- Is this ergonomic enough for ED ≤ 1.2 goal?

---

### Gap 6: Mutation During Iteration - Bounds Checking
**Type:** Underspecified Edge Case
**Priority:** MEDIUM

**Question:** What happens when mutation invalidates indices?

The spec says (line 196-201):
```
for i in vec {
    if vec[i].expired {
        vec.swap_remove(i);  // Invalidates subsequent indices
    }
}
```
"Compiler MUST NOT error. Runtime behavior: later indices may be invalid..."

**Unclear:**
- Does `vec[i]` panic when index becomes out-of-bounds due to removal?
- Or does the iterator skip invalidated indices?
- Should there be a `get_checked(i)` pattern for safe mutation?
- What's the detailed behavior when length changes during iteration?

---

### Gap 7: Error Propagation in Loops
**Type:** Underspecified Interaction
**Priority:** MEDIUM

**Question:** How does `?` interact with loop state and cleanup?

The spec shows:
```
for i in lines {
    let parsed = parse(&lines[i])?;  // Exits loop on error
}
```

**Unclear:**
- Does `?` behave differently than `break` for loop cleanup?
- For `drain()` loops: are remaining items dropped when `?` exits?
- Is this the same LIFO drop as `break`?
- How does this interact with `ensure` cleanup registered in loop body?

---

### Gap 8: drain() Implementation Details
**Type:** Underspecified Mechanism
**Priority:** MEDIUM

**Question:** How is `drain()` implemented without stored references?

The spec says:
- `drain()` yields owned values
- Early exit drops remaining items in LIFO order
- But Rask has "no storable references"

**Unclear:**
- What type does `.drain()` return?
- How does it track position without storing a reference to the collection?
- Is this a special compiler-supported iterator?
- Can drain iterators be stored in variables, or must they be consumed immediately?

**Example:**
```
let drainer = vec.drain();  // Can you do this?
for item in drainer { ... }  // Or must drain be inline?
```

---

### Gap 9: for-in Syntax Sugar Details
**Type:** Underspecified Desugaring
**Priority:** MEDIUM

**Question:** How does `for x in expr` desugar?

The spec defines behavior for collections but doesn't specify:
- Is there a trait-based iteration protocol?
- Can user types implement custom iteration?
- What's the exact desugaring of `for i in vec`?

**Example:**
```
for i in my_custom_collection { ... }  // How does this work?
```

**Impact:** Needed for generic iteration and custom collection types.

---

## LOW Priority Gaps

### Gap 10: Range Iteration Edge Cases
**Type:** Underspecified Edge Cases
**Priority:** LOW

**Question:** How do range variants behave?

The spec mentions `0..n` yields integers, but doesn't cover:
- Reversed ranges: `n..0` (empty? error?)
- Open ranges: `0..` (infinite iterator?)
- Unbounded: `..` (what does this mean?)
- Step ranges: `0..10 step 2` (if supported?)

---

### Gap 11: Zero-Sized Type Iteration Rationale
**Type:** Underspecified Rationale
**Priority:** LOW

**Question:** Why allow `Vec<()>` iteration?

Edge case table (line 256) says:
```
Vec<()> | Yields indices 0..len despite no data
```

**Unclear:**
- What's the use case for iterating `Vec<()>`?
- Is this just a natural consequence of the design, or intentionally supported?
- Any special handling needed?

Not critical, but would be good to document the rationale.

---

## Recommendations

**Process 3 HIGH priority gaps first:**
1. Gap 1: Collection borrowing during iteration (foundational)
2. Gap 2: Iterator adapter implementation (core ergonomics)
3. Gap 3: Pool iteration semantics (specification conflict)

**Then process MEDIUM gaps as needed for completeness.**

**LOW gaps can be addressed in future refinement passes.**

# Memory Model: Open Issues

Remaining stress points that need specification or resolution.

---

## 1. Expression-Scoped Aliasing Detection

Rule EC4 (aliasing rules apply to expression-scoped closures) requires sophisticated local analysis.

**The Issue:** `pool[h].modify(|e| { pool.remove(h) })` — compiler must detect mutation-while-borrowed inside complex expression chains.

**The Complexity:** Avoiding global escape analysis while catching these errors requires more specification.

**Preliminary Research:**

The detection is **intra-procedural** (local to each function) - no whole-program analysis needed:

1. **Track active borrows** within each expression (e.g., `pool[h]` borrows `pool`)
2. **Scan closure body** for variable accesses when closure is encountered
3. **Check conflicts** between closure's accesses and active borrows

This mirrors Rust's approach (RFC 2229 disjoint field capture) but simpler since expression-scoped closures can't escape. Method signatures declare borrow requirements (`read self`, `mut self`), so the compiler knows `modify()` borrows the receiver without analyzing its body.

**Complexity:** O(function size) - same as existing borrow checking.

**Status:** Detection mechanism researched, formal specification pending.

---

## Summary

| Issue | Primary Metric Risk | Severity |
|-------|---------------------|----------|
| 1. Expression-Scoped Aliasing | Local analysis complexity | Medium |

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
| 6 | Dual Borrowing Semantics | borrowing.md (load-bearing for mutation-during-iteration) |
| 7 | Scope-Constrained Closures | closures.md (BC1-BC5 rules) |

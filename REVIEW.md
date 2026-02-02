# Rask Language Design Review

A comprehensive assessment of the Rask language specification.

---

## Executive Summary

Rask achieves its stated goal: **safety that doesn't feel like safety**. The core insight—handle-based indirection with generation checks instead of a borrow checker—is genuinely novel and well-executed. This isn't "Rust with different syntax"; it's a meaningfully different approach to memory safety.

**Verdict:** The design is solid enough for prototype implementation. The specification is ~90% complete with no critical blockers. Remaining issues can be resolved during implementation.

**Key Innovations:**
- Pool/Handle system eliminates lifetime annotations while preserving memory safety
- Concurrency model achieves Go's simplicity with compile-time safety (affine handles)
- No async/await prevents ecosystem split
- Union error types compose naturally with `?` propagation

---

## Strengths

### Pool/Handle System
The star of the design. By routing all "reference-like" semantics through handles into pools:
- Lifetime annotations eliminated entirely
- Borrow checker complexity avoided
- Self-referential structures (graphs, trees, linked lists) become trivial
- Generation checks provide O(1) runtime safety net
- Local-only analysis preserved (CS ≥ 5× Rust target achievable)

### Concurrency Model
Better than Go's:

| Aspect | Go | Rust | Rask |
|--------|-----|------|------|
| Function coloring | No | Yes (async/await) | **No** |
| Forgotten task detection | Silent bug | Manual | **Compile error** |
| Spawn syntax | `go f()` | Complex | `spawn { }.detach()` |

Affine handles on tasks enforce "you must decide what happens to this task."

### No Async/Await
Bold and correct. I/O pauses automatically; IDE shows where. This eliminates the ecosystem split plaguing Rust, JavaScript, Python, and C#.

### Linear Types + `ensure`
Clean combination:
- `linear struct` for must-consume resources
- `ensure` for deferred cleanup without RAII's hidden destructor costs

More explicit than Rust's Drop, more ergonomic than manual cleanup.

### Union Error Types
`Result<T, IoError | ParseError>` with automatic widening via `?` solves error type proliferation without ceremony or opacity.

### IDE-First Philosophy
"Compiler Knowledge is Visible" means the language can be simpler because the IDE shows inferences. Right direction for modern tooling.

### Integer Overflow Safety
Panic in both debug and release is safer than Rust's divergent behavior. `Wrapping<T>` type is cleaner than custom operators.

---

## High Priority Issues

### 1. Dual Borrowing Semantics is Confusing
**Severity:** High | **Effort:** Documentation/Tooling

Block-scoped borrowing for plain values, expression-scoped for collections. Users will stumble:

> "Why can I hold a `&str` across statements but not a `vec[i]`?"

The answer (collections can grow, invalidating references) is correct but not intuitive. Programmers from any mainstream language will expect:
```
let entity = pool[h]
entity.health -= 10  // Why doesn't this work?
```

**Recommendation:**
- Invest heavily in error messages explaining *why* rules differ
- IDE should show borrow scope visually
- Consider whether terminology can clarify ("stable borrow" vs "volatile access"?)
- Alternative: Unify to expression-scoped for everything (trade string ergonomics for consistency)

### 2. Expression-Scoped Aliasing Detection Under-Specified
**Severity:** High | **Effort:** Specification

Rule EC4 requires detecting:
```
pool[h].modify(|e| { pool.remove(h) })  // Should error
```

The detection algorithm isn't fully specified. This could either:
- Require more complex analysis than claimed (violating "local only")
- Fall back to runtime error (generation check fails)

**Recommendation:** Commit to one approach:
- Option A: Specify the detection algorithm fully
- Option B: Accept runtime fallback (safe, just surprising) and document it

The generation check provides a safety net either way—it's memory-safe regardless.

### 3. Ambient Pool Scoping Breaks Transparency
**Severity:** High | **Effort:** Design Decision

`with pool { h.health }` where `h.health` auto-resolves to `pool[h].health` is convenient but breaks the "transparency" principle. A reader seeing `h.health` doesn't know it's a pool access without context.

```
with players {
    damage_player(player_handle, 10)  // Does this mutate players?
}

fn damage_player(h: Handle<Player>, amount: i32) {
    h.health -= amount  // Where does this pool come from?
}
```

**Questions:**
- Does the ambient scope cross module boundaries?
- How do you write helper functions that work with ambient pools?
- Is "code should be readable without IDE" still a goal?

**Recommendation:** Either require explicit pool in function signatures, or make ambient pools very visible. IDE annotations aren't enough—code should be readable standalone.

### 4. Linear Types + Pools Interaction Unspecified
**Severity:** High | **Effort:** Specification

`Pool<Linear>` is allowed but `Vec<Linear>` is not. But what happens here:

```
let files: Pool<File> = Pool::new()
let h1 = files.insert(File::open("a.txt")?)?
let h2 = files.insert(File::open("b.txt")?)?
// What if I drop `files` here without removing/closing?
```

Is dropping a `Pool<Linear>` with unconsumed elements:
- A compile error? (How? Compiler can't track dynamic contents)
- A runtime panic? (Defeats "mechanical correctness")
- Silent resource leak? (Defeats the point of linear types)

**Recommendation:** Specify explicitly. Likely answer: runtime panic on drop if non-empty, with `pool.drain()` pattern for safe cleanup.

---

## Medium Priority Issues

### 5. Closure-Based Sync Nested Access
**Severity:** Medium | **Effort:** Specification

`Mutex<T>` and `Shared<T>` use closures to prevent reference escape. The spec says "nested locks = compile error" but the detection algorithm isn't specified.

```
let config = Shared.new(AppConfig { db_pool: ConnectionPool::new() })

config.read(|c| {
    c.db_pool.checkout()?  // db_pool is also Mutex<Vec<Connection>>
    // Nested lock attempt - what happens?
})
```

Is this always detectable at compile time? What if the inner lock is behind a trait method?

**Recommendation:** Specify whether nested lock detection is:
- Syntactic (direct nested calls only)
- Semantic (all nested lock acquisition)
- Best-effort with runtime fallback

### 6. Generation Check Coalescing Under-Specified
**Severity:** Medium | **Effort:** Specification

The spec says "compiler automatically eliminates redundant generation checks" but doesn't specify:
- Is coalescing guaranteed or best-effort?
- Can users rely on it for performance-critical code?
- What's the escape hatch if it doesn't work?

If coalescing is critical for RO ≤ 1.10, it needs to be a guaranteed optimization.

**Recommendation:** Either:
- Guarantee coalescing with specified rules, or
- Provide explicit `pool.get_unchecked(h)` for when users need guaranteed zero-check access

### 7. Aliased Handles Allow Observable Mutation
**Severity:** Medium | **Effort:** Documentation

```
let h1 = pool.insert(entity)?
let h2 = h1  // h2 is a copy

pool[h1].health -= 10
pool[h2].health -= 10  // Both point to same entity
```

This is valid—both mutations work. The "aliasing XOR mutation" rule applies to borrows, not handles. But with ambient pools:

```
with pool {
    h1.health -= 10
    h2.health -= 10  // Same entity mutated twice through different handles
}
```

This is observable aliased mutation, which may confuse users expecting the aliasing rules to apply.

**Recommendation:** Document clearly that handles are like database primary keys—multiple copies can exist and all access the same row. The aliasing rule applies to borrows, not handle identity.

### 8. Closure Capture Rules Need Validation
**Severity:** Medium | **Effort:** User Study

Rules BC1-BC5 are well-specified on paper, but closure captures are notoriously hard to get right. The rules may not be predictable enough for PI ≥ 0.85 target.

**Recommendation:** Conduct user studies during prototyping. If users frequently guess wrong, simplify rules even at cost of flexibility.

### 9. Comptime Cannot Allocate
**Severity:** Medium | **Effort:** Design Decision

Comptime excludes heap allocation and I/O. This means "generate code from JSON schema" requires build scripts, not comptime—a meaningful limitation compared to Zig.

**Recommendation:** Acceptable, but clearly document the boundary. Consider whether limited comptime allocation (fixed-size arena) is worth the complexity.

---

## Low Priority Issues

### 10. SIMD Types Not Addressed
**Severity:** Low | **Effort:** Design

Should built-in vector types like `f32x4` exist?

**Recommendation:** Defer to post-MVP. Can be added as library types or compiler intrinsics later.

### 11. Attribute Syntax Not Finalized
**Severity:** Low | **Effort:** Low

`#[...]` vs `@...` not decided.

**Recommendation:** Pick one and document. `#[...]` matches Rust familiarity; `@...` is cleaner.

### 12. ~~Intra-Package Init Order~~ ✅ ADDRESSED
**Severity:** ~~Low~~ | **Status:** Resolved

**Resolution:** Added to [specs/structure/modules.md](specs/structure/modules.md):
- Package-level mutable state MUST use sync primitives (`Atomic`, `Mutex`, `Shared`)
- Intra-package init order: parallel topological sort of import DAG
- Files with no dependency relationship run concurrently (safe because sync required)
- At most one `init()` per file; circular init dependencies are compile errors

---

## Monitoring Items

Watch these during prototyping—not issues yet, but potential risks:

### 1. Ambient Pool Nesting Depth
`with pool { }` is ergonomic, but multi-pool contexts (`with (pool1, pool2) { }`) could become nested callback hell in complex code.

**Action:** Track nesting depth in real programs. If >2 is common, reconsider design.

### 2. ED Metric Validation
Ergonomic Delta (ED ≤ 1.2) is theoretical. Need to write the canonical programs and measure against Go.

**Action:** Implement HTTP server, grep, text editor in Rask and Go. Compare line counts and nesting depth.

### 3. Generation Check Overhead
RO ≤ 1.10 depends on generation check optimization (freezing, coalescing).

**Action:** Benchmark hot paths with and without optimization. If >10% overhead, invest more in optimization passes.

---

## Metrics Compliance

| Metric | Target | Status | Notes |
|--------|--------|--------|-------|
| **TC** (Transparency) | ≥ 0.90 | ⚠️ At Risk | Ambient pools may violate |
| **MC** (Mechanical Correctness) | ≥ 0.90 | ✅ Pass | 10/11 bug classes prevented (91%) |
| **UCC** (Use Case Coverage) | ≥ 0.80 | ✅ Pass | Calculated 0.92 |
| **PI** (Predictability) | ≥ 0.85 | ⚠️ Unvalidated | Dual borrowing may hurt |
| **ED** (Ergonomic Delta) | ≤ 1.2 | ⚠️ Unvalidated | Needs real programs |
| **SN** (Syntactic Noise) | ≤ 0.3 | ✅ Likely Pass | `?` and `ensure` minimal |
| **RO** (Runtime Overhead) | ≤ 1.10 | ⚠️ Unvalidated | Depends on coalescing |
| **CS** (Compilation Speed) | ≥ 5× Rust | ✅ Likely Pass | No whole-program analysis |

---

## Recommendation

**Begin prototype implementation.**

The design is sound. Critical systems (memory model, type system, concurrency) are well-specified and coherent. The Pool/Handle system is genuine innovation worth validating in practice.

**Implementation order:**
1. Core type system + ownership/move semantics
2. Pool/Handle with generation checks
3. Basic concurrency (spawn/join/detach)
4. Linear types + ensure
5. Channels
6. Comptime (can defer initially)
7. C interop (can defer initially)

**First milestone:** Implement the grep clone test program. This exercises:
- File I/O (linear types)
- String handling (block-scoped borrows)
- Error propagation (`?`)
- Basic control flow

Real programs will reveal whether the ergonomic tradeoffs are correct. The dual borrowing model and ambient pool scoping are the most likely pain points—watch user confusion there.

---

*Review date: 2026-02-02*

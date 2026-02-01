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

## Strengths (Preserve These)

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

### Memory Model

#### 1. Dual Borrowing Semantics is Confusing
**Severity:** High | **Effort:** Documentation/Tooling

Block-scoped borrowing for plain values, expression-scoped for collections. Users will stumble:

> "Why can I hold a `&str` across statements but not a `vec[i]`?"

The answer (collections can grow, invalidating references) is correct but not intuitive.

**Recommendation:**
- Invest heavily in error messages explaining *why* rules differ
- IDE should show borrow scope visually
- Consider whether terminology can clarify ("stable borrow" vs "volatile access"?)

#### 2. Expression-Scoped Aliasing Detection Under-Specified
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

### Concurrency

#### 3. ~~No Shared-State Primitives Beyond Channels~~ ✅ ADDRESSED
**Severity:** ~~High~~ | **Status:** Resolved

**Resolution:** Added [specs/concurrency/sync.md](specs/concurrency/sync.md) with:
- `Shared<T>` — Read-heavy concurrent access (`read(|v| ...)`, `write(|v| ...)`)
- `Mutex<T>` — Exclusive access for write-heavy patterns (`lock(|v| ...)`)
- Closure-based API prevents reference escape and deadlock
- Examples for config, metrics, connection pools, feature flags

Also updated:
- [ownership.md](specs/memory/ownership.md) — Rule T2.1 clarifies closure-based mutable access is the allowed exception
- [value-semantics.md](specs/memory/value-semantics.md) — Sync types never Copy

---

## Medium Priority Issues

### Memory Model

#### 4. Handle Size at Copy Threshold
**Severity:** Medium | **Effort:** Low

Default handle is exactly 16 bytes (pool_id: u32 + index: u32 + generation: u64), right at the copy threshold.

**Implications:**
- Handles are Copy (good)
- Any future expansion breaks Copy semantics
- No headroom for additional metadata

**Recommendation:** Consider 12-byte handles (u32 generation) as default, leaving room for future additions while remaining Copy.

#### 5. Closure Capture Rules Need Validation
**Severity:** Medium | **Effort:** User Study

Rules BC1-BC5 are well-specified on paper, but closure captures are notoriously hard to get right. The rules may not be predictable enough for PI ≥ 0.85 target.

**Recommendation:** Conduct user studies during prototyping. If users frequently guess wrong, simplify rules even at cost of flexibility.

### Type System

#### 6. Comptime Cannot Allocate
**Severity:** Medium | **Effort:** Design Decision

Comptime excludes:
- Heap allocation (no `Vec` at comptime)
- I/O (can't read files for codegen)

This means "generate code from JSON schema" requires build scripts, not comptime.

**Recommendation:** This is acceptable, but clearly document the boundary. Consider whether limited comptime allocation (fixed-size arena) is worth the complexity.

#### 7. Box<T> Semantics Only Mentioned in Passing
**Severity:** Medium | **Effort:** Specification

`Box<T>` appears in sum-types.md for recursive enums but has no dedicated spec. Questions:
- How does Box interact with pools?
- Is Box linear?
- What's the allocation strategy?

**Recommendation:** Add [specs/memory/box.md](specs/memory/box.md) specifying Box semantics.

### Specification Gaps

#### 8. Operator Precedence Not Specified
**Severity:** Medium | **Effort:** Low

No full operator precedence table. No specification for:
- Bitwise operators (`&`, `|`, `^`, `~`, `<<`, `>>`)
- Assignment operators (`+=`, `-=`, etc.)
- Comparison operator chaining

**Recommendation:** Add [specs/types/operators.md](specs/types/operators.md) with precedence table.

#### 9. ~~Build System Not Specified~~ ✅ ADDRESSED
**Severity:** ~~Medium~~ | **Status:** Resolved

**Resolution:** Added [specs/structure/build.md](specs/structure/build.md) with:
- `rask.build` file format and `build(ctx: BuildContext)` entry point
- BuildContext API for codegen, C compilation, external tools
- `.rask-gen/` auto-inclusion of generated files
- Build lifecycle and incremental build triggers
- `[build-dependencies]` for build-time-only packages

Also updated [specs/control/comptime.md](specs/control/comptime.md):
- Added `@embed_file` intrinsic for compile-time file embedding
- Clarified comptime vs build scripts boundary

#### 10. ~~Standard Library Scope Undefined~~ ✅ ADDRESSED
**Severity:** ~~Medium~~ | **Status:** Resolved

**Resolution:** Added [specs/stdlib/README.md](specs/stdlib/README.md) with:
- Batteries-included philosophy (24 modules total)
- Module organization by category
- Prelude specification (built-in vs import)
- 11 new modules: json, http, tls, cli, encoding, hash, url, unicode, terminal, csv, bits
- Clear "out of scope" criteria (frameworks, full crypto, regex)

### Ergonomics

#### 11. ~~Parameter Modes Not Consolidated~~ ✅ ADDRESSED
**Severity:** ~~Medium~~ | **Status:** Resolved

**Resolution:** Simplified from three modes to two:
- **Borrow** (default, no keyword) — temporary access, compiler infers read vs mutate
- **`take`** — ownership transfer

Added [specs/memory/parameters.md](specs/memory/parameters.md) consolidating parameter passing semantics.

Updated specs to use new terminology:
- [value-semantics.md](specs/memory/value-semantics.md) — parameter passing table
- [linear-types.md](specs/memory/linear-types.md) — `take self` for consuming methods
- [enums.md](specs/types/enums.md) — pattern binding mode inference

---

## Low Priority Issues

### Concurrency

#### 12. Select Arm Evaluation Order Unspecified
**Severity:** Low | **Effort:** Low

When multiple select arms are ready simultaneously, which fires? Options:
- Random
- First-listed
- Implementation-defined

**Recommendation:** Specify "first-ready with implementation-defined tie-breaking" or explicitly leave unspecified.

### Type System

#### 13. SIMD Types Not Addressed
**Severity:** Low | **Effort:** Design

Should built-in vector types like `f32x4` exist?

**Recommendation:** Defer to post-MVP. Can be added as library types or compiler intrinsics later.

#### 14. `char` Type Necessity Unclear
**Severity:** Low | **Effort:** Design

Is the 4-byte `char` type needed, or just use `u32` + validation?

**Recommendation:** Keep `char` for Unicode correctness. Document why it's distinct from `u32`.

### Tooling

#### 15. Benchmark/Fuzzing Support Missing
**Severity:** Low | **Effort:** Medium

Testing is specified (`test "name" {}` blocks), but no:
- `bench` blocks for benchmarking
- Property-based testing
- Fuzzing integration

**Recommendation:** Defer to post-MVP. Add [specs/stdlib/bench.md](specs/stdlib/bench.md) later.

### Specification Gaps

#### 16. Inline Assembly Not Specified
**Severity:** Low | **Effort:** Medium

`asm!` mentioned in unsafe.md but syntax/semantics not specified.

**Recommendation:** Defer to post-MVP. Not needed for initial implementation.

#### 17. Attribute Syntax Not Finalized
**Severity:** Low | **Effort:** Low

`#[...]` vs `@...` not decided.

**Recommendation:** Pick one and document. `#[...]` matches Rust familiarity; `@...` is cleaner.

---

## Monitoring Items

Watch these during prototyping—not issues yet, but potential risks:

### 1. Ambient Pool Ergonomics
`with pool { pool[h].foo }` is ergonomic, but multi-pool contexts (`with (pool1, pool2) { }`) could become nested callback hell in complex code.

**Action:** Track nesting depth in real programs. If >2 is common, reconsider design.

### 2. ED Metric Validation
Ergonomic Delta (ED ≤ 1.2) is theoretical. Need to write the 7 canonical programs and measure against Go.

**Action:** Implement HTTP server, grep, text editor in Rask and Go. Compare line counts and nesting depth.

### 3. Generation Check Optimization Effectiveness
RO ≤ 1.10 depends on generation check optimization (freezing, coalescing).

**Action:** Benchmark hot paths with and without optimization. If >10% overhead, invest more in optimization passes.

### 4. C Interop Implementation Complexity
Bundling libclang (or equivalent) for header parsing is ambitious.

**Action:** Prototype C interop early. If too complex, consider simpler FFI (explicit bindings like Rust).

### 5. Intra-Package Init Order
Currently "UNSPECIFIED"—could cause subtle bugs.

**Action:** Either specify deterministic order or require explicit dependency declaration between init functions.

---

## Metrics Compliance

| Metric | Target | Status | Notes |
|--------|--------|--------|-------|
| **TC** (Transparency) | ≥ 0.90 | ✅ Pass | All major costs visible |
| **MC** (Mechanical Correctness) | ≥ 0.90 | ✅ Pass | 10/11 bug classes prevented (91%) |
| **UCC** (Use Case Coverage) | ≥ 0.80 | ✅ Pass | Calculated 0.92 |
| **PI** (Predictability) | ≥ 0.85 | ⚠️ Unvalidated | Needs user study |
| **ED** (Ergonomic Delta) | ≤ 1.2 | ⚠️ Unvalidated | Needs real programs |
| **SN** (Syntactic Noise) | ≤ 0.3 | ✅ Likely Pass | `?` and `ensure` minimal |
| **RO** (Runtime Overhead) | ≤ 1.10 | ⚠️ Unvalidated | Depends on optimization |
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
- String handling
- Error propagation
- Basic control flow

Real programs will reveal whether the ergonomic tradeoffs are correct. The dual borrowing model and closure rules are the most likely pain points—watch user confusion there.

---

*Review date: 2026-02-02*

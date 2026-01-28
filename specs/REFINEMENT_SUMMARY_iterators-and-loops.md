# Refinement Summary: Iterators and Loops

**Date:** 2026-01-31
**Protocol:** REFINEMENT_PROTOCOL.md
**Category:** iterators-and-loops

## Analysis: 10 gaps identified

**Priority breakdown:**
- HIGH: 3 gaps
- MEDIUM: 5 gaps
- LOW: 2 gaps

**Analysis document:** [specs/iterators-and-loops_analysis.md](iterators-and-loops_analysis.md)

## Addressed: 3 HIGH priority gaps

### 1. Gap 1: Collection Borrowing During Iteration → v001 (integrated)
**Type:** Underspecified + Potential Conflict
**Question:** How can code iterate a collection and simultaneously access it?

**Resolution:**
- `for i in collection` does NOT borrow the collection
- Loop variable receives Copy value (index/handle)
- Collection remains accessible inside loop body
- Each `collection[i]` access follows expression-scoped borrow rules
- Mutations during iteration are allowed (programmer responsibility)

**Impact:** Foundational semantics clarified. Enables natural mutation patterns while maintaining safety.

### 2. Gap 2: Iterator Adapter Implementation & Closure Capture → v002 (integrated)
**Type:** Underspecified + Semantic Conflict
**Question:** How do iterator adapters work without violating "no storable references"?

**Resolution:**
- Adapters use **expression-scoped closures**
- Closures access outer scope WITHOUT capturing
- Closures called immediately during iteration, never stored
- Compiler enforces: closures accessing scope cannot be stored
- Lazy composition without intermediate allocations

**Impact:** Enables ergonomic filtering/mapping. Extends closure semantics with scope access mode.

### 3. Gap 3: Pool Iteration Semantics → v003 (integrated)
**Type:** Specification Conflict
**Question:** What does `for h in pool` yield? How does it differ from `for (h, item) in &pool`?

**Resolution:**
- Collections support multiple iteration modes:
  - **Index/Handle mode:** `for i in vec` → allows mutations
  - **Ref mode:** `for (h, item) in &pool` → read-only, ergonomic
  - **Drain mode:** `for item in vec.drain()` → consuming
- Ref mode: expression-scoped refs, mutations forbidden
- Clear use cases and examples for each mode

**Impact:** Resolves conflict between specs. Clarifies ergonomic patterns for Pool and Map iteration.

## Specification: specs/iterators-and-loops.md

**Before:** 324 lines
**After:** 452 lines
**Net change:** +128 lines

### Sections Added
1. **Loop Borrowing Semantics** — Core rule: index iteration creates no borrow
2. **Collection Iteration Modes** — Unified table for Vec/Pool/Map modes

### Sections Enhanced
3. **Mutation During Iteration** — Safety table, runtime behavior clarified
4. **Iterator Adapters** — Expression-scoped closure execution, storage rules
5. **Pool iteration** — Handle mode vs ref mode examples
6. **Map iteration** — Ref mode for non-Copy keys
7. **Integration Notes** — Closure mode distinction

## Remaining Issues

### MEDIUM Priority Gaps (5 remaining)
Not addressed in this pass (can be addressed in future refinement):

**Gap 4:** Iterator Adapter Type System
- Return types and composition rules for adapters
- Trait-based iteration protocol
- Generic iterator types

**Gap 5:** Map Iteration Ergonomics
- Recommendations for common patterns
- Comparison with other languages
- ED metric validation

**Gap 6:** Mutation During Iteration - Bounds Checking
- Detailed behavior when length changes
- Fallible access patterns for safe mutation
- Runtime semantics specification

**Gap 7:** Error Propagation in Loops
- How `?` interacts with loop state
- Drain + `?` cleanup semantics
- Ensure + loop interaction

**Gap 8:** drain() Implementation Details
- Exact type returned by `.drain()`
- Storage rules for drain iterators
- Implementation without stored references

**Gap 9:** for-in Syntax Sugar Details
- Trait-based iteration protocol
- Custom collection iteration
- Exact desugaring rules

### LOW Priority Gaps (2 remaining)
**Gap 10:** Range Iteration Edge Cases
**Gap 11:** Zero-Sized Type Iteration Rationale

## Versions: versions/iterators-and-loops/v001-v003

### v001: Collection Borrowing During Iteration
- Status: integrated
- Lines added: ~50
- Files: gap.md, elaboration.md, validation_notes.md, metadata.json, spec_before.md, spec_after.md

### v002: Iterator Adapter Implementation
- Status: integrated
- Lines added: ~60
- Extends closure semantics with scope access mode

### v003: Pool Iteration Semantics
- Status: integrated
- Lines added: ~50
- Resolves spec conflict, documents all iteration modes

## Notes

**All HIGH priority gaps addressed successfully.**

**No conflicts with CORE design.** All elaborations align with:
- No storable references
- Expression-scoped borrows
- Local analysis only
- Transparent costs

**One extension made:** Closure semantics extended with "scope access" mode (in addition to existing "capture by value" mode). This enables iterator adapters while maintaining safety constraints.

**Next steps:** Consider addressing MEDIUM priority gaps in future refinement pass, especially:
1. Gap 8 (drain implementation) — would complete the drain specification
2. Gap 7 (error propagation) — important for practical usage
3. Gap 4 (adapter type system) — needed for generic code

**Metrics validation:** The refined specification should be validated against ED ≤ 1.2 using test programs (grep, HTTP server, etc.) to ensure ergonomics meet design goals.

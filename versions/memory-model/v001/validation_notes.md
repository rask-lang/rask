# Validation Notes: Expression-Scoped Borrow Patterns (v001)

## Self-Validation Checklist

### ✅ Conflicts with CORE Design?
**NO.** The solution:
- Preserves expression-scoped borrowing for collections (CORE principle)
- Does NOT add new syntax (avoids complexity)
- Uses existing closure mechanism (already part of language)
- Aligns with "Local Analysis Only" (closure scope is lexically clear)

### ✅ Internally Consistent?
**YES.** The specification:
- Clearly defines when to use each pattern (direct vs. closure)
- Provides concrete examples for each use case
- Specifies error handling within closures
- Defines closure borrowing exclusivity rules

### ✅ Conflicts with Other Category Specs?
**NO.** Cross-checked:
- dynamic-data-structures.md: Already has `read()`/`modify()` API - specification is consistent
- sum-types---enums.md: No conflict with pattern matching
- ensure-cleanup.md: Closures can be used with `ensure` patterns
- string-handling.md: No conflict, strings use block-scoped borrows

### ✅ Complete Enough to Implement?
**YES.** Specification includes:
- Clear rules (ES-1 through ES-6)
- Pattern selection flowchart criteria
- Edge case handling
- Error propagation semantics
- Closure capture interaction
- Integration with iterators

### ✅ Concise?
**YES.** The elaboration document is detailed (for design record), but the integrated specification is:
- ~50 lines in memory-model.md
- ~25 lines in dynamic-data-structures.md
- Focused on practical patterns, not theoretical edge cases

## Design Decisions Made

1. **No new syntax:** Rejected `with` blocks or other block-scoped syntax. Closures are sufficient.

2. **Closures as canonical:** Explicitly documented that closures are THE way to do multi-statement access, not a workaround.

3. **Pattern selection guidance:** Provided clear criteria (1 line vs 2+ lines) to avoid confusion.

4. **ED validation:** Calculated ergonomic density showing compliance with ED ≤ 1.2 constraint using inline closures.

5. **Iterator integration:** Clarified that iterators yield handles/indices, not references, which works naturally with closure pattern.

## Rejected Alternatives

1. **Block-scoped syntax (`with` blocks):** Would add language complexity, conflict with "one way to do it" principle.

2. **Automatic lifetime extension:** Would require whole-program analysis, violates "Local Analysis Only" principle.

3. **Reference-returning methods:** Would require lifetime parameters, violates "No Storable References" principle.

## Integration Impact

- **Memory Model:** Added ~52 lines documenting multi-statement access pattern
- **Dynamic Data Structures:** Added ~25 lines clarifying why closures exist and when to use them
- **CORE Design:** Open question "Expression-Scoped Borrow Patterns" can be marked as RESOLVED
- **No breaking changes:** Specification documents existing capability, doesn't change semantics

## Remaining Work

None for this gap. The specification is complete and integrated.

Optional future work (non-blocking):
- User feedback on inline vs formatted closure preference
- IDE hints for "use .modify() for multi-statement" suggestion
- Documentation examples in user-facing guides

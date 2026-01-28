# Validation Notes for v001

## Self-Validation Checklist

### Does it conflict with CORE design?
**NO.**
- ✅ Aligns with "no storable references" (indices are values, not references)
- ✅ Aligns with "expression-scoped collection borrows" (each access independent)
- ✅ Aligns with "local analysis only" (no loop-level borrow tracking needed)
- ✅ Aligns with "transparent costs" (mutation dangers are visible, no hidden state)

### Is it internally consistent?
**YES.**
- Index iteration → no borrow → collection accessible ✅
- Drain iteration → mutable borrow → collection inaccessible ✅
- Clear distinction between the two modes ✅
- Desugaring rules are consistent with semantics ✅

### Does it conflict with other specs?
**NO.**
- `memory-model.md`: Expression-scoped collection borrows ✅ (lines 112-142)
- `dynamic-data-structures.md`: Collection access methods ✅ (lines 60-77)
- `ensure-cleanup.md`: No conflict, works together ✅

### Is it complete enough to implement?
**YES.**
- Clear desugaring rules provided ✅
- Clear borrowing semantics (no borrow created) ✅
- Clear mutation behavior specified ✅
- Error cases covered in table ✅
- Examples provided ✅

### Is it concise?
**YES.**
- Added ~50 lines to spec
- Used tables for edge cases
- One desugaring example
- No redundant prose

## Integration Approach

**Added new section:** "Loop Borrowing Semantics"
- Placed after "Loop Syntax" and before "Value Access"
- Logical flow: syntax → semantics → access patterns

**Enhanced existing section:** "Mutation During Iteration"
- Added safety table
- Clarified runtime behavior
- Linked back to "no borrow" semantics

## Decision Points

**No conflicts encountered.**
- Elaboration aligns with all existing specs
- No design decision needed from user
- Ready to integrate

## Result
**INTEGRATED** into `specs/iterators-and-loops.md`

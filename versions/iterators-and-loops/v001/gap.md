# Gap 1: Collection Borrowing During Iteration

**Type:** Underspecified + Potential Conflict
**Priority:** HIGH

## The Question
How can code iterate a collection and simultaneously access it?

The spec shows:
```
for i in vec {
    process(&vec[i]);
}
```

## Unclear Points
- Does `for i in vec` borrow `vec`?
- If yes, how can `vec[i]` work (accessing borrowed collection)?
- If no, what prevents mutation of `vec` during iteration (like `vec.clear()`)?
- This is the CORE ergonomic question for the entire design

## Related Constraints
- Memory model specifies expression-scoped borrows for collections
- No storable references principle
- Local analysis only
- Must be ergonomic (ED ≤ 1.2)

## Why This Matters
This is the fundamental ergonomic question for iteration. If not clearly specified, implementers won't know:
- Whether to create a borrow during `for` syntax
- Whether to error on `vec[i]` access inside loops
- How to enforce safety while allowing natural patterns

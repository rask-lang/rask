# Gap 2: Iterator Adapter Implementation & Closure Capture

**Type:** Underspecified + Semantic Conflict
**Priority:** HIGH

## The Question
How do iterator adapters work without violating "no storable references"?

The spec shows:
```
for i in vec.indices().filter(|i| vec[*i].active).take(10) {
    process(&vec[i]);
}
```

## Unclear Points
- The closure `|i| vec[*i].active` accesses `vec` from outer scope
- Spec says: "expression-scoped capture: closure is fully evaluated before next iteration"
- But `memory-model.md` says closures capture by value (copy or move), never by reference
- **CONFLICT:** How does the closure access `vec` without capturing it?
- What is the type of `.filter(...)`? It must hold the closure somehow.
- How does lazy evaluation work without stored references to the collection?
- Can adapter chains be stored in variables or must they be consumed inline?

## Related Constraints
- No storable references (CORE principle 3)
- Closures capture by value (memory-model.md)
- Expression-scoped collection borrows (memory-model.md)
- Must be ergonomic for real use cases

## Why This Matters
Iterator adapters are critical for ergonomics:
- Filtering, mapping, taking, skipping are common operations
- Without adapters, code becomes verbose and error-prone
- The implementation must be clear for compiler builders

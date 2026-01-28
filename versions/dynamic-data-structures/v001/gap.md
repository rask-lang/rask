# Analysis: Expression-Scoped Borrow Patterns

## Context

From CORE_DESIGN.md lines 419-432, expression-scoped collection access (`pool[h].field`) releases at semicolon, preventing multi-statement operations on borrowed collection elements.

**Problem:**
```
let user = users.get(id)?
user.name = "new"   // ERROR: borrow released at semicolon on previous line
```

**Current workaround:**
```
users.get(id)?.name = "new"  // OK: single expression
```

**Existing partial solution:** Closure-based access in dynamic-data-structures.md:
```
vec.modify(i, |v| {
    v.name = "new"
    v.age = 30
})?
```

## Identified Gaps

### Gap 1: Multi-Statement Operations Without Closures
**Priority:** HIGH
**Type:** Underspecified
**Question:** Should Rask provide explicit syntax for block-scoped collection access as an alternative to closures for better ergonomics?

**Analysis:**
- Closures work but add ceremony for simple multi-line mutations
- Chaining becomes verbose for 2-3 line operations
- Need to measure against ED ≤ 1.2 (Ergonomic Density) constraint

**Tradeoffs:**
1. **Status quo (expression-scoped only + closures):**
   - PRO: Simple mental model, no new syntax
   - PRO: Closures already solve the problem
   - CON: Closure ceremony for simple cases
   - CON: May not meet ED ≤ 1.2 for common patterns

2. **Add explicit block-scoped syntax:**
   - PRO: Cleaner for multi-line operations
   - PRO: May improve ED score
   - CON: Two ways to do the same thing (confusing)
   - CON: More syntax to learn
   - CON: Need to specify rules for when block-scope ends

3. **Extend expression scope rules:**
   - PRO: No new syntax
   - CON: Complex rules about what counts as "expression continuation"
   - CON: Potential for confusion about when borrow is released

### Gap 2: Ergonomic Validation Against ED ≤ 1.2
**Priority:** HIGH
**Type:** Missing
**Question:** What concrete examples demonstrate that the current approach meets or fails the ED ≤ 1.2 constraint?

**Need to specify:**
- Comparison of common patterns in Rask vs. Go
- Line count, nesting depth, mental overhead
- Decision threshold: when is closure overhead acceptable vs. when is block-scoped needed?

### Gap 3: Method Chaining vs. Multi-Statement
**Priority:** MEDIUM
**Type:** Underspecified
**Question:** How should the design guide users to choose between chaining, closures, and multi-statement?

**Patterns to clarify:**
```
// Pattern A: Chain (single expression)
pool[h].field.method().other_method()

// Pattern B: Closure (multi-statement)
pool.modify(h, |v| {
    v.field = compute()
    v.other = transform()
})?

// Pattern C: Extract + operate (copy out)
let value = pool[h].field
let result = value.process()
pool[h].result = result

// Pattern D: Multiple single accesses
pool[h].x = compute_x()
pool[h].y = compute_y()
```

When is each appropriate? What are the runtime costs?

### Gap 4: Error Handling in Multi-Statement Access
**Priority:** MEDIUM
**Type:** Underspecified
**Question:** How does error propagation (`?`) interact with block-scoped or closure-based access?

**Cases to specify:**
```
// With closure - works today
pool.modify(h, |v| {
    v.data = parse(input)?  // Error propagates out of closure
    v.timestamp = now()
    Ok(())
})?

// Hypothetical block-scoped syntax
with pool[h] as v {
    v.data = parse(input)?  // Should this propagate out of block?
    v.timestamp = now()
}
```

## Recommendation

**Specify a tiered approach:**

1. **Default (expression-scoped):** Remains for simple field access and method chaining
2. **Closure API (current):** Official pattern for multi-statement operations
3. **Do NOT add block-scoped syntax** unless ED validation shows closure overhead exceeds 1.2× threshold

**Rationale:**
- Closures already solve the problem
- Adding another mechanism increases language complexity
- Better to optimize closure syntax/ergonomics than add new syntax
- Meets "Local Analysis Only" principle - closure scope is lexically clear

## Next Steps

1. Write specification for closure-based multi-statement access patterns
2. Add examples comparing Rask closures to equivalent Go code for ED validation
3. Document when to use each pattern (chain vs. closure vs. copy-out)
4. Clarify error propagation within closures
5. Add this to memory-model.md and cross-reference from dynamic-data-structures.md

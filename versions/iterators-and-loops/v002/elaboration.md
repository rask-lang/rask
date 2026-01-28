# Elaboration: Iterator Adapter Implementation & Closure Capture

## Specification

### Iterator Adapter Semantics

**Core principle:** Iterator adapters operate on index/handle streams using **expression-scoped closures**. The closure is evaluated immediately during iteration and does not escape its scope.

### Adapter Types

Adapters are **lazy** and **non-storing**. They transform the iteration protocol without creating intermediate collections.

| Adapter | Signature | Behavior |
|---------|-----------|----------|
| `.filter(pred)` | `(Index -> bool) -> Iterator` | Yields indices where predicate is true |
| `.take(n)` | `usize -> Iterator` | Yields first n indices |
| `.skip(n)` | `usize -> Iterator` | Skips first n indices |
| `.rev()` | `() -> Iterator` | Reverses iteration order |
| `.map(f)` | `(Index -> R) -> Iterator<R>` | Transforms each index |

**Return type:** These return opaque iterator types that implement the iteration protocol. The specific type names are implementation details.

### Expression-Scoped Closure Execution

**Key insight:** Closures used in adapters are **called immediately** and **never stored**. They access outer scope variables without capturing them.

```
// Written:
for i in vec.indices().filter(|idx| vec[*idx].active) {
    body
}

// Desugaring:
{
    let _len = vec.len();
    let _pos = 0;
    while _pos < _len {
        let i = _pos;
        // Closure called HERE, in expression scope
        if (|idx| vec[*idx].active)(&i) {
            body
        }
        _pos += 1;
    }
}
```

**Analysis:**
- The closure `|idx| vec[*idx].active` receives `&i` as parameter
- Inside closure body, `vec` is accessed from outer scope (NOT captured)
- Closure is called immediately: `(|idx| ...)(& i)`
- Closure does not outlive the expression
- `vec` remains accessible in outer scope

**This is legal because:**
1. Closure doesn't escape the expression scope
2. Compiler can verify closure is called immediately, not stored
3. `vec` borrow from `vec[*idx]` is expression-scoped (released at closure return)
4. No reference is stored — closure execution is transient

### Closure Capture vs. Scope Access

**Two distinct modes:**

| Mode | Syntax | Semantics | Validity |
|------|--------|-----------|----------|
| **Capture** | `let f = \|\| vec.len()` | Closure moves/copies `vec` | ✅ Storable closure |
| **Scope access** | `vec.filter(\|i\| vec[*i].x)` | Closure accesses `vec` without capture | ❌ Cannot store |

**Scope access is allowed when:**
- Closure is passed directly to adapter (not assigned to variable)
- Adapter consumes closure immediately (doesn't store it)
- Closure doesn't outlive the expression/statement

**Forbidden:**
```
// ERROR: cannot store closure with scope access
let predicate = |i| vec[*i].active;
for i in vec.indices().filter(predicate) { ... }
```

**Why forbidden:** Storing the closure would mean `vec` access outlives the expression scope, violating expression-scoped borrow rules.

**Allowed with explicit capture:**
```
// OK: closure captures i32 values, not references
let min = 10;
let max = 20;
for i in vec.indices().filter(|idx| {
    let val = vec[*idx].value;
    val >= min && val <= max
}) {
    process(&vec[i]);
}
```

Here, `min` and `max` are Copy values captured by the closure. `vec` is accessed from scope without capture.

### Adapter Chaining

**Chaining is lazy composition:**

```
for i in vec.indices()
    .filter(|i| vec[*i].active)
    .filter(|i| vec[*i].score > 10)
    .take(5)
{
    results.push(vec[i].clone());
}
```

**Desugaring:** Each adapter wraps the previous iterator. Evaluation happens lazily during iteration.

```
// Conceptual desugaring (simplified):
{
    let _len = vec.len();
    let _pos = 0;
    let _taken = 0;
    while _pos < _len && _taken < 5 {
        let i = _pos;
        if vec[i].active {  // First filter
            if vec[i].score > 10 {  // Second filter
                // Body executes
                results.push(vec[i].clone());
                _taken += 1;
            }
        }
        _pos += 1;
    }
}
```

**Important:** Adapters do NOT create intermediate collections. They compose filtering logic that evaluates during iteration.

### Adapter Storage Rules

| Pattern | Legal | Reason |
|---------|-------|--------|
| `for i in vec.filter(\|i\| ...)` | ✅ Yes | Inline consumption |
| `let iter = vec.indices(); for i in iter { ... }` | ✅ Yes | Iterator has no closures yet |
| `let filtered = vec.filter(\|i\| vec[*i].x); ...` | ❌ No | Closure accesses scope |
| `let filtered = range.filter(\|i\| *i > 10); ...` | ✅ Yes | Closure doesn't access scope |

**General rule:** Iterator adapter chains CAN be stored in variables UNLESS a closure accesses outer scope variables. Compiler enforces this based on closure capture analysis.

### Iterator Methods Summary

**Core iteration protocol:**
```
trait Iterator {
    type Item;
    fn next(mutate self) -> Option<Item>;
}
```

**Adapter methods:**
```
fn filter(self, f: |&Item| -> bool) -> impl Iterator<Item>
fn take(self, n: usize) -> impl Iterator<Item>
fn skip(self, n: usize) -> impl Iterator<Item>
fn rev(self) -> impl Iterator<Item> where Item: DoubleEndedIterator
fn map<R>(self, f: |Item| -> R) -> impl Iterator<R>
```

**Consumer methods:**
```
fn collect<C: FromIterator>(self) -> C
fn count(self) -> usize
fn any(self, f: |Item| -> bool) -> bool
fn all(self, f: |Item| -> bool) -> bool
```

### Generalized For-Loop Desugaring

```
// Written:
for item in expr { body }

// Desugars to:
{
    let mut _iter = expr.into_iter();
    while let Some(item) = _iter.next() {
        body
    }
}
```

**Type requirements:**
- `expr` must have type implementing `IntoIterator`
- Collections (`Vec`, `Pool`, `Map`) implement `IntoIterator`
- Ranges implement `Iterator` directly
- Adapters return types implementing `Iterator`

### Edge Cases

| Case | Handling |
|------|----------|
| Closure panics in filter | Iteration stops, panic propagates |
| Closure accesses invalidated index | Runtime panic (bounds check) |
| `take(0)` | Loop body never executes |
| `filter` with no matches | Loop body never executes |
| Nested closures | Inner closure can access same scope as outer |
| Closure with early return | ❌ ERROR: return would escape closure scope |
| Closure with `?` | ✅ OK: propagates within closure, returns `Result` |

### Closure Restrictions

**In iterator adapter closures, you CANNOT:**
- `return` — would escape closure scope
- Store closure in variable when it accesses scope
- Capture mutable reference — conflicts with expression-scoped borrows

**You CAN:**
- Access variables from outer scope
- Use `?` to propagate errors (closure returns `Result`)
- Call functions that borrow parameters
- Copy/clone values from scope
- Access collection being iterated using expression-scoped borrows

## Self-Validation

### Does it conflict with CORE design?
**NO.**
- ✅ "No storable references" preserved — closures access scope transiently, don't capture references
- ✅ "Local analysis" works — compiler checks closure doesn't escape at call site
- ✅ "Expression-scoped borrows" extended naturally to closure bodies
- ✅ "Transparent costs" maintained — lazy evaluation, no hidden allocations

### Is it internally consistent?
**YES.**
- Closure scope access vs. capture distinction is clear ✅
- Lazy adapter semantics don't require stored references ✅
- For-loop desugaring works uniformly ✅
- Restrictions (no storing scope-accessing closures) are enforceable ✅

### Does it conflict with other specs?
**NO.**
- `memory-model.md`: Closure capture rules ✅ (extended with scope access mode)
- `dynamic-data-structures.md`: Collection access via closures ✅ (`.read()`, `.modify()` use same pattern)
- `ensure-cleanup.md`: No conflict ✅

**Note:** This extends the closure semantics in `memory-model.md` with a second mode (scope access vs. capture). This is an elaboration, not a contradiction.

### Is it complete enough to implement?
**YES.**
- Iterator trait specified ✅
- Adapter method signatures specified ✅
- Desugaring rules provided ✅
- Closure scope access rules clear ✅
- Storage restrictions defined ✅
- Edge cases covered ✅

### Is it concise?
**YES.** ~200 lines, focused on implementable semantics, tables for edge cases.

## Summary

**Key insight:** Iterator adapters use **expression-scoped closures** that access variables from outer scope without capturing them. Compiler enforces that such closures cannot be stored.

**Mechanism:**
1. Closures in adapters are called immediately during iteration
2. They can access outer scope (including the collection being iterated)
3. They cannot be stored in variables (would violate expression-scoped borrows)
4. Adapters compose lazily without intermediate allocations

**Rationale:** This gives ergonomic filtering/mapping while maintaining "no storable references" and "local analysis" constraints. The restriction (can't store scope-accessing closures) is natural and enforceable locally.

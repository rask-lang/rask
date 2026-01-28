# Gap 3: Pool Iteration Semantics

**Type:** Specification Conflict
**Priority:** HIGH

## The Question
What does `for h in pool` yield, and how does it differ from `for (h, item) in &pool`?

## Conflicting Specifications

**`iterators-and-loops.md` (lines 149-154):**
```
for h in pool {
    pool.remove(h)?.close()?;
}
```
Implies `h` is `Handle<T>`.

**`dynamic-data-structures.md` (lines 187-189):**
```
for (handle, item) in &pool { }  // handle: Handle<T>, item: &T
pool.handles() -> Iterator<Handle<T>>
```
Implies two different iteration modes exist.

## Unclear Points
- Is `for h in pool` syntax sugar for `for h in pool.handles()`?
- Or does `for h in pool` do something different than `for (h, item) in &pool`?
- What are ALL the iteration modes for `Pool<T>`?
- When would you use each mode?
- Do all collections (Vec, Map) have similar modes?

## Why This Matters
Pool iteration is common in game engines, entity systems, and graph algorithms. The ergonomics must be clear, and the modes must be well-justified.

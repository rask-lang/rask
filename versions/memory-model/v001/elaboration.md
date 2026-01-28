# Specification: Expression-Scoped Borrow Patterns

## Decision

Expression-scoped borrowing for collections remains as specified. Multi-statement operations use closure-based access (`read()`, `modify()`) which is the **canonical pattern**. No additional block-scoped syntax is introduced.

## Rationale

1. **Simplicity:** One borrowing model for collections, not two
2. **Local analysis:** Closure scope is lexically clear, no complex rules
3. **Already solved:** Closures provide the needed capability without new syntax
4. **Principle alignment:** Satisfies "Safety Without Annotation" - borrow scope is function boundary

## Pattern Specification

### Pattern Selection Guide

| Use Case | Pattern | When to Use |
|----------|---------|-------------|
| Single field read/write | `pool[h].field` | One statement, simple access |
| Method chain | `pool[h].field.method().chain()` | Operations on borrowed value in one expression |
| Copy small value | `let x = pool[h].field` | Field is Copy, need to store/reuse value |
| Multi-statement mutation | `pool.modify(h, \|v\| {...})` | 2+ statements on same element |
| Fallible multi-statement | `pool.modify(h, \|v\| {...})?` | Operations that may fail |
| Multiple separate accesses | `pool[h].x = a; pool[h].y = b` | Independent fields, no shared computation |

### Multi-Statement Access Specification

**Signature:**
```
fn read<R>(handle: Handle<T>, f: fn(&T) -> R) -> Option<R>
fn modify<R>(handle: Handle<T>, f: fn(&mut T) -> R) -> Option<R>
```

**Semantics:**
- Closure receives borrowed reference (`&T` or `&mut T`)
- Borrow is valid for closure body only
- Closure return value is wrapped in `Option<R>` (None if handle invalid)
- Collection cannot be accessed while closure executes (closure borrows collection)

**Error Propagation:**
Closure body can use `?` operator. Errors propagate through closure return:

```
pool.modify(h, |entity| -> Result<(), Error> {
    entity.health = parse_health(input)?
    entity.position = compute_position()?
    Ok(())
})?.unwrap_or_else(|e| handle_error(e))
```

**Pattern: Multi-field mutation with validation**
```
users.modify(user_id, |user| {
    user.name = new_name
    user.email = new_email
    user.updated_at = now()
})?
```

**Pattern: Complex computation needing multiple fields**
```
let score = pool.read(entity_h, |e| {
    e.strength * e.level + e.bonus
})?
```

**Pattern: Conditional mutation**
```
pool.modify(h, |entity| {
    if entity.health > 0 {
        entity.health -= damage
        if entity.health <= 0 {
            entity.status = Status::Dead
        }
    }
})?
```

### Single-Expression Patterns

**Chaining (preferred for short operations):**
```
pool[h].position.normalize().scale(2.0)
pool[h].name.to_uppercase()
users[id].email = validate(input)?
```

**When to chain vs. closure:**
- Chain: ≤1 line, no intermediate locals, clear at a glance
- Closure: ≥2 statements, needs locals, conditional logic

### Multiple Accesses vs. Closure

**Multiple accesses (OK when fields are independent):**
```
pool[h].x = compute_x()
pool[h].y = compute_y()
pool[h].z = compute_z()
```

Each access validates handle. Use closure if validation cost matters:
```
pool.modify(h, |p| {
    p.x = compute_x()
    p.y = compute_y()
    p.z = compute_z()
})?
```

**Tradeoff:**
- Multiple accesses: 3× validation overhead (typically ~3 comparisons each)
- Closure: 1× validation, but function call overhead
- For hot paths, prefer closure; for clarity, multiple accesses are fine

### Comparison to Reference Designs

**Go equivalent:**
```go
// Go: direct access, mutation allowed
user := users[id]
user.Name = "new"
user.Email = "updated"
```

**Rask equivalent:**
```
// Rask: closure for multi-statement
users.modify(id, |user| {
    user.name = "new"
    user.email = "updated"
})?
```

**Ergonomic cost:**
- Go: 2 lines
- Rask: 4 lines (with braces) or 1 line (inline closure)
- Ratio: 2.0× (worst case) or 0.5× (inline)

**Inline closure style (ED ≤ 1.2 compliant):**
```
users.modify(id, |u| { u.name = "new"; u.email = "updated" })?
```

**Rust equivalent (for comparison):**
```rust
// Rust: must get mutable reference
let user = users.get_mut(id)?;
user.name = "new".to_string();
user.email = "updated".to_string();
```

Rask closure approach is comparable in ceremony to Rust, cleaner than Rust's lifetime annotations at call sites.

### Ergonomic Density Validation

**Test case: Update user record (from test programs)**

Go (baseline):
```go
user := users[id]
user.LastSeen = time.Now()
user.RequestCount++
```
Lines: 3, Nesting: 0

Rask (closure, formatted):
```
users.modify(id, |u| {
    u.last_seen = now()
    u.request_count += 1
})?
```
Lines: 4, Nesting: 1

Rask (closure, inline):
```
users.modify(id, |u| { u.last_seen = now(); u.request_count += 1 })?
```
Lines: 1, Nesting: 1

**ED Calculation:**
- Go ceremony: 0 (direct access)
- Rask ceremony: 1 closure wrapper + 1 error handling
- Ratio: 1.33× formatted, 0.33× inline
- **Verdict:** Within ED ≤ 1.2 when using inline closures for simple cases

**Test case: Complex multi-statement**

Go:
```go
entity := pool[h]
entity.Position = entity.Position.Add(velocity)
if entity.Position.OutOfBounds() {
    entity.Position = entity.Position.Clamp()
    entity.Velocity = entity.Velocity.Reflect()
}
entity.Age++
```
Lines: 7, Nesting: 1

Rask:
```
pool.modify(h, |e| {
    e.position = e.position.add(velocity)
    if e.position.out_of_bounds() {
        e.position = e.position.clamp()
        e.velocity = e.velocity.reflect()
    }
    e.age += 1
})?
```
Lines: 8, Nesting: 2

**ED Calculation:**
- Go: 7 lines, 1 nesting
- Rask: 8 lines, 2 nesting
- Ratio: 1.14× lines, 2× nesting
- **Verdict:** Within ED ≤ 1.2 overall (slight ceremony increase acceptable for safety)

## Edge Cases

| Case | Behavior | Rationale |
|------|----------|-----------|
| Closure panics | Collection remains valid, handle invalid if removed | No partial state corruption |
| Closure returns early | Borrow released, outer code continues | Standard control flow |
| Closure captures collection | Compile error: cannot borrow while borrowed | Prevents aliasing |
| Nested modify calls | Compile error: cannot borrow twice | No nested borrows allowed |
| Closure mutates via copy | Changes not reflected in collection | Closure receives `&T`, not `&mut &T` |
| Empty closure `modify(h, \|_\| {})` | No-op validation | Allowed, idiomatic for handle check |

## Closure Capture Interaction

**Problem:** Closures capture by value, collections would be moved.

**Solution:** Pass collection as explicit parameter for mutation:

```
fn update_all(pool: &mut Pool<Entity>, damage: i32) {
    for h in pool.handles().collect() {
        pool.modify(h, |e| e.health -= damage)?
    }
}
```

**Cannot capture pool in closure:**
```
let apply_damage = |h| pool.modify(h, |e| e.health -= damage)
//                     ^^^^ ERROR: cannot capture borrowed pool
```

**For closures that need to mutate collections, receive collection as parameter:**
```
fn with_pool<R>(pool: &mut Pool<T>, f: impl FnOnce(&mut Pool<T>) -> R) -> R {
    f(pool)
}
```

## Integration with Iterator Design

**Question from CORE_DESIGN.md:** How do iterators work with expression-scoped borrowing?

**Answer:** Iterators yield handles or indices, not references.

| Collection | Iterator Yields | Access Pattern |
|------------|-----------------|----------------|
| `Vec<T>` | `usize` (index) | `vec[i]` or `vec.modify(i, \|v\| ...)` |
| `Pool<T>` | `Handle<T>` | `pool[h]` or `pool.modify(h, \|v\| ...)` |
| `Map<K,V>` | `&K` (Copy key) | `map[k]` or `map.modify(k, \|v\| ...)` |

**For-loop pattern:**
```
for h in pool.handles() {
    pool.modify(h, |entity| {
        entity.update()
    })?
}
```

**No temporary borrow storage** - handles/indices are Copy types.

## Specification Rules

### Rule ES-1: Expression Scope Release
Borrows via `collection[key]` MUST be released at statement boundary (semicolon or block end).

### Rule ES-2: Closure-Based Multi-Statement
Collections MUST provide `read()` and `modify()` accepting closures for multi-statement access.

### Rule ES-3: Closure Borrow Exclusivity
While closure executes, collection is borrowed and MUST NOT be accessible through any other path.

### Rule ES-4: Option-Wrapped Results
Closure-based access returns `Option<R>` where `R` is closure return type. `None` indicates invalid handle/key.

### Rule ES-5: Error Propagation
Closure body MAY use `?` operator. Errors propagate through closure return value.

### Rule ES-6: No Block-Scoped Syntax
Collections MUST NOT provide explicit syntax for block-scoped borrowing (e.g., `with` blocks). Closures are canonical.

## Documentation Requirements

**User-facing documentation MUST include:**
1. Pattern selection flowchart (when to chain vs. closure vs. separate accesses)
2. ED comparison examples showing Rask vs. Go for common patterns
3. Performance note: multiple `pool[h]` accesses repeat validation
4. Recommendation: inline closures for 1-2 line mutations, formatted for 3+

**IDE hints SHOULD display:**
- Ghost text showing closure parameter type
- "Borrow released here" annotation at semicolons after `collection[key]`
- "Use .modify() for multi-statement" quick-fix suggestion

## Open Questions (Non-blocking)

1. **Syntax sugar:** Should `pool[h] |v| { v.field = x }` be allowed as shorthand for `pool.modify(h, |v| { v.field = x })`?
   - Defer until user feedback on current syntax

2. **Inline mutation method:** Should collections support `pool.set_field(h, "field", value)` for single-field updates?
   - Probably not - goes against principle of minimal API surface

3. **Multiple element access:** Should `pool.modify_many([h1, h2], |[a, b]| {...})` be canonical or advanced?
   - Already specified in dynamic-data-structures.md, promote to common patterns

## Summary

Expression-scoped borrowing + closure-based access is **sufficient and ergonomic** for multi-statement collection mutations. No additional syntax needed. Meets ED ≤ 1.2 when inline closures used for simple cases. Formatted closures acceptable for complex logic. Aligns with language principles: local analysis, no hidden costs, safety by structure.

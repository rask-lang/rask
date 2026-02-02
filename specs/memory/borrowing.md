# Solution: Borrowing

## The Question
How do temporary references work? When can code read or mutate data without taking ownership?

## Decision
One borrowing principle: **views last as long as the source is stable.** Sources that can grow or shrink (Vec, Pool, Map) release views instantly. Sources that are fixed (strings, struct fields) allow views to persist until block end.

## Rationale
This design prevents "borrow checker wrestling"—the frustrating experience of writing code that looks fine, then hitting a confusing conflict 20 lines later.

For collections, views are instant: you use them inline or copy values out. There's no "wrong path" to walk down—the pattern is always clear. For fixed sources like strings, views persist naturally because the source can't invalidate them.

The result: one mental model, predictable behavior, no wrestling.

## Mental Model: One Rule

**Can the source grow or shrink?**

| Answer | What happens | Why |
|--------|--------------|-----|
| **Yes** (Vec, Pool, Map) | View is instant — released at semicolon | Growing/shrinking could invalidate the view |
| **No** (String, struct fields) | View persists — valid until block ends | Source can't change, so view stays valid |

That's the entire model. One question, one rule.

### Why This Prevents Wrestling

Collections release views instantly, which means you'll never write code like this:

<!-- test: skip -->
```rask
const entity = pool[h]        // ❌ ERROR: can't hold view from growable source
```

The error is immediate. The fix is obvious: use inline or copy out. No "but I stopped using it!" confusion.

### Quick Test

> **Can it grow?**
> - Vec, Pool, Map → Yes → View is instant
> - String, struct field, array → No → View persists

### Why Collections Have Instant Views

Collections can change structurally at any time:
- `Vec` may reallocate when capacity is exceeded (all element addresses change)
- `Pool` may compact or remove elements (handle becomes stale)
- `Map` may rehash on insert (all bucket positions change)

A persistent view would become dangling if the collection changes. Instant views eliminate this entire class of bugs—and eliminate the wrestling that comes with debugging them.

### Why Strings Have Persistent Views

A string's structure is fixed once created:
- Characters cannot be inserted/removed without creating a new string
- The backing memory cannot relocate during a view
- Slicing creates a view into existing memory

Since the source can't change, the view stays valid until the block ends. This enables ergonomic multi-statement string parsing without copying.

### The Pattern for Collections

Since collection views are instant, multi-statement access uses one of two patterns:

```rask
// Pattern 1: Copy out the value
const health = pool[h].health    // Value copied, view released
if health <= 0 { ... }

// Pattern 2: Closure for multi-statement mutation
pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
})
```

Both patterns are clear and predictable. No wrestling.

## Specification

### Persistent Views (Strings, Struct Fields)

Views into fixed sources persist until block end. These are sometimes called "stable borrows" in error messages.

| Rule | Description |
|------|-------------|
| **S1: Block duration** | Stable borrow valid from creation until end of enclosing block |
| **S2: Source outlives borrow** | Source must be valid for borrow's entire duration |
| **S3: No escape** | Cannot store in struct, return, or send cross-task |
| **S4: Duration extension** | Borrowing a temporary extends its duration to match borrow |
| **S5: Aliasing XOR mutation** | Source cannot be mutated while borrowed; mutable borrow excludes all other access |

**Basic usage:**
```rask
const line = read_line()
const key = line[0..eq]        // Borrow, valid until block ends
const value = line[eq+1..]     // Another borrow
validate(key)                // ✅ OK: key still valid
process(key, value)          // ✅ OK: both valid
```

**Lifetime extension (B4):**
```rask
const slice = get_string()[0..n]  // ✅ OK: temporary extended

// Equivalent to:
const _temp = get_string()
const slice = _temp[0..n]
// _temp lives as long as slice
```

**Chained temporaries:**

When multiple temporaries are created in a chain, ALL are extended:

```rask
const slice = get_container().get_inner()[0..n]

// Equivalent to:
const _temp1 = get_container()    // Container extended
const _temp2 = _temp1.get_inner() // Inner extended
const slice = _temp2[0..n]
// Both temporaries live as long as slice
```

**Method chains with intermediate allocations:**

```rask
const slice = get_string().to_uppercase().trim()[0..n]

// Equivalent to:
const _temp1 = get_string()           // Original string
const _temp2 = _temp1.to_uppercase()  // New allocation
const _temp3 = _temp2.trim()          // View into _temp2
const slice = _temp3[0..n]            // View into _temp3
// All temporaries extended
```

**The rule:** Every temporary in the chain that the borrow transitively depends on is extended. The compiler traces the dependency path and extends all values in that path.

**What is NOT extended:**

```rask
const slice = {
    const s = get_string()
    s[0..n]  // ❌ ERROR: s dies at block end
}
// slice would outlive s
```

Temporaries in inner blocks are NOT extended to outer blocks. Extension only works for temporaries created in the same statement as the borrow.

**Mutation blocked (B5):**
```rask
const s = String.new()
const slice = s[0..3]      // Read borrow active
s.push('!')              // ❌ ERROR: cannot mutate while borrowed
process(slice)
// Block ends, borrow released
s.push('!')              // ✅ OK: no active borrow
```

**Nested blocks:**
```rask
const s = "hello"
{
    const slice = s[0..3]
    {
        process(slice)   // ✅ OK: slice in scope
    }
}  // slice ends here
```

**Cannot extend to outer scope:**
```rask
let outer: ???
{
    const s = "hello"
    outer = s[0..3]      // ❌ ERROR: s dies before outer's scope
}
```

### Instant Views (Collections)

Views into growable sources (Pool, Vec, Map) are released at the semicolon. These are sometimes called "volatile access" in error messages.

| Rule | Description |
|------|-------------|
| **V1: Expression duration** | Access valid only within the expression |
| **V2: Released at semicolon** | Access ends when statement completes |
| **V3: Chain calls OK** | `pool[h].field.method()` is one expression |
| **V4: Same aliasing rules** | Aliasing XOR mutation still applies within expression |

**Why instant views prevent wrestling:**

If collection views persisted, you'd hit confusing errors:
```rask
// ❌ With persistent views, this would fail:
const entity = pool[h]         // View starts
entity.health -= damage
if entity.health <= 0 {
    pool.remove(h)           // ERROR: can't mutate collection while viewed
}
// "But I'm done using entity!" → Wrestling begins
```

Instant views make the pattern obvious:
```rask
// ✅ Instant views - clear pattern:
pool[h].health -= damage     // View released at semicolon
if pool[h].health <= 0 {     // New view
    pool.remove(h)           // No active view - OK
}
```

**Naming collection data:**

Use handles (which persist) and copy values out when needed:
```rask
const h = pool.find(pred)?     // Handle persists
const health = pool[h].health  // Copy out value
if health <= 0 {
    pool.remove(h)           // OK
}
```

### Multi-Statement Collection Access

**Problem:** Volatile access prevents multi-statement operations on collection elements.

**Solution:** Closure-based access via `read()` and `modify()` methods (canonical pattern).

| Method | Signature | Use Case |
|--------|-----------|----------|
| `read(key, f)` | `func(T) -> R → Option<R>` | Multi-statement read access |
| `modify(key, f)` | `func(T) -> R → Option<R>` | Multi-statement mutation |

**Basic usage:**
```rask
pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
})?
```

**Error propagation:**
```rask
users.modify(id, |user| -> Result<(), Error> {
    user.email = validate_email(input)?
    user.updated_at = now()
    Ok(())
})?
```

**Pattern selection:**

| Lines of code | Pattern | Example |
|---------------|---------|---------|
| 1 statement | Direct `collection[key]` | `pool[h].field = value` |
| Method chain | Direct `collection[key]` | `pool[h].pos.normalize().scale(2)` |
| 2+ statements | Closure `modify()` | See above |
| Needs `?` inside | Closure with Result | See above |

**Closure borrows collection exclusively:**
```rask
pool.modify(h, |e| {
    e.health -= 10
    pool.remove(other_h)     // ❌ ERROR: pool borrowed by closure
})
```

**For iteration + mutation, collect handles first:**
```rask
const handles = pool.handles().collect()
for h in handles {
    pool.modify(h, |e| e.update())?
}
```

### Aliasing Rules

Both borrowing modes enforce the same aliasing rules:

| Rule | Read borrow | Mutable borrow |
|------|-------------|----------------|
| Other reads | ✅ Allowed | ❌ Forbidden |
| Mutations | ❌ Forbidden | ❌ Forbidden |
| Number allowed | Unlimited | Exactly one |

**Aliasing XOR Mutation:**
- Multiple immutable borrows: OK
- One mutable borrow: OK
- Mixed (any mutable + any other): ERROR

### Borrow Checking

The compiler performs local borrow analysis:

| Check | When | Error |
|-------|------|-------|
| Lifetime validity | At borrow creation | "source doesn't live long enough" |
| Aliasing violation | At conflicting access | "cannot mutate while borrowed" |
| Escape attempt | At assignment/return | "borrow cannot escape scope" |

All checks are performed **locally** within the function. No cross-function analysis required.

### Error Messages

Error messages teach the "can it grow?" principle and provide clear fixes.

**Trying to hold a view from a growable source:**
```
ERROR: cannot hold view from growable source
   |
5  |  let entity = pool[h]
   |               ^^^^^^^ Pool can grow/shrink - view must be instant
6  |  entity.update()
   |  ^^^^^^ view already released

WHY: Pool, Vec, and Map can grow or shrink, which would invalidate
     any persistent view. Views are released at the semicolon.

FIX: Copy the value out, or use a closure:

  // Option 1: Copy out the fields you need
  const health = pool[h].health
  if health <= 0 { pool.remove(h) }

  // Option 2: Closure for multi-statement mutation
  pool.modify(h, |entity| {
      entity.health -= damage
      entity.last_hit = now()
  })
```

**Mutation during persistent view:**
```
ERROR: cannot mutate source while viewed
   |
3  |  let slice = line[0..5]
   |              ^^^^^^^^^ view created here
4  |  line.push('!')
   |  ^^^^^^^^^^^^^ cannot mutate - would invalidate view
5  |  process(slice)
   |          ^^^^^ view still active

WHY: Mutating a string might reallocate, invalidating the view.

FIX: Finish using the view first, or copy:

  // Finish using view first
  const slice = line[0..5]
  process(slice)
  line.push('!')  // OK - view ended

  // Or work with a copy
  const copy = line[0..5].to_string()
  line.push('!')  // OK - copy is independent
  process(copy)
```

**Mutation during closure:**
```
ERROR: cannot mutate collection inside its own closure
   |
5  |  pool.modify(h, |entity| {
   |  ---- pool borrowed here
6  |      entity.health -= 10
7  |      pool.remove(other)
   |      ^^^^^^^^^^^^^^^^^ cannot mutate pool here

FIX: Collect handles first, then mutate:

  const to_remove = pool.handles().filter(...).collect()
  for h in to_remove {
      pool.remove(h)
  }
```

**Error message principles:**
- Lead with "growable" vs "fixed" language
- Explain WHY (growth could invalidate)
- Show concrete fixes, not abstract advice

## Edge Cases

| Case | Handling |
|------|----------|
| Borrow from temporary | Temporary duration extended to match borrow |
| Chained temporaries | ALL temporaries in chain extended |
| Temporary in inner block | NOT extended to outer block |
| Nested borrows | Inner borrow must not outlive outer |
| Borrow across match arms | All arms see same borrow mode |
| Clone of borrowed | Allowed (creates independent copy) |
| Borrow of clone | Borrows the new copy, not original |
| Volatile access in method chain | Access spans entire chain |
| Mixed stable/volatile | Each follows its source's rules |

## Examples

### String Parsing (Stable Borrow)
<!-- test: parse -->
```rask
func parse_header(line: string) -> Option<(string, string)> {
    const colon = line.find(':')?
    const key = line[0..colon].trim()      // Stable borrow
    const value = line[colon+1..].trim()   // Another stable borrow
    Some((key.to_string(), value.to_string()))
}
```

### Entity Update (Volatile Access)
<!-- test: parse -->
```rask
func update_combat(pool: Pool<Entity>) {
    let targets: Vec<Handle<Entity>> = find_targets(pool)

    for h in targets {
        pool[h].health -= 10             // Volatile access
        if pool[h].health <= 0 {         // New volatile access
            pool.remove(h)               // No active borrow - OK
        }
    }
}
```

### Multi-Statement Mutation
<!-- test: parse -->
```rask
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> Result<(), Error> {
    pool.modify(h, |entity| {
        entity.strength += 10
        entity.defense += 5
        entity.buff_expiry = now() + Duration.seconds(30)
        log_buff_applied(entity.id)?
        Ok(())
    })?
}
```

## Quick Reference

| Aspect | Fixed Sources | Growable Sources |
|--------|---------------|------------------|
| Types | String, struct fields, arrays | Pool, Vec, Map |
| View duration | Until block ends | Until semicolon |
| Can store in `let`? | Yes | No (use inline or copy out) |
| Multi-statement use? | Direct | Closure (`read`/`modify`) or copy out |
| The test | Can't grow or shrink | Can grow or shrink |

## IDE Integration

The IDE makes view durations visible through ghost annotations.

### Ghost Annotations

| Context | Annotation |
|---------|------------|
| Persistent view | `[view: until line N]` |
| Instant view | `[instant: released at ;]` |
| Conflict site | `[conflict: viewed on line N]` |

**Example: Instant view (collection)**
```rask
const health = pool[h].health  // [instant: released at ;]
if health <= 0 {             // view already released
    pool.remove(h)           // OK - no conflict
}
```

**Example: Persistent view (string)**
```rask
const key = line[0..eq]        // [view: until line 8]
const value = line[eq+1..]     // [view: until line 8]
validate(key)                // [uses view from line 3]
process(key, value)          // [uses views from lines 3-4]
}                            // line 8: views released
```

### Hover Information

When hovering over a collection access:

```rask
pool[h].health
^^^^^^ Instant view from Pool<Entity>

Pool can grow/shrink, so this view is released at the semicolon.
For multi-statement access:
  • Copy:    let x = pool[h].health
  • Closure: pool.modify(h, |e| { ... })
```

When hovering over a string slice:

```rask
const key = line[0..eq]
    ^^^ Persistent view from String

String can't grow/shrink, so this view is valid until block end (line 15).
The source cannot be mutated while this view exists.
```

### Conflict Highlighting

When a borrow conflict would occur, the IDE highlights both the borrow source and the conflict site:

```rask
pool.modify(h, |entity| {    // [mutable borrow of pool]
    entity.health -= 10
    pool.remove(other)       // [conflict: pool borrowed on line 1]
                             //  ^^^^^^^^^^ highlighted in red
})
```

## Integration Notes

- **Value Semantics:** Borrowing is an alternative to copy/move (see [value-semantics.md](value-semantics.md))
- **Ownership:** Borrows temporarily suspend exclusive ownership (see [ownership.md](ownership.md))
- **Collections:** Full collection API in [collections.md](../stdlib/collections.md)
- **Pools:** Handle-based access in [pools.md](pools.md)

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior
- [Ownership Rules](ownership.md) — Single-owner model
- [Pools](pools.md) — Handle-based indirection
- [Collections](../stdlib/collections.md) — Vec, Map APIs

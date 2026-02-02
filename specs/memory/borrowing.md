# Solution: Borrowing

## The Question
How do temporary references work? When can code read or mutate data without taking ownership?

## Decision
Two borrowing modes: **stable borrowing** for plain values (strings, struct fields) and **volatile access** for collections (Pool, Vec, Map). Both enforce aliasing rules but with different ergonomic tradeoffs.

## Rationale
Stable borrowing enables ergonomic multi-statement operations on strings and struct fields. Volatile access for collections prevents the "borrowed collection can't be mutated" problem that would otherwise block common patterns like conditional removal.

The split is pragmatic: plain values benefit from named borrows that span multiple statements, while collections need to release borrows quickly to allow interleaved mutation.

## Mental Model: Stability Determines Borrowing

Rask has one borrowing principle: **borrows follow stability**.

| Source Category | Structural Stability | Borrowing Behavior |
|-----------------|---------------------|-------------------|
| **Stable** (String, struct fields) | Cannot grow, shrink, or relocate | Stable borrow — valid until block ends |
| **Volatile** (Pool, Vec, Map) | May insert, remove, resize, reallocate | Volatile access — released at semicolon |

**The rule is simple:** If the source might change between statements, the borrow cannot persist.

### Quick Heuristic

> **Can this source grow or shrink?**
> - Yes → Volatile access (released at semicolon)
> - No → Stable borrow (valid until block end)

### Why Collections Are Volatile

Collections can change structurally at any time:
- `Vec` may reallocate when capacity is exceeded (all element addresses change)
- `Pool` may compact or remove elements (handle becomes stale)
- `Map` may rehash on insert (all bucket positions change)

A borrow that persists across statements would become dangling if the collection changes. Therefore, collection accesses are volatile—they complete within the expression and release immediately.

### Why Strings Are Stable

A string's structure is fixed once created:
- Characters cannot be inserted/removed without creating a new string
- The backing memory cannot relocate during a borrow
- Slicing creates a view into existing memory

A stable source permits stable borrows—the reference remains valid until the block ends.

This is why `let key = line[0..n]; validate(key)` works (string can't change), but `let entity = pool[h]; entity.update()` fails (pool might change between statements).

**The solution for multi-statement container access:** Either copy the value out, or use a closure that holds the borrow for a defined scope:
```
// Copy out
let health = pool[h].health    // Value copied, borrow released
if health <= 0 { ... }

// Closure for multi-statement
pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
})
```

## Specification

### Stable Borrowing (Strings, Struct Fields)

Borrows from stable sources (strings, struct fields) persist until block end.

| Rule | Description |
|------|-------------|
| **S1: Block lifetime** | Stable borrow valid from creation until end of enclosing block |
| **S2: Source outlives borrow** | Source must be valid for borrow's entire lifetime |
| **S3: No escape** | Cannot store in struct, return, or send cross-task |
| **S4: Lifetime extension** | Borrowing a temporary extends its lifetime to match borrow |
| **S5: Aliasing XOR mutation** | Source cannot be mutated while borrowed; mutable borrow excludes all other access |

**Basic usage:**
```
let line = read_line()
let key = line[0..eq]        // Borrow, valid until block ends
let value = line[eq+1..]     // Another borrow
validate(key)                // ✅ OK: key still valid
process(key, value)          // ✅ OK: both valid
```

**Lifetime extension (B4):**
```
let slice = get_string()[0..n]  // ✅ OK: temporary extended

// Equivalent to:
let _temp = get_string()
let slice = _temp[0..n]
// _temp lives as long as slice
```

**Chained temporaries:**

When multiple temporaries are created in a chain, ALL are extended:

```
let slice = get_container().get_inner()[0..n]

// Equivalent to:
let _temp1 = get_container()    // Container extended
let _temp2 = _temp1.get_inner() // Inner extended
let slice = _temp2[0..n]
// Both temporaries live as long as slice
```

**Method chains with intermediate allocations:**

```
let slice = get_string().to_uppercase().trim()[0..n]

// Equivalent to:
let _temp1 = get_string()           // Original string
let _temp2 = _temp1.to_uppercase()  // New allocation
let _temp3 = _temp2.trim()          // View into _temp2
let slice = _temp3[0..n]            // View into _temp3
// All temporaries extended
```

**The rule:** Every temporary in the chain that the borrow transitively depends on is extended. The compiler traces the dependency path and extends all values in that path.

**What is NOT extended:**

```
let slice = {
    let s = get_string()
    s[0..n]  // ❌ ERROR: s dies at block end
}
// slice would outlive s
```

Temporaries in inner blocks are NOT extended to outer blocks. Extension only works for temporaries created in the same statement as the borrow.

**Mutation blocked (B5):**
```
let s = String::new()
let slice = s[0..3]      // Read borrow active
s.push('!')              // ❌ ERROR: cannot mutate while borrowed
process(slice)
// Block ends, borrow released
s.push('!')              // ✅ OK: no active borrow
```

**Nested blocks:**
```
let s = "hello"
{
    let slice = s[0..3]
    {
        process(slice)   // ✅ OK: slice in scope
    }
}  // slice ends here
```

**Cannot extend to outer scope:**
```
let outer: ???
{
    let s = "hello"
    outer = s[0..3]      // ❌ ERROR: s dies before outer's scope
}
```

### Volatile Access (Collections)

Access to collections (Pool, Vec, Map) is volatile—released at the semicolon.

| Rule | Description |
|------|-------------|
| **V1: Expression lifetime** | Access valid only within the expression |
| **V2: Released at semicolon** | Access ends when statement completes |
| **V3: Chain calls OK** | `pool[h].field.method()` is one expression |
| **V4: Same aliasing rules** | Aliasing XOR mutation still applies within expression |

**Why volatile for collections?**

Stable borrowing would prevent mutation:
```
// ❌ If stable, this would fail:
let entity = pool[h]         // Borrow starts
entity.health -= damage
if entity.health <= 0 {
    pool.remove(h)           // ERROR: can't mutate collection while borrowed
}
```

Volatile access allows:
```
// ✅ Volatile access works:
pool[h].health -= damage     // Access released at semicolon
if pool[h].health <= 0 {     // New access
    pool.remove(h)           // No active borrow - OK
}
```

**Naming collection data:**

Use handles (which persist) and copy values out when needed:
```
let h = pool.find(pred)?     // Handle persists
let health = pool[h].health  // Copy out value
if health <= 0 {
    pool.remove(h)           // OK
}
```

### Multi-Statement Collection Access

**Problem:** Volatile access prevents multi-statement operations on collection elements.

**Solution:** Closure-based access via `read()` and `modify()` methods (canonical pattern).

| Method | Signature | Use Case |
|--------|-----------|----------|
| `read(key, f)` | `fn(&T) -> R → Option<R>` | Multi-statement read access |
| `modify(key, f)` | `fn(&mut T) -> R → Option<R>` | Multi-statement mutation |

**Basic usage:**
```
pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status::Dead
    }
})?
```

**Error propagation:**
```
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
```
pool.modify(h, |e| {
    e.health -= 10
    pool.remove(other_h)     // ❌ ERROR: pool borrowed by closure
})
```

**For iteration + mutation, collect handles first:**
```
let handles = pool.handles().collect()
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

Error messages should explain **why** the rules differ. The goal is to teach the stability principle through errors.

**Volatile access spanning statements:**
```
ERROR: cannot hold reference from volatile source
   |
5  |  let entity = pool[h]
   |               ^^^^^^^ Pool is volatile - may change between statements
   |                       Access must be used within this expression
6  |  entity.update()
   |  ^^^^^^ reference no longer valid

WHY: Pool, Vec, and Map are volatile because they may:
  - Reallocate when growing (invalidating all references)
  - Remove elements (creating dangling references)
  - Rehash or compact (moving elements in memory)

FIX: Copy the value out, or use a closure for multi-statement access:

  // Option 1: Copy out the fields you need
  let health = pool[h].health
  if health <= 0 { pool.remove(h) }

  // Option 2: Closure for multi-statement mutation
  pool.modify(h, |entity| {
      entity.health -= damage
      entity.last_hit = now()
  })
```

**Mutation during stable borrow:**
```
ERROR: cannot mutate stable source while borrowed
   |
3  |  let slice = line[0..5]
   |              ^^^^^^^^^ stable borrow created here
4  |  line.push('!')
   |  ^^^^^^^^^^^^^ cannot mutate - would invalidate slice
5  |  process(slice)
   |          ^^^^^ borrow still active

WHY: Mutating a string might reallocate or shift contents,
     invalidating the stable borrow.

FIX: Either complete the borrow first, or clone:

  // Complete borrow first
  let slice = line[0..5]
  process(slice)
  line.push('!')  // OK - borrow ended

  // Or work with a clone
  let copy = line[0..5].to_string()
  line.push('!')  // OK - copy is independent
  process(copy)
```

**Mutation during closure borrow:**
```
ERROR: cannot mutate collection while borrowed
   |
5  |  pool.modify(h, |entity| {
   |  ---- mutable borrow of pool starts here
6  |      entity.health -= 10
7  |      pool.remove(other)
   |      ^^^^^^^^^^^^^^^^^ cannot mutate pool inside its own closure

FIX: Collect handles first, then mutate:
   |
5  |  let to_remove = pool.handles().filter(...).collect()
6  |  for h in to_remove {
7  |      pool.remove(h)
8  |  }
```

**Key principles:**
- Explain "volatile" or "stable" to teach the underlying reason
- Show WHY section explaining the structural instability
- Always suggest the idiomatic alternative (closure, copy, collect-first)
- Show concrete code fixes, not abstract advice

## Edge Cases

| Case | Handling |
|------|----------|
| Borrow from temporary | Temporary lifetime extended to match borrow |
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
```
fn parse_header(line: string) -> Option<(string, string)> {
    let colon = line.find(':')?
    let key = line[0..colon].trim()      // Stable borrow
    let value = line[colon+1..].trim()   // Another stable borrow
    Some((key.to_string(), value.to_string()))
}
```

### Entity Update (Volatile Access)
```
fn update_combat(pool: mut Pool<Entity>) {
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
```
fn apply_buff(pool: mut Pool<Entity>, h: Handle<Entity>) -> Result<(), Error> {
    pool.modify(h, |entity| -> Result<(), Error> {
        entity.strength += 10
        entity.defense += 5
        entity.buff_expiry = now() + Duration::seconds(30)
        log_buff_applied(entity.id)?
        Ok(())
    })?
}
```

## Stable vs Volatile: Quick Reference

| Aspect | Stable Borrow | Volatile Access |
|--------|---------------|-----------------|
| Sources | String, struct fields, arrays | Pool, Vec, Map |
| Duration | Until block ends | Until semicolon |
| Can store in `let`? | Yes | No (must use immediately) |
| Multiple per block? | Yes | Yes (each expression independent) |
| Multi-statement use? | Direct | Closure (`read`/`modify`) or copy out |
| Why? | Source structure is fixed | Source may change |

## IDE Integration

The IDE makes borrow scopes visible through ghost annotations, reducing the cognitive load of the stability-based borrowing model.

### Ghost Annotations

| Context | Annotation |
|---------|------------|
| Stable borrow | `[stable borrow: until line N]` |
| Volatile access | `[volatile: released at ;]` |
| After volatile access | `[access released]` (on hover) |
| Conflict site | `[conflict: borrowed on line N]` |

**Example: Volatile access (collection)**
```
let health = pool[h].health  // [volatile: released at ;]
if health <= 0 {             // pool access released
    pool.remove(h)           // OK - no conflict indicator
}
```

**Example: Stable borrow (string)**
```
let key = line[0..eq]        // [stable borrow: until line 8]
let value = line[eq+1..]     // [stable borrow: until line 8]
validate(key)                // [uses borrow from line 3]
process(key, value)          // [uses borrows from lines 3-4]
}                            // line 8: borrows released
```

### Hover Information

When hovering over a volatile collection access:

```
pool[h].health
^^^^^^ Volatile access from Pool<Entity>

This access is released at the semicolon because Pool
may change between statements. For multi-statement access:
  • Copy:    let x = pool[h].health
  • Closure: pool.modify(h, |e| { ... })
```

When hovering over a stable borrow:

```
let key = line[0..eq]
    ^^^ Stable borrow from String

This borrow is valid until the end of the current block (line 15).
The source string cannot be mutated while this borrow exists.
```

### Conflict Highlighting

When a borrow conflict would occur, the IDE highlights both the borrow source and the conflict site:

```
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

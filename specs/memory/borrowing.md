# Solution: Borrowing

## The Question
How do temporary references work? When can code read or mutate data without taking ownership?

## Decision
Two borrowing modes: **block-scoped** for plain values (strings, struct fields) and **expression-scoped** for collections (Pool, Vec, Map). Both enforce aliasing rules but with different ergonomic tradeoffs.

## Rationale
Block-scoped borrowing enables ergonomic multi-statement operations on strings and struct fields. Expression-scoped borrowing for collections prevents the "borrowed collection can't be mutated" problem that would otherwise block common patterns like conditional removal.

The split is pragmatic: plain values benefit from named borrows that span multiple statements, while collections need to release borrows quickly to allow interleaved mutation.

## Specification

### Block-Scoped Borrowing (Plain Values)

Borrows from plain values (strings, struct fields) are block-scoped.

| Rule | Description |
|------|-------------|
| **B1: Block lifetime** | Borrow valid from creation until end of enclosing block |
| **B2: Source outlives borrow** | Source must be valid for borrow's entire lifetime |
| **B3: No escape** | Cannot store in struct, return, or send cross-task |
| **B4: Lifetime extension** | Borrowing a temporary extends its lifetime to match borrow |
| **B5: Aliasing XOR mutation** | Source cannot be mutated while borrowed; mutable borrow excludes all other access |

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

### Expression-Scoped Borrowing (Collections)

Borrows from collections (Pool, Vec, Map) are expression-scoped.

| Rule | Description |
|------|-------------|
| **E1: Expression lifetime** | Borrow valid only within the expression |
| **E2: Released at semicolon** | Borrow ends when statement completes |
| **E3: Chain calls OK** | `pool[h].field.method()` is one expression |

**Why expression-scoped for collections?**

Block-scoped would prevent mutation:
```
// ❌ If block-scoped, this would fail:
let entity = pool[h]         // Borrow starts
entity.health -= damage
if entity.health <= 0 {
    pool.remove(h)           // ERROR: can't mutate collection while borrowed
}
```

Expression-scoped allows:
```
// ✅ Expression-scoped works:
pool[h].health -= damage     // Borrow released at semicolon
if pool[h].health <= 0 {     // New borrow
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

**Problem:** Expression-scoped borrows prevent multi-statement operations on collection elements.

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
| Expression-scoped in method chain | Borrow spans entire chain |
| Mixed block/expression | Each follows its own rules |

## Examples

### String Parsing (Block-Scoped)
```
fn parse_header(line: string) -> Option<(string, string)> {
    let colon = line.find(':')?
    let key = line[0..colon].trim()      // Block-scoped borrow
    let value = line[colon+1..].trim()   // Another borrow
    Some((key.to_string(), value.to_string()))
}
```

### Entity Update (Expression-Scoped)
```
fn update_combat(pool: mut Pool<Entity>) {
    let targets: Vec<Handle<Entity>> = find_targets(pool)

    for h in targets {
        pool[h].health -= 10             // Expression borrow
        if pool[h].health <= 0 {         // New expression borrow
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

## Integration Notes

- **Value Semantics:** Borrowing is an alternative to copy/move (see [value-semantics.md](value-semantics.md))
- **Ownership:** Borrows temporarily suspend exclusive ownership (see [ownership.md](ownership.md))
- **Collections:** Full collection API in [collections.md](../stdlib/collections.md)
- **Pools:** Handle-based access in [pools.md](pools.md)
- **Tooling:** IDE shows active borrow scopes, highlights conflicts

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior
- [Ownership Rules](ownership.md) — Single-owner model
- [Pools](pools.md) — Handle-based indirection
- [Collections](../stdlib/collections.md) — Vec, Map APIs

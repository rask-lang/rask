# Solution: Memory Model

## The Question
How does Rask achieve memory safety without garbage collection, reference counting overhead, or Rust-style lifetime annotations?

## Decision
Value semantics with single ownership, scoped borrowing (block-scoped for plain values, expression-scoped for collections), and handle-based indirection for graphs/dynamic structures.

## Rationale
The goal is "safety without annotation"—memory safety as a structural property, not extra work. By combining strict ownership with scoped borrowing that cannot escape, we eliminate use-after-free, dangling pointers, and data races without requiring lifetime parameters in signatures.

The split between block-scoped and expression-scoped borrowing is pragmatic: plain values (strings, structs) benefit from ergonomic multi-statement borrows, while collections need expression-scoped access to allow mutation patterns.

## Specification

### Value Semantics

All types are values. There is no distinction between "value types" and "reference types."

| Operation | Small types (≤16 bytes, Copy) | Large types |
|-----------|-------------------------------|-------------|
| Assignment `let y = x` | Copies | Moves (x invalid after) |
| Parameter passing | Copies | Moves (unless `read`/`mutate` mode) |
| Return | Copies | Moves |

**Copy eligibility:**
- Primitives: always Copy
- Structs: Copy if all fields are Copy AND total size ≤16 bytes
- Enums: Copy if all variants are Copy AND total size ≤16 bytes
- Collections (Vec, Pool, Map): never Copy (own heap memory)

### Ownership Rules

| Rule | Description |
|------|-------------|
| **O1: Single owner** | Every value has exactly one owner at any time |
| **O2: Move on assignment** | For non-Copy types, assignment transfers ownership |
| **O3: Invalid after move** | Source binding is invalid after move; use is compile error |
| **O4: Explicit clone** | To keep access while transferring, clone explicitly |

```
let a = Vec::new()
let b = a              // a moved to b
a.push(1)              // ❌ ERROR: a is invalid after move

let c = b.clone()      // Explicit clone - visible allocation
c.push(1)              // ✅ OK: c is independent copy
b.push(2)              // ✅ OK: b still valid
```

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

See [Dynamic Data Structures](dynamic-data-structures.md) for full collection API specification.

### Handle System

Handles provide stable references into collections without borrowing.

**Structure:** `Handle<T>` contains:
- `pool_id: u32` — identifies which pool
- `index: u32` — slot index
- `generation: u32` — validity counter

**Validation on access:**
```
pool[h].field   // Validates: pool_id matches, generation matches, index valid
```

| Check | Failure mode |
|-------|--------------|
| Pool ID mismatch | Panic: "handle from wrong pool" |
| Generation mismatch | Panic: "stale handle" |
| Index out of bounds | Panic: "invalid handle index" |

**Safe access:**
```
pool.get(h)   // Returns Option<T> (T: Copy), no panic
```

**Generation overflow:**

Saturating semantics. When a slot's generation reaches `u32::MAX`:
- Slot becomes permanently unusable (always returns `None`)
- No panic, no runtime check on every removal
- Pool gradually loses capacity (practically never happens: ~4B cycles per slot)

For high-churn scenarios: `Pool<T, u64>` uses 64-bit generations.

### Linear Types

Linear types must be consumed exactly once.

| Rule | Description |
|------|-------------|
| **L1: Must consume** | Linear value must be consumed before scope exit |
| **L2: Consume once** | Cannot consume same linear value twice |
| **L3: Read allowed** | Can borrow for reading without consuming |
| **L4: `ensure` satisfies** | Registering with `ensure` counts as consumption commitment |

**Consuming operations:**
- Calling a method that takes `transfer self`
- Passing to a function with `transfer` parameter
- Explicit consumption function (e.g., `file.close()`)

```
let file = open("data.txt")?   // file is linear
ensure file.close()            // Consumption committed
let data = file.read()?        // ✅ OK: can read after ensure
// Block exits: file.close() runs
```

**Linear + Error handling:**
```
fn process(file: File) -> Result<(), Error> {
    ensure file.close()        // Guarantees consumption
    let data = file.read()?    // Safe to use ? now
    transform(data)?
    Ok(())
}
```

### Closure Capture

Closures capture by value (copy or move), never by reference.

| Capture type | Behavior |
|--------------|----------|
| Small Copy types | Copied into closure |
| Large/non-Copy types | Moved into closure, source invalid |
| Block-scoped borrows | Allowed if closure doesn't escape borrow's scope |
| Expression-scoped borrows | Cannot capture (already released) |

**Capture with borrows:**
```
let slice = s[0..3]
items.filter(|x| x == slice)   // ✅ OK: closure called immediately, doesn't escape

let f = || process(slice)
return f                        // ❌ ERROR: closure escapes borrow's scope
```

**Mutating captured state:**
```
let counter = 0
let increment = || counter += 1  // Captures counter by copy
increment()
increment()
// counter is still 0! Each call mutates the closure's copy.

// For shared mutation, use Pool + Handle:
let pool = Pool::new()
let h = pool.insert(Counter{value: 0})
let increment = || pool[h].value += 1  // Moves pool into closure
```

### Cross-Task Ownership

Tasks are isolated. No shared mutable memory.

| Rule | Description |
|------|-------------|
| **T1: Send transfers** | Sending on channel transfers ownership |
| **T2: No shared mut** | Cannot share mutable references across tasks |
| **T3: Borrows don't cross** | Block-scoped borrows cannot be sent to other tasks |

```
let data = load_data()
channel.send(data)        // Ownership transferred
data.process()            // ❌ ERROR: data was sent

// Receiving:
let received = channel.recv()   // Ownership acquired
received.process()              // ✅ OK: we own it now
```

## Edge Cases

| Case | Handling |
|------|----------|
| Borrow from temporary | Temporary lifetime extended to match borrow |
| Nested borrows | Inner borrow must not outlive outer |
| Borrow across match arms | All arms see same borrow mode |
| Move in one branch | Value invalid in all subsequent code |
| Handle after remove | Generation mismatch → panic on `pool[h]`, None on `pool.get(h)` |
| Linear value in error path | Must be consumed or in `ensure`; compiler tracks |
| Clone of borrowed | Allowed (creates independent copy) |
| Borrow of clone | Borrows the new copy, not original |

## Examples

### String Parsing (Block-Scoped Borrows)
```
fn parse_header(line: string) -> Option<(string, string)> {
    let colon = line.find(':')?
    let key = line[0..colon].trim()      // Block-scoped borrow
    let value = line[colon+1..].trim()   // Another borrow
    Some((key.to_string(), value.to_string()))
}
```

### Entity System (Expression-Scoped + Handles)
```
fn update_combat(pool: mut Pool<Entity>) {
    let targets: Vec<Handle<Entity>> = find_targets(pool)

    for h in targets {
        pool[h].health -= 10             // Expression borrow
        if pool[h].health <= 0 {         // New expression borrow
            pool.remove(h)               // No borrow active - OK
        }
    }
}
```

### File Processing (Linear Types)
```
fn process_file(path: string) -> Result<Data, Error> {
    let file = open(path)?
    ensure file.close()

    let header = file.read_header()?
    if !header.valid {
        return Err(InvalidHeader)        // ensure runs, file closed
    }

    let data = file.read_body()?
    Ok(data)                             // ensure runs, file closed
}
```

## Integration Notes

- **Type System:** Borrow types are compiler-internal; user sees owned types and parameter modes
- **Generics:** Bounds can require Copy, which affects move/copy behavior
- **Pattern Matching:** Match arms share borrow mode; highest mode wins
- **Concurrency:** Channels transfer ownership; no shared-memory primitives in safe code
- **C Interop:** Raw pointers in unsafe blocks; convert to/from safe types at boundaries
- **Tooling:** IDE shows move/copy at each use site, borrow scopes, capture lists

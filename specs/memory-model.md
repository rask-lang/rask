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

#### Why Implicit Copy?

Implicit copy is a fundamental requirement for ergonomic value semantics, not an optional optimization.

**Without implicit copy, primitives would have move semantics:**
```
let x = 5
let y = x              // Without copy: x moved to y
print(x + y)           // ❌ ERROR: x was moved
```

Alternative approaches fail design constraints:

| Approach | Problem |
|----------|---------|
| Everything moves | Violates ES ≥ 0.85 (ergonomics); every int assignment invalidates source |
| Explicit `.clone()` for all | `let y = x.clone()` for every integer violates ED ≤ 1.2 (ceremony) |
| Special-case primitives only | Creates "value types" vs "reference types" distinction, violates Principle 2 (uniform value semantics) |
| Copy-on-write / GC | Violates RO ≤ 1.10 (runtime overhead), TC ≥ 0.90 (hidden costs) |

**Why a size threshold?**

Value semantics (Principle 2) requires uniform behavior: if `i32` copies, then `Point{x: i32, y: i32}` should also copy. But blind copying of large types violates cost transparency (TC ≥ 0.90).

The threshold balances ergonomics with visibility:
- **Below threshold:** Types behave like mathematical values (copy naturally)
- **Above threshold:** Explicit `.clone()` required (cost visible)

**Threshold criteria:**

| Criterion | Justification |
|-----------|---------------|
| **Platform ABI alignment** | Most ABIs pass ≤16 bytes in registers (x86-64 SysV, ARM AAPCS); copies are zero-cost |
| **Common type coverage** | Covers primitives, pairs, RGBA colors, 2D/3D points, small enums |
| **Cache efficiency** | 16 bytes = 1/4 cache line; small enough to not pollute cache |
| **Visibility boundary** | Large enough for natural types, small enough that copies stay obvious |

**Chosen threshold: 16 bytes**

Rationale:
- Matches x86-64 and ARM register-passing conventions (zero-cost copy)
- Covers `(i64, i64)`, `Point3D{x, y, z: f32}`, `RGBA{r, g, b, a: u8}`
- Small enough that silent copies don't violate cost transparency
- Consistent with Rust's typical Copy threshold (though Rust leaves it to type authors)

Types above 16 bytes MUST use explicit `.clone()` or move semantics, making allocation/copy cost visible.

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

### Closure Capture and Mutation

Rask has two kinds of closures with different capture semantics:

| Closure Kind | Capture Mode | Storage | Use Cases |
|--------------|--------------|---------|-----------|
| **Storable** | By value (copy/move) | Can be stored, returned | Callbacks, stored handlers, async tasks |
| **Expression-scoped** | Access outer scope | MUST be called immediately | Iterator adapters, immediate execution |

#### Storable Closures

Capture by value (copy or move), never by reference. Can be stored in variables, structs, or returned.

| Capture type | Behavior |
|--------------|----------|
| Small Copy types | Copied into closure |
| Large/non-Copy types | Moved into closure, source invalid |
| Block-scoped borrows | Allowed if closure doesn't escape borrow's scope |
| Mutable state | Requires Pool + Handle pattern |

**Basic capture:**
```
let name = "Alice"
let greet = || print("Hello, {name}")  // Copies name (String is small)
greet()  // "Hello, Alice"
// name still valid

let data = large_vec()
let process_data = || transform(data)  // Moves data
process_data()
// data invalid after move
```

**Closure parameters:**

Closures can accept parameters passed on each invocation:
```
let double = |x| x * 2
let result = double(5)  // 10

let format_user = |id| "User #{id}"
button.on_click(|event| {  // event passed by caller
    print(format_user(event.user_id))
})
```

**Mutating captured state (WRONG):**
```
let counter = 0
let increment = || counter += 1  // Captures counter by COPY
increment()
increment()
// counter is still 0! Each call mutates the closure's COPY.
```

**Mutating shared state (CORRECT - Pool + Handle):**
```
let state = Pool::new()
let h = state.insert(Counter{value: 0})

// Pattern 1: Capture handle only, receive pool as parameter
let increment = |state_pool| state_pool[h].value += 1
increment(state)  // Pass pool on each call
increment(state)  // Still valid

// Pattern 2: Use parameters only (no capture)
let increment2 = |state_pool, handle| state_pool[handle].value += 1
increment2(state, h)

// Pattern 3: For stored closures, capture handle + pass pool
button.on_click(|event, app_state| {
    // Closure captures h, receives app_state as parameter
    app_state[h].clicks += 1
    app_state[h].last_event = event
})
```

**Canonical pattern for stateful callbacks:**
```
struct App {
    state: Pool<AppState>,
    state_handle: Handle<AppState>,
}

fn setup_handlers(app: App) {
    let h = app.state_handle

    // Each handler captures its needed handles, receives app state
    button1.on_click(|event, state| {
        state[h].mode = Mode::Edit
    })

    button2.on_click(|event, state| {
        state[h].mode = Mode::View
    })

    button3.on_click(|event, state| {
        state[h].save()?
    })

    // Caller provides state when executing
    app.run()  // Framework calls closures with state parameter
}
```

**Borrow capture (limited scope):**
```
let slice = s[0..3]
let f = || process(slice)  // ✅ OK: captures borrow
f()                        // ✅ OK: called in same scope
return f                   // ❌ ERROR: closure escapes borrow's scope
```

#### Expression-Scoped Closures

Access outer scope WITHOUT capturing. MUST be called immediately within the expression.

| Rule | Description |
|------|-------------|
| **EC1: No capture** | Closure accesses outer scope directly |
| **EC2: Immediate execution** | Must be called before expression completes |
| **EC3: Cannot store** | Compile error if assigned to variable or returned |
| **EC4: Aliasing rules apply** | Mutable access excludes other access during execution |

**Read access (iterators):**
```
let items = vec![...]
let vec = vec![...]

// Closure accesses vec WITHOUT capturing it
items.filter(|i| vec[*i].active)
     .map(|i| vec[*i].value * 2)
     .collect()
// vec still valid after chain
```

**Mutable access (immediate callbacks):**
```
let app = AppState::new()

// Expression-scoped: closure executed immediately
button.on_click(|event| {
    app.counter += 1  // Mutates app directly
    app.last_click = event.timestamp
})?.execute_now()  // Must execute in same expression

// app still valid here

// ❌ ILLEGAL: Cannot store expression-scoped closure
let handler = button.on_click(|event| {
    app.counter += 1  // ERROR: captures mutable access to app
})
// Would violate aliasing - app borrowed while handler exists
```

**Storage detection:**

Compiler enforces immediate execution:
```
// ✅ Legal: Inline consumption
for i in items.filter(|i| vec[*i].active) {
    process(i)
}

// ❌ Illegal: Stored closure accesses outer scope
let f = items.filter(|i| vec[*i].active)
//          ^^^^^^^^^ ERROR: closure accesses 'vec' but iterator is stored
```

#### Choosing Between Closure Kinds

| Scenario | Use | Pattern |
|----------|-----|---------|
| Iterator adapter | Expression-scoped | `items.filter(\|i\| vec[*i].active)` |
| Event handler (immediate) | Expression-scoped | `btn.on_click(\|e\| app.x += 1)?.execute_now()` |
| Event handler (stored) | Storable + params | `btn.on_click(\|e, app\| app[h].x += 1)` |
| Async callback | Storable + params | `task.then(\|result, state\| state[h] = result)` |
| Pure transformation | Either | `\|x\| x * 2` (no outer access) |
| Multi-state mutation | Storable + Pool/Handle | Capture handles, receive pools as params |

#### Multiple Closures Sharing State

Use Pool + Handle pattern:
```
let app = AppState::new()
let state = Pool::new()
let h = state.insert(AppData{...})

// All closures capture same handle, receive state as parameter
button1.on_click(|_, s| s[h].mode = Mode::A)
button2.on_click(|_, s| s[h].mode = Mode::B)
button3.on_click(|_, s| s[h].save()?)

// Framework provides state to all handlers
app.run_with_state(state)
```

#### Closure Capture Summary

| Question | Answer |
|----------|--------|
| Can closures mutate outer variables? | No (capture is by copy/move) |
| How to share mutable state? | Pool + Handle, pass pool as parameter |
| Can closures accept parameters? | Yes, passed on each call |
| Can closures access outer scope mutably? | Yes, if expression-scoped (immediate execution) |
| Can I store a closure that mutates outer scope? | No (must be expression-scoped = immediate only) |
| Event handler pattern? | Storable: capture handles, receive state as param |
| Iterator pattern? | Expression-scoped: access outer scope, execute immediately |

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

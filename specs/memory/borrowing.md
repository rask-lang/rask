<!-- id: mem.borrowing -->
<!-- status: decided -->
<!-- summary: Block-scoped views for fixed-layout sources, inline access + `with` for growable sources -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Borrowing

A view into `point.x` can't go stale — struct fields sit at fixed offsets. But `vec[i]` points into a heap buffer that any `push` could reallocate, invalidating every reference into it. That difference determines how long you can hold a view.

**Fixed-layout sources** (struct fields, arrays) can't resize. Views persist until the block ends.

**Growable sources** (Vec, Pool, Map) own heap buffers that can reallocate. Each access is temporary — copy out the value for one expression, or use `with` for multi-statement access.

`string` is immutable and Copy (16 bytes, refcounted). String slices (`s[i..j]`) are temporary views — they can't be stored because the slice would dangle if the source string's refcount drops to zero. See `std.strings/S2`.

| Rule | Source | Access model | Why |
|------|--------|-------------|-----|
| **B1: Fixed = block-scoped** | Struct fields, arrays | View valid until block ends | Layout can't change |
| **B2: Growable = inline + `with`** | Vec, Pool, Map | Copy out (Copy types) or use `with` | Heap buffer can reallocate |
| **B3: String slices = inline only** | string | `s[i..j]` temporary for expression | Slice has no refcount; source could be freed |

**The test:** can the source resize? Vec/Pool/Map own heap buffers that can reallocate. Struct fields and arrays have fixed in-place layout. Strings are immutable but slices are temporary views (S2).

## Parameter and Receiver Borrows

| Rule | Description |
|------|-------------|
| **B3: Call duration** | Function parameters and method receivers are borrowed for the call duration |
| **B4: Element access follows source** | Indexing into a borrowed collection follows the collection's own rules |

| Annotation | Borrow Mode | Determined By |
|------------|-------------|---------------|
| (none) | Shared | Default — read-only, enforced |
| `mutate` | Exclusive | Mutable access, enforced |
| `take` | N/A | Ownership transfer, not a borrow |

<!-- test: skip -->
```rask
func process(items: Vec<Item>) {
    // items: borrowed for entire function
    // items[0].field: inline access, temporary borrow for the expression

    const first = items[0].name   // Copy out if Copy, temporary borrow if not
    items.push(new_item)          // OK: no view held
}
```

## Block-Scoped Views

Views into fixed sources (struct fields, arrays) persist until the block ends.

| Rule | Description |
|------|-------------|
| **S1: Block duration** | View valid from creation until end of enclosing block |
| **S2: Source outlives borrow** | Source must be valid for borrow's entire duration |
| **S3: No escape** | Cannot store in struct, return, or send cross-task |
| **S4: Duration extension** | Borrowing a temporary extends its duration to match borrow |
| **S5: Exclusive access** | Source cannot be mutated while borrowed; mutable borrow excludes all other access |

<!-- test: skip -->
```rask
const point = get_point()
const x = point.x               // View, valid until block ends
const y = point.y               // Another view
validate(x)                    // OK: x still valid
process(x, y)                  // OK: both valid
```

**Duration extension (S4):**
<!-- test: skip -->
```rask
const x = get_point().x         // OK: temporary extended

// Equivalent to:
const _temp = get_point()
const x = _temp.x
// _temp lives as long as x
```

Every temporary in the chain that the borrow transitively depends on is extended. Temporaries in inner blocks are NOT extended to outer blocks.

<!-- test: compile-fail -->
```rask
const x = {
    const p = get_point()
    p.x  // ERROR: p dies at block end
}
// x would outlive p
```

**String slices are temporary (S2):**
<!-- test: compile-fail -->
```rask
const s = "hello world"
const slice = s[0..5]    // ERROR: string slices can't be stored
```

String slices are temporary views into the string's buffer — storing one would create a dangling reference if the source string is freed. Use `.to_string()` or `Span` indices:
<!-- test: skip -->
```rask
const s = "hello world"
const owned = s[0..5].to_string()  // copy to owned string
process(owned)                     // OK: independent value

const span = Span(0, 5)            // or store indices
process(s[span])                   // resolve inline
```

## Inline Expression Access

Single-expression access to collection elements works inline. The compiler creates a temporary borrow for the duration of the expression.

| Rule | Description |
|------|-------------|
| **E1: Expression duration** | Inline access valid only within the expression |
| **E2: Chain calls OK** | `pool[h].field.method()` is one expression |
| **E3: Lvalue in-place** | `collection[key].field = value` is in-place mutation, not copy-modify-discard |
| **E4: Rvalue copies or errors** | `const x = collection[key]` copies if Copy, compile error if not |
| **E5: Sync inline access** | `shared.read().chain`, `shared.write().chain`, and `mutex.lock().chain` follow E1-E4 rules. Lock held for expression duration, released at expression end. Standalone `.read()`/`.write()`/`.lock()` without chaining is a compile error |

<!-- test: skip -->
```rask
pool[h].health -= damage     // In-place mutation (E3)
if pool[h].health <= 0 {     // New inline access
    pool.remove(h)           // No active borrow - OK
}

const hp = pool[h].health    // Copy out i32 (E4, Copy type)
process(pool[h].name)        // Temporary borrow for call duration (E1)
pool[h].pos.normalize()      // Method chain (E2)
```

**Sync primitive inline access (E5):**
<!-- test: skip -->
```rask
const timeout = config.read().timeout           // Copy out (E4)
const name = config.read().user.name             // Copy out (string is Copy)
config.write().timeout = 60.seconds             // In-place mutation (E3)
queue.lock().push(item)                         // Mutex inline access

// Multi-statement still needs with
with config.write() as c {
    c.timeout = 60.seconds
    c.max_retries = 5
}
```

For non-Copy types, `const x = collection[key]` is a compile error. Use `.clone()` or `with` for multi-statement access.

## Multi-Statement Access (`with...as`)

`with` is a first-class block scope for multi-statement access to collection elements, Cell, Shared, and Mutex values. Not sugar for closures — `return`, `try`, `break`, and `continue` work naturally.

| Rule | Description |
|------|-------------|
| **W1: First-class block** | `with` is a real scope — `return` exits the function, `try` propagates to the enclosing function, `break`/`continue` work for surrounding loops |
| **W2: No structural mutation (Vec/Map/string)** | Source collection cannot be structurally mutated inside the `with` block (no insert, remove, push, pop, clear). Reading and writing other elements via inline access is allowed |
| **W2a: Pool insert allowed** | `pool.insert()` is allowed inside `with pool[h]` — compiler re-resolves bindings after insert (handles survive reallocation per `mem.pools/PL9`) |
| **W2b: Pool remove(other) allowed** | `pool.remove(other_h)` is allowed if `other_h` is not the bound handle variable — compiler re-resolves; runtime panic if aliased (same semantics as W3) |
| **W2c: Pool remove(bound) forbidden** | `pool.remove(h)` where `h` is the bound handle is a compile error — you can't remove the element you're borrowing |
| **W2d: Pool clear forbidden** | `pool.clear()` is always a compile error inside `with` — invalidates everything |
| **W3: Aliasing check** | Multiple bindings from same collection: runtime panic if same key/handle |
| **W4: Error semantics** | Panics on invalid handle/OOB (matches direct indexing) |
| **W5: Mutable binding** | `with` bindings are always mutable. Read-only access is enforced by the source (e.g., `shared.read()` prevents mutation). Compiler warns when binding is never mutated |
| **W6: Value production** | Block can produce a value (last expression) — `with` works in expression context |
| **W7: One-liner shorthand** | `with X as v: expr` — no braces for single expressions (parallels `if cond: expr`) |

### Syntax

```
with <source>[<key>] as <binding> { <body> }
with <source>[<key>] as <binding>: <expr>

// Multiple elements
with <source>[<key1>] as <binding1>, <source>[<key2>] as <binding2> { <body> }
```

### Examples

<!-- test: skip -->
```rask
// Multi-statement access
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}

// Multiple elements from same collection
with pool[h1] as e1, pool[h2] as e2 {
    e1.health -= e2.attack    // Runtime panic if h1 == h2
}

// Expression context — produces a value
const name = with pool[h] as entity { entity.name }

// One-liner shorthand
with pool[h] as e: e.health -= 10

// return/try/break work naturally
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> void or Error {
    with pool[h] as entity {
        entity.strength += 10
        entity.defense += 5
        entity.buff_expiry = now() + Duration.seconds(30)
        try log_buff_applied(entity.id)
    }
}
```

### Structural mutation restriction (W2)

For Vec and Map: structural mutations are forbidden inside the `with` block — operations that add, remove, or reallocate elements (insert, remove, push, pop, clear). Reading and writing other elements via inline access works normally. (Strings are immutable — `with` doesn't apply to them.)

<!-- test: compile-fail -->
```rask
with vec[i] as item {
    item.count += 1
    vec.push(new_item)       // ERROR: structural mutation inside with block
}
```

**Pool exception (W2a–W2d):** Pool handles survive reallocation (`mem.pools/PL9`). The compiler exploits this — `insert` and `remove(other)` are allowed inside `with pool[h]` blocks. After each structural mutation, the compiler re-resolves the binding by re-validating the handle (~1ns generation check).

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.health -= pool[other_h].bonus    // OK: inline read of other element
    pool[other_h].last_attacker = Some(h)   // OK: inline write to other element

    // Pool-specific: insert and remove(other) are allowed
    const ally = pool.insert(new_ally)  // OK: re-resolves entity binding  [re-resolved]
    entity.allies.push(ally)                // entity still valid after insert
    pool.remove(expired_h)                  // OK: re-resolves  [re-resolved]
}
```

Removing the bound handle or clearing the pool remain compile errors:

<!-- test: compile-fail -->
```rask
with pool[h] as entity {
    entity.health -= 10
    pool.remove(h)           // ERROR: removing the bound element (W2c)
}
```

<!-- test: compile-fail -->
```rask
with pool[h] as entity {
    pool.clear()             // ERROR: clears everything (W2d)
}
```

If `remove(other_h)` happens to alias the bound handle at runtime, the re-resolution panics with "stale handle" — same aliasing semantics as W3.

For multi-statement access to multiple elements, the comma syntax is still preferred:
<!-- test: skip -->
```rask
with pool[h1] as e1, pool[h2] as e2 {
    e1.health -= e2.attack    // Runtime panic if h1 == h2
}
```

The same-handle restriction still applies — accessing `pool[h]` (same handle variable as the `with` binding) inside the block is a compile error. Use the binding instead.

For iteration + mutation, use mutable iteration (`std.iteration/I4`) or collect handles:
<!-- test: skip -->
```rask
// Mutable iteration (preferred for in-place mutation)
for mutate entity in pool {
    entity.update()
}

// Handle collection (for structural mutation like remove)
const handles = pool.handles().collect()
for h in handles {
    with pool[h] as e { e.update() }
}
```

### Unified `with` across container types

One syntax for all container types that hold values behind indirection. Bindings are always mutable — read-only access is enforced by the source.

| Container | Access | Read-only access |
|-----------|--------|------------------|
| Pool/Vec/Map | `with pool[h] as e { ... }` | — (just don't mutate) |
| Cell | `with cell as v { ... }` | — (just don't mutate) |
| Shared | `with shared.write() as v { ... }` | `with shared.read() as v { ... }` (mutation is compile error) |
| Mutex | `with mutex as v { ... }` | — (always exclusive) |

Shared requires explicit `.read()` or `.write()` — bare `with shared as v` is a compile error. For all other types, `with` gives mutable access. The compiler warns if a binding is never mutated (consider whether you need the `with` block at all, or use `.read()` for Shared).

See [cell.md](cell.md) for Cell specifics, [sync.md](../concurrency/sync.md) for Shared/Mutex specifics.

## Disjoint Field Borrowing

When you pass `value.field` to a function, the borrow checker tracks the borrow at field granularity. Two borrows on different fields of the same struct don't conflict.

| Rule | Description |
|------|-------------|
| **F1: Field-level tracking** | Passing `value.field` to a `mutate` parameter borrows only that field |
| **F2: Non-overlapping** | Borrows on disjoint fields of the same struct can coexist |
| **F3: Whole-object conflict** | A borrow of the whole struct conflicts with any field-level borrow |
| **F4: Closure captures** | Closures capture variables at field granularity — disjoint field captures on different threads don't conflict |

<!-- test: skip -->
```rask
struct GameState {
    entities: Pool<Entity>
    score: i32
}

// Takes the field directly — decoupled from GameState
func movement_system(mutate entities: Pool<Entity>, dt: f32) {
    for h in entities {
        entities[h].position.x += entities[h].velocity.dx * dt
    }
}

func update_score(mutate score: i32, points: i32) {
    score += points
}

func update(mutate state: GameState, dt: f32) {
    movement_system(state.entities, dt)   // Borrows state.entities
    update_score(state.score, 10)          // Borrows state.score (no conflict — F2)
}
```

**Parallel field access (F4):**

<!-- test: skip -->
```rask
func parallel_update(mutate state: GameState, dt: f32) {
    scoped {
        // Compiler sees: captures state.entities mutably
        spawn(|| {
            for h in state.entities {
                state.entities[h].position.x += state.entities[h].velocity.dx * dt
            }
        })
        // Compiler sees: captures state.score mutably — disjoint, no conflict
        spawn(|| {
            state.score += 10
        })
    }
}
```

Functions take concrete field types, not struct-coupled projections. This means `movement_system` works with any `Pool<Entity>`, not just one from `GameState`.

## Access Rules

Many read-only borrows can coexist, OR one mutable borrow can exist — but not both. This prevents one piece of code from modifying data while another is reading it.

| Rule | Read borrow | Mutable borrow |
|------|-------------|----------------|
| **A1: Other reads** | Allowed | Forbidden |
| **A2: Mutations** | Forbidden | Forbidden |
| **A3: Count** | Unlimited | Exactly one |

This is sometimes called "aliasing XOR mutation" — you can alias (have multiple references) or mutate, but not both at the same time.

## Borrow Checking

All checks are performed **locally** within the function. No cross-function analysis.

| Check | When | Error |
|-------|------|-------|
| Duration validity | At borrow creation | "source doesn't live long enough" |
| Aliasing violation | At conflicting access | "cannot mutate while borrowed" |
| Escape attempt | At assignment/return | "borrow cannot escape scope" |

## Error Messages

Error messages use value-based framing. No "statement-scoped" or "view released at semicolon" language.

**Non-Copy element access [E4]:**
```
ERROR [mem.borrowing/E4]: cannot bind non-Copy collection element
   |
5  |  const entity = pool[h]
   |               ^^^^^^^ Entity is not Copy
6  |  entity.update()
   |  ^^^^^^ cannot use — element was not copied out

WHY: Collection elements that aren't Copy can't be assigned to variables.
     Use .clone() for a copy, or with for multi-statement access.

FIX: Use with for multi-statement access:

  with pool[h] as entity {
      entity.update()
  }

  // Or clone if you need an independent copy:
  const entity = pool[h].clone()
```

**Storing view from string [B2]:**
```
ERROR [mem.borrowing/B2]: cannot store string slice
   |
3  |  const slice = line[0..5]
   |                ^^^^^^^^^^ string slices can't be stored

WHY: String slices are temporary views without their own refcount.
     Storing one would dangle if the source is freed.

FIX 1: Copy to owned string:

  const copy = line[0..5].to_string()

FIX 2: Store indices:

  const view = Span(0, 5)
  process(line[view])
```

**Structural mutation inside with — Vec/Map/string [W2]:**
```
ERROR [mem.borrowing/W2]: cannot push to `vec` inside with block — vec can reallocate
   |
5  |  with vec[i] as item {
   |  ---- element borrowed here
6  |      item.count += 1
7  |      vec.push(new_item)
   |      ^^^^^^^^^^^^^^^^^^ structural mutation not allowed inside with block

WHY: Vec/Map can reallocate, invalidating the borrowed element.
     Pool handles survive reallocation — use Pool if you need insert/remove inside with.

FIX: Move the structural mutation outside the with block:

  with vec[i] as item { item.count += 1 }
  vec.push(new_item)
```

**Removing bound handle inside with — Pool [W2c]:**
```
ERROR [mem.borrowing/W2c]: cannot remove `h` inside with block — it's the bound element
   |
5  |  with pool[h] as entity {
   |  ---- element borrowed here
6  |      entity.health -= 10
7  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ removing the element you're borrowing

WHY: Removing the bound element frees its memory. The binding would dangle.

FIX: Move the removal outside the with block:

  const should_remove = with pool[h] as e { e.health -= 10; e.health <= 0 }
  if should_remove { pool.remove(h) }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Borrow from temporary | S4 | Temporary duration extended to match borrow |
| Chained temporaries | S4 | ALL temporaries in chain extended |
| Temporary in inner block | S3 | NOT extended to outer block |
| Nested borrows | S1 | Inner borrow must not outlive outer |
| Borrow across match arms | S1 | All arms see same borrow mode |
| Clone of borrowed | — | Allowed (creates independent copy) |
| Borrow of clone | — | Borrows the new copy, not original |
| Inline access in method chain | E2 | Access spans entire chain |
| `collection[key].field = value` | E3 | In-place lvalue mutation |
| `const x = collection[key]` (non-Copy) | E4 | Compile error |
| `shared.read().field` | E5 | Expression-scoped read lock, same as E1-E4 |
| `shared.write().field = value` | E5 | Expression-scoped write lock, in-place mutation |
| `mutex.lock().field` | E5 | Expression-scoped exclusive lock |
| `shared.read()` standalone | E5 | Compile error — must access a field or method |
| Multiple sync accesses in one expression | E5/DL4 | Compile error — deadlock risk (see `conc.sync/DL4`) |
| `with` and `return` | W1 | Exits the function |
| `with` and `try` | W1 | Propagates to enclosing function |
| `with` and `break`/`continue` | W1 | Applies to surrounding loop |
| Nested `with` same collection | W2 | Compile error (use comma syntax) |
| Inline read of other element inside `with` | W2 | Allowed |
| Inline write of other element inside `with` | W2 | Allowed (runtime panic if same handle) |
| Structural mutation inside `with` (Vec/Map/string) | W2 | Compile error |
| `pool.insert()` inside `with pool[h]` | W2a | Allowed — re-resolves binding |
| `pool.remove(other_h)` inside `with pool[h]` | W2b | Allowed — re-resolves; runtime panic if aliased |
| `pool.remove(h)` inside `with pool[h]` (bound handle) | W2c | Compile error |
| `pool.clear()` inside `with pool[h]` | W2d | Compile error |
| Disjoint field borrows | F2 | Non-overlapping fields can be borrowed simultaneously |
| Field borrow + whole-struct borrow | F3 | Compile error: struct already borrowed |
| Same field borrowed twice | F2 | Compile error: field already borrowed |

## Quick Reference

| Aspect | Fixed Sources | Growable Sources |
|--------|---------------|------------------|
| Types | Struct fields, arrays | Pool, Vec, Map, string |
| View duration | Until block ends (block-scoped) | Expression only (inline access) |
| **Parameter borrows** | Block-scoped (call duration) | Block-scoped (call duration) |
| Can store in `const`? | Yes | Copy types only |
| Multi-statement use? | Direct | `with...as` or copy out |
| The test | Can't grow or shrink | Can grow or shrink |

| Aspect | Sync Primitives (Shared, Mutex) |
|--------|---------------------------------|
| Inline access | `.read()/.write()/.lock()` + field chain (E5) |
| View duration | Expression only (lock released at expression end) |
| Can store in `const`? | Copy types only |
| Multi-statement use? | `with...as` |

## Examples

### String Parsing
<!-- test: parse -->
```rask
func parse_header(line: string) -> Option<(string, string)> {
    const colon = try line.find(':')
    const key = line[0..colon].trim().to_string()      // Copy out (B2)
    const value = line[colon+1..].trim().to_string()   // Copy out (B2)
    Some((key, value))
}
```

### Entity Update (Inline Access)
<!-- test: parse -->
```rask
func update_combat(pool: Pool<Entity>) {
    mut targets: Vec<Handle<Entity>> = find_targets(pool)

    for h in targets {
        pool[h].health -= 10             // Inline access (E1)
        if pool[h].health <= 0 {         // New inline access
            pool.remove(h)               // No active borrow - OK
        }
    }
}
```

### Multi-Statement Mutation
<!-- test: parse -->
```rask
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> void or Error {
    with pool[h] as entity {
        entity.strength += 10
        entity.defense += 5
        entity.buff_expiry = now() + Duration.seconds(30)
        try log_buff_applied(entity.id)
    }
}
```

---

## Appendix (non-normative)

### Rationale

**B1/B2 (fixed vs growable):** I wanted to avoid "borrow checker wrestling" — code that looks fine then explodes 20 lines later. Collections can change structurally — `Vec` reallocates, `Pool` compacts, `Map` rehashes. Block-scoped views into them would dangle. Inline access + `with` kills this bug class.

**S3 (no escape):** The cost is more `.clone()` calls. I think that's better than scope annotations leaking into function signatures.

**W1 (with as first-class block):** The biggest concrete win over the old closure-based `modify()`. Closures can't propagate `return`/`try`/`break` to the enclosing function — `with` can. One access pattern for every container type.

**W2 (structural mutation):** The compiler categorizes each collection method as structural (changes element count or triggers reallocation: insert, remove, push, pop, clear) or non-structural (reads/writes existing elements). For Vec/Map/string, structural mutations are forbidden inside `with` — they could invalidate the borrowed element.

**W2a–W2d (pool exception):** Pool handles survive reallocation (PL9) — that's the entire point of handles. I decided to exploit this inside `with` blocks rather than apply the same restriction as Vec/Map. After `pool.insert()` or `pool.remove(other)`, the compiler re-resolves the binding by re-validating the handle (~1ns generation check). If a `remove(other_h)` aliased the bound handle at runtime, the re-resolution panics "stale handle" — same aliasing semantics as W3. The cost is per-type rules in the compiler, but pools already have their own rules (context clauses, generation coalescing, frozen modifiers). One more isn't conceptual overhead — it's the handle abstraction doing what it was designed for.

**W5 (always mutable):** `with` exists for multi-statement access — and the overwhelming majority of cases involve mutation. If you just need to read, inline access often suffices. Making bindings always mutable eliminates the `const` keyword from `with` entirely, removing a concept that caused confusion: for `Shared<T>`, `const` previously changed the lock type (shared vs exclusive), while for everything else it only controlled binding mutability. With explicit `.read()`/`.write()` on Shared, there's no need for `const` on `with` bindings. The compiler warns when a `with` binding is never mutated.

**E5 (sync inline access):** Collections got inline access through `[]` indexing — `pool[h].field` works without `with`. Sync primitives didn't have an equivalent. `.read()`, `.write()`, and `.lock()` now serve the same role: they produce expression-scoped access to the inner value. The lock is visible in the dot-chain (`config.read().timeout`), so cost transparency is preserved. `with` blocks remain for multi-statement access — inline is just the single-expression shorthand.

**Why string slices are temporary:** Strings are immutable and refcounted, but a slice (`s[i..j]`) is a raw view into the buffer without its own refcount. Storing it would require either a hidden view type or borrow tracking — both contradict the "no storable references" principle. The cost is `.to_string()` calls or `Span` indices — visible, simple, no borrow tracking needed.

**Inline access is still a temporary borrow:** `process(pool[h].name)` where `name` is a string — it's a temporary borrow for the expression. The user sees: "you can use it inline, or copy it out, or use `with`." Value-based framing, borrow-based implementation. Users don't need to understand the implementation.

### Patterns & Guidance

**The pattern for collections:**

<!-- test: skip -->
```rask
// Pattern 1: Copy out the value (Copy types)
const health = pool[h].health    // Value copied
if health <= 0 { ... }

// Pattern 2: with for multi-statement access
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
}

// Pattern 3: One-liner shorthand
with pool[h] as e: e.health -= damage
```

**The pattern for parsers (zero-copy via indices):**

Block-scoped borrowing means parsers can't return references into input buffers. Two patterns handle this.

*Simple case:* `Span` stores `(start, end)` indices. Resolve against the original input inline:

<!-- test: parse -->
```rask
struct Token {
    kind: TokenKind
    span: Span
}

func tokenize(input: string) -> Vec<Token> {
    mut tokens = Vec.new()
    mut pos = 0
    // scan() returns positions — no allocations per token
    for (start, end, kind) in scan(input) {
        tokens.push(Token { kind, span: Span(start, end) })
    }
    return tokens
}

// Caller resolves spans against original input
const source = try read_file(path)
const tokens = tokenize(source)
for tok in tokens {
    process(source[tok.span])  // inline access, no copy
}
```

*Shared buffer case:* `StringPool` gives validated handle-based access when multiple functions share the buffer. See `std.strings` for the full tokenizer pattern.

The cost vs Rust: one `.to_string()` call per token if you need owned strings, zero copies if you keep spans and resolve inline. For hot parsers, the StringPool pattern avoids allocations entirely.

### IDE Integration

The IDE makes access patterns visible through ghost annotations.

| Context | Annotation |
|---------|------------|
| Block-scoped view | `[view: until line N]` |
| Inline access | `[inline access]` |
| `with` block | `[bound: lines N-M]` |
| Conflict site | `[conflict: viewed on line N]` |
| Pool re-resolution (W2a/W2b) | `[re-resolved]` |

<!-- test: skip -->
```rask
// Inline access (collection)
const health = pool[h].health  // [inline access]
if health <= 0 {             // health is a Copy value
    pool.remove(h)           // OK - no conflict
}
```

<!-- test: skip -->
```rask
// Block-scoped view (struct field)
const pos = entity.position    // [view: until line 8]
const vel = entity.velocity    // [view: until line 8]
normalize(pos)               // [uses view from line 3]
apply(pos, vel)              // [uses views from lines 3-4]
}                            // line 8: views released
```

Hover information shows the access type, duration, and suggested patterns for that source type.

**String ownership:** `string` is Copy (immutable, refcounted, 16 bytes) — no `.clone()` needed. See `std.strings/S1` and the [Why Immutable Strings?](../stdlib/strings.md#why-immutable-strings) appendix for rationale.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value-semantics`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Boxes](boxes.md) — The container family whose `with` access follows these rules (`mem.boxes`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
- [Collections](../stdlib/collections.md) — Vec, Map APIs (`std.collections`)
- [Cell](cell.md) — Single-value `with` access (`mem.cell`)
- [Synchronization](../concurrency/sync.md) — `Shared<T>`/`Mutex<T>` `with` access (`conc.sync`)
- [Structs](../types/structs.md) — Struct definition, methods, value semantics (`type.structs`)

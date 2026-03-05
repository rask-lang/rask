<!-- id: mem.borrowing -->
<!-- status: decided -->
<!-- summary: Block-scoped views for fixed-layout sources, inline access + `with` for growable sources -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Borrowing

A view into `point.x` can't go stale — struct fields sit at fixed offsets. But `vec[i]` points into a heap buffer that any `push` could reallocate, invalidating every reference into it. That difference determines how long you can hold a view.

**Fixed-layout sources** (struct fields, arrays) can't resize. Views persist until the block ends.

**Growable sources** (Vec, Pool, Map, string) own heap buffers that can reallocate. Each access is temporary — copy out the value for one expression, or use `with` for multi-statement access.

| Rule | Source | Access model | Why |
|------|--------|-------------|-----|
| **B1: Fixed = block-scoped** | Struct fields, arrays | View valid until block ends | Layout can't change |
| **B2: Growable = inline + `with`** | Vec, Pool, Map, string | Copy out (Copy types) or use `with` | Heap buffer can reallocate |

**The test:** can the source resize? Strings own heap-allocated UTF-8 buffers — they're growable, same as Vec. Struct fields and arrays have fixed in-place layout.

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

**Strings are growable (B2), not block-scoped:**
<!-- test: compile-fail -->
```rask
const s = "hello world"
const slice = s[0..5]    // ERROR: string slices can't be stored
```

Strings own heap buffers — same category as Vec. Use `.to_string()` or `string_view` indices:
<!-- test: skip -->
```rask
const s = "hello world"
const owned = s[0..5].to_string()  // copy to owned string
process(owned)                     // OK: independent value

const view = string_view(0, 5)     // or store indices
process(s[view])                   // resolve inline
```

## Inline Expression Access

Single-expression access to collection elements works inline. The compiler creates a temporary borrow for the duration of the expression.

| Rule | Description |
|------|-------------|
| **E1: Expression duration** | Inline access valid only within the expression |
| **E2: Chain calls OK** | `pool[h].field.method()` is one expression |
| **E3: Lvalue in-place** | `collection[key].field = value` is in-place mutation, not copy-modify-discard |
| **E4: Rvalue copies or errors** | `const x = collection[key]` copies if Copy, compile error if not |

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

For non-Copy types, `const x = collection[key]` is a compile error. Use `.clone()` or `with` for multi-statement access.

## Multi-Statement Access (`with...as`)

`with` is a first-class block scope for multi-statement access to collection elements, Cell, Shared, and Mutex values. Not sugar for closures — `return`, `try`, `break`, and `continue` work naturally.

| Rule | Description |
|------|-------------|
| **W1: First-class block** | `with` is a real scope — `return` exits the function, `try` propagates to the enclosing function, `break`/`continue` work for surrounding loops |
| **W2: No destructive mutation** | Source collection cannot be destructively mutated inside the `with` block (no remove, clear). Reading and writing other elements via inline access is allowed. **Pool exception:** `pool.insert()` is allowed — pools guarantee stable element addresses across growth (`mem.pools/PL10`) |
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
const name = with pool[h] as entity { entity.name.clone() }

// One-liner shorthand
with pool[h] as e: e.health -= 10

// return/try/break work naturally
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> () or Error {
    with pool[h] as entity {
        entity.strength += 10
        entity.defense += 5
        entity.buff_expiry = now() + Duration.seconds(30)
        try log_buff_applied(entity.id)
    }
}
```

### Destructive mutation restriction (W2)

Destructive mutations on the source collection are forbidden inside the `with` block — operations that remove or invalidate elements (remove, pop, clear). Reading and writing other elements via inline access works normally.

<!-- test: compile-fail -->
```rask
with pool[h] as entity {
    entity.health -= 10
    pool.remove(other_h)     // ERROR: destructive mutation inside with block
}
```

Reading and writing other elements is allowed. **Pool.insert() is also allowed** — pools use stable-address storage, so inserts never move existing elements (`mem.pools/PL10`):

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.health -= pool[other_h].bonus    // OK: inline read of other element
    pool[other_h].last_attacker = Some(h)   // OK: inline write to other element
    pool.insert(new_entity)                 // OK: pool addresses are stable
    pool.remove(other_h)                    // ERROR: removal can invalidate elements
}
```

For Vec and Map, all structural mutations (push, insert, remove, clear) remain forbidden — these collections may reallocate their backing storage on growth.

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
5  |  let entity = pool[h]
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

WHY: Strings own heap buffers that can reallocate. Slices are
     temporary — use inline or copy out.

FIX 1: Copy to owned string:

  const copy = line[0..5].to_string()

FIX 2: Store indices:

  const view = string_view(0, 5)
  process(line[view])
```

**Destructive mutation inside with [W2]:**
```
ERROR [mem.borrowing/W2]: cannot destructively mutate collection inside with block
   |
5  |  with pool[h] as entity {
   |  ---- element borrowed here
6  |      entity.health -= 10
7  |      pool.remove(other)
   |      ^^^^^^^^^^^^^^^^^ removal not allowed inside with block

WHY: remove and clear can invalidate the borrowed element.
     Reading, writing, and inserting are fine for pools.

FIX: Move the removal outside the with block:

  const should_remove = pool[h].health <= 0
  if should_remove {
      pool.remove(other)
  }
```

**Vec/Map structural mutation inside with [W2]:**
```
ERROR [mem.borrowing/W2]: cannot structurally mutate Vec inside with block
   |
3  |  with items[i] as item {
   |  ---- element borrowed here
4  |      items.push(new_item)
   |      ^^^^^^^^^^^^^^^^^^^^^ push can reallocate, invalidating borrowed element

WHY: Vec and Map may reallocate backing storage on growth.
     Pool.insert() is safe — pools use stable-address storage.

FIX: Move the mutation outside the with block, or use a separate collection.
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
| `with` and `return` | W1 | Exits the function |
| `with` and `try` | W1 | Propagates to enclosing function |
| `with` and `break`/`continue` | W1 | Applies to surrounding loop |
| Nested `with` same collection | W2 | Compile error (use comma syntax) |
| Inline read of other element inside `with` | W2 | Allowed |
| Inline write of other element inside `with` | W2 | Allowed (runtime panic if same handle) |
| `pool.insert()` inside `with pool[h]` | W2 | Allowed — stable addresses |
| `vec.push()` inside `with vec[i]` | W2 | Compile error — may reallocate |
| `pool.remove()` / `clear()` inside `with` | W2 | Compile error |
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
    let targets: Vec<Handle<Entity>> = find_targets(pool)

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
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> () or Error {
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

**W2 (no destructive mutation):** The compiler categorizes collection methods by their impact on existing elements. Destructive operations (remove, pop, clear) can invalidate the borrowed element — forbidden inside `with`. For Vec and Map, growth operations (push, insert) can also reallocate the backing buffer — also forbidden.

Pool is the exception. Pools use stable-address storage (chunked allocation), so inserting new elements never moves existing ones. Banning `pool.insert()` inside `with` forced awkward data flow restructuring — collect data, exit the block, then insert. I decided the safety cost of allowing it is zero (addresses don't move) and the ergonomic cost of banning it is real.

**W5 (always mutable):** `with` exists for multi-statement access — and the overwhelming majority of cases involve mutation. If you just need to read, inline access often suffices. Making bindings always mutable eliminates the `const` keyword from `with` entirely, removing a concept that caused confusion: for `Shared<T>`, `const` previously changed the lock type (shared vs exclusive), while for everything else it only controlled binding mutability. With explicit `.read()`/`.write()` on Shared, there's no need for `const` on `with` bindings. The compiler warns when a `with` binding is never mutated.

**Why strings are value-access, not block-scoped:** Strings own heap buffers — structurally the same as Vec. Block-scoped string views would require a hidden view type distinct from `string` and borrow-of-borrow tracking when views are passed to functions. This contradicts the "no storable references" principle. The cost is `.to_string()` calls or `string_view` indices — visible, simple, no borrow tracking needed.

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

*Simple case:* `string_view` stores `(start, end)` indices. Resolve against the original input inline:

<!-- test: parse -->
```rask
struct Token {
    kind: TokenKind
    span: string_view
}

func tokenize(input: string) -> Vec<Token> {
    let tokens = Vec.new()
    let pos = 0
    // scan() returns positions — no allocations per token
    for (start, end, kind) in scan(input) {
        tokens.push(Token { kind, span: string_view(start, end) })
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

**String ownership patterns (reducing clone noise):** Most string `.clone()` calls are unnecessary — O3 borrow inference handles the common case. See the `std.strings` appendix [Why Not COW Strings?](../stdlib/strings.md#why-not-cow-strings) for the full pattern catalog.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value-semantics`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
- [Collections](../stdlib/collections.md) — Vec, Map APIs (`std.collections`)
- [Cell](cell.md) — Single-value `with` access (`mem.cell`)
- [Synchronization](../concurrency/sync.md) — `Shared<T>`/`Mutex<T>` `with` access (`conc.sync`)
- [Structs](../types/structs.md) — Struct definition, methods, value semantics (`type.structs`)

<!-- id: mem.borrowing -->
<!-- status: decided -->
<!-- summary: Block-scoped views for fixed-layout sources, value-based access for collections with `with` for multi-statement binding -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Borrowing

Views last as long as the source is stable. Sources with fixed layout (struct fields, arrays) keep views until the block ends. Sources with heap buffers (collections, strings) give you values — copy for Copy types, error for non-Copy. Multi-statement access to collection elements uses `with`.

| Rule | Source | Access model | Why |
|------|--------|-------------|-----|
| **B1: Fixed = block-scoped** | Struct fields, arrays | View valid until block ends | Fixed layout, no reallocation possible |
| **B2: Growable = value access** | Vec, Pool, Map, string | Copy out (Copy types) or use `with` | Heap buffer could reallocate |

The dividing line is **"has a heap buffer"** vs **"doesn't."** Strings own heap-allocated UTF-8 buffers — they go in B2 regardless of `const`/`let`. Struct fields and arrays have fixed in-place layout — they go in B1.

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

**Strings are value-access (B2), not block-scoped:**
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
| **W2: Source frozen** | Source collection cannot be accessed inside the `with` block (no structural mutations, no other element access) |
| **W3: Aliasing check** | Multiple bindings from same collection: runtime panic if same key/handle |
| **W4: Error semantics** | Panics on invalid handle/OOB (matches direct indexing) |
| **W5: Mutable default** | `as v` is mutable (default); `as const v` is read-only (explicit). Compiler warns when Shared takes write lock but binding is never mutated |
| **W6: Value production** | Block can produce a value (last expression) — `with` works in expression context |
| **W7: One-liner shorthand** | `with X as v: expr` — no braces for single expressions (parallels `if cond: expr`) |

### Syntax

```
with <source>[<key>] as [const] <binding> { <body> }
with <source>[<key>] as [const] <binding>: <expr>

// Multiple elements
with <source>[<key1>] as [const] <binding1>, <source>[<key2>] as [const] <binding2> { <body> }
```

### Examples

<!-- test: skip -->
```rask
// Mutable multi-statement access (default — no keyword needed)
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}

// Read-only access (explicit const)
with pool[h] as const entity {
    log("{entity.name} at {entity.position}")
}

// Multiple elements from same collection
with pool[h1] as e1, pool[h2] as e2 {
    e1.health -= e2.attack    // Runtime panic if h1 == h2
}

// Expression context — produces a value
const name = with pool[h] as const entity { entity.name.clone() }

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

### Source freezing (W2)

The source is frozen for the duration of the `with` block. No access to the collection is allowed inside the block — not even reading other elements.

<!-- test: compile-fail -->
```rask
with pool[h] as entity {
    entity.health -= 10
    pool.remove(other_h)     // ERROR: pool frozen inside with block
}
```

<!-- test: compile-fail -->
```rask
with pool[h] as entity {
    entity.health -= 10
    const other = pool[other_h].health   // ERROR: pool frozen inside with block
}
```

For accessing multiple elements, use the comma syntax:
<!-- test: skip -->
```rask
with pool[h1] as e1, pool[h2] as const e2 {
    e1.health -= e2.attack
}
```

For iteration + mutation, collect handles first:
<!-- test: skip -->
```rask
const handles = pool.handles().collect()
for h in handles {
    with pool[h] as e { e.update() }
}
```

### Unified `with` across container types

One syntax for all container types that hold values behind indirection.

| Container | Mutate (default) | Read-only |
|-----------|------------------|-----------|
| Pool/Vec/Map | `with pool[h] as e { ... }` | `with pool[h] as const e { ... }` |
| Cell | `with cell as v { ... }` | `with cell as const v { ... }` |
| Shared | `with shared as v { ... }` (write lock) | `with shared as const v { ... }` (read lock) |
| Mutex | `with mutex as v { ... }` (exclusive lock) | `with mutex as const v { ... }` (exclusive lock) |

Mutex always takes an exclusive lock regardless. The `const` distinction controls whether the binding is mutable inside the block, not the lock mode. For Shared, the distinction matters: `as const v` takes a shared read lock (concurrent readers OK), `as v` takes an exclusive write lock. The compiler warns when a Shared write lock is taken but the binding is never mutated — suggests adding `const`.

See [cell.md](cell.md) for Cell specifics, [sync.md](../concurrency/sync.md) for Shared/Mutex specifics.

## Field Projections for Partial Borrowing

Borrowing a struct borrows all of it. Field projections (`Type.{field1, field2}`) borrow only specific fields.

| Rule | Description |
|------|-------------|
| **P1: Syntax** | `value.{field1, field2}` creates a projection of the named fields |
| **P2: Type syntax** | `Type.{field1}` in function params accepts a projection |
| **P3: Non-overlapping** | Projections with disjoint fields can be borrowed simultaneously |
| **P4: Parallel safe** | Non-overlapping mutable projections can be sent to different threads |

<!-- test: skip -->
```rask
struct GameState {
    entities: Pool<Entity>
    score: i32
}

// Only borrows `entities` - other fields remain available
func movement_system(state: GameState.{entities}, dt: f32) {
    for h in state.entities {
        state.entities[h].position.x += state.entities[h].velocity.dx * dt
    }
}

// Only borrows `score` - can run alongside movement_system
func update_score(state: GameState.{score}, points: i32) {
    state.score += points
}
```

See [types/structs.md](../types/structs.md) for projection type syntax (`type.structs/P1`).

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

**Source frozen inside with [W2]:**
```
ERROR [mem.borrowing/W2]: cannot access collection inside its own with block
   |
5  |  with pool[h] as entity {
   |  ---- pool frozen here
6  |      entity.health -= 10
7  |      pool.remove(other)
   |      ^^^^^^^^^^^^^^^^^ cannot access pool here

FIX: Use comma syntax for multiple elements, or collect handles first:

  with pool[h] as e1, pool[other] as e2 {
      e1.health -= e2.attack
  }
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
| Nested `with` same collection | W2 | Compile error (source frozen) |

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

**B1/B2 (fixed vs growable):** I wanted to avoid "borrow checker wrestling" — code that looks fine then explodes 20 lines later. Collections can change structurally — `Vec` reallocates, `Pool` compacts, `Map` rehashes. Block-scoped views into them would dangle. Value-based access kills this bug class.

**S3 (no escape):** The cost is more `.clone()` calls. I think that's better than scope annotations leaking into function signatures.

**W1 (with as first-class block):** The biggest concrete win over the old closure-based `modify()`. Closures can't propagate `return`/`try`/`break` to the enclosing function — `with` can. One access pattern for every container type.

**W2 (source frozen):** Start strict — freeze the entire collection during `with`. The comma syntax handles the multi-element case. Relaxing to "structural mutations only" is a future option if real code needs it.

**W5 (mutable default):** `with` exists for multi-statement access — and 3 out of 4 container types (Pool/Vec/Map, Cell, Mutex) are used for mutation in the overwhelming majority of cases. If you just need to read, inline access often suffices. Defaulting to mutable saves `mutate` on every `with` block. The outlier is `Shared<T>` (read-heavy by design), where the compiler warns when a write lock is taken but the binding is never mutated. Function parameters default to immutable because they're contracts affecting all callers — `with` bindings are local to the block, different context, different default.

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

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value-semantics`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
- [Collections](../stdlib/collections.md) — Vec, Map APIs (`std.collections`)
- [Cell](cell.md) — Single-value `with` access (`mem.cell`)
- [Synchronization](../concurrency/sync.md) — `Shared<T>`/`Mutex<T>` `with` access (`conc.sync`)

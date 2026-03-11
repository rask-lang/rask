<!-- id: mem.alloc -->
<!-- status: proposed -->
<!-- summary: Custom allocators via using clauses; scope-restricted arena lifetimes -->
<!-- depends: memory/context-clauses.md, memory/ownership.md, stdlib/collections.md -->

# Custom Allocators

Collections use the global allocator by default. `using` clauses let functions receive a different allocator — same mechanism as pool contexts, zero overhead, compile-time checked.

## Allocator Trait

| Rule | Description |
|------|-------------|
| **AL1: Trait definition** | `Allocator` trait requires `alloc`, `dealloc`, `realloc` |
| **AL2: Global default** | `Global` implements `Allocator`, is zero-sized, used when no context present |
| **AL3: Fallible allocation** | `alloc` returns `&raw u8 or AllocError`; `realloc` same |

<!-- test: skip -->
```rask
trait Allocator {
    func alloc(self, size: usize, align: usize) -> &raw u8 or AllocError
    func dealloc(self, ptr: &raw u8, size: usize, align: usize)
    func realloc(self, ptr: &raw u8, old_size: usize, new_size: usize, align: usize) -> &raw u8 or AllocError
}

// Zero-sized, no pointer stored, no overhead
struct Global {}

extend Global: Allocator {
    func alloc(self, size: usize, align: usize) -> &raw u8 or AllocError {
        // delegates to system malloc
    }
    // ...
}
```

## Collection Type Parameter

| Rule | Description |
|------|-------------|
| **AL4: Default type parameter** | `Vec<T>` is sugar for `Vec<T, Global>`. Same for `Map`, `string` |
| **AL5: Zero-cost default** | `Global` is a ZST — collections carry no allocator pointer when using it |
| **AL6: Non-default stored** | Non-Global allocators are stored as a reference in the collection |

<!-- test: skip -->
```rask
const v = Vec.new()               // Vec<i32, Global> — no allocator pointer
const v2 = Vec.new(arena)         // Vec<i32, Arena> — stores &Arena

// Vec<T> and Vec<T, Global> are the same type
func sum(v: Vec<i32>) -> i32 { ... }     // accepts Vec<i32, Global>
```

## Using Allocator Context

| Rule | Description |
|------|-------------|
| **AL7: Unnamed context** | `using Allocator` — threads allocator as hidden parameter; collections auto-resolve it |
| **AL8: Named context** | `using alloc: Allocator` — same as AL7, plus creates a local binding `alloc` for direct use |
| **AL9: Resolution** | Same rules as pool contexts (CC4): local vars → params → self fields → own `using` clause |
| **AL10: Propagation** | A function's `using Allocator` satisfies callees requiring the same context |
| **AL11: No context = Global** | Functions without `using Allocator` always use Global for new allocations |

Unnamed (`using Allocator`) is enough when you only need collections to auto-resolve. Named (`using alloc: Allocator`) is needed when you reference the allocator directly — passing it to `Pool.new(alloc)`, calling `alloc.reset()`, etc.

<!-- test: skip -->
```rask
// Unnamed — collections auto-resolve, don't need the allocator directly
func build_index(items: Vec<Item>) -> Map<string, Item> using Allocator {
    const map = Map.new()    // uses the caller's allocator
    for item in items {
        map.insert(item.name, item)
    }
    return map
}

// Named — need direct access to pass allocator to Pool
func init_world() using alloc: Allocator {
    const entities = Pool.new(alloc)     // pool backed by this allocator
    const spatial = Map.new()            // also uses alloc via auto-resolution
    // ...
}

// Default: always uses Global, no annotation needed
func build_index_default(items: Vec<Item>) -> Map<string, Item> {
    const map = Map.new()    // always Global
    // ...
}
```

## Scoped Allocator Blocks

| Rule | Description |
|------|-------------|
| **AL12: Using block** | `using expr { body }` sets the allocator for all allocations lexically in `body` |
| **AL13: Scope restriction** | Values allocated in a `using` allocator block cannot escape that block |
| **AL14: Lexical only** | The allocator override applies to allocations in the block's lexical scope, not to callees (unless they declare `using Allocator`) |

<!-- test: skip -->
```rask
func process() {
    const result = Vec.new()            // Global

    using Arena.scoped(1.megabytes()) {
        const scratch = Vec.new()       // Arena — cannot escape this block
        scratch.push(1)
        scratch.push(2)

        for x in scratch {
            result.push(x)             // copies value into Global-backed Vec
        }

        // return scratch              // COMPILE ERROR: arena-scoped, cannot escape
    }
    // arena freed, all scratch memory gone

    do_stuff(result)
}
```

## Visibility

| Rule | Description |
|------|-------------|
| **AL15: Public declaration required** | Public functions that create collections with a context allocator must declare `using Allocator` |
| **AL16: Private inference** | Private functions can have `using Allocator` inferred from collection construction in allocator context |

Parallel to CC6/CC7 for pool contexts.

## Compiler Desugaring

<!-- test: skip -->
```rask
// What you write:
func build_index(items: Vec<Item>) -> Map<string, Item> using Allocator {
    const map = Map.new()
    return map
}

// What the compiler generates (conceptual):
func build_index(items: Vec<Item>, __ctx_alloc: &Allocator) -> Map<string, Item, __A> {
    const map = Map.__new_with(__ctx_alloc)
    return map
}
```

## Closures

| Rule | Description |
|------|-------------|
| **AL17: Immediate closure inheritance** | Expression-scoped closures inherit the enclosing allocator context |
| **AL18: Storable closure exclusion** | Storable closures cannot capture allocator context — must pass explicitly |

Parallel to CC9/CC10.

## Interaction with Pools

| Rule | Description |
|------|-------------|
| **AL19: Pool backing allocator** | `Pool.new()` uses Global. `Pool.new(alloc)` uses a named allocator for backing storage |
| **AL20: Pool context orthogonal** | `using Pool<T>` and `using Allocator` are independent contexts — a function can declare both |

<!-- test: skip -->
```rask
func game_frame() {
    const frame_arena = Arena.scoped(4.megabytes())

    using frame_arena {
        const particles = Pool.new()       // Pool auto-resolves arena allocator
        spawn_particles(particles)

        for h in particles.cursor() {
            update_particle(h)
        }
    }
    // arena freed — pool and all particles gone
}

// Named context — need direct access to pass allocator
func build_world() using alloc: Allocator {
    const entities = Pool.new(alloc)       // explicit: pool backed by alloc
    const index = Map.new()                // implicit: auto-resolves alloc
    // ...
}

func update_particle(h: Handle<Particle>) using Pool<Particle> {
    h.lifetime -= 1
}
```

## Standard Allocators

| Allocator | Description |
|-----------|-------------|
| `Global` | System allocator (malloc/free). ZST, default |
| `Arena` | Bump allocator. Bulk free on drop. No individual dealloc |
| `FixedBuffer` | Allocates from a fixed `[u8; N]` buffer. No system calls. Embedded-friendly |

<!-- test: skip -->
```rask
// Arena — bulk allocation, freed together
const arena = Arena.new(1.megabytes())
using arena {
    const tokens = Vec.new()
    const nodes = Vec.new()
    // ... parse ...
}
// all memory freed at once

// FixedBuffer — no malloc, embedded-safe
let buf: [u8; 4096] = [0; 4096]
const alloc = FixedBuffer.new(buf)
using alloc {
    const data = Vec.new()
    data.push(42)        // allocated from buf, no system call
}
```

## Error Messages

**Arena-scoped value escaping [AL13]:**
```
ERROR [mem.alloc/AL13]: arena-scoped value cannot escape using block
   |
5  |  using Arena.scoped(1.megabytes()) {
6  |      const v = Vec.new()
7  |      return v
   |      ^^^^^^^^ v allocated from arena, cannot leave this scope
   |
WHY: Arena memory is freed when the using block exits. Returning
     arena-allocated data would create a dangling pointer.

FIX: Copy the data into an outer-scoped collection:

  const result = Vec.new()          // Global
  using Arena.scoped(1.megabytes()) {
      const v = Vec.new()           // Arena
      for x in v { result.push(x) }
  }
```

**Missing context on public function [AL15]:**
```
ERROR [mem.alloc/AL15]: public function creates collection with context allocator without declaring it
   |
1  |  public func build(items: Vec<Item>) -> Map<string, Item> {
   |                                         ^^^^^^^^^^^^^^^^^^ allocates with context
   |
FIX: Add a using clause:

  public func build(items: Vec<Item>) -> Map<string, Item> using Allocator {
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Nested `using` blocks | AL12 | Inner block overrides outer for its scope |
| Arena-allocated value assigned to outer variable | AL13 | Compile error |
| Collection created in `using` block passed to callee | AL14 | OK — callee receives it by reference, doesn't escape |
| Multiple allocators in scope | CC8 (reused) | Compile error — use named context (AL8) to disambiguate |
| `using Allocator` + `using Pool<T>` on same function | AL20 | Both contexts threaded independently |
| Arena-backed Pool, handle sent via channel | AL13 | Compile error — handle type includes arena scope |
| `try_push` on FixedBuffer-backed Vec | AL3 | Returns Err(PushError) when buffer full |
| Comptime allocations | — | Use compiler arena (CT17), not runtime allocators |

---

## Appendix (non-normative)

### Rationale

**AL4 (default type parameter):** Most code doesn't need custom allocators. Making Global the default and zero-sized means the common case has zero cost — no extra pointer, no vtable, no indirection. The allocator type only appears when you need it.

**AL7/AL8 (unnamed vs named):** Follows pool contexts exactly (CC1/CC2). Most allocator usage is implicit — collections auto-resolve. Named contexts exist for the cases where you need to pass the allocator explicitly (e.g., `Pool.new(alloc)`). The name is local to the function, not part of the API.

**AL13 (scope restriction):** Rask doesn't have lifetime annotations. Scope restriction is the alternative: the compiler enforces that arena-allocated data doesn't outlive the arena, without requiring the user to annotate lifetimes. This is the same strategy as `with` block restrictions for borrows (mem.borrowing/W2).

**AL14 (lexical only):** Implicit propagation through callees (dynamic scoping) would break transparency of cost — you couldn't tell from reading a function whether it uses an arena. Lexical scoping keeps the behavior visible. Functions that want caller-controlled allocation explicitly opt in via `using Allocator`.

**Why not type-erased allocators?** Type erasure (storing `&dyn Allocator`) adds a vtable call to every allocation and makes every Vec 8 bytes larger. Since the common case is Global (which is zero-sized), the default type parameter approach is strictly better — zero cost when you don't use custom allocators, monomorphized when you do.

**Why not Zig-style parameter passing?** Zig passes allocators as explicit function parameters, which makes every allocating function's signature viral. Rask's `using` mechanism provides the same compile-time threading without signature pollution.

### Patterns & Guidance

**Request-scoped arena (web server):**
<!-- test: skip -->
```rask
func handle_request(req: Request) -> Response {
    using Arena.scoped(256.kilobytes()) {
        const params = parse_query(req.url)     // arena, if parse_query uses Allocator
        const body = try parse_json(req.body)
        const result = process(params, body)

        // Response is built and returned — its data copied to Global
        return Response.json(result)
    }
}
```

**Compiler pass:**
<!-- test: skip -->
```rask
func typecheck(ast: Ast) -> TypedAst {
    using Arena.scoped(16.megabytes()) {
        const types = Pool.new()       // Pool backed by arena
        const scopes = Pool.new()      // Pool backed by arena

        const result = resolve_types(ast, types, scopes)
        return result.freeze()         // copies result to Global
    }
}
```

**Embedded fixed-buffer:**
<!-- test: skip -->
```rask
func sensor_loop() {
    let buf: [u8; 2048] = [0; 2048]
    const alloc = FixedBuffer.new(buf)

    using alloc {
        const readings = Vec.with_capacity(64)
        loop {
            readings.clear()               // reuse buffer, no reallocation
            collect_readings(readings)
            transmit(readings)
        }
    }
}
```

### See Also

- [Context Clauses](context-clauses.md) — `using` clause mechanics (`mem.context`)
- [Pools](pools.md) — Handle-based storage, typed arenas (`mem.pools`)
- [Borrowing](borrowing.md) — Scope restrictions for growable sources (`mem.borrowing`)
- [Collections](../stdlib/collections.md) — Vec, Map allocation semantics (`std.collections`)

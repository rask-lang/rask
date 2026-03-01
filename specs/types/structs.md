<!-- id: type.structs -->
<!-- status: decided -->
<!-- summary: Named product types with value semantics, extend blocks, explicit field visibility -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Structs and Product Types

Named product types with value semantics, structural Copy, `extend` blocks for methods, explicit visibility per field.

## Struct Definition

| Rule | Description |
|------|-------------|
| **S1: Named fields** | All fields MUST have names (no tuple structs) |
| **S2: Explicit types** | All fields MUST have explicit types (no inference) |
| **S3: Field ordering** | Default layout (`@layout(Rask)`): compiler reorders fields for optimal alignment. `@layout(C)`: declaration order preserved. Field *access* is always by name — reordering doesn't change semantics |
| **S4: Visibility** | Default: package-visible. `public` makes externally visible |

<!-- test: parse -->
```rask
struct Name {
    field1: Type1
    field2: Type2
}
```

## Field Visibility

| Declaration | Same Package | External |
|-------------|--------------|----------|
| `field: T` | Read + Write | Not visible |
| `public field: T` | Read + Write | Read + Write |

| Struct Type | External Literal | External Pattern Match |
|-------------|------------------|------------------------|
| All fields `public` | Allowed | All fields bindable |
| Any non-public field | Forbidden (factory required) | Only `public` fields bindable |

<!-- test: skip -->
```rask
public struct Request {
    public method: string
    public path: string
    id: u64              // package-only
}

// External code:
const r = Request { method: "GET", path: "/" }  // ERROR: `id` not visible
const r = new_request("GET", "/")               // OK: factory

match r {
    Request { method, path, .. } => ...       // OK: public fields only
}
```

## Value Semantics

| Property | Rule |
|----------|------|
| Copy | Struct is Copy if: all fields Copy AND size <=16 bytes AND not `@unique` |
| Move | Non-Copy structs move on assignment; source invalidated |
| Clone | Auto-derived if all fields implement Clone |

<!-- test: parse -->
```rask
@unique
struct UserId {
    id: u64    // 8 bytes, would be Copy, but forced move-only
}
```

`@unique` prevents accidental copying of values like IDs, tokens, handles where duplicates would be logic errors. Forces explicit `.clone()` when intentional.

See `mem.ownership` for complete Copy/move semantics.

## Methods

| Rule | Description |
|------|-------------|
| **M1: Default borrow** | `self` without modifier means borrow (mutability inferred from usage) |
| **M2: Visibility** | Methods follow same `public`/package rules as structs |
| **M3: Same module** | `extend` blocks MUST be in the same module as the struct definition |
| **M4: Self type** | `self` always refers to the extended struct type |
| **M5: Multiple blocks** | Multiple `extend` blocks for the same type are allowed (for organization) |

| Declaration | Mode | Effect |
|-------------|------|--------|
| `self` | Borrow | Read-only borrow (default) |
| `mutate self` | Mutate | Mutable borrow |
| `take self` | Take | Consumes struct |
| (no self) | Static | Associated function, no instance |

<!-- test: parse -->
```rask
struct Point {
    x: i32
    y: i32
}

extend Point {
    func distance(self, other: Point) -> f64 {
        const dx = self.x - other.x
        const dy = self.y - other.y
        sqrt((dx*dx + dy*dy) as f64)
    }

    func origin() -> Point {
        Point { x: 0, y: 0 }
    }
}
```

**Static methods:**
<!-- test: parse -->
```rask
struct Config {
    values: Map<string, string>
}

extend Config {
    func new() -> Config {                      // Static: no self
        Config { values: Map.new() }
    }

    func from_file(path: string) -> Config or Error {
        // ...
    }
}

const c = Config.new()                          // Called on type
```

## Construction Patterns

**Literal construction (when visible):**
<!-- test: skip -->
```rask
const p = Point { x: 10, y: 20 }
```

**Factory functions (idiomatic for encapsulation):**
<!-- test: parse -->
```rask
public struct Connection {
    socket: Socket        // non-pub
    public state: State
}

extend Connection {
    public func new(addr: string) -> Connection or Error {
        const socket = try connect(addr)
        Ok(Connection { socket, state: State.Connected })
    }
}
```

**Update syntax (functional update):**
<!-- test: skip -->
```rask
const p2 = Point { x: 5, ..p1 }    // Copy p1, override x
```

| Syntax | Requirement |
|--------|-------------|
| `{ x: v, ..source }` | Source must be same type; unspecified fields copied/moved |
| All-public struct | Works externally |
| Mixed visibility | Works only within package |

## Generics

<!-- test: parse -->
```rask
struct Pair<T, U> {
    first: T
    second: U
}

const p: Pair<i32, string> = Pair { first: 1, second: "hello" }
```

<!-- test: skip -->
```rask
struct SortedVec<T: Ord> {
    items: Vec<T>
}

extend SortedVec<T: Ord> {
    func insert(self, item: T) {
        // ... maintain sorted order
    }
}
```

Bounds checked at instantiation site. See `type.generics`.

## Unit Structs

| Property | Value |
|----------|-------|
| Size | 0 bytes |
| Use cases | Type-level markers, phantom types, trait carriers |

<!-- test: parse -->
```rask
struct Marker {}
```

## Memory Layout

| Attribute | Behavior |
|-----------|----------|
| (default) | `@layout(Rask)` — compiler reorders fields for minimal padding. User writes logical order; compiler uses optimal physical order |
| `@layout(C)` | C layout rules: declaration order, C alignment, no reordering. Required for FFI structs |
| `@packed` | Remove padding (may cause unaligned access) |
| `@align(N)` | Minimum alignment of N bytes |
| `@binary` | Binary wire format (see [Binary Structs](binary.md)) |

The default `@layout(Rask)` means users don't think about field ordering for performance. The compiler sorts fields largest-alignment-first to minimize padding. Since field access is by name, reordering has no semantic effect.

IDE shows actual memory layout (field offsets, total size, padding) on hover over a struct definition.

<!-- test: parse -->
```rask
@layout(C)
public struct CPoint {
    x: i32
    y: i32
}
```

Types with `extern "C"` must use `@layout(C)`. See `struct.modules` for C interop details.

## Pattern Matching

<!-- test: skip -->
```rask
match point {
    Point { x: 0, y } => println("on y-axis at {y}"),
    Point { x, y: 0 } => println("on x-axis at {x}"),
    Point { x, y } => println("at ({x}, {y})")
}
```

**Partial patterns:**
<!-- test: skip -->
```rask
let Point { x, .. } = point    // Ignore other fields
```

Visibility in patterns: same package gets all fields; external gets only `public` fields and MUST use `..` for non-public fields.

## Field Projection Types

Projection types let functions accept only specific fields, enabling partial borrowing without lifetime annotations. A projection `T.{a, b}` is a restricted struct view — you see only the named fields, nothing else.

### Core Rules

| Rule | Description |
|------|-------------|
| **P1: Field subset** | `T.{a, b}` creates a type with only the named fields from `T` |
| **P2: Borrow scope** | Each projected field follows normal borrowing independently |
| **P3: No overlap** | Multiple projections can borrow simultaneously if fields don't overlap |
| **P4: Parallel safe** | Non-overlapping mutable projections can be sent to different scoped threads |

<!-- test: skip -->
```rask
struct GameState {
    entities: Pool<Entity>
    player: Handle<Entity>?
    score: i32
    game_over: bool
}

// Only borrows the `entities` field, leaving other fields available
func movement_system(mutate state: GameState.{entities}, dt: f32) {
    for h in state.entities {
        state.entities[h].position.x += state.entities[h].velocity.dx * dt
    }
}

// Only borrows `score` — can run alongside movement_system
func update_score(mutate state: GameState.{score}, points: i32) {
    state.score += points
}

// Can call multiple systems that use different projections
func update(mutate state: GameState, dt: f32) {
    movement_system(state.{entities}, dt)     // Borrows entities
    update_score(state.{score}, 10)           // Borrows score (no conflict)
}
```

### Access and Restrictions

| Rule | Description |
|------|-------------|
| **P5: Field access by name** | Projected fields accessed by name: `proj.field`. Non-projected fields are a compile error |
| **P6: Flat projections** | Projections name direct fields only. `T.{a.b}` is invalid — project `a`, then access `.b` normally |
| **P7: No method dispatch** | Methods defined on `T` cannot be called on `T.{a, b}`. Methods on individual fields work normally |
| **P8: Local binding** | `const p = value.{a, b}` creates a block-scoped projection (`mem.borrowing/S1`–`S3`) |
| **P9: Borrow and mutate only** | Projections combine with borrow (default) and `mutate`. `take` is invalid — partial ownership transfer not supported |
| **P10: Not a type constructor** | Projection types cannot appear as generic type arguments, struct field types, or return types |

**P5 — field access:**

<!-- test: skip -->
```rask
func heal(mutate state: Player.{health}) {
    state.health += 10       // OK: health is projected
    state.inventory          // ERROR: not in projection
}
```

**P6 — flat projections:**

<!-- test: skip -->
```rask
// INVALID: nested projection
func bad(state: Player.{stats.health}) { ... }

// VALID: project the parent, access subfields normally
func good(mutate state: Player.{stats}) {
    state.stats.health += 10
}
```

**P7 — no method dispatch:**

<!-- test: skip -->
```rask
func example(state: GameState.{entities}) {
    state.entities.len()      // OK: method on Pool<Entity>
    state.is_game_over()      // ERROR: GameState method, not available on projection
}
```

**P8 — local binding:**

<!-- test: skip -->
```rask
func update(mutate state: GameState) {
    const proj = state.{entities, score}  // Block-scoped projection
    proj.entities[h].health -= 10         // OK: entities is projected
    proj.score += 100                     // OK: score is projected
    state.game_over                       // ERROR: state borrowed through projection
}   // proj released at block end
```

**P9 — no take:**

<!-- test: skip -->
```rask
func bad(take state: GameState.{entities}) { ... }  // ERROR: take on projection
func ok(mutate state: GameState.{entities}) { ... }  // OK: mutable borrow
func ok2(state: GameState.{entities}) { ... }         // OK: read-only borrow
```

**P10 — not a type constructor:**

<!-- test: skip -->
```rask
// ALL INVALID:
struct Holder { partial: GameState.{entities} }  // No projection in struct fields
func bad() -> GameState.{entities} { ... }       // No projection return types
func bad2<T>(x: T.{field}) { ... }               // No projection of generic types
```

| Pattern | Benefit |
|---------|---------|
| ECS systems | Each system borrows only the components it needs |
| Parallel access | Non-overlapping projections can be sent to scoped threads |
| API clarity | Function signature shows exactly which fields are accessed |
| Local splitting | `const a = val.{x}; const b = val.{y}` — disjoint local borrows |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty struct | S1 | Valid (unit struct), size 0 |
| Single field | S1 | Valid, no special treatment |
| Recursive field | S2 | MUST use `Owned<T>` or `Handle<T>` for indirection |
| Self-referential | S2 | Use `Handle<Self>` using Pool |
| Large struct (>16 bytes) | — | Move semantics; explicit `.clone()` for copy |
| Struct in Vec | — | Allowed if non-linear |
| Linear field | — | Struct becomes linear; must be consumed |
| Generic instantiation | — | Bounds checked; Copy determined per instantiation |
| Projection of non-existent field | P1 | Compile error: "field 'x' does not exist on T" |
| Nested field projection | P6 | Compile error: "projections are flat — project 'stats', then access subfields" |
| Overlapping projections | P3 | Compile error: "projection 'entities' overlaps with existing borrow" |
| Projection with `take` | P9 | Compile error: "cannot take partial ownership — use borrow or mutate" |
| Projection in struct field | P10 | Compile error: "projection types cannot appear in struct definitions" |
| Projection as return type | P10 | Compile error: "projection types cannot be returned" |
| Projection of generic type | P10 | Compile error: "cannot project generic type parameter" |
| Method call on projection | P7 | Compile error: "method 'foo' is defined on T, not on T.{a}" |
| Closure capturing projection | P8 | Follows `mem.closures/SL1` — closure scope-limited to projection |
| Projection to scoped thread | P4 | Valid if thread joined before projection expires (mechanism TBD) |

## Examples

### Data Transfer Object
<!-- test: parse -->
```rask
public struct User {
    public id: u64
    public name: string
    public email: string
}

extend User {
    func validate(self) -> () or Error {
        if self.email.contains("@") { Ok(()) }
        else { Err(Error.invalid("email")) }
    }
}
```

### Encapsulated State
<!-- test: parse -->
```rask
public struct Counter {
    value: i64    // non-pub: controlled access
}

extend Counter {
    public func new() -> Counter {
        Counter { value: 0 }
    }

    public func increment(self) {
        self.value += 1
    }

    public func get(self) -> i64 {
        self.value
    }
}
```

### Linear Resource Wrapper
<!-- test: parse -->
```rask
@resource
struct FileHandle {
    fd: i32
}

extend FileHandle {
    func open(path: string) -> FileHandle or Error {
        const fd = unsafe { libc.open(path.cstr(), O_RDONLY) }
        if fd < 0 { return Err(Error.io()) }
        Ok(FileHandle { fd })
    }

    func close(take self) -> () or Error {
        unsafe { libc.close(self.fd) }
        Ok(())
    }
}
```

---

## Appendix (non-normative)

### Rationale

**S4 (visibility):** Field visibility is explicit — `public` on each field — so API boundaries are clear without scanning the whole struct.

**M3 (same module):** Methods in separate `extend` blocks keep data and behavior distinct, but requiring same-module keeps the type's behavior discoverable.

**No default field values:** Explicit construction shows all values (transparency). Factory functions handle defaults clearly. Avoids hidden initialization order issues. Pattern for defaults:

<!-- test: parse -->
```rask
struct Config {
    timeout: u32
    retries: u32
}

extend Config {
    func default() -> Config {
        Config { timeout: 30, retries: 3 }
    }

    func with_timeout(timeout: u32) -> Config {
        Config { timeout, ..Config.default() }
    }
}
```

Zero-initialization is not automatic. Explicit factory if needed.

### Patterns & Guidance

**Projection use cases (P1-P10):** See `mem.borrowing` for how projections enable parallelism without lifetime annotations. See `mem.parameters` for projection parameter modes.

**When to use projections vs passing fields directly:**

| Scenario | Approach |
|----------|----------|
| Function needs one field, no parallel concern | Pass the field directly: `func f(pool: Pool<Entity>)` |
| Parallel systems need disjoint access | Projections: `func f(mutate state: State.{entities})` |
| Function signature should document field usage | Projections |
| Generic function accepting any pool | Pass the field directly |

**P6 (flat projections):** I considered `T.{stats.health}` for deep projections but it requires tracking borrow paths through multiple struct levels — cross-struct analysis the borrow checker deliberately avoids. Project the parent field and access subfields normally. If deep splitting is needed, flatten the struct.

**P7 (no methods):** Allowing methods on projections would require analyzing which fields a method reads — that's cross-function analysis. Methods are contracts with the full type; projections are borrowing constraints. Different concerns, kept separate.

**P9 (no take):** Taking a projection would leave the struct partially moved. Rust allows this in limited cases; I think the complexity isn't worth it. Destructure the struct if you need to take individual fields.

**P10 (not a type constructor):** Projections are a borrowing mechanism, not a type system feature. Storing them would require tracking borrow provenance in the type system — exactly the lifetime annotations I'm trying to avoid. They live at parameter boundaries and in local blocks, nowhere else.

### See Also

- `mem.ownership` — Copy/move semantics, 16-byte threshold
- `mem.value-semantics` — Value semantics foundation
- `mem.borrowing` — View scoping, projection borrowing (`mem.borrowing/P1`–`P4`)
- `mem.parameters` — Projection parameter modes (`mem.parameters/PM4`–`PM6`)
- `type.generics` — Generic bounds and instantiation
- `struct.modules` — C interop details
- [Binary Structs](binary.md) — `@binary` wire format layout

### Remaining Issues

1. **Tuple structs** — Should `struct Point(i32, i32)` be supported for positional construction?
2. **Scoped thread projections (P4)** — The mechanism for sending projections to scoped threads needs design. See TODO.md.

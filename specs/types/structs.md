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
| **S3: Field ordering** | Fields appear in declaration order; compiler MAY reorder for layout |
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
| `@layout(C)` | C layout rules: declaration order, C alignment, no reordering |
| `@packed` | Remove padding (may cause unaligned access) |
| `@align(N)` | Minimum alignment of N bytes |
| `@binary` | Binary wire format (see [Binary Structs](binary.md)) |

Default (`@layout(Rask)`): compiler may reorder for optimal packing; natural alignment of largest field; no guaranteed offsets.

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

Projection types let functions accept only specific fields, enabling partial borrowing without lifetime annotations.

| Rule | Description |
|------|-------------|
| **P1: Field subset** | `T.{a, b}` accepts only named fields |
| **P2: Borrow scope** | Each field follows normal borrowing independently |
| **P3: No overlap** | Multiple projections can borrow simultaneously if fields don't overlap |
| **P4: Nested access** | Projected fields accessed and mutated normally |

<!-- test: skip -->
```rask
struct GameState {
    entities: Pool<Entity>
    player: Handle<Entity>?
    score: i32
    game_over: bool
}

// Only borrows the `entities` field, leaving other fields available
func movement_system(entities: GameState.{entities}, dt: f32) {
    for h in entities {
        entities[h].position.x += entities[h].velocity.dx * dt
    }
}

// Can call multiple systems that use different projections
func update(state: GameState, dt: f32) {
    movement_system(state.{entities}, dt)     // Borrows entities
    update_score(state.{score}, 10)           // Borrows score (no conflict)
}
```

| Pattern | Benefit |
|---------|---------|
| ECS systems | Each system borrows only the components it needs |
| Parallel access | Non-overlapping projections can be used across threads |
| API clarity | Function signature shows exactly which fields are accessed |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty struct | S1 | Valid (unit struct), size 0 |
| Single field | S1 | Valid, no special treatment |
| Recursive field | S2 | MUST use `Owned<T>` or `Handle<T>` for indirection |
| Self-referential | S2 | Use `Handle<Self>` with Pool |
| Large struct (>16 bytes) | — | Move semantics; explicit `.clone()` for copy |
| Struct in Vec | — | Allowed if non-linear |
| Linear field | — | Struct becomes linear; must be consumed |
| Generic instantiation | — | Bounds checked; Copy determined per instantiation |

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

**Projection use cases (P1-P4):** See `mem.borrowing` for how projections enable parallelism without lifetime annotations.

### See Also

- `mem.ownership` — Copy/move semantics, 16-byte threshold
- `mem.value-semantics` — Value semantics foundation
- `mem.borrowing` — View scoping, projection borrowing
- `type.generics` — Generic bounds and instantiation
- `struct.modules` — C interop details
- [Binary Structs](binary.md) — `@binary` wire format layout

### Remaining Issues

1. **Tuple structs** — Should `struct Point(i32, i32)` be supported for positional construction?

<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Solution: Structs and Product Types

## The Question
How are structs defined, constructed, used in Rask?

## Decision
Named product types with value semantics, structural Copy, `extend` blocks for methods, explicit visibility per field.

## Rationale
Structs are fundamental data composition. Follow value semantics (Principle 2): no implicit sharing, predictable layout. Field visibility explicit—`public` on each field—makes API boundaries clear. Methods in separate `extend` blocks keep data and behavior distinct. Construction uses literals when fields accessible, factory functions otherwise. Layout compiler-controlled by default; `@layout(C)` for C interop.

## Specification

### Struct Definition

<!-- test: parse -->
```rask
struct Name {
    field1: Type1
    field2: Type2
}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **S1: Named fields** | All fields MUST have names (no tuple structs) |
| **S2: Explicit types** | All fields MUST have explicit types (no inference) |
| **S3: Field ordering** | Fields appear in declaration order; compiler MAY reorder for layout |
| **S4: Visibility** | Default: package-visible. `pub` makes externally visible |

### Field Visibility

| Declaration | Same Package | External |
|-------------|--------------|----------|
| `field: T` | Read + Write | Not visible |
| `public field: T` | Read + Write | Read + Write |

**Construction implications:**

| Struct Type | External Literal | External Pattern Match |
|-------------|------------------|------------------------|
| All fields `pub` | Allowed | All fields bindable |
| Any non-public field | Forbidden (factory required) | Only `pub` fields bindable |

**Example:**
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

### Value Semantics

Structs follow the same ownership rules as all Rask values.

| Property | Rule |
|----------|------|
| Copy | Struct is Copy if: all fields Copy AND size ≤16 bytes AND not `@unique` |
| Move | Non-Copy structs move on assignment; source invalidated |
| Clone | Auto-derived if all fields implement Clone |

**Unique structs (opt-out of copying):**
<!-- test: parse -->
```rask
@unique
struct UserId {
    id: u64    // 8 bytes, would be Copy, but forced move-only
}
```

**Use case:** Prevent accidental copying of unique values like IDs, tokens, handles where duplicates would be logic errors. Forces explicit `.clone()` when intentional.

See [Ownership](../memory/ownership.md) for complete Copy/move semantics.

### Methods

Methods in `extend` blocks, separate from definition.

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

**Self parameter modes:**

| Declaration | Mode | Effect |
|-------------|------|--------|
| `self` | Borrow | Read-only borrow (default) |
| `mutate self` | Mutate | Mutable borrow |
| `take self` | Take | Consumes struct |
| (no self) | Static | Associated function, no instance |

**Rules:**

| Rule | Description |
|------|-------------|
| **M1: Default borrow** | `self` without modifier means borrow (mutability inferred from usage) |
| **M2: Visibility** | Methods follow same `pub`/package rules as structs |
| **M3: Same module** | `extend` blocks MUST be in the same module as the struct definition |
| **M4: Self type** | `self` always refers to the extended struct type |
| **M5: Multiple blocks** | Multiple `extend` blocks for the same type are allowed (for organization) |

**Static methods (associated functions):**
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

### Construction Patterns

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

### Generics

Structs can be parameterized by types.

<!-- test: parse -->
```rask
struct Pair<T, U> {
    first: T
    second: U
}

const p: Pair<i32, string> = Pair { first: 1, second: "hello" }
```

**Bounds:**
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

Bounds checked at instantiation site. See [Generics](generics.md).

### Unit Structs

Zero-sized structs (no fields) are valid.

<!-- test: parse -->
```rask
struct Marker {}
```

| Property | Value |
|----------|-------|
| Size | 0 bytes |
| Use cases | Type-level markers, phantom types, trait carriers |

### Memory Layout

**Default (`@layout(Rask)`):**
- Compiler may reorder for optimal packing
- Alignment: natural of largest field
- No guaranteed offsets

**C-compatible (`@layout(C)`):**
<!-- test: parse -->
```rask
@layout(C)
public struct CPoint {
    x: i32
    y: i32
}
```

| Attribute | Behavior |
|-----------|----------|
| `@layout(C)` | C layout rules: declaration order, C alignment, no reordering |
| `@packed` | Remove padding (may cause unaligned access) |
| `@align(N)` | Minimum alignment of N bytes |
| `@binary` | Binary wire format (see [Binary Structs](binary.md)) |

**C interop:** Types with `extern "C"` must use `@layout(C)`.

**Binary formats:** For network protocols or file formats with bit-level layouts, use `@binary`. See [Binary Structs](binary.md).

See [Modules](../structure/modules.md) for C interop details.

### Default Values

No default field values in definitions.

**Rationale:**
- Explicit construction shows all values (transparency)
- Factory functions handle defaults clearly
- Avoids hidden initialization order issues

**Pattern for defaults:**
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

**Zero-initialization:** Not automatic. Explicit factory if needed.

### Edge Cases

| Case | Handling |
|------|----------|
| Empty struct | Valid (unit struct), size 0 |
| Single field | Valid, no special treatment |
| Recursive field | MUST use `Owned<T>` or `Handle<T>` for indirection |
| Self-referential | Use `Handle<Self>` with Pool |
| Large struct (>16 bytes) | Move semantics; explicit `.clone()` for copy |
| Struct in Vec | Allowed if non-linear |
| Linear field | Struct becomes linear; must be consumed |
| Generic instantiation | Bounds checked; Copy determined per instantiation |

### Pattern Matching

Structs support destructuring in patterns.

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

**Visibility in patterns:**
- Same package: all fields available
- External: only `pub` fields; `..` MUST be used for non-public fields

### Field Projection Types

Projection types let functions accept only specific fields, enabling partial borrowing without lifetime annotations.

**Syntax:**
<!-- test: skip -->
```rask
func system(state: MyStruct.{field1, field2}) {
    // Can only access field1 and field2
}
```

**Rules:**

| Rule | Description |
|------|-------------|
| **P1: Field subset** | `T.{a, b}` accepts only named fields |
| **P2: Borrow scope** | Each field follows normal borrowing independently |
| **P3: No overlap** | Multiple projections can borrow simultaneously if fields don't overlap |
| **P4: Nested access** | Projected fields accessed and mutated normally |

**Example from game_loop.rk:**
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

**Use cases:**

| Pattern | Benefit |
|---------|---------|
| ECS systems | Each system borrows only the components it needs |
| Parallel access | Non-overlapping projections can be used across threads |
| API clarity | Function signature shows exactly which fields are accessed |

See [Borrowing](../memory/borrowing.md) for how projections enable parallelism without lifetime annotations.

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

## Integration Notes

- **Memory Model:** Structs follow value semantics; Copy determined by size + fields + `move` keyword
- **Type System:** Structural trait matching applies; `public` types can satisfy `public` traits across packages
- **Generics:** Generic structs instantiated at use site; constraints propagate to methods
- **Concurrency:** Structs sent across channels transfer ownership; fields cannot be borrowed cross-task
- **C Interop:** Use `@layout(C)` for stable layout; only C-compatible field types allowed
- **Pattern Matching:** Destructuring follows visibility rules; inferred binding modes like enums
- **Tooling:** IDE SHOULD show: inferred Copy/Move, field layout, method modes as ghost annotations

---

## Remaining Issues

### High Priority
(none)

### Medium Priority
(none)

### Low Priority
1. **Tuple structs** — Should `struct Point(i32, i32)` be supported for positional construction?

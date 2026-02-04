# Solution: Structs and Product Types

## The Question
How are structs (product types) defined, constructed, and used in Rask? Covers field definitions, methods, visibility, construction patterns, and memory layout.

## Decision
Named product types with value semantics, structural Copy determination, `extend` block method definitions, and explicit visibility per field.

## Rationale
Structs are the fundamental way to compose data. They follow value semantics (Principle 2): no implicit sharing, predictable memory layout. Field visibility is explicit—`pub` on each field—making API boundaries clear. Methods are defined in separate `extend` blocks, keeping data layout and behavior distinct. Construction uses struct literals when all fields are accessible, factory functions otherwise. Layout is compiler-controlled by default; `@layout(C)` available for C interop.

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

**Use case:** Prevent accidental copying of semantically unique values like IDs, tokens, or handles where duplicate values would be a logic error. Forces explicit `.clone()` when copying is intentional.

See [Ownership](../memory/ownership.md) for complete Copy/move semantics.

### Methods

Methods are defined in `extend` blocks, separate from the struct definition.

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
| `self` | Borrow | Borrows, struct valid after (compiler infers read vs mutate) |
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

    func from_file(path: string) -> Result<Config, Error> {
        // ...
    }
}

const c = Config.new()                          // Called on type
```

### Construction Patterns

**Literal construction (when visible):**
```rask
const p = Point { x: 10, y: 20 }
```

**Factory functions (idiomatic for encapsulation):**
```rask
public struct Connection {
    socket: Socket        // non-pub
    public state: State
}

extend Connection {
    public func new(addr: string) -> Result<Connection, Error> {
        const socket = connect(addr)?
        Ok(Connection { socket, state: State.Connected })
    }
}
```

**Update syntax (functional update):**
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

let p: Pair<i32, string> = Pair { first: 1, second: "hello" }
```

**Bounds:**
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
- Compiler MAY reorder fields for optimal packing
- Alignment: natural alignment of largest field
- No guaranteed field offsets

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

**C interop requirement:** Types used with `extern "C"` MUST use `@layout(C)`.

**Binary formats:** For parsing network protocols or file formats with bit-level layouts, use `@binary`. See [Binary Structs](binary.md).

See [Modules](../structure/modules.md) for C interop details.

### Default Values

Rask does NOT support default field values in struct definitions.

**Rationale:**
- Explicit construction shows all values (cost transparency)
- Factory functions handle defaults with clear naming
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

**Zero-initialization:** Not automatic. Use explicit factory if needed.

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

```rask
match point {
    Point { x: 0, y } => println("on y-axis at {y}"),
    Point { x, y: 0 } => println("on x-axis at {x}"),
    Point { x, y } => println("at ({x}, {y})")
}
```

**Partial patterns:**
```rask
let Point { x, .. } = point    // Ignore other fields
```

**Visibility in patterns:**
- Same package: all fields available
- External: only `pub` fields; `..` MUST be used for non-public fields

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
    func validate(self) -> Result<(), Error> {
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
    func open(path: string) -> Result<FileHandle, Error> {
        const fd = unsafe { libc.open(path.cstr(), O_RDONLY) }
        if fd < 0 { return Err(Error.io()) }
        Ok(FileHandle { fd })
    }

    func close(take self) -> Result<(), Error> {
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

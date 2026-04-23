<!-- id: type.structs -->
<!-- status: decided -->
<!-- summary: Named product types with value semantics, extend blocks, `private` keyword for encapsulation -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Structs and Product Types

Named product types with value semantics, structural Copy, `extend` blocks for methods, `private` keyword for encapsulation.

## Struct Definition

| Rule | Description |
|------|-------------|
| **S1: Named fields** | All fields MUST have names (no tuple structs) |
| **S2: Explicit types** | All fields MUST have explicit types (no inference) |
| **S3: Field ordering** | Default layout (`@layout(Rask)`): compiler reorders fields for optimal alignment. `@layout(C)`: declaration order preserved. Field *access* is always by name — reordering doesn't change semantics |
| **S4: Visibility** | Default: package-visible. `private` restricts to `extend` blocks only. `public` for external |

<!-- test: parse -->
```rask
struct Name {
    field1: Type1
    field2: Type2
}
```

## Field Visibility

| Declaration | `extend` blocks | Same Package | External |
|-------------|-----------------|--------------|----------|
| `private field: T` | Read + Write | Not visible | Not visible |
| `field: T` | Read + Write | Read + Write | Not visible |
| `public field: T` | Read + Write | Read + Write | Read + Write |

| Struct Fields | Literal Construction | Pattern Match |
|---------------|---------------------|---------------|
| All `public` | Anyone | All fields bindable by anyone |
| No `private` fields | Same package (or `extend` blocks) | All fields within package; `public` only externally |
| Any `private` field | Only `extend` blocks | Only visible fields; must use `..` for hidden fields |

<!-- test: skip -->
```rask
public struct Request {
    public method: string       // anyone can read/write
    public path: string         // anyone can read/write
    id: u64                     // same package (default)
    private buffer: Vec<u8>     // extend blocks only
}

// Same package, outside extend block:
const r = new_request("GET", "/")             // factory required (buffer is private)
r.id                                          // OK: package-visible (default)
r.buffer                                      // ERROR: private

// External code:
r.method                                      // OK: public
r.id                                          // ERROR: package-only

match r {
    Request { method, path, .. } => ...       // OK: public fields only
}
```

## Value Semantics

| Property | Rule |
|----------|------|
| Copy | Struct is Copy if: all fields Copy AND size <=16 bytes AND not `@unique` |
| Move | Non-Copy structs move on assignment; source invalidated |
| Cloneable | Auto-derived if all fields implement Cloneable |

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
| **M2: Visibility** | Methods follow same `public`/`private`/package rules. `private` methods in `extend` blocks are only callable from that type's `extend` blocks |
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

**Literal construction (when all fields visible):**
<!-- test: parse -->
```rask
public struct Point {
    public x: i32
    public y: i32
}

const p = Point { x: 10, y: 20 }   // OK: all fields public
```

**Factory functions (idiomatic for encapsulation):**
<!-- test: parse -->
```rask
public struct Connection {
    private socket: Socket
    public state: State
}

extend Connection {
    public func new(addr: string) -> Connection or Error {
        const socket = try connect(addr)
        return Connection { socket, state: State.Connected }  // OK: inside extend block
    }
}
```

**Update syntax (functional update):**
<!-- test: parse -->
```rask
const p2 = Point { x: 5, ..p1 }    // OK: all fields public, copy p1, override x
```

| Syntax | Requirement |
|--------|-------------|
| `{ x: v, ..source }` | Source must be same type; unspecified fields copied/moved |
| All-public struct | Works anywhere |
| No `private` fields | Works within package |
| Any `private` field | Works only in `extend` blocks |

## Generics

<!-- test: parse -->
```rask
struct Pair<T, U> {
    public first: T
    public second: U
}

const p: Pair<i32, string> = Pair { first: 1, second: "hello" }
```

<!-- test: skip -->
```rask
struct SortedVec<T: Comparable> {
    private items: Vec<T>
}

extend SortedVec<T: Comparable> {
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

Assuming `Point` with `public` fields (see Construction Patterns above):

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
mut Point { x, .. } = point    // Bind visible fields, ignore rest
```

Visibility in patterns: `extend` blocks see all fields; same package sees package-visible + `public` fields; external sees only `public` fields. `private` fields require `..`.

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
        if self.email.contains("@") { return }
        return Error.invalid("email")
    }
}
```

### Encapsulated State
<!-- test: parse -->
```rask
public struct Counter {
    private value: i64
}

extend Counter {
    public func new() -> Counter {
        Counter { value: 0 }       // OK: inside extend block
    }

    public func increment(self) {
        self.value += 1             // OK: inside extend block
    }

    public func get(self) -> i64 {
        self.value                  // OK: inside extend block
    }
}
// counter.value from outside extend block → compile error (private)
```

### Linear Resource Wrapper
<!-- test: parse -->
```rask
@resource
struct FileHandle {
    private fd: i32
}

extend FileHandle {
    func open(path: string) -> FileHandle or Error {
        const fd = unsafe { libc.open(path.cstr(), O_RDONLY) }
        if fd < 0 { return Error.io() }
        return FileHandle { fd }
    }

    func close(take self) -> () or Error {
        unsafe { libc.close(self.fd) }
        return
    }
}
```

---

## Appendix (non-normative)

### Rationale

**S4 (visibility):** Fields default to package-visible — same as functions and types. Same package = same team. `private` restricts to `extend` blocks for when you need encapsulation (invariant protection). `public` widens to external access. This keeps data-oriented design zero-ceremony while giving an explicit opt-in for encapsulation. I think the `private` keyword pulling its weight by signaling intent — "this field has invariants" — is more informative than a silent default.

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

**Disjoint field borrowing:** Functions that need a single field should take that field's type directly: `func f(mutate entities: Pool<Entity>)`. The borrow checker tracks field-level borrows at call sites — passing `state.entities` borrows only that field. See `mem.borrowing/F1`–`F4`.

### See Also

- `mem.ownership` — Copy/move semantics, 16-byte threshold
- `mem.value-semantics` — Value semantics foundation
- `mem.borrowing` — View scoping, disjoint field borrowing (`mem.borrowing/F1`–`F4`)
- `type.generics` — Generic bounds and instantiation
- `struct.modules` — C interop details
- [Binary Structs](binary.md) — `@binary` wire format layout

### Remaining Issues

1. **Tuple structs** — Should `struct Point(i32, i32)` be supported for positional construction?

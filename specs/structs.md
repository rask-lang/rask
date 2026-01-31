# Solution: Structs and Product Types

## The Question
How are structs (product types) defined, constructed, and used in Rask? Covers field definitions, methods, visibility, construction patterns, and memory layout.

## Decision
Named product types with value semantics, structural Copy determination, in-block method definitions, and explicit visibility per field.

## Rationale
Structs are the fundamental way to compose data. They follow value semantics (Principle 2): no implicit sharing, predictable memory layout. Field visibility is explicit—`pub` on each field—making API boundaries clear. Methods are defined inside the struct block (like enums) for locality. Construction uses struct literals when all fields are accessible, factory functions otherwise. Layout is compiler-controlled by default; `#[repr(C)]` available for C interop.

## Specification

### Struct Definition

```
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
| `pub field: T` | Read + Write | Read + Write |

**Construction implications:**

| Struct Type | External Literal | External Pattern Match |
|-------------|------------------|------------------------|
| All fields `pub` | Allowed | All fields bindable |
| Any non-pub field | Forbidden (factory required) | Only `pub` fields bindable |

**Example:**
```
pub struct Request {
    pub method: String
    pub path: String
    id: u64              // package-only
}

// External code:
let r = Request { method: "GET", path: "/" }  // ERROR: `id` not visible
let r = new_request("GET", "/")               // OK: factory

match r {
    Request { method, path, .. } => ...       // OK: pub fields only
}
```

### Value Semantics

Structs follow the same ownership rules as all Rask values.

| Property | Rule |
|----------|------|
| Copy | Struct is Copy if: all fields Copy AND size ≤16 bytes AND not `move struct` |
| Move | Non-Copy structs move on assignment; source invalidated |
| Clone | Auto-derived if all fields implement Clone |

**Move-only structs (opt-out):**
```
move struct UserId {
    id: u64    // 8 bytes, would be Copy, but forced move
}
```

See [Memory Model](memory-model.md) for complete Copy/move semantics.

### Methods

Methods are defined inside the struct block.

```
struct Point {
    x: i32
    y: i32

    fn distance(self, other: Point) -> f64 {
        let dx = self.x - other.x
        let dy = self.y - other.y
        sqrt((dx*dx + dy*dy) as f64)
    }

    fn origin() -> Point {
        Point { x: 0, y: 0 }
    }
}
```

**Self parameter modes:**

| Declaration | Mode | Effect |
|-------------|------|--------|
| `self` | Read | Borrows, struct valid after |
| `mutate self` | Mutate | Borrows mutably, may modify |
| `transfer self` | Transfer | Consumes struct |
| (no self) | Static | Associated function, no instance |

**Rules:**

| Rule | Description |
|------|-------------|
| **M1: Default read** | `self` without modifier means read-only borrow |
| **M2: Visibility** | Methods follow same `pub`/package rules as fields |
| **M3: No external methods** | Methods MUST be defined in struct block (no extension methods) |
| **M4: Self type** | `self` always refers to enclosing struct type |

**Static methods (associated functions):**
```
struct Config {
    values: Map<String, String>

    fn new() -> Config {                      // Static: no self
        Config { values: Map::new() }
    }

    fn from_file(path: String) -> Result<Config, Error> {
        // ...
    }
}

let c = Config::new()                         // Called on type
```

### Construction Patterns

**Literal construction (when visible):**
```
let p = Point { x: 10, y: 20 }
```

**Factory functions (idiomatic for encapsulation):**
```
pub struct Connection {
    socket: Socket        // non-pub
    pub state: State

    pub fn new(addr: String) -> Result<Connection, Error> {
        let socket = connect(addr)?
        Ok(Connection { socket, state: State::Connected })
    }
}
```

**Update syntax (functional update):**
```
let p2 = Point { x: 5, ..p1 }    // Copy p1, override x
```

| Syntax | Requirement |
|--------|-------------|
| `{ x: v, ..source }` | Source must be same type; unspecified fields copied/moved |
| All-pub struct | Works externally |
| Mixed visibility | Works only within package |

### Generics

Structs can be parameterized by types.

```
struct Pair<T, U> {
    first: T
    second: U
}

let p: Pair<i32, String> = Pair { first: 1, second: "hello" }
```

**Bounds:**
```
struct SortedVec<T: Ord> {
    items: Vec<T>

    fn insert(mutate self, item: T) {
        // ... maintain sorted order
    }
}
```

Bounds checked at instantiation site. See [Generics](generics.md).

### Unit Structs

Zero-sized structs (no fields) are valid.

```
struct Marker {}
```

| Property | Value |
|----------|-------|
| Size | 0 bytes |
| Use cases | Type-level markers, phantom types, trait carriers |

### Memory Layout

**Default (`#[repr(Rask)]`):**
- Compiler MAY reorder fields for optimal packing
- Alignment: natural alignment of largest field
- No guaranteed field offsets

**C-compatible (`#[repr(C)]`):**
```
#[repr(C)]
pub struct CPoint {
    x: i32
    y: i32
}
```

| Attribute | Behavior |
|-----------|----------|
| `#[repr(C)]` | C layout rules: declaration order, C alignment, no reordering |
| `#[repr(packed)]` | Remove padding (may cause unaligned access) |
| `#[repr(align(N))]` | Minimum alignment of N bytes |

**C interop requirement:** Types used with `extern "C"` MUST use `#[repr(C)]`.

See [Module System](module-system.md) for C interop details.

### Default Values

Rask does NOT support default field values in struct definitions.

**Rationale:**
- Explicit construction shows all values (cost transparency)
- Factory functions handle defaults with clear naming
- Avoids hidden initialization order issues

**Pattern for defaults:**
```
struct Config {
    timeout: u32
    retries: u32

    fn default() -> Config {
        Config { timeout: 30, retries: 3 }
    }

    fn with_timeout(timeout: u32) -> Config {
        Config { timeout, ..Config::default() }
    }
}
```

**Zero-initialization:** Not automatic. Use explicit factory if needed.

### Edge Cases

| Case | Handling |
|------|----------|
| Empty struct | Valid (unit struct), size 0 |
| Single field | Valid, no special treatment |
| Recursive field | MUST use `Box<T>` or `Handle<T>` for indirection |
| Self-referential | Use `Handle<Self>` with Pool |
| Large struct (>16 bytes) | Move semantics; explicit `.clone()` for copy |
| Struct in Vec | Allowed if non-linear |
| Linear field | Struct becomes linear; must be consumed |
| Generic instantiation | Bounds checked; Copy determined per instantiation |

### Pattern Matching

Structs support destructuring in patterns.

```
match point {
    Point { x: 0, y } => println("on y-axis at {y}"),
    Point { x, y: 0 } => println("on x-axis at {x}"),
    Point { x, y } => println("at ({x}, {y})")
}
```

**Partial patterns:**
```
let Point { x, .. } = point    // Ignore other fields
```

**Visibility in patterns:**
- Same package: all fields available
- External: only `pub` fields; `..` MUST be used for non-pub fields

## Examples

### Data Transfer Object
```
pub struct User {
    pub id: u64
    pub name: String
    pub email: String

    fn validate(self) -> Result<(), Error> {
        if self.email.contains("@") { Ok(()) }
        else { Err(Error::invalid("email")) }
    }
}
```

### Encapsulated State
```
pub struct Counter {
    value: i64    // non-pub: controlled access

    pub fn new() -> Counter {
        Counter { value: 0 }
    }

    pub fn increment(mutate self) {
        self.value += 1
    }

    pub fn get(self) -> i64 {
        self.value
    }
}
```

### Linear Resource Wrapper
```
move struct FileHandle {
    fd: i32

    fn open(path: String) -> Result<FileHandle, Error> {
        let fd = unsafe { libc::open(path.cstr(), O_RDONLY) }
        if fd < 0 { return Err(Error::io()) }
        Ok(FileHandle { fd })
    }

    fn close(transfer self) -> Result<(), Error> {
        unsafe { libc::close(self.fd) }
        Ok(())
    }
}
```

## Integration Notes

- **Memory Model:** Structs follow value semantics; Copy determined by size + fields + `move` keyword
- **Type System:** Structural trait matching applies; `pub` types can satisfy `pub` traits across packages
- **Generics:** Generic structs instantiated at use site; bounds propagate to methods
- **Concurrency:** Structs sent across channels transfer ownership; fields cannot be borrowed cross-task
- **C Interop:** Use `#[repr(C)]` for stable layout; only C-compatible field types allowed
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

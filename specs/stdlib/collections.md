# Collections (Vec and Map)

Unified collection types (`Vec<T>`, `Map<K,V>`) with optional capacity constraints, closure-based access for scoped borrows, fallible allocation.

For handle-based sparse storage (`Pool<T>`), see [pools.md](../memory/pools.md).

## Specification

### Collection Types

| Type | Purpose | Creation |
|------|---------|----------|
| `Vec<T>` | Ordered, indexed | `Vec.new()` (unbounded)<br>`Vec.with_capacity(n)` (bounded)<br>`Vec.fixed(n)` (pre-allocated, bounded)<br>`Vec.from([T; N])` (from array literal) |
| `Map<K,V>` | Key-value associative | `Map.new()` (unbounded)<br>`Map.with_capacity(n)` (bounded)<br>`Map.from([(K,V); N])` (from array of pairs) |

**When to use which:**
- `Vec<T>` — Ordered data, access by position, elements don't need stable identity
- `Map<K,V>` — Lookup by arbitrary key, no ordering guarantees
- `Pool<T>` — Elements reference each other (graphs, trees), need stable handles. See [pools.md](../memory/pools.md)

**Capacity semantics:**
- Unbounded: `capacity() == None`, grows indefinitely
- Bounded: `capacity() == Some(n)`, cannot exceed `n` elements
- Fixed: Bounded + pre-allocated at creation

### From Literal Constructors

**Convenience constructors for creating collections from literal values:**

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `Vec.from(arr)` | `[T; N] -> Vec<T>` | Copy array elements into new Vec |
| `Map.from(pairs)` | `[(K,V); N] -> Map<K,V>` | Build Map from key-value pairs |

**Examples:**
```rask
// Vec from array literal
const items = Vec.from([1, 2, 3, 4, 5])

// Map from pairs
const config = Map.from([
    ("host", "localhost"),
    ("port", 8080),
])
```

**Note:** Array literals `[...]` already create Vec values, so `Vec.from([1, 2, 3])` is equivalent to `[1, 2, 3]`. The explicit constructor is provided for API clarity and consistency.

### Allocation - All Fallible

**ALL growth operations return `Result` with rejected value on failure:**

| Operation | Returns | Error Type |
|-----------|---------|------------|
| `vec.push(x)` | `Result<(), PushError<T>>` | `Full(T)` or `Alloc(T)` |
| `vec.extend(iter)` | `Result<(), ExtendError<T>>` | Contains first rejected item |
| `vec.reserve(n)` | `Result<(), AllocError>` | No data to return |
| `map.insert(k, v)` | `Result<Option<V>, InsertError<V>>` | `Full(V)` or `Alloc(V)` |

**Error types:**
```rask
enum PushError<T> {
    Full(T),   // Bounded collection at capacity
    Alloc(T),  // Allocation failed
}
```

**Convenience methods:**
```rask
try vec.push(x)             // Propagate error
vec.push(x).unwrap()        // Panic on error
vec.push_or_panic(x)        // Explicit panic variant
```

### Vec - Indexed Access

**Expression-scoped borrows via `[]`:**
```rask
vec[i].field              // Read field (expression-scoped borrow)
vec[i].field = value      // Mutate field (expression-scoped mutable borrow)
const x = vec[i]          // Copy out (T: Copy only)
```

**Methods for safe access:**

| Method | Returns | Constraint | Panics |
|--------|---------|------------|--------|
| `vec[i]` | `T` | `T: Copy` | Yes (OOB) |
| `vec[i].field` | expression-scoped borrow | None | Yes (OOB) |
| `vec.get(i)` | `Option<T>` | `T: Copy` | No |
| `vec.get_clone(i)` | `Option<T>` | `T: Clone` | No |
| `vec.read(i, \|v\| R)` | `Option<R>` | None | No |
| `vec.modify(i, \|v\| R)` | `Option<R>` | None | No |

**Closure access (for multi-statement operations):**

```rask
const name = try vec.read(i, |v| v.name.clone())  // Option<string>
try vec.modify(i, |v| v.count += 1)               // Option<()>
```

**Pattern selection guide:**
- 1 statement → `vec[i].field = x`
- Method chain → `vec[i].value.method().chain()`
- 2+ statements → `try vec.modify(i, |v| { ... })`
- Error propagation → `try vec.modify(i, |v| -> Result { ... })`

### Map - Key-Based Access

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map[k]` | `V` | Panics if missing (V: Copy) |
| `map[k].field` | expression-scoped borrow | Panics if missing |
| `map.get(k)` | `Option<V>` | Copy out (V: Copy) |
| `map.get_clone(k)` | `Option<V>` | Clone out (V: Clone) |
| `map.read(k, \|v\| R)` | `Option<R>` | Read if exists |
| `map.modify(k, \|v\| R)` | `Option<R>` | Mutate if exists |
| `map.remove(k)` | `Option<V>` | Remove and return |

**Entry API for get-or-insert patterns:**

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map.ensure(k, \|\| v)` | `Result<(), InsertError>` | Insert if missing, no-op if present |
| `map.ensure_modify(k, \|\| v, \|v\| R)` | `Result<R, InsertError>` | Insert if missing, then mutate |

```rask
// Ensure user exists, then update
try map.ensure(user_id, || User.new(user_id))
try map.modify(user_id, |u| u.last_seen = now())

// Or combined
try map.ensure_modify(user_id, || User.new(user_id), |u| {
    u.last_seen = now()
    u.visit_count += 1
})
```

### Multi-Element Mutation

**Explicit disjoint operations:**

| Operation | Signature | Semantics |
|-----------|-----------|-----------|
| `vec.swap(i, j)` | `()` | Swap two indices (panics if equal) |
| `vec.modify_many([i, j, k], \|[a, b, c]\| R)` | `Option<R>` | Mutate multiple (panics if duplicates) |

**Disjointness enforcement:**
- Runtime check for distinctness before entering closure
- Panics if duplicates detected

### Iteration

**Iteration modes:**
```rask
for i in vec { }              // i: usize (index iteration)
for item in vec.iter() { }    // item: borrowed T (ref iteration)
for item in vec.take_all() { } // item: T (consuming iteration)
```

**Conditional removal:**
```rask
// Remove matching elements (no allocation)
vec.remove_where(|x| x.expired) -> usize  // Returns count

// Remove and collect
vec.drain_where(|x| x.expired) -> Vec<T>  // Allocates

// Retain non-matching
vec.retain(|x| !x.expired)
```

### Shrinking

**Infallible, best-effort:**
```rask
vec.shrink_to_fit()      // Shrink to len, may keep larger if realloc fails
vec.shrink_to(n)         // Shrink to at least n capacity
```

Shrinking never fails — if the allocator can't provide a smaller block, the collection keeps its current allocation.

### In-Place Construction

**Construct directly in collection storage:**
```rask
const idx = try vec.push_with(|slot| {
    slot.field1 = compute_expensive()
    slot.field2 = [0; 1000]
})
```

Avoids constructing on stack then moving. Useful for large types.

### Linear Resources in Collections

**Linear resource types CANNOT be stored in Vec or Map:**
```rask
let files: Vec<File> = Vec.new()  // COMPILE ERROR
```

**Reason:** Collection drop would call `T.drop()` for each element, but linear resource drop can fail (returns `Result`), and collection drop can't propagate errors.

**Solution:** Use `Pool<Linear>` with explicit consumption. See [pools.md](../memory/pools.md#linear-types-in-pools).

### Slice Descriptors

Slices (`[]T`) are ephemeral fat pointers (ptr + len) that can't be stored. For long-lived slices, use `SliceDescriptor<T>`.

**Problem:** Slices are expression-scoped borrows:
```rask
// COMPILE ERROR: Can't store slice
struct View {
    data: &[u8],  // Slice is not storable
}
```

**Solution:** Store the "recipe" for a slice instead of the slice itself:

```rask
struct SliceDescriptor<T> {
    handle: Handle<T>,    // 8 bytes
    range: Range,         // 8 bytes (start..end)
}
```

**Usage with string or Vec:**
```rask
const strings: Pool<string> = Pool.new()
const s = try strings.insert("Hello World")

// Create storable slice descriptor
const slice_desc = s.slice(0..5)   // SliceDescriptor { handle: s, range: 0..5 }

// Later, access the slice
with strings {
    for i in slice_desc.range {
        const char = slice_desc.handle[i]
    }
}
```

**Properties:**
- Exactly 16 bytes → copyable by value semantics
- Works with frozen pools (zero generation checks)
- Storable in structs, collections, channels
- Bounds checked at access time, not creation

**Methods:**

| Method | Returns | Description |
|--------|---------|-------------|
| `handle.slice(range)` | `SliceDescriptor<T>` | Create descriptor |
| `desc.len()` | `usize` | Length of range |
| `desc.is_empty()` | `bool` | Range is empty |
| `desc.iter()` | Iterator | Iterate (requires ambient pool) |

**When to use:**
- Storing references to substrings or sub-vectors
- Event systems with text ranges
- Undo buffers with slices of document state
- Any place you'd want `[]T` but need to store it

### Capacity Introspection

| Method | Returns | Semantics |
|--------|---------|-----------|
| `vec.len()` | `usize` | Current element count |
| `vec.capacity()` | `Option<usize>` | `None` = unbounded, `Some(n)` = max capacity |
| `vec.is_bounded()` | `bool` | `capacity().is_some()` |
| `vec.remaining()` | `Option<usize>` | `None` = unbounded, `Some(n)` = slots available |
| `vec.allocated()` | `usize` | Current allocation size (may exceed len) |

### Edge Cases

| Case | Handling |
|------|----------|
| `vec[usize.MAX]` | Panic (bounds check) |
| `vec.get(usize.MAX)` | Returns `None` |
| `Vec.fixed(0).push(x)` | Returns `Err(PushError.Full(x))` |
| OOM on unbounded `push()` | Returns `Err(PushError.Alloc(x))` |
| `modify_many([i, i], _)` | Panic (duplicate index) |
| ZST in `Vec<()>` | `len()` tracks count, no storage allocated |
| `Vec<LinearResource>` | Compile error |
| Closure panics in `modify` | Collection left in valid state |

### Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Vec<T>` | if `T: Send` | if `T: Sync` |
| `Map<K,V>` | if `K,V: Send` | if `K,V: Sync` |

### FFI

**Vec ↔ C:**
```rask
vec.as_ptr() -> *const T       // unsafe
vec.as_mut_ptr() -> *mut T     // unsafe
Vec.from_raw_parts(ptr, len, cap) -> Vec<T>  // unsafe
```

### Comptime Collections with Freeze

At compile time, collections use a **compiler-managed allocator** and must be **frozen** to escape comptime as const data. See [comptime.md](../control/comptime.md#comptime-collections-with-freeze) for full details.

**The `.freeze()` method:**

| Collection | `freeze()` Returns | Description |
|------------|-------------------|-------------|
| `Vec<T>` | `[T; N]` | Fixed-size array, size inferred from length |
| `Map<K,V>` | Static map | Perfect hash or similar compile-time representation |
| `string` | `str` | string literal |

**Example:**
```rask
const PRIMES: [u32; _] = comptime {
    const v = Vec<u32>.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }
    }
    v.freeze()
}

const KEYWORDS: Map<str, TokenKind> = comptime {
    const m = Map<str, TokenKind>.new()
    m.insert("if", TokenKind.If)
    m.insert("else", TokenKind.Else)
    m.freeze()
}
```

**Rules:**
- `.freeze()` is only valid in comptime context (compile error at runtime)
- Unfrozen collections cannot escape comptime (compile error)
- Subject to comptime memory limits (256MB total, 16MB per array)
- After freeze, the data is immutable const

**Why freeze?**
- Makes the "materialization" step explicit
- Compiler knows exactly what escapes comptime
- Familiar collection APIs, no new types to learn

## Examples

### Literal Construction

**Vec from array:**
```rask
const items = Vec.from([1, 2, 3, 4, 5])
```

**Map from pairs:**
```rask
const scores = Map.from([["alice", 100], ["bob", 95], ["charlie", 87]])
```

### Web Server Request Buffer
<!-- test: skip -->
```rask
func handle_requests(buffer: Vec<Request>) -> () or Error {
    loop {
        const req = try receive_request()

        match buffer.push(req) {
            Ok(()) => {}
            Err(PushError.Full(rejected)) => {
                try process_batch(buffer)
                buffer.clear()
                try buffer.push(rejected)
            }
            Err(PushError.Alloc(rejected)) => return Err(Error.OutOfMemory)
        }
    }
}
```

### Session Cache with Ensure
```rask
func track_session(cache: Map<SessionId, Session>, id: SessionId) -> () or Error {
    try cache.ensure_modify(id,
        || Session.new(id),
        |s| {
            s.last_seen = now()
            s.request_count += 1
        }
    )
    Ok(())
}
```

## Integration Notes

- **Memory Model:** Collections own their data (value semantics). No lifetime parameters required.
- **Type System:** Generic code works uniformly: `func process<T>(v: Vec<T>)` handles bounded and unbounded transparently.
- **Error Handling:** All allocations return `Result`, composable with `try`. Rejected values are returned for retry/logging.
- **Concurrency:** Collections are not `Sync` by default. Send ownership via channels. Use `Arc<Mutex<Vec<T>>>` for shared mutable access.
- **Compiler:** No whole-program analysis needed. Expression-scoped borrows determined by syntax. Closure borrow checking is local.
- **C Interop:** Use `Vec` for sequential data (FFI-friendly layout).

## See Also

- [Pools](../memory/pools.md) — Handle-based sparse storage for graphs, entity systems
- [Borrowing](../memory/borrowing.md) — One rule: views last as long as the source is stable
- [Iterator Protocol](iterator-protocol.md) — Iterator trait and adapters

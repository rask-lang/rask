# Solution: Collections (Vec and Map)

## The Question
How do growable collections (vectors, hash maps) work? Are they bounded at creation, do they take explicit allocator parameters, or do they grow implicitly?

## Decision
Unified collection types (`Vec<T>`, `Map<K,V>`) with optional capacity constraints set at creation, closure-based access for scoped borrows, and fallible allocation.

For handle-based sparse storage (`Pool<T>`), see [pools.md](../memory/pools.md).

## Rationale
This balances Rask's core constraints: transparent costs (all allocations return `Result`), and local analysis (closures enforce scoped access via existing borrow rules). Capacity is a runtime property, not a type parameter, enabling generic code to work uniformly across bounded and unbounded collections.

## Specification

### Collection Types

| Type | Purpose | Creation |
|------|---------|----------|
| `Vec<T>` | Ordered, indexed | `Vec.new()` (unbounded)<br>`Vec.with_capacity(n)` (bounded)<br>`Vec.fixed(n)` (pre-allocated, bounded) |
| `Map<K,V>` | Key-value associative | `Map.new()` (unbounded)<br>`Map.with_capacity(n)` (bounded) |

**When to use which:**
- `Vec<T>` — Ordered data, access by position, elements don't need stable identity
- `Map<K,V>` — Lookup by arbitrary key, no ordering guarantees
- `Pool<T>` — Elements reference each other (graphs, trees), need stable handles. See [pools.md](../memory/pools.md)

**Capacity semantics:**
- Unbounded: `capacity() == None`, grows indefinitely
- Bounded: `capacity() == Some(n)`, cannot exceed `n` elements
- Fixed: Bounded + pre-allocated at creation

### Allocation - All Fallible

**ALL growth operations return `Result` with rejected value on failure:**

| Operation | Returns | Error Type |
|-----------|---------|------------|
| `vec.push(x)` | `Result<(), PushError<T>>` | `Full(T)` or `Alloc(T)` |
| `vec.extend(iter)` | `Result<(), ExtendError<T>>` | Contains first rejected item |
| `vec.reserve(n)` | `Result<(), AllocError>` | No data to return |
| `map.insert(k, v)` | `Result<Option<V>, InsertError<V>>` | `Full(V)` or `Alloc(V)` |

**Error types:**
```
enum PushError<T> {
    Full(T),   // Bounded collection at capacity
    Alloc(T),  // Allocation failed
}
```

**Convenience methods:**
```
vec.push(x)?                // Propagate error
vec.push(x).unwrap()        // Panic on error
vec.push_or_panic(x)        // Explicit panic variant
```

### Vec - Indexed Access

**Expression-scoped borrows via `[]`:**
```
vec[i].field              // Read field (expression-scoped borrow)
vec[i].field = value      // Mutate field (expression-scoped mutable borrow)
let x = vec[i]            // Copy out (T: Copy only)
```

**Methods for safe access:**

| Method | Returns | Constraint | Panics |
|--------|---------|------------|--------|
| `vec[i]` | `T` | `T: Copy` | Yes (OOB) |
| `vec[i].field` | expression-scoped `&T` | None | Yes (OOB) |
| `vec.get(i)` | `Option<T>` | `T: Copy` | No |
| `vec.get_clone(i)` | `Option<T>` | `T: Clone` | No |
| `vec.read(i, \|v\| R)` | `Option<R>` | None | No |
| `vec.modify(i, \|v\| R)` | `Option<R>` | None | No |

**Closure access (for multi-statement operations):**

```
let name = vec.read(i, |v| v.name.clone())?  // Option<String>
vec.modify(i, |v| v.count += 1)?             // Option<()>
```

**Pattern selection guide:**
- 1 statement → `vec[i].field = x`
- Method chain → `vec[i].value.method().chain()`
- 2+ statements → `vec.modify(i, |v| { ... })?`
- Error propagation → `vec.modify(i, |v| -> Result { ... })?`

### Map - Key-Based Access

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map[k]` | `V` | Panics if missing (V: Copy) |
| `map[k].field` | expression-scoped `&V` | Panics if missing |
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

```
// Ensure user exists, then update
map.ensure(user_id, || User.new(user_id))?
map.modify(user_id, |u| u.last_seen = now())?

// Or combined
map.ensure_modify(user_id, || User.new(user_id), |u| {
    u.last_seen = now()
    u.visit_count += 1
})?
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

**Standard borrowing semantics:**
```
for item in &vec { }      // item: &T
for item in &mut vec { }  // item: &mut T
for item in vec { }       // item: T (consuming)
```

**Conditional removal:**
```
// Remove matching elements (no allocation)
vec.remove_where(|x| x.expired) -> usize  // Returns count

// Remove and collect
vec.drain_where(|x| x.expired) -> Vec<T>  // Allocates

// Retain non-matching
vec.retain(|x| !x.expired)
```

### Shrinking

**Infallible, best-effort:**
```
vec.shrink_to_fit()      // Shrink to len, may keep larger if realloc fails
vec.shrink_to(n)         // Shrink to at least n capacity
```

Shrinking never fails — if the allocator can't provide a smaller block, the collection keeps its current allocation.

### In-Place Construction

**Construct directly in collection storage:**
```
let idx = vec.push_with(|slot| {
    slot.field1 = compute_expensive()
    slot.field2 = [0; 1000]
})?
```

Avoids constructing on stack then moving. Useful for large types.

### Linear Resources in Collections

**Linear types CANNOT be stored in Vec or Map:**
```
let files: Vec<File> = Vec.new()  // COMPILE ERROR
```

**Reason:** Collection drop would need to call `T::drop()` for each element, but linear resource drop can fail (returns `Result`), and collection drop cannot propagate errors.

**Solution:** Use `Pool<Linear>` with explicit consumption. See [pools.md](../memory/pools.md#linear-types-in-pools).

### Slice Descriptors

Slices (`&[T]`) are ephemeral fat pointers (ptr + len) that cannot be stored. For long-lived slices, use `SliceDescriptor<T>`.

**Problem:** Slices are expression-scoped borrows:
```
// COMPILE ERROR: Can't store slice
struct View {
    data: &[u8],  // Slice is not storable
}
```

**Solution:** Store the "recipe" for a slice instead of the slice itself:

```
struct SliceDescriptor<T> {
    handle: Handle<T>,    // 8 bytes
    range: Range,         // 8 bytes (start..end)
}
```

**Usage with String or Vec:**
```
let strings: Pool<String> = Pool::new()
let s = strings.insert("Hello World")?

// Create storable slice descriptor
let slice_desc = s.slice(0..5)   // SliceDescriptor { handle: s, range: 0..5 }

// Later, access the slice
with strings {
    for i in slice_desc.range {
        let char = slice_desc.handle[i]
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
- Any place you'd want `&[T]` but need to store it

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
| `vec[usize::MAX]` | Panic (bounds check) |
| `vec.get(usize::MAX)` | Returns `None` |
| `Vec.fixed(0).push(x)` | Returns `Err(PushError::Full(x))` |
| OOM on unbounded `push()` | Returns `Err(PushError::Alloc(x))` |
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
```
vec.as_ptr() -> *const T       // unsafe
vec.as_mut_ptr() -> *mut T     // unsafe
Vec.from_raw_parts(ptr, len, cap) -> Vec<T>  // unsafe
```

## Examples

### Web Server Request Buffer
```
fn handle_requests(buffer: &mut Vec<Request>) -> Result<(), Error> {
    loop {
        let req = receive_request()?

        match buffer.push(req) {
            Ok(()) => {}
            Err(PushError::Full(rejected)) => {
                process_batch(buffer)?
                buffer.clear()
                buffer.push(rejected)?
            }
            Err(PushError::Alloc(rejected)) => {
                return Err(Error::OutOfMemory)
            }
        }
    }
}
```

### Session Cache with Ensure
```
fn track_session(cache: &mut Map<SessionId, Session>, id: SessionId) -> Result<(), Error> {
    cache.ensure_modify(id,
        || Session.new(id),
        |s| {
            s.last_seen = now()
            s.request_count += 1
        }
    )?
    Ok(())
}
```

## Integration Notes

- **Memory Model:** Collections own their data (value semantics). No lifetime parameters required.
- **Type System:** Generic code works uniformly: `fn process<T>(v: &Vec<T>)` handles bounded and unbounded transparently.
- **Error Handling:** All allocations return `Result`, composable with `?` operator. Rejected values are returned for retry/logging.
- **Concurrency:** Collections are not `Sync` by default. Send ownership via channels. Use `Arc<Mutex<Vec<T>>>` for shared mutable access.
- **Compiler:** No whole-program analysis needed. Expression-scoped borrows determined by syntax. Closure borrow checking is local.
- **C Interop:** Use `Vec` for sequential data (FFI-friendly layout).

## See Also

- [Pools](../memory/pools.md) — Handle-based sparse storage for graphs, entity systems
- [Borrowing](../memory/borrowing.md) — Expression-scoped vs block-scoped borrowing
- [Iterator Protocol](iterator-protocol.md) — Iterator trait and adapters

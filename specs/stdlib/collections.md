<!-- id: std.collections -->
<!-- status: decided -->
<!-- summary: Vec and Map with fallible allocation, expression-scoped borrows, optional capacity bounds -->
<!-- depends: memory/borrowing.md, memory/pools.md, memory/value-semantics.md -->

# Collections (Vec and Map)

Vec and Map with optional capacity constraints, expression-scoped borrows, fallible allocation. For handle-based sparse storage, see `mem.pools`.

## Collection Types

| Rule | Description |
|------|-------------|
| **C1: Value ownership** | Collections own their data. No lifetime parameters |
| **C2: Fallible allocation** | All growth operations return `Result` with rejected value on failure |
| **C3: Expression-scoped borrows** | Element access via `[]` is expression-scoped (released at `;`) |
| **C4: No linear resources** | `Vec<Linear>` and `Map<K, Linear>` are compile errors. Use `Pool<Linear>` |

| Type | Purpose | Creation |
|------|---------|----------|
| `Vec<T>` | Ordered, indexed | `Vec.new()`, `Vec.with_capacity(n)`, `Vec.fixed(n)`, `Vec.from([T; N])` |
| `Map<K,V>` | Key-value associative | `Map.new()`, `Map.with_capacity(n)`, `Map.from([(K,V); N])` |

## Capacity Semantics

| Rule | Description |
|------|-------------|
| **CP1: Unbounded** | `capacity() == None`, grows indefinitely |
| **CP2: Bounded** | `capacity() == Some(n)`, cannot exceed `n` elements |
| **CP3: Fixed** | Bounded + pre-allocated at creation |

## Allocation

All growth operations return `Result` with rejected value on failure (C2).

| Operation | Returns | Error Type |
|-----------|---------|------------|
| `vec.push(x)` | `Result<(), PushError<T>>` | `Full(T)` or `Alloc(T)` |
| `vec.extend(iter)` | `Result<(), ExtendError<T>>` | Contains first rejected item |
| `vec.reserve(n)` | `Result<(), AllocError>` | No data to return |
| `map.insert(k, v)` | `Result<Option<V>, InsertError<V>>` | `Full(V)` or `Alloc(V)` |

<!-- test: skip -->
```rask
enum PushError<T> {
    Full(T),   // Bounded collection at capacity
    Alloc(T),  // Allocation failed
}

try vec.push(x)             // Propagate error
vec.push(x).unwrap()        // Panic on error
vec.push_or_panic(x)        // Explicit panic variant
```

## From Literal Constructors

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `Vec.from(arr)` | `[T; N] -> Vec<T>` | Copy array elements into new Vec |
| `Map.from(pairs)` | `[(K,V); N] -> Map<K,V>` | Build Map from key-value pairs |

Array literals `[...]` already create Vec values, so `Vec.from([1, 2, 3])` is equivalent to `[1, 2, 3]`. The explicit constructor exists for API clarity.

## Vec -- Indexed Access

| Rule | Description |
|------|-------------|
| **V1: Copy out** | `vec[i]` copies T when T: Copy. Panics on OOB |
| **V2: Expression borrow** | `vec[i].field` borrows for expression, released at `;` |
| **V3: Safe get** | `vec.get(i)` returns `Option<T>` (T: Copy), no panic |

| Method | Returns | Constraint | Panics |
|--------|---------|------------|--------|
| `vec[i]` | `T` | `T: Copy` | Yes (OOB) |
| `vec[i].field` | expression-scoped borrow | None | Yes (OOB) |
| `vec.get(i)` | `Option<T>` | `T: Copy` | No |
| `vec.get_clone(i)` | `Option<T>` | `T: Clone` | No |
| `vec.read(i, \|v\| R)` | `Option<R>` | None | No |
| `vec.modify(i, \|v\| R)` | `Option<R>` | None | No |

<!-- test: skip -->
```rask
vec[i].field              // Read field (expression-scoped borrow)
vec[i].field = value      // Mutate field (expression-scoped mutable borrow)
const x = vec[i]          // Copy out (T: Copy only)

const name = try vec.read(i, |v| v.name.clone())  // Option<string>
try vec.modify(i, |v| v.count += 1)               // Option<()>
```

## Map -- Key-Based Access

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map[k]` | `V` | Panics if missing (V: Copy) |
| `map[k].field` | expression-scoped borrow | Panics if missing |
| `map.get(k)` | `Option<V>` | Copy out (V: Copy) |
| `map.get_clone(k)` | `Option<V>` | Clone out (V: Clone) |
| `map.read(k, \|v\| R)` | `Option<R>` | Read if exists |
| `map.modify(k, \|v\| R)` | `Option<R>` | Mutate if exists |
| `map.remove(k)` | `Option<V>` | Remove and return |

### Entry API

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map.ensure(k, \|\| v)` | `Result<(), InsertError>` | Insert if missing, no-op if present |
| `map.ensure_modify(k, \|\| v, \|v\| R)` | `Result<R, InsertError>` | Insert if missing, then mutate |

<!-- test: skip -->
```rask
try map.ensure(user_id, || User.new(user_id))
try map.ensure_modify(user_id, || User.new(user_id), |u| {
    u.last_seen = now()
    u.visit_count += 1
})
```

## Multi-Element Mutation

| Rule | Description |
|------|-------------|
| **D1: Disjoint required** | `modify_many` and `swap` require distinct indices. Panics on duplicates |

| Operation | Signature | Semantics |
|-----------|-----------|-----------|
| `vec.swap(i, j)` | `()` | Swap two indices (panics if equal) |
| `vec.modify_many([i, j, k], \|[a, b, c]\| R)` | `Option<R>` | Mutate multiple (panics if duplicates) |

## Iteration

See `std.iteration` for full iteration spec.

<!-- test: skip -->
```rask
for i in vec { }              // i: usize (index iteration)
for item in vec.iter() { }    // item: borrowed T (ref iteration)
for item in vec.take_all() { } // item: T (consuming iteration)
```

### Conditional Removal

| Method | Returns | Notes |
|--------|---------|-------|
| `vec.remove_where(\|x\| bool)` | `usize` | Remove matching, return count. No allocation |
| `vec.drain_where(\|x\| bool)` | `Vec<T>` | Remove and collect. Allocates |
| `vec.retain(\|x\| bool)` | `()` | Retain non-matching |

## Shrinking

Infallible, best-effort. If the allocator can't provide a smaller block, the collection keeps its current allocation.

<!-- test: skip -->
```rask
vec.shrink_to_fit()      // Shrink to len
vec.shrink_to(n)         // Shrink to at least n capacity
```

## In-Place Construction

<!-- test: skip -->
```rask
const idx = try vec.push_with(|slot| {
    slot.field1 = compute_expensive()
    slot.field2 = [0; 1000]
})
```

Avoids constructing on stack then moving. Useful for large types.

## Slice Descriptors

Slices (`[]T`) are ephemeral fat pointers that can't be stored. `SliceDescriptor<T>` stores the "recipe" instead.

<!-- test: skip -->
```rask
struct SliceDescriptor<T> {
    handle: Handle<T>,    // 8 bytes
    range: Range,         // 8 bytes (start..end)
}
```

| Rule | Description |
|------|-------------|
| **SD1: Copyable** | Exactly 16 bytes, copyable by value semantics |
| **SD2: Storable** | Can be stored in structs, collections, channels |
| **SD3: Lazy bounds** | Bounds checked at access time, not creation |

| Method | Returns | Description |
|--------|---------|-------------|
| `handle.slice(range)` | `SliceDescriptor<T>` | Create descriptor |
| `desc.len()` | `usize` | Length of range |
| `desc.is_empty()` | `bool` | Range is empty |
| `desc.iter()` | Iterator | Iterate (requires ambient pool) |

## Capacity Introspection

| Method | Returns | Semantics |
|--------|---------|-----------|
| `vec.len()` | `usize` | Current element count |
| `vec.capacity()` | `Option<usize>` | `None` = unbounded, `Some(n)` = max capacity |
| `vec.is_bounded()` | `bool` | `capacity().is_some()` |
| `vec.remaining()` | `Option<usize>` | `None` = unbounded, `Some(n)` = slots available |
| `vec.allocated()` | `usize` | Current allocation size (may exceed len) |

## Comptime Collections with Freeze

At compile time, collections use a compiler-managed allocator and must be frozen to escape comptime as const data. See `ctrl.comptime` for full details.

| Collection | `freeze()` Returns | Description |
|------------|-------------------|-------------|
| `Vec<T>` | `[T; N]` | Fixed-size array, size inferred from length |
| `Map<K,V>` | Static map | Perfect hash or similar compile-time representation |
| `string` | `str` | String literal |

| Rule | Description |
|------|-------------|
| **F1: Comptime only** | `.freeze()` is only valid in comptime context |
| **F2: Required to escape** | Unfrozen collections cannot escape comptime |
| **F3: Memory limits** | Subject to comptime memory limits (256MB total, 16MB per array) |
| **F4: Immutable result** | After freeze, the data is immutable const |

<!-- test: skip -->
```rask
const PRIMES: [u32; _] = comptime {
    const v = Vec<u32>.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }
    }
    v.freeze()
}
```

## Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Vec<T>` | if `T: Send` | if `T: Sync` |
| `Map<K,V>` | if `K,V: Send` | if `K,V: Sync` |

## FFI

<!-- test: skip -->
```rask
vec.as_ptr() -> *T             // unsafe (immutable access)
vec.as_mut_ptr() -> *T         // unsafe (mutable access)
Vec.from_raw_parts(ptr, len, cap) -> Vec<T>  // unsafe
```

## Error Messages

```
ERROR [std.collections/C4]: linear resource type in Vec
   |
3  |  let files: Vec<File> = Vec.new()
   |             ^^^^^^^^^ File is a linear resource

WHY: Collection drop calls T.drop() for each element, but linear resource
     drop can fail (returns Result), and collection drop can't propagate errors.

FIX: Use Pool<File> with explicit consumption:

  const pool: Pool<File> = Pool.new()
```

```
ERROR [std.collections/C2]: push failed on bounded collection
   |
5  |  try vec.push(item)
   |       ^^^^^^^^^^^^^^ collection at capacity

WHY: Bounded collections cannot exceed their capacity limit.

FIX: Process existing items first, or use an unbounded collection:

  vec.clear()
  try vec.push(item)
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `vec[usize.MAX]` | V1 | Panic (bounds check) |
| `vec.get(usize.MAX)` | V3 | Returns `None` |
| `Vec.fixed(0).push(x)` | C2 | Returns `Err(PushError.Full(x))` |
| OOM on unbounded `push()` | C2 | Returns `Err(PushError.Alloc(x))` |
| `modify_many([i, i], _)` | D1 | Panic (duplicate index) |
| ZST in `Vec<()>` | — | `len()` tracks count, no storage allocated |
| `Vec<LinearResource>` | C4 | Compile error |
| Closure panics in `modify` | — | Collection left in valid state |

---

## Appendix (non-normative)

### Rationale

**C2 (fallible allocation):** All allocations can fail. Returning the rejected value in the error lets callers retry or log without losing data.

**C3 (expression-scoped):** Collections can grow/shrink, invalidating persistent views. Expression-scoped borrows kill this bug class. See `mem.borrowing/B1`.

**C4 (no linear resources):** Collection drop can't propagate errors from linear resource cleanup. `Pool<T>` with explicit consumption is the right pattern.

### Patterns & Guidance

**When to use which collection:**
- `Vec<T>` — Ordered data, access by position, elements don't need stable identity
- `Map<K,V>` — Lookup by arbitrary key, no ordering guarantees
- `Pool<T>` — Elements reference each other (graphs, trees), need stable handles

**Pattern selection for element access:**
- 1 statement: `vec[i].field = x`
- Method chain: `vec[i].value.method().chain()`
- 2+ statements: `try vec.modify(i, |v| { ... })`
- Error propagation: `try vec.modify(i, |v| -> Result { ... })`

**Slice descriptors — when to use:**
- Storing references to substrings or sub-vectors
- Event systems with text ranges
- Undo buffers with slices of document state

### See Also

- `mem.pools` — Handle-based sparse storage for graphs, entity systems
- `mem.borrowing` — View duration rules
- `std.iteration` — Iterator modes and adapters

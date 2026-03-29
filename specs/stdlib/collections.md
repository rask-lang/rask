<!-- id: std.collections -->
<!-- status: decided -->
<!-- summary: Vec and Map with inline access + `with`, optional capacity bounds, fallible try_ variants -->
<!-- depends: memory/borrowing.md, memory/pools.md, memory/value-semantics.md -->

# Collections (Vec and Map)

Vec and Map with optional capacity constraints, inline element access, fallible allocation. For handle-based sparse storage, see `mem.pools`.

## Collection Types

| Rule | Description |
|------|-------------|
| **C1: Value ownership** | Collections own their data. No lifetime parameters |
| **C2: Panic on alloc failure** | Growth operations (`push`, `insert`, `extend`) panic on OOM. Fallible variants (`try_push`, `try_insert`, `try_extend`) return `Result` with rejected value |
| **C3: Inline access** | Element access via `[]` is inline (expression-scoped). Multi-statement access via `with` |
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

Growth operations panic on failure (C2). Fallible `try_` variants return `Result` with the rejected value for code that needs to handle allocation failure — bounded collections, embedded, or OOM-aware paths.

| Operation | Returns | On failure |
|-----------|---------|------------|
| `vec.push(x)` | `()` | Panics |
| `vec.try_push(x)` | `() or PushError<T>` | Returns `Err(Full(T))` or `Err(Alloc(T))` |
| `vec.extend(iter)` | `()` | Panics |
| `vec.try_extend(iter)` | `() or ExtendError<T>` | Returns first rejected item |
| `vec.reserve(n)` | `()` | Panics |
| `vec.try_reserve(n)` | `() or AllocError` | Returns error |
| `map.insert(k, v)` | `Option<V>` | Panics |
| `map.try_insert(k, v)` | `Option<V> or InsertError<V>` | Returns `Err(Full(V))` or `Err(Alloc(V))` |

<!-- test: parse -->
```rask
enum PushError<T> {
    Full(T),   // Bounded collection at capacity
    Alloc(T),  // Allocation failed
}

vec.push(x)                 // Panics on OOM or full (like Rust/Go)
try vec.try_push(x)         // Propagate error (bounded collections, embedded)
```

## From Literal Constructors

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `Vec.from(arr)` | `[T; N] -> Vec<T>` | Copy array elements into new Vec |
| `Map.from(pairs)` | `[[K,V]; N] -> Map<K,V>` | Build Map from key-value pair arrays |

Array literals `[...]` already create Vec values, so `Vec.from([1, 2, 3])` is equivalent to `[1, 2, 3]`. The explicit constructor exists for API clarity.

**Note:** Key-value pairs for `Map.from()` are represented as 2-element arrays `[key, value]` rather than tuple syntax. Native tuple support may be added in the future. Example:

```rask
const users = Map.from([
    ["alice", User.new("Alice")],
    ["bob", User.new("Bob")],
])
```

## Vec -- Indexed Access

| Rule | Description |
|------|-------------|
| **V1: Copy out** | `vec[i]` copies T when T: Copy. Panics on OOB |
| **V2: Expression borrow** | `vec[i].field` borrows for expression, released at `;` |
| **V3: Safe get** | `vec.get(i)` returns `Option<T>` (T: Copy), no panic |

| Method | Returns | Constraint | Panics |
|--------|---------|------------|--------|
| `vec[i]` | `T` | `T: Copy` | Yes (OOB) |
| `vec[i].field` | inline access (expression-scoped) | None | Yes (OOB) |
| `vec.get(i)` | `Option<T>` | `T: Copy` | No |
| `vec.get_clone(i)` | `Option<T>` | `T: Cloneable` | No |
| `with vec[i] as v { ... }` | block value (mutable) | None | Yes (OOB) |
| `vec.insert(i, x)` | `()` | None | Yes (OOB or alloc) |
| `vec.remove(i)` | `T` | None | Yes (OOB) |

### Positional Insert/Remove

| Rule | Description |
|------|-------------|
| **V4: Insert at index** | `vec.insert(i, x)` inserts before position `i`, shifting later elements right. Panics on `i > len()` or alloc failure |
| **V5: Remove at index** | `vec.remove(i)` removes and returns the element at `i`, shifting later elements left. Panics on `i >= len()` |

<!-- test: skip -->
```rask
vec[i].field              // Read field (inline access)
vec[i].field = value      // Mutate field (in-place)
const x = vec[i]          // Copy out (T: Copy only)

// Multi-statement access (mutable by default)
with vec[i] as v {
    v.count += 1
    v.last_updated = now()
}

// One-liner shorthand
with vec[i] as v: v.count += 1

// Expression context — produces a value
const name = with vec[i] as v { v.name.clone() }
```

## Map Key Constraints

| Rule | Description |
|------|-------------|
| **K1: Float key warning** | `Map<f32, V>` and `Map<f64, V>` produce a compile-time warning. NaN != NaN by IEEE 754, which breaks map lookup invariants — a NaN key can be inserted but never found |

## Map -- Key-Based Access

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map[k]` | `V` | Panics if missing (V: Copy) |
| `map[k].field` | inline access (expression-scoped) | Panics if missing |
| `map.get(k)` | `Option<V>` | Copy out (V: Copy) |
| `map.get_clone(k)` | `Option<V>` | Clone out (V: Cloneable) |
| `with map[k] as v { ... }` | block value (mutable) | Panics if missing |
| `map.remove(k)` | `Option<V>` | Remove and return |

### Entry API

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map.ensure(k, \|\| v)` | `()` | Insert if missing, no-op if present. Panics on alloc failure |
| `map.ensure_modify(k, \|\| v, \|v\| R)` | `R` | Insert if missing, then mutate. Panics on alloc failure |

<!-- test: parse -->
```rask
map.ensure(user_id, || User.new(user_id))
map.ensure_modify(user_id, || User.new(user_id), |u| {
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

<!-- test: parse -->
```rask
for item in vec { }              // item: borrowed T (value iteration, default)
for i in 0..vec.len() { }        // i: usize (index iteration, explicit)
for item in vec.take_all() { }   // item: T (consuming iteration)
```

### Conditional Removal

| Method | Returns | Notes |
|--------|---------|-------|
| `vec.remove_where(\|x\| bool)` | `usize` | Remove matching, return count. No allocation |
| `vec.drain_where(\|x\| bool)` | `Vec<T>` | Remove and collect. Allocates |
| `vec.retain(\|x\| bool)` | `()` | Retain non-matching |

## Sorting

| Rule | Description |
|------|-------------|
| **SO1: Stable by default** | `sort()` preserves relative order of equal elements |
| **SO2: In-place** | Sorting mutates the Vec. No new allocation (may use O(log n) stack) |
| **SO3: Comparable required** | `sort()` requires `T: Comparable`. Custom ordering uses `sort_by` |

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `vec.sort()` | `() -> ()` | Stable sort, `T: Comparable` |
| `vec.sort_by(cmp)` | `(\|T, T\| -> Ordering) -> ()` | Stable sort with custom comparator |
| `vec.sort_by_key(f)` | `(\|T\| -> K) -> ()` where `K: Comparable` | Stable sort by extracted key |

<!-- test: skip -->
```rask
let scores = [3, 1, 4, 1, 5]
scores.sort()
// [1, 1, 3, 4, 5]

let users = get_users()
users.sort_by_key(|u| u.name)
users.sort_by(|a, b| b.score.compare(a.score))  // descending
```

## Vec Convenience Methods

| Method | Signature | Trait Required | Notes |
|--------|-----------|----------------|-------|
| `vec.contains(item)` | `(T) -> bool` | `T: Equal` | Linear scan |
| `vec.first()` | `() -> T?` | `T: Copy` | First element or None |
| `vec.last()` | `() -> T?` | `T: Copy` | Last element or None |
| `vec.reverse()` | `(mutate self)` | None | In-place reversal |
| `vec.dedup()` | `(mutate self)` | `T: Equal` | Remove consecutive duplicates |

<!-- test: skip -->
```rask
let items = [3, 1, 4, 1, 5]
items.contains(4)             // true
items.first()                 // Some(3)
items.last()                  // Some(5)
items.reverse()               // [5, 1, 4, 1, 3]

items.sort()                  // [1, 1, 3, 4, 5]
items.dedup()                 // [1, 3, 4, 5]
```

## Map Convenience Methods

| Method | Returns | Notes |
|--------|---------|-------|
| `map.contains_key(k)` | `bool` | Check key existence without copying value |
| `map.keys()` | expression-scoped iterator | Iterate over keys |
| `map.values()` | expression-scoped iterator | Iterate over values |

<!-- test: parse -->
```rask
const scores = Map.from([["alice", 10], ["bob", 20]])
scores.contains_key("alice")      // true
for name in scores.keys() { println(name) }
for score in scores.values() { println(format("{}", score)) }
```

## Shrinking

Infallible, best-effort. If the allocator can't provide a smaller block, the collection keeps its current allocation.

<!-- test: parse -->
```rask
vec.shrink_to_fit()      // Shrink to len
vec.shrink_to(n)         // Shrink to at least n capacity
```

## In-Place Construction

<!-- test: parse -->
```rask
const idx = vec.push_with(|slot| {
    slot.field1 = compute_expensive()
    slot.field2 = [0; 1000]
})
```

Avoids constructing on stack then moving. Useful for large types.

## Slice Descriptors

Slices (`[]T`) are ephemeral fat pointers that can't be stored. `SliceDescriptor<T>` stores the "recipe" instead.

<!-- test: parse -->
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
| `for x in desc` | Iterator | Iterate (requires ambient pool) |

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

<!-- test: parse -->
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
PANIC [std.collections/C2]: push failed — collection at capacity
   |
5  |  vec.push(item)
   |       ^^^^^^^^^ bounded collection is full

FIX: Use try_push to handle capacity limits:

  match vec.try_push(item) {
      Ok(()) => {},
      Err(PushError.Full(item)) => process_overflow(item),
  }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `vec[usize.MAX]` | V1 | Panic (bounds check) |
| `vec.get(usize.MAX)` | V3 | Returns `None` |
| `Vec.fixed(0).push(x)` | C2 | Panics (capacity 0). Use `try_push` to handle |
| OOM on unbounded `push()` | C2 | Panics. Use `try_push` for OOM-aware code |
| `vec.insert(n, x)` where `n > len()` | V4 | Panic (bounds check) |
| `vec.remove(n)` where `n >= len()` | V5 | Panic (bounds check) |
| `with vec[i] as e1, vec[i] as e2` | D1 | Panic (duplicate index) |
| ZST in `Vec<()>` | — | `len()` tracks count, no storage allocated |
| `Vec<LinearResource>` | C4 | Compile error |
| `Map<f32, V>` or `Map<f64, V>` | K1 | Compile-time warning (NaN breaks lookups) |
| Panic inside `with` | — | Collection left in valid state |
| `sort()` on empty Vec | SO1 | No-op |
| `sort()` where `T: !Comparable` | SO3 | Compile error — use `sort_by` |
| `sort_by` comparator panics | SO2 | Vec left in valid but unspecified order |

---

## Appendix (non-normative)

### Rationale

**C2 (panic on alloc failure):** I considered making all growth operations return `Result` (and did, initially). In practice, 98% of push calls ignored the error — application code can't meaningfully recover from OOM on unbounded collections. Rust's `Vec::push` and Go's `append` both panic on OOM. The `try_` variants exist for the cases that matter: bounded collections, embedded systems, and allocation-aware code. The rejected value is still returned in the error so callers can retry or log without losing data.

**C3 (inline access):** Collections can grow/shrink, invalidating any held views. Inline expression access kills this bug class. Multi-statement access uses `with`. See `mem.borrowing/B2`.

**C4 (no linear resources):** Collection drop can't propagate errors from linear resource cleanup. `Pool<T>` with explicit consumption is the right pattern.

### Patterns & Guidance

**When to use which collection:**
- `Vec<T>` — Ordered data, access by position, elements don't need stable identity
- `Map<K,V>` — Lookup by arbitrary key, no ordering guarantees
- `Pool<T>` — Elements reference each other (graphs, trees), need stable handles

**Pattern selection for element access:**
- 1 statement: `vec[i].field = x`
- Method chain: `vec[i].value.method().chain()`
- 2+ statements: `with vec[i] as v { ... }`
- Error propagation: `with vec[i] as v { try validate(v) }`

**Slice descriptors — when to use:**
- Storing references to substrings or sub-vectors
- Event systems with text ranges
- Undo buffers with slices of document state

### See Also

- `mem.pools` — Handle-based sparse storage for graphs, entity systems
- `mem.borrowing` — View duration rules
- `std.iteration` — Iterator modes and adapters

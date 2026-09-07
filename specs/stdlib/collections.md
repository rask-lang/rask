<!-- id: std.collections -->
<!-- status: decided -->
<!-- summary: Vec and Map with inline access + `with`, optional capacity bounds, fallible try_ variants -->
<!-- depends: memory/borrowing.md, memory/pools.md, memory/value-semantics.md -->

# Collections (Vec, Map and Set)

Vec, Map and Set with optional capacity constraints, inline element access, fallible allocation. For handle-based sparse storage, see `mem.pools`.

## Collection Types

| Rule | Description |
|------|-------------|
| **C1: Value ownership** | Collections own their data. No lifetime parameters |
| **C2: Panic on alloc failure** | Growth operations (`push`, `insert`, `push_all`) panic on OOM. Fallible variants (`try_push`, `try_insert`, `try_push_all`) return `T or E` with the rejected value |
| **C3: Inline access** | Element access via `[]` is inline (expression-scoped). Multi-statement access via `with` |
| **C4: No linear resources** | `Vec<Linear>` and `Map<K, Linear>` are compile errors. Use `Pool<Linear>` |

| Type | Purpose | Creation |
|------|---------|----------|
| `Vec<T>` | Ordered, indexed | `Vec.new()`, `Vec.with_capacity(n)`, `Vec.fixed(n)`, `Vec.from([T; N])` |
| `Map<K,V>` | Key-value associative | `Map.new()`, `Map.with_capacity(n)`, `Map.from([(K,V); N])` |
| `Set<T>` | Unique values | `Set.new()` |

## Set

| Rule | Description |
|------|-------------|
| **C5: Membership is `contains`** | `s.contains(v)`, not `contains_key`. A set has no keys, and `Vec` and `string` already spell it this way. `Map` keeps `contains_key` because there a key is one of two things you could mean |
| **C6: Insert and remove report change** | `s.insert(v)` and `s.remove(v)` return `bool` — whether the set changed. `insert` on a value already present is not an error, it answers `false` |
| **C7: A Map underneath** | `Set<T>` is `Map<T, bool>`, written in Rask, so both backends run one source and a set's hashing, growth and iteration order are the map's. `T` carries the same key constraints (C-key rules below) |
| **C8: `to_vec`, not `iter`** | The values come out as `s.to_vec()`. A stored iterator isn't a thing (SEQ31) — an adapter chain terminates in the expression that starts it — so a set hands back what it built and the name says so |

<!-- test: skip -->
```rask
mut seen: Set<string> = Set.new()
if seen.insert(name) {
    // first time we've seen this one
}
if seen.contains(other) { … }
seen.remove(name)
for v in seen.to_vec() { … }
```

`Set` has been in BI1's always-available list from the start. It resolved as a
builtin and had no methods at all until #1017 — `Set.new()` type-checked and
`s.insert(x)` was "no method `insert` found" three lines later.

## Capacity Semantics

| Rule | Description |
|------|-------------|
| **CP1: Unbounded** | `capacity() == none`, grows indefinitely. `with_capacity(n)` is a pre-allocation hint, not a bound — it stays unbounded |
| **CP2: Bounded** | `capacity()` present with value `n`, cannot exceed `n` elements. `is_bounded()` says which, `is_full()` says whether it's there, `remaining()` says how much room is left (`none` when unbounded) |
| **CP3: Fixed** | Bounded + pre-allocated at creation — `Vec.fixed(n)`. A bound of 0 is legal: the vector is permanently full |

## Allocation

Growth operations panic on failure (C2). Fallible `try_` variants return `T or E` with the rejected value for code that needs to handle allocation failure — bounded collections, embedded, or OOM-aware paths.

| Operation | Returns | On failure |
|-----------|---------|------------|
| `vec.push(x)` | `void` | Panics |
| `vec.try_push(x)` | `void or GrowError<T>` | Returns `GrowError.Full(T)` or `GrowError.NoMemory(T)` |
| `vec.push_all(iter)` | `void` | Panics |
| `vec.try_push_all(iter)` | `void or GrowError<T>` | Returns first rejected item. `push_all`, not `extend` — that word is the declaration keyword |
| `vec.reserve(n)` | `void` | Panics |
| `vec.try_reserve(n)` | `void or ReserveError` | Returns why; nothing was rejected, so nothing comes back |
| `map.insert(k, v)` | `V?` | Panics |
| `map.try_insert(k, v)` | `V? or GrowError<V>` | Returns `GrowError.Full(V)` or `GrowError.NoMemory(V)` |

<!-- test: parse -->
```rask
enum GrowError<T> {
    Full(T),      // Bounded collection at capacity
    NoMemory(T),  // Allocation failed
}

enum ReserveError {
    Full,
    NoMemory,
}

vec.push(x)                 // Panics on OOM or full (like Rust/Go)
try vec.try_push(x)         // Propagate error (bounded collections, embedded)
```

**One name across the family (C2a).** `push`, `push_all` and `insert` fail for the
same two reasons and hand back the same thing, so they share `GrowError<T>`
rather than getting a per-method error type each. A function that grows any
collection can then name its error, and `std.collections` spends one name on the
concept instead of three (`std.api/SD1`, `SD2`). `try_reserve` is the one that
differs in shape — there's no rejected value to return, only a reason — so it
keeps its own type.

## From Literal Constructors

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `Vec.from(arr)` | `[T; N] -> Vec<T>` | Copy array elements into new Vec |
| `Map.from(pairs)` | `[[K,V]; N] -> Map<K,V>` | Build Map from key-value pair arrays |

Array literals `[...]` already create Vec values, so `Vec.from([1, 2, 3])` is equivalent to `[1, 2, 3]`. The explicit constructor exists for API clarity.

| Rule | Description |
|------|-------------|
| **C4: The slot picks the shape** | `[a, b, c]` is a `Vec<T>` where the position it fills says so, a `[T; N]` where that says so, and a `[T; N]` where nothing says anything. One literal, and the destination decides — `let xs: Vec<i64> = [1, 2, 3]` and `let a: [i64; 3] = [1, 2, 3]` are both the literal doing what it was asked |
| **C5: Elements fill their slot** | An element coerces into the element type the same way a struct field's value does, so `[1, none, 3]` is a `Vec<i64?>` or a `[i64?; 3]` and the present ones acquire their tag. An empty literal takes the destination's shape whole |

**Note:** Key-value pairs for `Map.from()` are represented as 2-element arrays `[key, value]` rather than tuple syntax. Native tuple support may be added in the future. Example:

```rask
let users = Map.from([
    ["alice", User.new("Alice")],
    ["bob", User.new("Bob")],
])
```

## Vec -- Indexed Access

| Rule | Description |
|------|-------------|
| **V1: Copy out** | `vec[i]` copies T when T: Copy. Panics on OOB |
| **V2: Expression borrow** | `vec[i].field` borrows for expression, released at `;` |
| **V3: Safe get** | `vec.get(i)` returns `T?` (T: Copy), no panic |
| **V8: Integer index** | The index is **any integer type** (no `as usize`). Checked at compile time — a non-integer index is a compile error (`type.operators/IX1`, `E0819`). The value is range-checked at access; negative or too-large panics, no wraparound |
| **V9: A count is unsigned** | `len()`, `capacity()` and every count answer `usize`. Accumulate them into `usize`/`u64`, not `i64` — that needs a policy (CV1a) and the cast proves nothing |

**V9, and why `len()` isn't signed.** Go, Java and Swift all answer `int` here, and
it is more convenient: any counter type works. It's the wrong trade for Rask. A
length cannot be negative, and a type that says so is information, not ceremony —
throwing it away to save a keystroke is the deal commitment 5 exists to refuse.
It also means `len() - 1` on an empty collection panics (`type.overflow/OV1`)
instead of quietly producing `-1` for someone to index with.

The friction people expect here doesn't materialise, because a count flows into
another count with no conversion at all:

<!-- test: skip -->
```rask
mut total: u64 = 0
for v in batches {
    total += v.len()        // no cast — both sides are counts
}
```

`let n: i64 = v.len()` *is* an error, and correctly: `usize` → `i64` is the one
int→int pair that can genuinely lose a value. Declaring the counter `i64` in the
first place is the bug that error is pointing at.

| Method | Returns | Constraint | Panics |
|--------|---------|------------|--------|
| `vec[i]` | `T` | `T: Copy` | Yes (OOB) |
| `vec[i].field` | inline access (expression-scoped) | none | Yes (OOB) |
| `vec.get(i)` | `T?` | `T: Copy` | No |
| `vec.get_clone(i)` | `T?` | `T: Cloneable` | No |
| `with vec[i] as v { ... }` | block value (mutable) | none | Yes (OOB) |
| `vec.insert(i, x)` | `()` | none | Yes (OOB or alloc) |
| `vec.remove(i)` | `T` | none | Yes (OOB) |
| `vec.remove_unordered(i)` | `T` | none | Yes (OOB) |
| `vec.pop()` | `T?` | none | No |

### Positional Insert/Remove

| Rule | Description |
|------|-------------|
| **V4: Insert at index** | `vec.insert(i, x)` inserts before position `i`, shifting later elements right. Panics on `i > len()` or alloc failure |
| **V5: Remove at index** | `vec.remove(i)` removes and returns the element at `i`, shifting later elements left. Panics on `i >= len()` |
| **V6: Pop last** | `vec.pop()` removes and returns the last element as `T?`. Returns `none` on empty vec |
| **V7: Unordered remove** | `vec.remove_unordered(i)` removes and returns the element at `i` by swapping in the last element — O(1), does not preserve order. Panics on `i >= len()` |

<!-- test: skip -->
```rask
vec[i].field              // Read field (inline access)
vec[i].field = value      // Mutate field (in-place)
let x = vec[i]          // Copy out (T: Copy only)

// Multi-statement access (mutable by default)
with vec[i] as v {
    v.count += 1
    v.last_updated = now()
}

// One-liner shorthand
with vec[i] as v: v.count += 1

// Expression context — produces a value
let name = with vec[i] as v { v.name.clone() }
```

## Map Key Constraints

| Rule | Description |
|------|-------------|
| **K1: Float key warning** | `Map<f32, V>` and `Map<f64, V>` produce a compile-time warning. NaN != NaN by IEEE 754, which breaks map lookup invariants — a NaN key can be inserted but never found |
| **K2: Key-typed index** | `map[k]` is checked against `K` at compile time — a wrong key type is a compile error (`type.operators/IX2`, `E0819`). An unsuffixed integer literal adapts to an integer `K` |

## Map -- Key-Based Access

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map[k]` | `V` | Panics if missing (V: Copy) |
| `map[k].field` | inline access (expression-scoped) | Panics if missing |
| `map.get(k)` | `V?` | Copy out (V: Copy) |
| `map.get_clone(k)` | `V?` | Clone out (V: Cloneable) |
| `with map[k] as v { ... }` | block value (mutable) | Panics if missing |
| `map.remove(k)` | `V?` | Remove and return |

### Entry API

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map.insert_if_missing(k, \|\| v)` | `()` | Insert if missing, no-op if present. Panics on alloc failure |
| `map.modify_with_default(k, \|\| v, \|v\| R)` | `R` | Insert default if missing, then mutate. One hash lookup. Panics on alloc failure |

Named for what they do — `ensure` is taken by the cleanup keyword (`ctrl.ensure`) and means something else.

<!-- test: parse -->
```rask
map.insert_if_missing(user_id, || User.new(user_id))
map.modify_with_default(user_id, || User.new(user_id), |u| {
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
| `vec.modify_many([i, j, k], \|[a, b, c]\| R)` | `R?` | Mutate multiple (panics if duplicates) |

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
| `vec.take_where(\|x\| bool)` | `Vec<T>` | Remove matching and return them. Allocates |

Two forms: count or collect. There is no `retain` — invert the predicate on `remove_where`.

## Sorting

| Rule | Description |
|------|-------------|
| **SO1: Stable by default** | `sort()` preserves relative order of equal elements |
| **SO2: In-place** | Sorting mutates the Vec. No new allocation (may use O(log n) stack) |
| **SO3: Comparable required** | `sort()` requires `T: Comparable`. Custom ordering uses `sort_by` |

`sort_by` and `sort_by_key` are where the guarantee earns its keep: a comparator can look at part of an element and a key extractor does by definition, so two that tie can differ in every other field and their order *is* observable. `sort_by` is backed by a merge sort; `sort_by_key` compares extracted keys and only ever swaps a pair the comparison calls strictly less, so equal keys never cross.

`sort` hands off to the platform sort, which is faster and needs no scratch buffer. That's sound wherever `Comparable` compares whole elements — two that tie are then indistinguishable and there is nothing for stability to preserve — and it isn't where a hand-written `compare` ignores a field. See #942.

| Method | Signature | Semantics |
|--------|-----------|-----------|
| `vec.sort()` | `() -> void` | Stable sort, `T: Comparable` |
| `vec.sort_by(cmp)` | `(\|T, T\| -> Ordering) -> void` | Stable sort with custom comparator |
| `vec.sort_by_key(f)` | `(\|T\| -> K) -> void` where `K: Comparable` | Stable sort by extracted key |

<!-- test: skip -->
```rask
mut scores = [3, 1, 4, 1, 5]
scores.sort()
// [1, 1, 3, 4, 5]

mut users = get_users()
users.sort_by_key(|u| u.name)
users.sort_by(|a, b| b.score.compare(a.score))  // descending
```

## Vec Convenience Methods

| Method | Signature | Trait Required | Notes |
|--------|-----------|----------------|-------|
| `vec.contains(item)` | `(T) -> bool` | `T: Equal` | Linear scan |
| `vec.first()` | `() -> T?` | `T: Copy` | First element or `none`. On a `Vec<T?>` the result is `T??` — outer layer says "vec was empty", inner says "slot was empty" (`type.optionals/OPT28`) |
| `vec.last()` | `() -> T?` | `T: Copy` | Last element or `none`. Same layering on a `Vec<T?>` |
| `vec.reverse()` | `(mutate self)` | none | In-place reversal |
| `vec.remove_adjacent_duplicates()` | `(mutate self)` | `T: Equal` | The name says the limitation: only runs of equal neighbors collapse. Sort first for full dedup |

<!-- test: skip -->
```rask
mut items = [3, 1, 4, 1, 5]
items.contains(4)             // true
items.first()                 // 3
items.last()                  // 5
items.reverse()               // [5, 1, 4, 1, 3]

items.sort()                            // [1, 1, 3, 4, 5]
items.remove_adjacent_duplicates()      // [1, 3, 4, 5]
```

## Map Convenience Methods

| Method | Returns | Notes |
|--------|---------|-------|
| `map.contains_key(k)` | `bool` | Check key existence without copying value |
| `map.keys()` | expression-scoped iterator | Iterate over keys |
| `map.values()` | expression-scoped iterator | Iterate over values |

<!-- test: parse -->
```rask
let scores = Map.from([["alice", 10], ["bob", 20]])
scores.contains_key("alice")      // true
for name in scores.keys() { println(name) }
for score in scores.values() { println(format("{}", score)) }
```

**Iteration order is unspecified and seeded per process** — don't depend on it. The hash seed varies between production runs (and across sim seeds, so order-dependent code fails under test rather than in production). Need a stable order? Sort explicitly: `map.keys().to_vec().sort()`. See `determinism/D7`. `{m:debug}` is the one place that can't ask you to sort, so it sorts by key itself (`std.fmt/G5`).

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
let idx = vec.push_with(|slot| {
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
| `for x in desc` | `Sequence<T>` | Iterate (requires ambient pool) |

## Capacity Introspection

| Method | Returns | Semantics |
|--------|---------|-----------|
| `vec.len()` | `usize` | Current element count |
| `vec.capacity()` | `usize?` | `none` = unbounded, value = max capacity |
| `vec.is_bounded()` | `bool` | `capacity()?` |
| `vec.remaining()` | `usize?` | `none` = unbounded, value = slots available |
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
let PRIMES: [u32; _] = comptime {
    let v = Vec<u32>.new()
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
   |               ^^^^^^^^^ File is a linear resource

WHY: Collection drop calls T.drop() for each element, but linear resource
     drop can fail (returns an error type), and collection drop can't propagate errors.

FIX: Use Pool<File> with explicit consumption:

  let pool: Pool<File> = Pool.new()
```

```
PANIC [std.collections/C2]: push failed — collection at capacity
   |
5  |  vec.push(item)
   |       ^^^^^^^^^ bounded collection is full

FIX: Use try_push to handle capacity limits:

  if vec.try_push(item) is GrowError.Full(overflow) {
      process_overflow(overflow)
  }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `vec[usize.MAX]` | V1 | Panic (bounds check) |
| `vec.get(usize.MAX)` | V3 | Returns `none` |
| `Vec.fixed(0).push(x)` | C2 | Panics (capacity 0). Use `try_push` to handle |
| OOM on unbounded `push()` | C2 | Panics. Use `try_push` for OOM-aware code |
| `vec.insert(n, x)` where `n > len()` | V4 | Panic (bounds check) |
| `vec.remove(n)` where `n >= len()` | V5 | Panic (bounds check) |
| `with vec[i] as e1, vec[i] as e2` | D1 | Panic (duplicate index) |
| ZST in `Vec<void>` | — | `len()` tracks count, no storage allocated |
| `Vec<LinearResource>` | C4 | Compile error |
| `Map<f32, V>` or `Map<f64, V>` | K1 | Compile-time warning (NaN breaks lookups) |
| Panic inside `with` | — | Collection left in valid state |
| `sort()` on empty Vec | SO1 | No-op |
| `sort()` where `T: !Comparable` | SO3 | Compile error — use `sort_by` |
| `sort_by` comparator panics | SO2 | Vec left in valid but unspecified order |

---

## Appendix (non-normative)

### Rationale

**C2 (panic on alloc failure):** I considered making all growth operations fallible (and did, initially). In practice, 98% of push calls ignored the error — application code can't meaningfully recover from OOM on unbounded collections. Rust's `Vec::push` and Go's `append` both panic on OOM. The `try_` variants exist for the cases that matter: bounded collections, embedded systems, and allocation-aware code. The rejected value is still returned in the error so callers can retry or log without losing data.

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
- `std.iteration` — Collection iteration modes
- `type.sequence` — Sequence protocol, adapters, terminals

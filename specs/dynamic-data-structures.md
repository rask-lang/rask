# Solution: Dynamic Data Structures

## The Question
How do growable collections (vectors, hash maps) work? Are they bounded at creation, do they take explicit allocator parameters, or do they grow implicitly?

## Decision
Unified collection types (`Vec<T>`, `Pool<T>`, `Map<K,V>`) with optional capacity constraints set at creation, closure-based access for scoped borrows, and runtime pool identity checking for handle safety.

## Rationale
This balances Rask's core constraints: no lifetime parameters (handles use runtime pool IDs), transparent costs (all allocations return `Result`), and local analysis (closures enforce scoped access via existing borrow rules). Capacity is a runtime property, not a type parameter, enabling generic code to work uniformly across bounded and unbounded collections.

## Specification

### Collection Types

| Type | Purpose | Creation |
|------|---------|----------|
| `Vec<T>` | Ordered, indexed | `Vec.new()` (unbounded)<br>`Vec.with_capacity(n)` (bounded)<br>`Vec.fixed(n)` (pre-allocated, bounded) |
| `Pool<T>` | Handle-based sparse storage | `Pool.new()` (unbounded)<br>`Pool.with_capacity(n)` (bounded) |
| `Map<K,V>` | Key-value associative | `Map.new()` (unbounded)<br>`Map.with_capacity(n)` (bounded) |

**When to use which:**
- `Vec<T>` — Ordered data, access by position, elements don't need stable identity
- `Pool<T>` — Elements reference each other (graphs, trees), need stable handles across mutations
- `Map<K,V>` — Lookup by arbitrary key, no ordering guarantees

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
| `pool.insert(x)` | `Result<Handle<T>, InsertError<T>>` | `Full(T)` or `Alloc(T)` |
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

### Indexed Access (Vec)

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

**Closure access (canonical for multi-statement operations):**

Option wraps the *result*, not the access. Inside the closure, you have a valid reference:
```
let name = vec.read(i, |v| v.name.clone())?  // Option<String>
vec.modify(i, |v| v.count += 1)?             // Option<()>
```

**Why closures?** Expression-scoped borrows release at semicolon. Closures enable multi-statement access:
```
// ❌ Cannot name the borrow:
let item = vec[i]   // ERROR: borrow released at semicolon
item.field = x

// ✅ Use closure instead:
vec.modify(i, |item| {
    item.field = x
    item.other = y
})?
```

**Pattern selection guide:**
- 1 statement → `vec[i].field = x`
- Method chain → `vec[i].value.method().chain()`
- 2+ statements → `vec.modify(i, |v| { ... })?`
- Error propagation → `vec.modify(i, |v| -> Result { ... })?`

See [Memory Model](memory-model.md#multi-statement-collection-access) for borrowing semantics.

### Handle-Based Access (Pool)

**Handles are opaque identifiers with configurable sizes:**
```
Pool<T, PoolId=u32, Index=u32, Gen=u64>  // Defaults

Handle<T> = {
    pool_id: PoolId,   // Unique per pool instance
    index: Index,      // Slot in internal storage
    generation: Gen,   // Version counter
}
```

**Handle size** = `sizeof(PoolId) + sizeof(Index) + sizeof(Gen)`

Default: `4 + 4 + 8 = 16 bytes` (exactly at copy threshold).

**Common configurations:**
| Config | Size | Pools | Slots | Gens | Use Case |
|--------|------|-------|-------|------|----------|
| `Pool<T>` | 16 bytes | 4B | 4B | ∞ | General purpose |
| `Pool<T, Gen=u32>` | 12 bytes | 4B | 4B | 4B | Smaller handles |
| `Pool<T, PoolId=u16, Index=u16, Gen=u32>` | 8 bytes | 64K | 64K | 4B | Memory-constrained |

**Copy rule:** Handle is Copy if total size ≤ 16 bytes
```

**Access via handle:**

| Method | Returns | Semantics |
|--------|---------|-----------|
| `pool[h].field` | expression-scoped `&T` | Panics if invalid |
| `pool[h].field = x` | expression-scoped `&mut T` | Panics if invalid |
| `pool.get(h)` | `Option<T>` | Copy out (T: Copy) |
| `pool.get_clone(h)` | `Option<T>` | Clone out (T: Clone) |
| `pool.read(h, \|v\| R)` | `Option<R>` | Read in place, return result |
| `pool.modify(h, \|v\| R)` | `Option<R>` | Mutate in place, return result |
| `pool.remove(h)` | `Option<T>` | Remove and return ownership |

**Handle validation:**
- Wrong `pool_id`: returns `None` (runtime check)
- Stale `generation`: returns `None` (slot was removed/reused)
- Invalid `index`: returns `None` (out of bounds)

**Generation overflow:** Saturating. When a slot's generation reaches `u32::MAX`, the slot becomes permanently unusable (always returns `None`). No panic, no runtime check on every removal — just stops being reusable.

For high-churn scenarios: `Pool<T, u64>` uses 64-bit generations (~18 quintillion cycles per slot).

### Map Access

**Key-based lookup:**

| Method | Returns | Semantics |
|--------|---------|-----------|
| `map[k]` | `V` | Panics if missing (T: Copy) |
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
| `pool.modify_many([h1, h2], \|[a, b]\| R)` | `Option<R>` | Mutate multiple handles (panics if duplicates) |

**Disjointness enforcement:**
- Runtime check for distinctness before entering closure
- Panics if duplicates detected (visible cost, like bounds check)

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

**For pools:**
```
for (handle, item) in &pool { }  // handle: Handle<T>, item: &T
pool.handles() -> Iterator<Handle<T>>
```

### Shrinking

**Infallible, best-effort:**
```
vec.shrink_to_fit()      // Shrink to len, may keep larger if realloc fails
vec.shrink_to(n)         // Shrink to at least n capacity
pool.shrink_to_fit()     // Compact internal storage
```

Shrinking never fails — if the allocator can't provide a smaller block, the collection keeps its current allocation.

### In-Place Construction

**Construct directly in collection storage:**
```
let h = pool.insert_with(|slot| {
    slot.field1 = compute_expensive()
    slot.field2 = [0; 1000]
})?
```

Avoids constructing on stack then moving. Useful for large types.

### Linear Resources

**FORBIDDEN in `Vec<T>`:**
```
let files: Vec<File> = Vec.new()  // COMPILE ERROR
```

**Linear types cannot be collection elements because:**
- `Vec<T>` drop would need to call `T::drop()` for each element
- Linear resource drop can fail (returns `Result`)
- Collection drop cannot propagate errors

**Use `Pool<Linear>` with explicit consumption:**
```
let files: Pool<File> = Pool.new()
// ...
for h in files.handles() {
    let file = files.remove(h).unwrap()
    file.close()?  // Explicit close, error propagates
}
```

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
| Stale handle access | Returns `None` (generation mismatch) |
| Wrong-pool handle | Returns `None` (pool_id mismatch) |
| `modify_many([i, i], _)` | Panic (duplicate index) |
| ZST in `Vec<()>` | `len()` tracks count, no storage allocated |
| `Vec<LinearResource>` | Compile error |
| `Pool<LinearResource>` | Allowed, must explicitly consume each |
| Closure panics in `modify` | Collection left in valid state |
| Generation overflow | Slot becomes permanently dead (saturates at max) |
| Pool ID overflow | Panic (runtime error) |

### Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Vec<T>` | if `T: Send` | if `T: Sync` |
| `Pool<T>` | if `T: Send` | if `T: Sync` |
| `Map<K,V>` | if `K,V: Send` | if `K,V: Sync` |
| `Handle<T>` | Yes (Copy) | Yes |

### FFI

**Vec ↔ C:**
```
vec.as_ptr() -> *const T       // unsafe
vec.as_mut_ptr() -> *mut T     // unsafe
Vec.from_raw_parts(ptr, len, cap) -> Vec<T>  // unsafe
```

**Handle ↔ C:**
```
handle.to_raw() -> (u32, u32, u32)  // (pool_id, index, generation)
Handle.from_raw(pool_id, index, gen) -> Handle<T>  // unsafe
```

**Pool → C:**
```
pool.to_vec() -> Vec<T>  // O(n), allocates, consumes pool
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

### Graph with Handles
```
struct Node {
    data: String,
    edges: Vec<Handle<Node>>,
}

fn build_graph() -> Result<Pool<Node>, Error> {
    let nodes = Pool.new()

    let a = nodes.insert(Node { data: "A", edges: Vec.new() })?
    let b = nodes.insert(Node { data: "B", edges: Vec.new() })?
    let c = nodes.insert(Node { data: "C", edges: Vec.new() })?

    // Add edges using expression-scoped mutation
    nodes[a].edges.push(b)?
    nodes[a].edges.push(c)?
    nodes[b].edges.push(c)?

    Ok(nodes)
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

### Conditional Cleanup
```
fn cleanup_expired(pool: &mut Pool<User>) -> usize {
    pool.remove_where(|user| user.expired())
}
```

## Integration Notes

- **Memory Model**: Collections own their data (value semantics). Handles are not references, so no lifetime parameters required.
- **Type System**: Generic code works uniformly: `fn process<T>(v: &Vec<T>)` handles bounded and unbounded transparently.
- **Error Handling**: All allocations return `Result`, composable with `?` operator. Rejected values are returned for retry/logging.
- **Concurrency**: Collections are not `Sync` by default. Send ownership via channels. Use `Arc<Mutex<Vec<T>>>` for shared mutable access.
- **Compiler**: No whole-program analysis needed. Expression-scoped borrows determined by syntax. Closure borrow checking is local. Handle validation is runtime O(1) comparison.
- **C Interop**: Use `Vec` for sequential data (FFI-friendly layout). Convert `Pool` to `Vec` for C boundaries. Handles cannot cross FFI safely (contain runtime IDs).

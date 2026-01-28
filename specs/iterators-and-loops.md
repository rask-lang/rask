# Solution: Iterators and Loops

## The Question
How can collections be iterated when borrows cannot be stored, without lifetime parameters, while maintaining ergonomics comparable to Go?

## Decision
Loops yield indices/handles (never borrowed values). Access uses existing collection borrow rules. Value extraction follows the 16-byte Copy threshold. Ownership transfer uses explicit `consume()`.

## Rationale
Index-based iteration eliminates the need for stored references while preserving Rask's core constraints: no lifetime annotations, local analysis only, transparent costs. The existing expression-scoped collection access rules extend naturally to loop bodies without new concepts. Copy extraction for small types (≤16 bytes) matches assignment semantics, while explicit `clone()` makes large copies visible.

## Specification

### Loop Syntax

```
for <binding> in <collection> { ... }
```

| Collection Type | Binding Type | Semantics |
|----------------|--------------|-----------|
| `Vec<T>` | `usize` | Index into vec |
| `Pool<T>` | `Handle<T>` | Generational handle |
| `Map<K,V>` | `K` (requires K: Copy) | Key (copied) |
| `Range` (`0..n`) | Integer | Range value |

**No mode annotations.** Loop variables are always owned indices/handles (Copy types).

### Loop Borrowing Semantics

**Core Rule:** `for i in collection` does NOT borrow the collection. The loop variable receives a Copy value (index or handle), and the collection remains accessible within the loop body.

| Loop Syntax | Ownership Transfer | Collection Access Inside Loop |
|-------------|-------------------|------------------------------|
| `for i in vec` | **NO** | ✅ Allowed: `vec[i]`, `vec.push()`, etc. |
| `for h in pool` | **NO** | ✅ Allowed: `pool[h]`, `pool.remove()`, etc. |
| `for k in map` | **NO** | ✅ Allowed: `map[k]`, `map.insert()`, etc. |
| `for item in vec.consume()` | **YES** | ❌ Forbidden: consume consumed vec |

**Desugaring:**
```
// Index iteration (Vec, Pool, Map):
for i in vec { body }
// Equivalent to:
{
    let _len = vec.len();
    let _pos = 0;
    while _pos < _len {
        let i = _pos;
        body
        _pos += 1;
    }
}

// Consume iteration (consuming):
for item in vec.consume() { body }
// Equivalent to:
{
    let mut _consumer = vec.consume();  // Takes ownership, vec now empty
    while let Some(item) = _consumer.next() {
        body
    }
    // _consumer drops here, dropping any remaining items
}
```

**Why no borrow?**
- Indices are Copy values, not references
- Each `vec[i]` access is independent (expression-scoped)
- Enables mutation patterns: `for i in vec { vec[i].field = x }`
- Local analysis only — no loop-level borrow tracking
- Same semantics as Go, C, Zig

**Implication:** Collection length captured at loop start. Mutations during iteration may invalidate indices (programmer responsibility).

### Collection Iteration Modes

Collections support multiple iteration modes depending on access needs:

| Collection | Index/Handle Mode | Ref Mode | Consume Mode |
|------------|-------------------|----------|--------------|
| `Vec<T>` | `for i in vec` → `usize` | N/A | `for item in vec.consume()` → `T` |
| `Pool<T>` | `for h in pool` → `Handle<T>` | `for (h, item) in &pool` → `(Handle<T>, &T)` | `for item in pool.consume()` → `T` |
| `Map<K,V>` | `for k in map` → `K` (K: Copy) | `for (k, v) in &map` → `(&K, &V)` | `for (k,v) in map.consume()` → `(K, V)` |

**Vec iteration:**
- Index mode only (ref mode not needed—indices are cheap)
- Consume mode for ownership transfer

**Pool iteration:**
- Handle mode for mutations/removals
- Ref mode for ergonomic read access (both handle AND data)
- Consume mode for consumption

**Map iteration:**
- Key mode for Copy keys only
- Ref mode for all key types (no cloning)
- Consume mode for consumption

**Ref mode semantics:**

```
// Pool ref iteration:
for (h, entity) in &pool {
    print(h, entity.position);  // Both handle and data available
    pool.remove(h);              // ERROR: cannot mutate during ref iteration
}

// Equivalent to:
for h in pool.handles() {
    let entity = &pool[h];      // Expression-scoped borrow
    print(h, entity.position);
    // Mutation forbidden here - see enforcement below
}  // Borrow released, but still in ref iteration block
```

#### Ref Mode Mutation Prevention

**Enforcement Mechanism:** Compile-time syntactic analysis within the ref loop block.

**Rule:** The compiler tracks that a collection is being ref-iterated and forbids all mutation operations within that `for` loop's body, even between iterations when no expression-scoped borrow is active.

**What's Forbidden:**

| Operation | In Ref Mode Loop | Error |
|-----------|------------------|-------|
| `pool.remove(h)` | ❌ Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool.insert(item)` | ❌ Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool.clear()` | ❌ Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool[h].field = x` | ✅ Allowed | Mutates item in place, doesn't invalidate iteration |
| `other_pool.remove(h2)` | ✅ Allowed | Different collection |

**Why This Works:**

1. **No stored borrow:** The loop doesn't store a borrow variable that lives across iterations
2. **Expression-scoped access:** Each `entity` variable is expression-scoped (ends at statement boundary)
3. **Compile-time analysis:** The compiler syntactically identifies `for ... in &collection` and marks the collection as "ref-iterated" within that block scope
4. **Local analysis only:** Analysis is limited to the lexical scope of the loop body—no cross-function tracking needed

**Comparison with Handle Mode:**

```
// Handle mode: mutations allowed (programmer responsibility)
for h in pool {
    if pool[h].expired {
        pool.remove(h);  // ✅ Allowed - but may invalidate other handles
    }
}

// Ref mode: mutations forbidden (compiler prevents invalidation)
for (h, entity) in &pool {
    if entity.expired {
        pool.remove(h);  // ❌ Compile error
    }
}
```

**Design Rationale:**

- Handle mode: Maximum flexibility, programmer ensures safety
- Ref mode: Convenient read access with compiler-enforced safety
- No actual borrow tracking across iterations needed (local analysis)
- Mutation prevention is syntactic (based on loop form), not semantic (based on runtime borrow state)

**Ref mode borrows:** Despite the `&` syntax, ref mode does NOT create a collection-level borrow variable that lives across iterations. Each iteration creates fresh expression-scoped refs. However, the compiler forbids collection mutations syntactically within the ref loop block to prevent invalidation.

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Index/Handle | Need to mutate or remove during iteration |
| Ref | Read-only access, avoid cloning large items |
| Consume | Consuming all items, transferring ownership |

### Value Access

Access follows existing expression-scoped collection rules:

| Expression | Behavior | Constraint |
|------------|----------|------------|
| `vec[i]` where T: Copy (≤16 bytes) | Copies out T | T: Copy |
| `vec[i].field` where field: Copy | Copies out field | field: Copy |
| `vec[i].method()` | Borrows for call, releases at `;` | Expression-scoped |
| `&vec[i]` passed to function | Borrows for call duration | Cannot store in callee |
| `vec[i] = value` | Mutates in place | - |
| `vec[i]` where T: !Copy | **ERROR**: cannot move | Use `.clone()` or `.consume()` |

**Rule:** Each `collection[index]` access is independent. Borrow released at statement end (semicolon).

### Examples: Common Patterns

**Copy types (≤16 bytes):**
```
for i in ids {
    let id = ids[i];  // i32 copies implicitly
    process(id);
}
```

**Non-Copy types (require clone):**
```
for i in users {
    if users[i].is_admin {
        return users[i].clone();  // Explicit clone
    }
}
```

**Read-only processing (no clone needed):**
```
for i in documents {
    print(&documents[i].title);  // Borrows for call
}
```

**In-place mutation:**
```
for i in users {
    users[i].login_count += 1;  // Mutate directly
}
```

**Nested loops:**
```
for i in vec {
    for j in vec {
        compare(&vec[i], &vec[j]);  // Both borrows valid
    }
}
```

**Closures (capture index or cloned value):**
```
for i in events {
    let event_id = events[i].id;  // Copy field
    handlers.push(|| handle(event_id));  // Captures id
}

// Or clone for non-Copy:
for i in events {
    let event = events[i].clone();
    handlers.push(|| handle(event));  // Captures clone
}
```

### Consume: Consuming Iteration

**Syntax:** `collection.consume()`

Yields owned values, consuming the collection:

```
for item in vec.consume() {
    process(item);  // item is owned T
}
// vec is now empty
```

| Collection | Method | Yields | Returns |
|------------|--------|--------|---------|
| `Vec<T>` | `.consume()` | `T` | `VecConsume<T>` |
| `Pool<T>` | `.consume()` | `T` | `PoolConsume<T>` |
| `Map<K,V>` | `.consume()` | `(K, V)` | `MapConsume<K,V>` |

#### Consume Implementation: Ownership Transfer, Not Borrowing

**Design Principle:** Consume does NOT violate "no storable references." The consume iterator owns the collection's internal data; the original collection is left empty.

**Vec<T> Consume Mechanics:**

```
// Conceptual implementation:
fn Vec<T>.consume(self) -> VecConsume<T> {
    let buffer = self._take_buffer();  // Transfers ownership of internal buffer
    // self is now: ptr=null, len=0, cap=0 (valid empty state)
    VecConsume {
        buffer: buffer,      // Owns the data (not a reference!)
        position: 0,
        end: buffer.len,
    }
}
```

**Key Properties:**
1. `.consume()` takes ownership of collection (`self`, not `&mut self`)
2. Collection's internal buffer transferred to consume iterator
3. Original collection left in valid empty state (len=0, no allocation)
4. Consume iterator owns the data—no stored reference to another value
5. When consume iterator drops, remaining items dropped in LIFO order

**Type Signatures:**

| Method | Signature |
|--------|-----------|
| `Vec<T>.consume()` | `fn(self) -> VecConsume<T>` |
| `Pool<T>.consume()` | `fn(self) -> PoolConsume<T>` |
| `Map<K,V>.consume()` | `fn(self) -> MapConsume<K,V>` |

**Consume Iterator Interface:**

Each consume iterator implements:
```
trait Iterator<T> {
    fn next(&mut self) -> Option<T>
}
```

Calling `.next()` mutates iterator state (position) but does NOT access the original collection (which is now empty).

**Storage Rules:**

| Pattern | Legal | Reason |
|---------|-------|--------|
| `for item in vec.consume() { ... }` | ✅ Yes | Inline consumption |
| `let consumer = vec.consume()` | ✅ Yes | Consumer owns data |
| `consumers.push(vec.consume())` | ✅ Yes | Can store (no reference) |
| `vec.consume(); vec.push(x)` | ❌ No | vec was consumed |

**Why This Works:**

The consume iterator is NOT a reference—it's a value that owns data. Similar to how a `Vec<T>` owns its buffer, a `VecConsume<T>` owns its buffer. No references stored, no lifetimes needed.

**Comparison:**

| Concept | Violates "No Storable Refs"? | Reason |
|---------|------------------------------|--------|
| Iterator storing `&Vec` | ❌ YES | Stores reference to another value |
| VecConsume owning buffer | ✅ NO | Owns data, not a reference |
| Vec owning buffer | ✅ NO | Owns data, not a reference |

**Early Exit and Drop Semantics:**

```
for file in files.consume() {
    if file.is_locked() {
        break;  // Remaining files DROPPED
    }
    file.close()?;
}
```

When the loop exits (break, return, `?`):
1. Current iteration's `file` variable drops normally
2. Consume iterator (`VecConsume`) drops
3. Destructor iterates remaining items, dropping each in LIFO order
4. Original collection remains empty (was already consumed)

**Compiler Requirements:**
- Consume iterator MUST drop remaining items in LIFO order in its destructor
- Compiler MUST prevent use of original collection after `.consume()` (moved)

**IDE Requirements:**
- IDE SHOULD warn on early exit: `break /* drops N remaining items */`
- IDE SHOULD show ghost annotation on `.consume()` call: `/* consumes collection */`

### Linear Types

**Index iteration is forbidden for `Vec<Linear>`:**

```
// COMPILE ERROR:
for i in files {
    files[i].close()?;  // ERROR: cannot move out of index
}

// Required:
for file in files.consume() {
    file.close()?;
}
```

**Error message:** "cannot iterate `Vec<{Linear}>` by index; use `.consume()` to consume"

**Pool iteration works** (handles are Copy):
```
// Handle mode: allows mutations/removals
for h in pool {
    pool.remove(h)?.close()?;  // Explicit remove + consume
}

// Ref mode: ergonomic read access
for (h, entity) in &pool {
    print(h, entity.name);  // No cloning needed
}
```

### Map Iteration

**Key mode requires Copy keys:**

```
// OK: u64 is Copy
let counts: Map<u64, u32> = ...;
for id in counts {
    print(counts[id]);
}

// ERROR: string is not Copy
let config: Map<string, string> = ...;
for key in config {  // ERROR: string is not Copy
    print(config[key]);
}
```

**Ref mode works for all key types:**
```
// OK: no cloning needed
for (key, value) in &config {
    print(key, value);
}
```

**Consume for ownership transfer:**
```
for (key, value) in config.consume() {
    process_owned(key, value);
}
```

### Mutation During Iteration

**Allowed:** Because index iteration does NOT borrow the collection, mutations are permitted but are **programmer responsibility**.

| Pattern | Safety | Notes |
|---------|--------|-------|
| `for i in vec { vec[i].field = x }` | ✅ Safe | In-place mutation doesn't invalidate index |
| `for i in vec { vec.push(x)? }` | ⚠️ Unsafe | New elements not visited; original length captured |
| `for i in vec { vec.swap_remove(i) }` | ⚠️ Unsafe | Later indices refer to wrong elements |
| `for i in vec { vec.clear() }` | ⚠️ Unsafe | All subsequent accesses panic (out of bounds) |

Compiler MUST NOT error on these patterns. Runtime behavior:
- Out-of-bounds access → panic
- Wrong element accessed → silent logic bug
- This is programmer responsibility (same as C, Go, Zig)

**Safe removal pattern:**
```
// Reverse iteration avoids invalidation:
for i in (0..vec.len()).rev() {
    if vec[i].expired {
        vec.swap_remove(i);  // Safe: doesn't affect earlier indices
    }
}
```

Or consume + filter:
```
let vec = vec.consume().filter(|item| !item.expired).collect();
```

### Iterator Adapters

Adapters operate on index/handle streams using **lazy evaluation**. They transform the iteration protocol without creating intermediate collections.

| Adapter | Behavior | Signature |
|---------|----------|-----------|
| `.filter(pred)` | Yields indices where predicate is true | `(\|&Index\| -> bool) -> Iterator` |
| `.take(n)` | Yields first n indices | `(usize) -> Iterator` |
| `.skip(n)` | Skips first n indices | `(usize) -> Iterator` |
| `.rev()` | Reverses iteration order | `() -> Iterator` |
| `.map(f)` | Transforms each index | `(\|Index\| -> R) -> Iterator<R>` |

**Example:**
```
for i in vec.indices().filter(|i| vec[*i].active).take(10) {
    process(&vec[i]);
}
```

**Desugaring:** Adapters compose filtering logic evaluated during iteration.
```
// Conceptual:
for i in 0..vec.len() {
    if vec[i].active {  // filter
        process(&vec[i]);
        if ++count >= 10 { break }  // take
    }
}
```

**Expression-scoped closure execution:**
- Closure `|i| vec[*i].active` receives `&Index` parameter
- Closure accesses `vec` from outer scope WITHOUT capturing it
- Closure is called immediately during iteration, never stored
- Legal because closure doesn't escape expression scope

**Storage rules:**

| Pattern | Legal | Reason |
|---------|-------|--------|
| `for i in vec.filter(\|i\| ...)` | ✅ Yes | Inline consumption |
| `let iter = vec.indices()` | ✅ Yes | No closure yet |
| `let f = vec.filter(\|i\| vec[*i].x)` | ❌ No | Closure accesses scope |
| `let f = range.filter(\|i\| *i > 10)` | ✅ Yes | Closure doesn't access scope |

**General rule:** Adapter chains can be stored UNLESS a closure accesses outer scope variables (compiler enforces).

**Lazy evaluation:** Adapters evaluate on-demand. No intermediate allocations. `take(10)` stops iteration after 10 matches.

### Edge Cases

| Case | Handling |
|------|----------|
| Empty collection | Loop body never executes |
| `Vec<Linear>` index iteration | COMPILE ERROR: use `.consume()` |
| `Map<String, V>` iteration | COMPILE ERROR: keys must be Copy; use `.consume()` |
| Out-of-bounds index | PANIC (same as outside loop) |
| Invalid handle | PANIC (generation mismatch) |
| `break value` for !Copy | Requires `.clone()`: `break vec[i].clone()` |
| Mutation during iteration | ALLOWED (programmer responsibility) |
| Consume + early exit | Drops remaining items (LIFO) |
| Infinite range (`0..`) | Works (lazy, never terminates unless broken) |
| Zero-sized types (`Vec<()>`) | Yields indices 0..len despite no data |

### Error Handling

**Fallible operations use `?`:**

```
for i in lines {
    let parsed = parse(&lines[i])?;  // Exits loop on error
    results.push(parsed);
}
```

**Fallible access:**

```
for i in 0..items.len() {
    if let Some(item) = items.get(i) {
        process(item);  // Safe for potentially invalid indices
    }
}
```

## Integration Notes

- **Copy threshold:** Types ≤16 bytes are Copy-eligible. Loop extraction follows same rules as assignment.
- **Expression-scoped borrows:** `collection[i]` borrow ends at semicolon, consistent with existing collection access.
- **Linear tracking:** Consume satisfies linear consumption requirements. Index access forbidden for linear collections.
- **Pattern matching:** No mode inference in loops (unlike `match`). Loops always yield indices; usage determines clone necessity.
- **Closures:** Two modes: (1) Capture by value (storable), (2) Expression-scoped execution accessing outer scope (not storable). Iterator adapters use mode 2.
- **Ensure cleanup:** Works with consume—`ensure` fires before drop of remaining items.
- **Concurrency:** Index-based iteration is thread-safe if collection is not mutated. Shared access requires synchronization (separate feature).

## Examples

**Find and return:**
```
fn find_admin(users: Vec<User>) -> Option<User> {
    for i in users {
        if users[i].is_admin {
            return Some(users[i].clone());
        }
    }
    None
}
```

**Collect filtered:**
```
let admins = Vec::new();
for i in users {
    if users[i].is_admin {
        admins.push(users[i].clone());
    }
}
```

**Linear resource cleanup:**
```
for file in files.consume() {
    file.close()?;
}
```

**Lazy filter + early exit:**
```
for i in records.indices().filter(|i| records[*i].matches(query)).take(10) {
    results.push(records[i].clone());
}
```


## Remaining Issues

### MEDIUM Priority Gaps (5 remaining)
Not addressed in this pass (can be addressed in future refinement):

**Gap 4:** Iterator Adapter Type System
- Return types and composition rules for adapters
- Trait-based iteration protocol
- Generic iterator types

**Gap 5:** Map Iteration Ergonomics
- Recommendations for common patterns
- Comparison with other languages
- ED metric validation

**Gap 6:** Mutation During Iteration - Bounds Checking
- Detailed behavior when length changes
- Fallible access patterns for safe mutation
- Runtime semantics specification

**Gap 7:** Error Propagation in Loops
- How `?` interacts with loop state
- Consume + `?` cleanup semantics
- Ensure + loop interaction

**Gap 8:** consume() Implementation Details ✅ RESOLVED
- Type signatures and storage rules now specified
- Implementation mechanism clarified (owns buffer, not reference)

**Gap 9:** for-in Syntax Sugar Details
- Trait-based iteration protocol
- Custom collection iteration
- Exact desugaring rules

### LOW Priority Gaps (2 remaining)
**Gap 10:** Range Iteration Edge Cases
**Gap 11:** Zero-Sized Type Iteration Rationale
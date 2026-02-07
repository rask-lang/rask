# Iterator Protocol and Adapters

See also: [README.md](README.md)

## Iterator Adapters

Adapters operate on index/handle streams using **lazy evaluation**. Transform iteration protocol without intermediate collections.

| Adapter | Behavior | Signature |
|---------|----------|-----------|
| `.filter(pred)` | Yields indices where predicate is true | `(\|Index\| -> bool) -> Iterator` |
| `.take(n)` | Yields first n indices | `(usize) -> Iterator` |
| `.skip(n)` | Skips first n indices | `(usize) -> Iterator` |
| `.rev()` | Reverses iteration order | `() -> Iterator` |
| `.map(f)` | Transforms each index | `(\|Index\| -> R) -> Iterator<R>` |

**Example:**
```rask
for i in vec.indices().filter(|i| vec[i].active).take(10) {
    process(vec[i])
}
```

**Desugaring:** Adapters compose filtering logic evaluated during iteration.
```rask
// Conceptual:
for i in 0..vec.len() {
    if vec[i].active {  // filter
        process(vec[i])
        if ++count >= 10 { break }  // take
    }
}
```

**Expression-scoped closure execution:**
- Closure receives `Index` parameter
- Accesses `vec` from outer scope without capturing
- Called immediately during iteration, never stored
- Legal because doesn't escape expression scope

**Storage rules:**

| Pattern | Legal | Reason |
|---------|-------|--------|
| `for i in vec.filter(\|i\| ...)` | ✅ Yes | Inline consumption |
| `let iter = vec.indices()` | ✅ Yes | No closure yet |
| `let f = vec.filter(\|i\| vec[i].x)` | ❌ No | Closure accesses scope |
| `let f = range.filter(\|i\| *i > 10)` | ✅ Yes | Closure doesn't access scope |

**General rule:** Adapter chains can be stored unless closure accesses outer scope (compiler enforces).

**Lazy:** Evaluate on-demand. No intermediate allocations. `take(10)` stops after 10 matches.

## Iterator Type System

**Core Iterator Trait:**

```rask
trait Iterator<Item> {
    func next(self) -> Option<Item>
}
```

All iterators MUST implement this trait. The `Item` type is what the iterator yields.

**Built-In Iterator Types:**

| Collection | Method | Returns | Item Type |
|------------|--------|---------|-----------|
| `Vec<T>` | `.indices()` | `RangeIterator` | `usize` |
| `Pool<T>` | (default for-in) | `PoolHandleIterator<T>` | `Handle<T>` |
| `Pool<T>` | `.iter()` | `PoolRefIterator<T>` | `(Handle<T>, borrowed T)` |
| `Pool<T>` | `.take_all()` | `PoolTakeAll<T>` | `T` |
| `Map<K,V>` | (for Copy keys) | `MapKeyIterator<K>` | `K` |
| `Map<K,V>` | `.iter()` | `MapRefIterator<K,V>` | `(borrowed K, borrowed V)` |
| `Map<K,V>` | `.take_all()` | `MapTakeAll<K,V>` | `(K, V)` |
| `Vec<T>` | `.take_all()` | `VecTakeAll<T>` | `T` |
| Range | `0..n` | `RangeIterator` | Integer type |

**Adapter Return Types:**

Adapters return type-erased iterator wrappers that maintain the Item type:

| Adapter | Input | Returns | Item Type |
|---------|-------|---------|-----------|
| `.filter(pred)` | `Iterator<T>` | `Filter<T, Pred>` | `T` |
| `.map(f)` | `Iterator<T>` | `Map<T, R, F>` | `R` |
| `.take(n)` | `Iterator<T>` | `Take<T>` | `T` |
| `.skip(n)` | `Iterator<T>` | `Skip<T>` | `T` |
| `.rev()` | `Iterator<T>` | `Rev<T>` | `T` (requires bidirectional) |

**Type Composition:**

Adapters compose through generic type nesting:

```rask
vec.indices()           → RangeIterator
  .filter(|i| ...)      → Filter<usize, ClosureType>
  .take(10)             → Take<usize>
```

Each adapter wraps the previous iterator type. The final type is:
`Take<Filter<usize, ClosureType>>`

**Compiler Requirements:**

1. **Type inference:** Must infer full iterator chain types
2. **Monomorphization:** Must fully monomorphize (no virtual dispatch)
3. **Inlining:** Should inline for zero-cost
4. **Lifetime tracking:** Must enforce expression-scoped closures

**Custom Iterator Implementation:**

Collections can implement custom iteration by providing methods that return types implementing `Iterator<Item>`:

```rask
// INCORRECT - cannot store borrows in structs:
struct BadIterator<T> {
    collection: MyCollection<T>,  // ERROR: cannot store borrowed value
    pos: usize,
}

// CORRECT approach: Own index/state, not reference to collection
struct MyIterator {
    start: usize,
    end: usize,
    step: usize,
}

extend MyIterator with Iterator<usize> {
    func next(self) -> Option<usize> {
        if self.start >= self.end { return None }
        const val = self.start;
        self.start += self.step;
        Some(val)
    }
}
```

**Key Constraint:** Custom iterators must not store references (violates "no storable references"). Must store Copy-able indices, handles, or owned data.

## For-In Desugaring Protocol

**Complete Desugaring Rules:**

The `for <binding> in <expr>` syntax desugars based on the type of `<expr>` and whether it's moved or borrowed.

**Decision Tree:**

| Expression Form | Desugars To | Notes |
|----------------|-------------|-------|
| `for x in range` (Range type) | Direct range loop | Built-in, no method call |
| `for x in collection` | `collection.into_iter()` | Index/handle iteration |
| `for x in collection.iter()` | Ref iteration | Borrows for reading |
| `for x in collection.take_all()` | Consuming iteration | Takes ownership of elements |

**Built-In Collection Methods:**

Collections MUST implement one or more of these methods to support for-in:

| Collection | Method | Signature | For-In Syntax |
|------------|--------|-----------|---------------|
| `Vec<T>` | `.into_iter()` | `func(self) -> RangeIterator` | `for i in vec` |
| `Pool<T>` | `.into_iter()` | `func(self) -> PoolHandleIterator<T>` | `for h in pool` |
| `Pool<T>` | `.iter()` | `func(self) -> PoolRefIterator<T>` | `for (h, x) in pool.iter()` |
| `Map<K,V>` (K: Copy) | `.into_iter()` | `func(self) -> MapKeyIterator<K>` | `for k in map` |
| `Map<K,V>` | `.iter()` | `func(self) -> MapRefIterator<K,V>` | `for (k, v) in map.iter()` |

**Note:** `.into_iter()` for Vec/Pool/Map does NOT consume the collection—it returns an index/handle iterator. Use `.take_all()` for ownership transfer.

**Complete Desugaring Examples:**

**Example 1: Vec index iteration**

```rask
// User writes:
for i in vec {
    print(vec[i]);
}

// Desugars to:
{
    const _iter = vec.into_iter();  // Returns RangeIterator (0..vec.len())
    loop {
        const i = match _iter.next() {
            Some(val) => val,
            None => break,
        };
        print(vec[i]);
    }
}
```

**Example 2: Pool ref iteration**

```rask
// User writes:
for (h, entity) in pool.iter() {
    print(h, entity.name)
}

// Desugars to:
{
    const _iter = pool.iter()  // Returns PoolRefIterator<T>
    loop {
        let (h, entity) = match _iter.next() {
            Some(val) => val,
            None => break,
        }
        print(h, entity.name)
        // Note: entity is borrowed (expression-scoped), released here
    }
}
```

**Example 3: Consume iteration**

```rask
// User writes:
for item in vec.take_all() {
    process(item);
}

// Desugars to:
{
    const _iter = vec.take_all();  // Returns VecTakeAll<T>, vec now empty
    loop {
        const item = match _iter.next() {
            Some(val) => val,
            None => break,
        };
        process(item);
    }
    // _iter drops here, dropping any remaining items
}
```

**Example 4: Range iteration**

```rask
// User writes:
for i in 0..n {
    body
}

// Desugars to:
{
    let _pos = 0;
    const _end = n;
    loop {
        if _pos >= _end { break; }
        const i = _pos;
        body
        _pos += 1;
    }
}
```

**Compiler Method Resolution:**

When the compiler sees `for x in expr`:

1. **Check if expr is a Range type** → Use built-in range desugaring (no method call)
2. **Check if expr is a method call like `.iter()` or `.take_all()`** → Use returned iterator directly
3. **Otherwise** → Call `expr.into_iter()`, require `Iterator<Item>` return type

**Error Cases:**

| Pattern | Error | Message |
|---------|-------|---------|
| `for x in vec` where Vec doesn't have `.into_iter()` | Compile error | "cannot iterate over `Vec<T>`: missing `into_iter()` method" |
| `for x in map.iter()` where Map doesn't have `.iter()` | Compile error | "`Map<K,V>` has no method `iter()`" |
| `.into_iter()` returns non-Iterator type | Compile error | "`.into_iter()` must return type implementing `Iterator<T>`" |

**Custom Collection Example:**

```rask
// Define custom collection:
struct Grid<T> {
    data: Vec<T>,
    width: usize,
    height: usize,
}

// Implement iteration:
extend<T> Grid<T> {
    func into_iter(take self) -> GridIterator {
        GridIterator { width: self.width, height: self.height, row: 0, col: 0 }
    }
}

struct GridIterator {
    width: usize,
    height: usize,
    row: usize,
    col: usize,
}

extend GridIterator with Iterator<(usize, usize)> {
    func next(self) -> Option<(usize, usize)> {
        if self.row >= self.height { return None; }
        const pos = (self.row, self.col);
        self.col += 1;
        if self.col >= self.width {
            self.col = 0;
            self.row += 1;
        }
        Some(pos)
    }
}

// Usage:
for (row, col) in grid {
    print(grid.data[row * grid.width + col]);
}
```

**Key Points:**

1. Iterator MUST NOT store references (only Copy data like indices)
2. `into_iter()` can take `self` but still leave collection usable (e.g., Vec index iteration)
3. `iter()` takes `self` (borrowed) and can yield references (expression-scoped)
4. Collection remains accessible in loop body (unless consumed)

**Interaction with Break and Continue:**

```rask
for i in vec {
    if cond { break; }      // Calls _iter.drop(), exits loop
    if cond2 { continue; }  // Skips to next iteration, calls _iter.next()
}
```

When `break` exits the loop, the iterator variable `_iter` is dropped normally. For consume iterators, this triggers LIFO drop of remaining items.

**Nested Loops:**

```rask
for i in vec {
    for j in vec {
        // Both iterators active, independent
        compare(vec[i], vec[j])
    }
}

// Desugars to:
{
    const _iter1 = vec.into_iter()
    loop {
        const i = match _iter1.next() { Some(v) => v, None => break }
        {
            const _iter2 = vec.into_iter()
            loop {
                const j = match _iter2.next() { Some(v) => v, None => break }
                compare(vec[i], vec[j])
            }
        }
    }
}
```

Each loop gets its own iterator. For index-based iteration, this is cheap (Copy state).

**Performance Guarantees:**

- Iterator chains must match hand-written loop performance
- No heap allocations for standard adapters
- Closure inlining must eliminate call overhead
- Optimizer must fuse chains into single loops

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Collection Iteration](collection-iteration.md) - Iteration modes for Vec, Pool, Map
- [Edge Cases](edge-cases.md) - ZST iteration and other edge cases

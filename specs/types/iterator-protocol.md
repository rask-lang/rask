<!-- id: type.iterators -->
<!-- status: decided -->
<!-- summary: Iterator trait, adapter chains, for-in desugaring protocol -->
<!-- depends: types/traits.md, types/generics.md, stdlib/collections.md -->

# Iterator Protocol and Adapters

Core `Iterator<Item>` trait with lazy adapters. `for-in` desugars to `.iterate()` / `.take_all()` method calls.

## Iterator Trait

| Rule | Description |
|------|-------------|
| **I1: Core trait** | All iterators implement `Iterator<Item>` with `func next(self) -> Option<Item>` |
| **I2: Monomorphization** | Iterator chains fully monomorphized, no virtual dispatch |
| **I3: Inlining** | Adapters inlined for zero-cost abstraction |
| **I4: No stored references** | Custom iterators must not store references — only Copy-able indices, handles, or owned data |

<!-- test: parse -->
```rask
trait Iterator<Item> {
    func next(self) -> Option<Item>
}
```

## Adapters

| Rule | Description |
|------|-------------|
| **AD1: Lazy evaluation** | Adapters transform iteration without intermediate collections |
| **AD2: Type composition** | Adapters compose through generic type nesting |
| **AD3: Storage restriction** | Adapter chains can be stored unless closure accesses outer scope |

| Adapter | Behavior | Signature |
|---------|----------|-----------|
| `.filter(pred)` | Yields items where predicate is true | `(\|Item\| -> bool) -> Filter<Item, Pred>` |
| `.take(n)` | Yields first n items | `(usize) -> Take<Item>` |
| `.skip(n)` | Skips first n items | `(usize) -> Skip<Item>` |
| `.rev()` | Reverses iteration order (requires bidirectional) | `() -> Rev<Item>` |
| `.map(f)` | Transforms each item | `(\|Item\| -> R) -> Map<Item, R, F>` |

<!-- test: skip -->
```rask
for i in vec.indices().filter(|i| vec[i].active).take(10) {
    process(vec[i])
}
```

**Closure execution:** Closures receive the item parameter, access outer scope without capturing, are called immediately during iteration, and never stored. This is legal because they don't escape expression scope.

| Pattern | Legal | Reason |
|---------|-------|--------|
| `for i in vec.filter(\|i\| ...)` | Yes | Inline consumption |
| `let iter = vec.indices()` | Yes | No closure yet |
| `let f = vec.filter(\|i\| vec[i].x)` | No | Closure accesses scope |
| `let f = range.filter(\|i\| *i > 10)` | Yes | Closure doesn't access scope |

## Built-In Iterator Types

| Collection | Method | Returns | Item Type |
|------------|--------|---------|-----------|
| `Vec<T>` | (default for-in) | `VecRefIterator<T>` | borrowed `T` |
| `Vec<T>` | `.take_all()` | `VecTakeAll<T>` | `T` |
| `Pool<T>` | (default for-in) | `PoolRefIterator<T>` | borrowed `T` |
| `Pool<T>` | `.handles()` | `PoolHandleIterator<T>` | `Handle<T>` |
| `Pool<T>` | `.take_all()` | `PoolTakeAll<T>` | `T` |
| `Map<K,V>` | (default for-in) | `MapRefIterator<K,V>` | `(K, borrowed V)` |
| `Map<K,V>` | `.keys()` | `MapKeyIterator<K>` | `K` |
| `Map<K,V>` | `.take_all()` | `MapTakeAll<K,V>` | `(K, V)` |
| Range | `0..n` | `RangeIterator` | Integer type |

## For-In Desugaring

| Rule | Description |
|------|-------------|
| **D1: Range** | `for x in range` — built-in range loop, no method call |
| **D2: Collection value** | `for x in collection` — calls `collection.iterate()` yielding borrowed elements |
| **D3: Consuming** | `for x in collection.take_all()` — takes ownership of elements |
| **D4: Index mode** | `for i in 0..vec.len()` or `for h in pool.handles()` — explicit index/handle iteration |
| **D5: Method resolution** | Check Range type first, then explicit method call, then `.iterate()` |

<!-- test: skip -->
```rask
// Vec value iteration (default)
for item in vec {
    print(item.name)
}
// Desugars to:
{
    const _iter = vec.iterate()  // VecRefIterator (yields borrowed T)
    loop {
        const item = match _iter.next() {
            Some(val) => val,
            None => break,
        }
        print(item.name)
    }
}
```

<!-- test: skip -->
```rask
// Index iteration (explicit)
for i in 0..vec.len() {
    print(vec[i])
}
// Desugars to:
{
    const _iter = (0..vec.len()).iterate()  // RangeIterator
    loop {
        const i = match _iter.next() {
            Some(val) => val,
            None => break,
        }
        print(vec[i])
    }
}
```

## Break, Continue, and Nested Loops

| Rule | Description |
|------|-------------|
| **L1: Break cleans up iterator** | `break` exits loop; iterator cleaned up normally |
| **L2: Continue calls next** | `continue` skips to next iteration |
| **L3: Independent iterators** | Nested loops get independent iterators |
| **L4: Consume cleanup** | For consume iterators, break triggers LIFO cleanup of remaining items |

## Custom Iterators

| Rule | Description |
|------|-------------|
| **CU1: Implement trait** | Collections provide methods returning types implementing `Iterator<Item>` |
| **CU2: No stored references** | Iterator structs must store Copy data (indices, handles), not references |
| **CU3: iterate contract** | `.iterate()` for Vec/Pool/Map returns value iterator (borrowed elements) — does NOT consume |

<!-- test: skip -->
```rask
struct GridIterator {
    width: usize,
    height: usize,
    row: usize,
    col: usize,
}

extend GridIterator with Iterator<(usize, usize)> {
    func next(self) -> Option<(usize, usize)> {
        if self.row >= self.height { return None }
        const pos = (self.row, self.col)
        self.col += 1
        if self.col >= self.width {
            self.col = 0
            self.row += 1
        }
        return Some(pos)
    }
}
```

## Error Messages

```
ERROR [type.iterators/D2]: cannot iterate over type
   |
3  |  for x in my_value
   |           ^^^^^^^^ `MyType` has no `iterate()` method

WHY: for-in requires the expression to be a Range, or to have
     an iterate() method returning Iterator<Item>.

FIX: Implement iterate() on MyType, or use a range.
```

```
ERROR [type.iterators/CU2]: iterator stores reference
   |
2  |  struct BadIterator<T> {
3  |      collection: MyCollection<T>,  // borrowed value
   |      ^^^^^^^^^^ cannot store borrowed value in iterator

WHY: Iterators must not store references (violates "no storable references").

FIX: Store Copy-able indices or handles instead.
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Empty collection | I1 | `.next()` returns `None` immediately |
| Break in consume iterator | L4 | Remaining items cleaned up in LIFO order |
| Nested loops same collection | L3 | Independent iterators, cheap for index-based |
| `.iterate()` returns non-Iterator | D5 | Compile error |
| Closure escapes expression scope | AD3 | Compile error |

---

## Appendix (non-normative)

### Rationale

**I4 (no stored references):** Rask's "no storable references" rule applies to iterators. Storing a reference to the collection would create lifetime complexity. Index-based iteration avoids this entirely.

**CU3 (iterate doesn't consume):** Vec's `.iterate()` returns a value iterator (borrowed elements), not an owning iterator. The collection remains accessible in the loop body. Use `.take_all()` for ownership transfer.

### Patterns & Guidance

**Consume iteration:**

<!-- test: skip -->
```rask
for item in vec.take_all() {
    process(item)
}
// vec is now empty
```

**Value iteration (default):**

<!-- test: skip -->
```rask
for entity in pool {
    print(entity.name)
}
```

**Handle iteration (explicit):**

<!-- test: skip -->
```rask
for h in pool.handles() {
    pool[h].update()
    if pool[h].dead {
        pool.remove(h)
    }
}
```

**Performance guarantees:**
- Iterator chains must match hand-written loop performance
- No heap allocations for standard adapters
- Closure inlining eliminates call overhead
- Optimizer fuses chains into single loops

### See Also

- `type.traits` — Trait definitions
- `std.collections` — Vec, Pool, Map APIs
- `mem.borrowing` — Statement-scoped vs block-scoped views

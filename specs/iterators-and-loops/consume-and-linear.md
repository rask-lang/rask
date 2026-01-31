# Consume Iteration and Linear Types

See also: [README.md](README.md)

## Consume: Consuming Iteration

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

## Consume Implementation: Ownership Transfer, Not Borrowing

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

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Collection Iteration](collection-iteration.md) - Iteration modes for Vec, Pool, Map
- [Mutation and Errors](mutation-and-errors.md) - Error handling with consume

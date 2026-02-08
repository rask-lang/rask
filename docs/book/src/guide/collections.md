# Collections and Handles

> **Placeholder:** Brief overview. For detailed specifications, see the [collections](https://github.com/rask-lang/rask/blob/main/specs/stdlib/collections.md) and [pools](https://github.com/rask-lang/rask/blob/main/specs/memory/pools.md) specs.

## Three Collection Types

**Vec<T>** - Ordered, indexed access:
```rask
const v = Vec.new()
try v.push(1)
try v.push(2)
const first = v[0]         // Copy out (if T: Copy)
```

**Map<K,V>** - Key-value lookup:
```rask
const m = Map.new()
try m.insert("key", "value")
const val = m.get("key")   // Returns Option<V>
```

**Pool\<T\>** - Handle-based storage for graphs:
```rask
const pool = Pool.new()
const h1 = try pool.insert(Node.new())
const h2 = try pool.insert(Node.new())
h1.next = h2               // Store handle, not reference
```

## Why Handles?

References can't be stored in Rask (no lifetime annotations). For graphs, cycles, and entity systems, use `Pool\<T\>` with handles:

```rask
struct Node {
    value: i32,
    next: Option<Handle<Node>>,
}

const pool = Pool.new()
const h1 = try pool.insert(Node { value: 1, next: None })
const h2 = try pool.insert(Node { value: 2, next: Some(h1) })
```

Handles are validated at runtime:
- Pool ID check (right pool?)
- Generation check (still valid?)
- Index bounds check

## Iteration

```rask
for i in vec {
    println(vec[i])        // Index iteration
}

for h in pool {
    println(pool[h].value) // Handle iteration
}

for i in 0..10 {
    println(i)             // Range iteration
}
```

## Next Steps

- [Error Handling](error-handling.md)
- [Formal collections spec](https://github.com/rask-lang/rask/blob/main/specs/stdlib/collections.md)
- [Formal pools spec](https://github.com/rask-lang/rask/blob/main/specs/memory/pools.md)

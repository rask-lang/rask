# Ownership and Memory

> **Placeholder:** Brief overview. For detailed specifications, see the [ownership](https://github.com/rask-lang/rask/blob/main/specs/memory/ownership.md) and [borrowing](https://github.com/rask-lang/rask/blob/main/specs/memory/borrowing.md) specs.

## Core Principles

Rask's memory model is built on three principles:

1. **Single ownership** - Every value has one owner
2. **Move semantics** - Assigning/passing transfers ownership
3. **Scoped borrowing** - Temporary access that can't escape

## Ownership Transfer

```rask
const s1 = string.new("hello")
const s2 = s1              // s1 moved to s2, s1 is now invalid
// println(s1)             // Error: s1 has been moved
```

## Borrowing

Borrowing gives temporary access without transferring ownership:

```rask
func print_len(s: string) {
    println(s.len())       // Borrows s
}

const text = string.new("hello")
print_len(text)            // text is borrowed
println(text)              // text still valid here
```

The borrow lasts only for the function call - `text` remains valid after.

## Copy vs Move

Small types (≤16 bytes) copy implicitly:

```rask
const x: i32 = 42
const y = x                // Copy (i32 is small)
println(x)                 // x still valid
```

Large types move:

```rask
const v1 = Vec.new()
const v2 = v1              // Move (Vec is large)
// println(v1.len())       // Error: v1 has been moved
```

To keep access, explicitly clone:

```rask
const v1 = Vec.new()
const v2 = v1.clone()      // Explicit copy
println(v1.len())          // Both valid
println(v2.len())
```

## Why No Storable References?

Rask doesn't allow storing references in structs or returning them. This eliminates lifetime annotations:

- ✗ No `'a` lifetime parameters
- ✗ No borrow checker fights
- ✓ Simple ownership rules
- ✓ Predictable behavior

For graphs and cycles, use [handles](collections.md) instead of references.

## Next Steps

- [Collections and Handles](collections.md)
- [Formal ownership spec](https://github.com/rask-lang/rask/blob/main/specs/memory/ownership.md)
- [Formal borrowing spec](https://github.com/rask-lang/rask/blob/main/specs/memory/borrowing.md)

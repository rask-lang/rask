# Basic Syntax

> **Placeholder:** Minimal content for now. See [examples](../examples/README.md) and [formal specs](../reference/specs-link.md) for details.

## Variables

```rask
const x = 42          // Immutable binding
let y = 0             // Mutable binding
y = 5                 // Reassignment
```

**When to use:**
- `const` - binding won't be reassigned (even if value is mutated via methods)
- `let` - binding will be reassigned (e.g., `let x = 0; x = 1`)

## Functions

```rask
func add(a: i32, b: i32) -> i32 {
    return a + b      // Explicit return required
}
```

Functions require explicit `return` for values (unlike Rust's implicit returns).

## Control Flow

```rask
if x > 0 {
    println("positive")
} else {
    println("zero or negative")
}

// Inline if (expression context)
const sign = if x > 0: "+" else: "-"

for i in 0..10 {
    println(i)
}

match result {
    Result.Ok(v) => println(v),
    Result.Err(e) => println("Error: {}", e),
}
```

## Types

```rask
const a: i32 = 42       // Signed integers: i8, i16, i32, i64
const b: u64 = 100      // Unsigned: u8, u16, u32, u64
const c: f64 = 3.14     // Floats: f32, f64
const d: bool = true    // Boolean
const s: string = "hi"  // String (lowercase!)
```

## Next Steps

- [Ownership](ownership.md)
- [Collections](collections.md)
- [Examples](../examples/README.md)

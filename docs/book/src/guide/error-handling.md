# Error Handling

> **Placeholder:** Brief overview. For detailed specifications, see the [error types](https://github.com/dritory/rask/blob/main/specs/types/error-types.md) spec.

## Result Types

Operations that can fail return `Result<T, E>`, or using shorthand: `T or E`

```rask
func parse_number(s: string) -> i64 or ParseError {
    // Implementation
}

const result = parse_number("42")
match result {
    Result.Ok(n) => println("Parsed: {}", n),
    Result.Err(e) => println("Error: {}", e),
}
```

## Error Propagation with `try`

The `try` operator extracts the success value or returns early with the error:

```rask
func process() -> () or Error {
    const file = try fs.open("data.txt")  // Returns Error if fails
    const data = try file.read()          // Returns Error if fails
    process_data(data)
}
```

Without `try`, you'd need nested matches for every fallible operation.

## Resource Cleanup with `ensure`

```rask
func read_file(path: string) -> string or IoError {
    const file = try fs.open(path)
    ensure file.close()           // Runs at scope exit, even on error

    const data = try file.read()  // Can use try after ensure
    return data
}
```

The `ensure` keyword guarantees cleanup runs even if `try` returns early.

## Optional Values

`Option<T>` or `T?` for values that may be absent:

```rask
const m = Map.new()
try m.insert("key", 42)

const val = m.get("key")      // Returns Option<i64>
if val is Some(v) {
    println("Found: {}", v)
} else {
    println("Not found")
}

// Or use the ?? operator for defaults:
const v = m.get("key") ?? 0
```

## Next Steps

- [Concurrency](concurrency.md)
- [Formal error types spec](https://github.com/dritory/rask/blob/main/specs/types/error-types.md)
- [Formal ensure spec](https://github.com/dritory/rask/blob/main/specs/control/ensure.md)

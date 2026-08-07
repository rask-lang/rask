# Grep Clone

A command-line tool for searching files with pattern matching.

**Full source:** [grep_clone.rk](https://github.com/rask-lang/rask/blob/main/examples/grep_clone.rk)

## Key Concepts Demonstrated

- CLI argument parsing
- File I/O with error handling
- String operations (split, contains, trim)
- Resource cleanup with `ensure`
- Pattern matching with enums

## Highlights

### Resource Management

<!-- test: parse -->
```rask
func search_file(path: string, pattern: string) -> void or IoError {
    let file = try fs.open(path)
    ensure file.close()  // Guaranteed cleanup

    for line in file.lines() {
        if line.contains(pattern): println(line)
    }
}
```

The `ensure` keyword guarantees `file.close()` runs even on early returns or errors.

### Error Handling

<!-- test: parse -->
```rask
enum GrepError {
    NoPattern,
    NoFiles,
    FileError(string),
}

func parse_args(args: Vec<string>) -> Options or GrepError {
    // Returns `Options or GrepError` — the caller must handle the error branch
}
```

`T or E` is the error type; there's no `Result<T, E>` and no `Ok`/`Err` constructors. Returning
a bare `Options` auto-wraps into the success branch. Callers pick one of three words:

<!-- test: skip -->
```rask
let opts = try parse_args(args)                            // propagate to my caller
let opts = parse_args(args) catch e => return usage(e)     // handle it, exit here
let opts = parse_args(args) catch _ => Options.default()   // handle it, carry on
```

`catch` always binds — `e =>` to use the error, `_ =>` to drop it — so a discarded error is
visible in the source. The `??` operator is the same idea for optionals (absence, not failure);
the two never overlap.

> `catch` is specified but not yet implemented in the compiler — that block is marked `skip` so
> the doc tests don't fail on it. `try` and `??` work today.

### String Processing

<!-- test: parse -->
```rask
for line in file.lines() {
    if case_insensitive {
        if line.to_lowercase().contains(pattern.to_lowercase()) {
            println(line)
        }
    } else {
        if line.contains(pattern) {
            println(line)
        }
    }
}
```

## Running It

```bash
rask run examples/grep_clone.rk "pattern" file1.txt file2.txt
rask run examples/grep_clone.rk -i "case-insensitive" *.txt
```

## What You'll Learn

- How to parse command-line arguments in Rask
- Error handling with `T or E`, `try`, and `catch`
- Resource management with `ensure`
- String manipulation and iteration

[View full source →](https://github.com/rask-lang/rask/blob/main/examples/grep_clone.rk)

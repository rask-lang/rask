# Grep Clone

A command-line tool for searching files with pattern matching.

**Full source:** [grep_clone.rk](https://github.com/dritory/rask/blob/main/examples/grep_clone.rk)

## Key Concepts Demonstrated

- CLI argument parsing
- File I/O with error handling
- String operations (split, contains, trim)
- Resource cleanup with `ensure`
- Pattern matching with enums

## Highlights

### Resource Management

```rask
func search_file(path: string, pattern: string) -> () or IoError {
    const file = try fs.open(path)
    ensure file.close()  // Guaranteed cleanup

    for line in file.lines() {
        if line.contains(pattern): println(line)
    }
}
```

The `ensure` keyword guarantees `file.close()` runs even on early returns or errors.

### Error Handling

```rask
enum GrepError {
    NoPattern,
    NoFiles,
    FileError(string),
}

func parse_args(args: Vec<string>) -> Options or GrepError {
    // Returns Result type, caller must handle errors
}
```

### String Processing

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
rask grep_clone.rk "pattern" file1.txt file2.txt
rask grep_clone.rk -i "case-insensitive" *.txt
```

## What You'll Learn

- How to parse command-line arguments in Rask
- Error handling patterns with `Result` types
- Resource management with `ensure`
- String manipulation and iteration

[View full source →](https://github.com/dritory/rask/blob/main/examples/grep_clone.rk)

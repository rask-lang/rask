# Level 2 — Ownership & Errors

Do move semantics and error handling feel invisible or annoying?

## Challenges

1. [Stack Machine](stack-machine.md) — structs, mutate self, Option
2. [File Analyzer](file-analyzer.md) — file I/O, try, error propagation
3. [Contacts Book](contacts.md) — Vec of structs, searching, ownership friction

## What You Need

Everything from [Level 1](../01-core-feel/) plus:

### Ownership

Values ≤16 bytes copy automatically (i32, f64, bool, small structs). Bigger values move:

```rask
const a = 42
const b = a           // copy — a is still valid
use(a)                // fine

const s = "hello"
const t = s           // move — s is now invalid
// use(s)             // compile error
```

### Structs and Methods

```rask
struct Stack {
    items: Vec<f64>
}

extend Stack {
    func push(mutate self, item: f64) {    // mutate = can modify self
        self.items.push(item)
    }

    func pop(mutate self) -> f64? {        // returns Option<f64>
        return self.items.pop()
    }

    func peek(self) -> f64? {              // read-only (no mutate)
        return self.items.last()
    }

    func new() -> Stack {                  // static method (no self)
        return Stack { items: Vec.new() }
    }
}

let s = Stack.new()
s.push(42.0)
```

### Parameter Modes

```rask
func display(data: Vec<i32>) { ... }          // borrow (default): read-only
func sort(mutate data: Vec<i32>) { ... }      // mutate: can modify
func consume(take data: Vec<i32>) { ... }     // take: takes ownership

display(data)            // just pass it
sort(mutate data)        // mark mutation at call site
consume(own data)        // mark ownership transfer at call site
```

### Option (`T?`)

```rask
// Check and unwrap
if val is Some(v) {
    use(v)
}

// Guard pattern — unwrap or return
const v = maybe_val is Some else { return }

// Default value
const v = maybe_val ?? default_value

// Force unwrap (panics if None)
const v = maybe_val!
```

### Error Handling (`T or E`)

```rask
func read_file(path: string) -> string or IoError {
    const file = try File.open(path)     // try = unwrap or propagate error
    ensure file.close()                  // ensure = guaranteed cleanup
    return try file.read_all()
}
```

`try` before an expression: if it's `Err`, return that error to the caller. If it's `Ok`, unwrap the value.

`ensure` schedules cleanup that runs when the scope exits, no matter what (early return, error, normal exit).

### Result Methods

| Method | What It Does |
|--------|-------------|
| `result.on_err(default)` | Value or default (discards error) |
| `result.ok()` | Converts to Option (Ok→Some, Err→None) |
| `result!` | Force-unwrap (panics on Err) |

### File I/O

```rask
import fs
import cli

const args = cli.parse()
const path = args.positional()[0]

const file = try fs.open(path)
ensure file.close()

const text = try file.read_text()
const lines = text.split("\n")
```

### String Methods

```rask
s.len()                   // character count
s.split("\n")             // split into Vec<string>
s.split_whitespace()      // split on whitespace
s.contains("pattern")     // substring search
s.trim()                  // strip whitespace
s.starts_with("prefix")
s.to_lowercase()
```

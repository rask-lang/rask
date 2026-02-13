# Level 4 — Data Modeling

Do structs + enums + match cover real domain modeling?

## Challenges

1. [JSON-lite Parser](json-parser.md) — recursive enums, parsing, extend
2. [Task Scheduler](scheduler.md) — enum ordering, Vec as priority queue

## What You Need

Everything from [Level 2](../02-ownership-errors/) plus:

### Recursive Enums

Enums can reference themselves through heap-allocated types:

```rask
enum Expr {
    Number(f64)
    Add(left: Vec<Expr>, right: Vec<Expr>)    // Vec wraps the recursion
    Neg(inner: Vec<Expr>)
}
```

`Vec<Expr>` is heap-allocated, so the enum has a fixed size. You'd use `Vec` with one element as a box here.

### Generics

```rask
func max<T: Comparable>(a: T, b: T) -> T {
    if a > b { return a }
    return b
}
```

`T: Comparable` means "T must support comparison." The compiler generates a specialized copy for each concrete type.

### Traits

```rask
trait Printable {
    func to_string(self) -> string
}

extend Point with Printable {
    func to_string(self) -> string {
        return "({self.x}, {self.y})"
    }
}
```

Structural trait satisfaction — if the methods match, the trait is satisfied. Some traits require explicit `extend...with`.

### Extend Blocks

Add methods to any type, including enums:

```rask
enum Priority { Low, Medium, High, Critical }

extend Priority {
    func rank(self) -> i32 {
        return match self {
            Low => 0,
            Medium => 1,
            High => 2,
            Critical => 3,
        }
    }

    func is_urgent(self) -> bool {
        return self.rank() >= 2
    }
}
```

### Sorting and Comparison

```rask
// Sort a Vec with a comparison function
items.sort_by(|a, b| a.priority.rank().compare(b.priority.rank()))

// Find max
const best = items.max_by_key(|item| item.score)
```

### String Parsing

```rask
const s = "[1, \"hello\", true]"
const chars = s.chars()           // iterate characters
const sub = s[start..end]         // substring
s.starts_with("[")
s.trim()

// Character checks
c == '"'
c == ','
c.is_digit()
c.is_whitespace()
```

### Pattern Matching — Advanced

```rask
// Nested patterns
match json {
    Array(items) => {
        for item in items {
            match item {
                Number(n) => println("num: {n}"),
                Str(s) => println("str: {s}"),
                _ => println("other"),
            }
        }
    }
    _ => {}
}

// Guard clauses
match value {
    Number(n) if n > 0 => println("positive"),
    Number(n) => println("non-positive: {n}"),
    _ => {}
}
```

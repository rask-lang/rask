<!-- id: raido.syntax -->
<!-- status: proposed -->
<!-- summary: Raido syntax — dynamic subset of Rask -->
<!-- depends: raido/values.md -->

# Syntax

Dynamic subset of Rask. Same `{}` blocks, `match`/`=>`, `if`/`else if`, `for`/`in`, closures. No type annotations.

## Lexical

- Newline-terminated statements. Semicolons optional.
- `//` line comments, `/* */` block comments.
- `"hello {name}"` string interpolation (double-quoted). `'raw string'` (single-quoted, no interpolation).
- Numbers: `42` (int), `3.14` (number), `0xff`, `0b1010`, `1_000_000`.

## Variables

```raido
const x = 42           // immutable local
let y = 10             // mutable local
global config = {:}    // explicit global (no accidental globals)
```

## Functions and Closures

```raido
func greet(name) {
    return "Hello, {name}"
}

const double = |x| x * 2
const add = |a, b| { a + b }

func sum(nums...) {       // rest parameter → array
    let total = 0
    for n in nums {
        total = total + n
    }
    return total
}
```

Functions require explicit `return` (matches Rask). Blocks in expression context use implicit last expression.

## Control Flow

```raido
if health <= 0 {
    die()
} else if health < 20 {
    warn("low")
} else {
    fight()
}

while queue_size() > 0 {
    process(dequeue())
}

for item in inventory { print(item) }
for i in 0..10 { print(i) }           // 0 through 9 (exclusive)
for i in 0..=10 { print(i) }          // 0 through 10 (inclusive)
for name, score in leaderboard { print("{name}: {score}") }

match state {
    "idle" => wait(),
    "patrol" => move_to(next_waypoint()),
    _ => error("unknown: {state}"),
}

// Expression context
const sign = if x > 0 { "+" } else { "-" }
const color = match status { "active" => "green", _ => "gray" }
```

## Operators

| Precedence | Operators |
|-----------|-----------|
| 1 (highest) | `!`, `-` (unary) |
| 2 | `*`, `/`, `%` |
| 3 | `+`, `-` |
| 4 | `<`, `>`, `<=`, `>=`, `==`, `!=` |
| 5 | `&&` |
| 6 (lowest) | `\|\|` |

`&&`/`||` short-circuit and return operand values. No `//` integer division (conflicts with comments) — use `math.floor(a / b)`.

No `#` length operator — use `len()` from core. No `..` concat operator — string interpolation covers it, `string.concat()` for the rest.

## Collections

```raido
const colors = ["red", "green", "blue"]  // array (0-indexed)
const point = {x: 10, y: 20}            // map
const empty = {:}                        // empty map ({} is empty block)
```

## Error Handling

`try` propagates errors to the caller, matching Rask's error handling syntax:

```raido
func load_config(path) {
    const data = try read_file(path)       // propagate on error
    return parse(data)
}

// Catch and handle — replaces pcall
const config = try load_config("app.cfg") else |e| {
    log("fallback: {e}")
    return default_config()
}

// Raise an error
error("invalid state")

// Assert
assert(x > 0, "x must be positive")
```

`try expr` — if `expr` errors, propagate to caller. `try expr else |e| { ... }` — catch and handle. This is the same pattern as Rask, adapted for dynamic types.

## Keywords

`break`, `const`, `else`, `false`, `for`, `func`, `global`, `if`, `in`, `let`, `match`, `nil`, `return`, `true`, `try`, `while`, `yield`

`coroutine()`, `error()`, and `assert()` are built-in functions (core), not keywords.

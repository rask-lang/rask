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
| 1 (highest) | `!`, `-` (unary), `#` (length) |
| 2 | `*`, `/`, `%` |
| 3 | `+`, `-` |
| 4 | `..` (concat) |
| 5 | `<`, `>`, `<=`, `>=`, `==`, `!=` |
| 6 | `&&` |
| 7 (lowest) | `\|\|` |

`&&`/`||` short-circuit and return operand values. No `//` integer division (conflicts with comments) — use `math.floor(a / b)`.

## Collections

```raido
const colors = ["red", "green", "blue"]  // array (0-indexed)
const point = {x: 10, y: 20}            // map
const empty = {:}                        // empty map ({} is empty block)
```

## Keywords

`and`, `break`, `case`, `const`, `else`, `false`, `for`, `func`, `global`, `if`, `in`, `let`, `match`, `nil`, `not`, `or`, `return`, `true`, `while`, `yield`

# Level 1 — Core Feel

Does basic Rask feel natural? Are the keywords intuitive?

## Challenges

1. [FizzBuzz](fizzbuzz.md) — loops, conditionals, printing
2. [Temperature Converter](temps.md) — enums, match, arithmetic
3. [Word Counter](wordcount.md) — strings, Map, iteration

## What You Need

### Variables

```rask
const x = 42              // immutable
let y = 0                 // mutable
y = y + 1
const name = "Alice"      // string (lowercase type)
const pi: f64 = 3.14      // explicit type
```

### Functions

```rask
func add(a: i32, b: i32) -> i32 {
    return a + b           // explicit return required
}

func greet(name: string) {
    println("Hello, {name}")   // string interpolation
}
```

### Control Flow

```rask
// If
if x > 0 {
    println("positive")
} else if x == 0 {
    println("zero")
} else {
    println("negative")
}

// For loop with range
for i in 1..101 {         // 1 to 100 inclusive
    println(i)
}

// For loop over collection
for word in words {
    process(word)
}

// While
while condition {
    work()
}
```

### Enums and Match

```rask
enum Direction {
    North
    South
    East
    West
}

enum Shape {
    Circle(f64)                         // payload
    Rectangle(width: f64, height: f64)  // named fields
}

// Match produces a value in expression context
const area = match shape {
    Circle(r) => 3.14 * r * r,
    Rectangle(w, h) => w * h,
}

// Match as statement (side effects)
match direction {
    North => go_up(),
    South => go_down(),
    _ => stay(),
}
```

### Collections

```rask
// Vec
const v = Vec.new()
v.push(1)
v.push(2)
const len = v.len()
const first = v[0]

const v2 = Vec.from([1, 2, 3])

// Map
const m = Map.new()
m.insert("key", 42)
const val = m.get("key")    // returns Option
```

### Printing

```rask
println("text")              // with newline
print("text")                // without newline
println("x = {x}")           // interpolation
println("{a} + {b} = {a+b}") // expressions in interpolation
```

### Operators

`+` `-` `*` `/` `%` (modulo) `==` `!=` `<` `>` `<=` `>=` `&&` `||` `!`

Arithmetic panics on overflow. Integer division truncates toward zero.

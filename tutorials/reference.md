# Rask Quick Reference

Everything you need for the tutorials in one place.

## Basics

```rask
const x = 42                  // immutable binding
let y = 0                     // mutable binding
y = 1                         // reassign

const s = "hello"             // string (lowercase, heap-allocated)
const n: i32 = 10             // explicit type annotation
```

## Functions

```rask
func add(a: i32, b: i32) -> i32 {
    return a + b              // explicit return required
}

func greet(name: string) {    // no return type = returns ()
    println("Hello, {name}")
}
```

Functions require explicit `return`. Blocks in expression context (match arms, if/else assigned to a variable) use implicit last expression.

## Parameter Modes

| Mode | Syntax | Meaning |
|------|--------|---------|
| Borrow (default) | `param: T` | Read-only, caller keeps value |
| Mutate | `mutate param: T` | Can modify, caller keeps modified value |
| Take | `take param: T` | Takes ownership, caller loses value |

```rask
func display(user: User) { ... }           // borrow
func rename(mutate user: User) { ... }     // mutate
func archive(take user: User) { ... }      // take ownership
```

At the call site, `mutate` and `take` are visible:
```rask
rename(mutate user)
archive(own user)       // 'own' at call site for 'take' params
```

## Control Flow

```rask
if x > 0 {
    process(x)
}

if x > 0: do_thing()                   // single-expression form

for i in 0..10 { println(i) }          // range (0 to 9)
for item in collection { use(item) }   // iteration

while condition { work() }

// Match (expression context — produces a value)
const label = match status {
    Active => "green",
    Failed => "red",
}

// Match (statement context — side effects)
match event {
    Click(pos) => handle(pos),
    Key(k) => process(k),
}
```

## Structs and Methods

```rask
struct Point {
    x: f64
    y: f64
}

extend Point {
    func distance(self, other: Point) -> f64 {
        const dx = self.x - other.x
        const dy = self.y - other.y
        return (dx * dx + dy * dy).sqrt()
    }

    func origin() -> Point {                  // static method (no self)
        return Point { x: 0.0, y: 0.0 }
    }
}

const p = Point { x: 1.0, y: 2.0 }
p.distance(Point.origin())
```

## Enums

```rask
enum Color {
    Red
    Green
    Blue
    Custom(r: u8, g: u8, b: u8)
}

match color {
    Red => println("red"),
    Custom(r, g, b) => println("{r},{g},{b}"),
    _ => println("other"),
}
```

Variant access: `Color.Red`, `Color.Custom(255, 0, 0)`.

## Collections

```rask
// Vec
const v = Vec.new()
v.push(1)
v.push(2)
const first = v[0]         // index access
const len = v.len()

const v2 = Vec.from([1, 2, 3])

// Map
const m = Map.new()
m.insert("key", "value")
const val = m.get("key")   // returns Option
```

## Optionals (`T?` / `Option<T>`)

```rask
func find(id: i32) -> User? {
    if id == 0 { return none }
    return load(id)
}

// Pattern matching
if result is Some(user) {
    process(user)
}

// Guard pattern
const user = find(42) is Some else { return }

// Default value
const name = user?.name ?? "anonymous"

// Force unwrap (panics if None)
const user = find(42)!
```

## Error Handling (`T or E` / Result)

```rask
func read_file(path: string) -> string or IoError {
    const file = try File.open(path)    // propagates error on failure
    ensure file.close()                 // guaranteed cleanup
    return try file.read_all()
}

// try = "unwrap or return error to caller"
// ensure = "run this when the scope exits, no matter what"
```

Error unions for multiple error types:
```rask
func load() -> Config or (IoError | ParseError) {
    const text = try read_file("config.toml")
    return try parse(text)
}
```

## Ownership Rules

- Values ≤16 bytes: copy automatically (i32, f64, bool, Point, Handle)
- Values >16 bytes: move on assignment (string, Vec, Map)
- After a move, the source is invalid — compile error if used
- `.clone()` for explicit deep copy of big values

```rask
const a = 42
const b = a           // copy (small)
use(a)                // fine

const s = "hello"
const t = s           // move (big)
// use(s)             // compile error: s was moved
```

## Concurrency

```rask
import async.spawn
import thread.{Thread, ThreadPool}

// Green tasks (lightweight, for I/O)
using Multitasking {
    const h = spawn(|| { work() })
    const result = try h.join()

    spawn(|| { fire_and_forget() }).detach()
}

// Channels
let (tx, rx) = Channel<i32>.buffered(10)
tx.send(42)
const val = rx.recv()

// Shared state
const data = Shared.new(initial_value)
data.read(|d| use(d))
data.write(|d| modify(d))
```

## Imports

```rask
import io
import fs
import cli
import http
import mylib.{Parser, Lexer}
```

## Common Patterns

```rask
// String interpolation
println("x = {x}, y = {y}")

// Early return on error
const file = try fs.open(path)

// Iteration with index
for (i, item) in items.enumerate() {
    println("{i}: {item}")
}

// Chaining
const result = items
    .filter(|i| i.active)
    .map(|i| i.value)
    .collect()
```

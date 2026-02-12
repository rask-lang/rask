# Rask Syntax Design

**Goals:**
- Intuitive for Python devs (clean, readable, minimal ceremony)
- Not irritating for Rust devs (expression-oriented, pattern matching, explicit ownership)
- Not verbose like Go (no `if err != nil` noise, good inference)
- Syntactic Noise ≤ 0.3 (at most 30% ceremony tokens)

---

## Design Principles

### 1. Newlines Are Statement Terminators
No semicolons required. Use `;` only for multiple statements on one line. 
Of course, if you want you can have them, it works both ways.

```rask
const x = 1
const y = 2
const z = 3; const w = 4  // Multiple on one line
```

**Rationale:** Python devs expect newlines to matter. Rust devs won't care — semicolons are just noise most of the time.

### 2. Colon for Inline, Braces for Multi-line

Single-expression blocks use `:`. Multi-statement blocks use `{ }`.

```rask
// Inline (single expression after colon)
if x > 0: return x
const sign = if x > 0: "+" else: "-"

// Multi-line (braces and return required)
if x > 0 {
    process(x)
    return x
}
```

**Parsing rule:** `:` takes one expression until newline or next keyword. `{ }` for multiple statements.

**Why:** Python-style colon is cleaner for simple cases. Braces are explicit when you need them.

### 3. Minimal Type Annotations
Types inferred within function bodies. Public signatures require explicit types; private functions can omit them entirely.

```rask
const x = 42            // i32 inferred (within body)
const y: u64 = 42       // Explicit when needed

// Public: full signature required
public func add(a: i32, b: i32) -> i32 {
    return a + b
}

// Private: types optional (compiler infers from body)
func add(a, b) {
    return a + b
}
// Compiler infers: func add<T: Numeric>(a: T, b: T) -> T
```

See [Gradual Constraints](types/gradual-constraints.md) for full rules on omitted types, bounds, and return types.

### 4. Keywords Are English Words
Try to use readable keywords, not symbols or abbreviations.

| Concept | Rask | Rust | Go |
|---------|------|------|-----|
| Variable binding | `let` | `let` | `:=` or `var` |
| Function | `func` | `fn` | `func` |
| Return | `return` | `return` | `return` |
| Match | `match` | `match` | `switch` |
| Struct | `struct` | `struct` | `type...struct` |
| Visibility | `public` | `pub` | Capitalization |

### 5. Expression-Oriented
Everything that can be an expression is one. Blocks in expression context produce values implicitly; function bodies require explicit `return`.

```rask
const result = {
    const temp = compute()
    transform(temp)  // Last expression is the value (expression context)
}

// Function bodies: require explicit return
func compute() -> i32 {
    return 42  // Explicit return required
}
```

---

## Core Syntax

### Comments

```rask
// Line comment

/* Block comment
   can span lines */

/// Doc comment for the following item
/// Supports markdown
```

### Literals

```rask
// Numbers
42                  // i32
42u64               // u64 (suffix)
3.14                // f64
3.14f32             // f32
0xFF                // Hex
0b1010              // Binary
0o777               // Octal
1_000_000           // Underscores for readability

// Strings
"hello"             // string literal
"line 1\nline 2"    // Escape sequences
"""
Multi-line
string literal
"""

// Characters
'a'
'\n'

// Booleans
true
false
```

### Variable Bindings

```rask
const x = 42                  // Immutable binding
const name = "Alice"
let counter = 0               // Mutable binding
counter = 1                   // Reassignment

let x = "shadow"              // Shadowing allowed (IDE shows ghost annotation)
```

| Syntax | Meaning |
|--------|---------|
| `const x = v` | Immutable — cannot reassign |
| `let x = v` | Mutable — can reassign |
| `x = v` | Reassignment (variable must exist) |

**Why:** `const` means "constant" (won't change). `let` means "let it vary" (can change). Opposite of Rust but more intuitive.

---

## Declarations

### Functions

```rask
func greet(name: string) {
    println("Hello, {name}")
}

func add(a: i32, b: i32) -> i32 {
    return a + b              // Explicit return required
}

func divide(a: f64, b: f64) -> f64 or Error {
    if b == 0.0: return Err(Error.DivByZero)
    return a / b              // Auto-wrapped in Ok (explicit return)
}
```

**Private functions — types optional (gradual constraints):**
It is possible to gradually introduce type constrains in private functions. This makes it easier to write prototype code, but is discouraged in production code. Public functions must declare types.

```rask
func double(x) {
    return x * 2          // Inferred: func double<T: Numeric>(x: T) -> T
}
func greet(name) {
    println("Hi, {name}")  // Inferred: func greet(name: string) -> ()
}

// Partial annotation — mix explicit and inferred
func process(data: Vec<Record>, handler) -> () or Error {
    try handler(data)
}
// handler type inferred from usage

// Public: MUST have full types
public func serve(port: i32) -> () or Error { ... }
```

**Parameter modes:**
Default is read-only borrow. Mutation and ownership transfer are explicit.
```rask
func validate(data: Data)            // Read-only borrow (default)
func update(mutate data: Data)       // Mutable borrow (explicit)
func consume(take data: Data)        // Takes ownership
```

**Named arguments (optional, order-fixed):**
```rask
func create_user(name: string, email: string, admin: bool)

// Positional (IDE shows names as ghost text)
create_user("Alice", "alice@x.com", false)

// Named (must match declaration order)
create_user(name: "Alice", email: "alice@x.com", admin: false)
```
Named arguments improve readability but don't allow reordering. IDE shows parameter names as ghost annotations even for positional calls.

**Default arguments:**
```rask
func connect(host: string, port: i32 = 8080, timeout: i32 = 30)
func greet(name: string, greeting: string = "Hello")

// Calls
connect("localhost")                      // port=8080, timeout=30
connect("localhost", 443)                 // timeout=30
connect("localhost", timeout: 60)         // port=8080, named skips
connect(host: "localhost", timeout: 60)   // All named
```

| Rule | Description |
|------|-------------|
| Constants only | Defaults must be compile-time constants |
| Order | Optional params must come after required params |
| Skip with named | Named args can skip optional params (uses default) |

**Methods (in extend blocks):**
```rask
struct Point {
    x: i32
    y: i32
}

extend Point {
    func distance(self, other: Point) -> f64 {
        const dx = self.x - other.x
        const dy = self.y - other.y
        return sqrt((dx*dx + dy*dy) as f64)
    }

    func origin() -> Point {        // Static (no self)
        return Point { x: 0, y: 0 }
    }
}
```

Methods go in `extend` blocks, separate from data. Keeps struct/enum definitions focused on data layout.

### Structs

```rask
struct User {
    public name: string          // public = visible outside package
    public email: string
    password_hash: string     // Package-private
}

// Construction
const user = User {
    name: "Alice"
    email: "alice@example.com"
    password_hash: hash(pwd)
}

// Update syntax
const updated = User { email: "new@example.com", ..user }
```

**Unique structs** (can't be copied, can be dropped):
```rask
@unique
struct UserId {
    id: u64
}
// Prevents accidental duplication — each instance is unique
```

**Linear structs** (must consume exactly once):
```rask
@resource
struct File {
    fd: i32
}

extend File {
    func close(take self) -> () or Error {
        // ...
    }
}
// Compiler error if you forget to call close()
```

| Attribute | Copy | Drop | Use Case |
|-----------|------|------|----------|
| (none) | If ≤16 bytes | Yes | Normal values |
| `@unique` | Never | Yes | Unique IDs, tokens |
| `@resource` | Never | Never | Files, connections |

### Enums (Sum Types)

```rask
enum Status {
    Pending
    Active
    Completed(timestamp: i64)
    Failed(error: string)
}

enum Option<T> {
    Some(T)
    None
}

enum Result<T, E> {
    Ok(T)
    Err(E)
}
```
ne
```rask
enum Option<T> {
    Some(T)
    None
}

extend Option<T> {
    func is_some(self) -> bool {
        return match self {
            Some(_) => true,
            None => false,
        }
    }

    func unwrap(take self) -> T {
        return match self {
            Some(v) => v,
            None => panic("unwrap on None"),
        }
    }
}
```

### Traits

```rask
trait Display {
    func display(self) -> string
}

trait Iterator<T> {
    func next(self) -> Option<T>
}
```

**Structural matching:** If a type has the right methods, it satisfies the trait automatically.

```rask
struct Point {
    x: i32
    y: i32
}

extend Point {
    func display(self) -> string {
        return "{self.x}, {self.y}"
    }
}
// Point satisfies Display automatically
```

**Explicit trait implementation:** Use `extend Type with Trait` when you want to document intent:
```rask
extend Point with Display {
    func display(self) -> string {
        return "({self.x}, {self.y})"
    }
}
```

**Runtime polymorphism:** Use `any Trait` for heterogeneous collections:
```rask
const widgets: []any Widget = [button, textbox, slider]
for w in widgets: w.draw()    // Dynamic dispatch
```

### Generics

Unknown PascalCase identifiers in type position are automatically generic parameters:

```rask
// T is automatically a type parameter (no <T> declaration needed)
func identity(x: T) -> T {
    return x
}

func map(list: List<Item>, f: |Item| -> Result) -> List<Result> {
    // Item and Result are type parameters
    // ... implementation ...
}

struct Pair {
    first: T
    second: U
}
```

**Same name = same type:**
```rask
func swap(a: T, b: T) -> (T, T) {
    return (b, a)  // Both T must be the same type
}
```

**Omitted types entirely (gradual constraints):**
```rask
func identity(x) {
    return x                // Inferred generic: func identity<T>(x: T) -> T
}
func sum(items) {
    return items.sum()      // Inferred: func sum<T: Numeric>(items: Vec<T>) -> T
}

// Mix: explicit type + inferred bounds
func sort(items: Vec<T>) {
    items.sort()            // T is auto-generic (PascalCase), bound inferred as T: Comparable
}
```

**Context clauses with `using`:**
```rask
// Unnamed context (mutable by default)
func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount
}

// Frozen context (read-only, accepts FrozenPool too)
func get_health(h: Handle<Player>) using frozen Pool<Player> -> i32 {
    return h.health
}

// Named context (auto-resolution + structural operations)
func spawn(count: i32) using enemies: Pool<Enemy> -> Vec<Handle<Enemy>> {
    const handles = Vec.new()
    for i in 0..count {
        handles.push(try enemies.insert(Enemy.new()))
    }
    return handles
}

// Multiple contexts
func transfer(from: Handle<Player>, to: Handle<Player>, item: Handle<Item>)
    using players: Pool<Player>, items: Pool<Item>
{
    from.inventory.remove(item)
    to.inventory.add(item)
    items[item].owner = Some(to)
}
```

**Constraints with `where`:**
```rask
func sort(items: Vec<T>) -> Vec<T> where T: Ord {
    // ...
}

func process(data: T) -> U where T: Input, U: Output {
    // ...
}

// Multiple constraints on same type
func debug_sort(items: Vec<T>) where T: Ord + Debug {
    // ...
}
```

**Combined `with` and `where`:**
```rask
func process_all(handles: Vec<Handle<T>>)
    using Pool<T>
    where T: Processable
{
    for h in handles {
        h.process()
    }
}

// Full signature order: generics → params → return → with → where
public func complex<K, V>(map: Map<K, V>, key: K) -> V or NotFound
    using values: Pool<V>
    where K: HashKey, V: Clone
{
    const v_handle = try map.get(key)
    return v_handle.clone()
}
```

**Explicit declaration (disambiguation):**
```rask
// When a name conflicts with a real type, force it generic with <>
func make_item<Item>(x: Item) -> Item {
    return x
}

// Also useful for clarity in complex signatures
struct Cache<Key, Value> {
    data: Map<Key, Value>
}
```

---

## Control Flow

### If/Else

```rask
// Inline (colon + single expression)
if x > 0: println("positive")
if x > 0: return x

// Multi-line (braces)
if x > 0 {
    println("positive")
} else {
    println("non-positive")
}

// Else if
if x > 0: "positive"
else if x < 0: "negative"
else: "zero"

// As expression
const sign = if x > 0: "+" else if x < 0: "-" else: "0"

// Complex conditions (parentheses required for multi-line)
if (x > 0 && y < 10): handle()
```

**Rules:**
- No parens for simple conditions
- Parens required when condition spans multiple lines
- `:` for single expression, `{ }` for multiple statements

### Match

```rask
// Arms use => (clear separation from type annotations in patterns)
match status {
    Pending => println("waiting..."),
    Active => println("running"),
    Completed(ts) => println("done at {ts}"),
    Failed(e) => println("error: {e}"),
}

// As expression
const message = match status {
    Pending => "waiting",
    Active => "running",
    _ => "other",
}

// Multi-statement arm (braces)
match status {
    Pending => handle_pending(),
    Failed(e) => {
        log(e)
        notify_admin()
        return Err(e)
    }
}

// Pattern guards
match response {
    Ok(body) if body.len() > 0 => process(body),
    Ok(_) => handle_empty(),
    Err(e) => handle_error(e),
}

// Destructuring
match point {
    Point { x: 0, y } => println("on y-axis at {y}"),
    Point { x, y: 0 } => println("on x-axis at {x}"),
    Point { x, y } => println("at ({x}, {y})"),
}
```

### Pattern Matching in Conditions: `is`

Match a single pattern in `if` or `while` with automatic binding:

```rask
// Check enum variant with binding
if state is Connected(sock): sock.send(data)

if result is Ok(value) {
    process(value)
} else {
    handle_error()
}

// Loop while pattern matches
while reader.next() is Some(line) {
    process(line)
}

// Combined with other conditions
if state is Connected(sock) && sock.is_ready() {
    sock.send(data)
}
```

**When to use `is` vs other constructs:**

| Use Case | Recommended |
|----------|-------------|
| Check Option with binding | `if opt is Some` |
| Check other enum variant | `if x is Variant` |
| Exhaustive handling | `match` |
| Loop over iterator | `for x in iter` |

`is` is non-exhaustive — unmatched patterns skip the block. Use `match` when you need to handle all cases.

**Guard pattern with `let ... is ... else`:**

Early exits where bindings need to escape to outer scope:

```rask
let value = result is Ok else { return Err(e) }
// value available here

let sock = state is Connected else { panic("not connected") }
let item = queue.pop() is Some else { break }
let (a, b) = pair is Some else { return None }
```

The `else` block must diverge (`return`, `break`, `panic`).

### Loops

**Infinite loop with value:**
```rask
// Loop that produces a value via 'break'
const input = loop {
    const x = read_input()
    if x.is_valid(): break x    // Exit loop with value
    println("Invalid, try again")
}
```

**Infinite loop without value:**
```rask
loop {
    const conn = server.accept()
    spawn { handle(conn) }.detach()
}
```

**While:**
```rask
while queue.len() > 0 {
    const task = queue.pop()
    process(task)
}

// Inline
while running: process_next()
```

**For-in:**
```rask
for item in items: process(item)

for i in 0..10 {
    println("{i}")
}

for (key, value) in map {
    println("{key}: {value}")
}

// Step ranges
for i in (0..100).step(2): process_even(i)      // 0, 2, 4, ..., 98
for i in (10..0).step(-1): countdown(i)         // 10, 9, 8, ..., 1
for x in (0.0..1.0).step(0.25): interpolate(x)  // 0.0, 0.25, 0.5, 0.75
```

**Labels:**
```rask
outer: for row in rows {
    for cell in row {
        if cell == target: break outer
    }
}

// Break with value from labeled loop
const result = search: loop {
    for item in items {
        if item.matches(): break search item
    }
    if no_more(): break search None
}
```

### Control Transfer

| Keyword | Meaning |
|---------|---------|
| `return value` | Exit function with value |
| `return` | Exit function with `()` |
| `break` | Exit loop |
| `break label` | Exit labeled loop |
| `break value` | Exit `loop` with value |
| `break label value` | Exit labeled `loop` with value |
| `continue` | Next iteration |
| `continue label` | Next iteration of labeled loop |

---

## Ownership & Memory Syntax

Pool access, `ensure` cleanup, `@resource` types, and projection syntax are shown in the sections above (see [Structs](#structs), [Generics](#generics), [Parameter modes](#functions)). For full semantics, see:

- [pools.md](memory/pools.md) — Handle-based access, context clauses
- [ensure.md](control/ensure.md) — Deferred cleanup
- [resource-types.md](memory/resource-types.md) — Must-consume types
- [parameters.md](memory/parameters.md) — Projections (`Type.{field}`)

---

## Error Handling Syntax

```rask
// Option shorthand
const x: i32? = Some(42)
const name = user?.profile?.name    // None if any step is None
const port = config.port ?? 8080
const must_exist = optional!

// Result
func load_config() -> Config or (IoError | ParseError) {
    const content = try read_file("config.json")
    const config = try parse_json(content)
    return config                                   // Auto-wrapped in Ok
}
```

See [error-types.md](types/error-types.md), [optionals.md](types/optionals.md).

---

## Concurrency Syntax

```rask
// Spawn and join
const handle = spawn { compute() }
const result = try handle.join()
spawn { background_work() }.detach()

// Channels
const (tx, rx) = Channel<Message>.buffered(100)
try tx.send(msg)
const msg = try rx.recv()

// Select
select {
    rx1 -> msg: process1(msg),
    rx2 -> msg: process2(msg),
    Timer.after(5.seconds) -> _: handle_timeout(),
}

// Shared state
const config = Shared.new(AppConfig.default())
config.read(|c| c.timeout)
config.write(|c| c.timeout = 60.seconds)
```

See [concurrency/](concurrency/).

---

## Attributes

`@` prefix (familiar from Python decorators, Java annotations).

```rask
@layout(C)
struct CPoint {
    x: i32
    y: i32
}

@deprecated("use new_function instead")
func old_function() { ... }

@inline
func hot_path() { ... }

@no_alloc
func interrupt_handler() {
    // Compile error if any allocation occurs
    // Cannot: grow Vec, create string, use .clone() on heap types
}

func main() {
    // Program entry point — func main() is auto-detected
    // @entry available for non-main entry point names
}

test "addition" {
    assert(1 + 1 == 2)
}

```

### Attribute Summary

| Attribute | Target | Effect |
|-----------|--------|--------|
| `@entry` | Function | Marks non-main function as entry point (optional) |
| `@inline` | Function | Hint to inline |
| `@no_alloc` | Function | Compile error on heap allocation |
| `@deprecated(msg)` | Any | Warn on use |
| `@layout(C)` | Struct | C-compatible memory layout |
| `@packed` | Struct | Remove padding |
| `@align(N)` | Struct | Minimum N-byte alignment |
| `@binary` | Struct | Wire format with bit-level fields |
| `@unique` | Struct | Disable implicit copy |
| `@resource` | Struct | Must consume, cannot copy or drop |

---

## Modules and Imports

```rask
// File: math/vector.rk
public struct Vec3 {
    public x: f32
    public y: f32
    public z: f32
}

// File: main.rk
import math.vector.Vec3
import math.vector.*           // Import all public items
import math.vector.Vec3 as V3  // Alias

// Visibility
public struct Public { ... }      // Visible to dependents
struct PackagePrivate { ... }  // Default, package only
```

---

## Comparison Examples

### HTTP Handler

**Go:**
```go
func handler(w http.ResponseWriter, r *http.Request) {
    body, err := io.ReadAll(r.Body)
    if err != nil {
        http.Error(w, err.Error(), 500)
        return
    }

    var req Request
    err = json.Unmarshal(body, &req)
    if err != nil {
        http.Error(w, err.Error(), 400)
        return
    }

    result, err := process(req)
    if err != nil {
        http.Error(w, err.Error(), 500)
        return
    }

    json.NewEncoder(w).Encode(result)
}
```

**Rask:**
```rask
func handler(w: ResponseWriter, r: Request) -> () or HttpError {
    const body = try r.body.read_all()
    const req = try json.parse<Request>(body)
    const result = try process(req)
    return w.write_json(result)
}
```

**Noise comparison:**
- Go: 20 lines, ~40% error handling
- Rask: 5 lines, error handling is `try`

### File Processing

**Python:**
```python
def process_file(path):
    with open(path) as f:
        content = f.read()
    return transform(content)
```

**Rask:**
```rask
func process_file(path: string) -> Data or IoError {
    const file = try File.open(path)
    ensure file.close()
    const content = try file.read_all()
    return transform(content)
}
```

Similar structure, explicit error handling and resource cleanup.

### Iteration

**Python:**
```python
active_users = [u for u in users if u.active]
names = [u.name for u in active_users]
```

**Rask:**
```rask
const active_users = users.iter().filter(|u| u.active).collect()
const names = active_users.iter().map(|u| u.name).collect()
```

---

## Verified Examples

<!-- test: run | Hello, World -->
```rask
func greet(name: string) {
    println("Hello, {name}")
}

func main() {
    greet("World")
}
```

<!-- test: run | positive -->
```rask
const x = 5
if x > 0 {
    println("positive")
} else {
    println("non-positive")
}
```

<!-- test: run | 0\n1\n2 -->
```rask
for i in 0..3 {
    println("{i}")
}
```

<!-- test: run | two -->
```rask
const n = 2
match n {
    1 => println("one"),
    2 => println("two"),
    _ => println("other"),
}
```

<!-- test: run | 6 -->
```rask
const v = Vec.new()
v.push(1)
v.push(2)
v.push(3)
let sum = 0
for i in 0..v.len() {
    sum += v[i]
}
println("{sum}")
```

---

## Summary

| Feature | Rask Syntax | Notes |
|---------|-------------|-------|
| Statement separator | Newline | `;` optional for multiple per line |
| Inline blocks | `: expr` | Single expression after colon |
| Multi-line blocks | `{ }` | Multiple statements |
| Types | `: Type` | Inference reduces annotations |
| Functions | `func name(params) -> Type` | Familiar |
| Immutable binding | `const x = ...` | Cannot reassign |
| Mutable binding | `let x = ...` | Can reassign |
| Mutable | `mutate param` | Explicit mutable borrow |
| Ownership | `take param` | Explicit when consuming |
| Optional | `T?` | Type and chaining |
| Result | `T or E` | Same as `Result<T, E>` |
| Error prop | `try expr` | Prefix keyword |
| Match | `match x { ... }` | Expression with `=>` arms |
| Pattern condition | `if x is Pattern` | Non-exhaustive, binds `v` |
| Guard extraction | `let v = x is P else { }` | Binds to outer scope |
| Loops | `for x in xs: ...` | Inline or braced |
| Loop value | `break expr` | Exit loop with value |
| Attributes | `@name` | Familiar from Python/Java |
| Omitted types | `func f(x) { x + 1 }` | Private functions only; see [gradual constraints](types/gradual-constraints.md) |
| Generics | Implicit PascalCase | `where` for constraints |
| Closures | `\|x\| expr` | Rust-style pipes |
| Named args | `name: value` | Order-fixed, optional (IDE ghosts) |
| Default args | `param = value` | Constants only, after required |
| Projections | `Type.{field}` | Partial borrows |
| Interpolation | `"{x}"` | In all strings |
| Comments | `//` and `/* */` | Standard |

The syntax is immediately readable for Python, Rust, or Go devs, with minimal ceremony and visible ownership semantics.

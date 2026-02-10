# Rask: The Complete Walkthrough

I wrote this so I could explain Rask to anyone—including myself. No jargon without
explanation, no hand-waving. If you've written code in any language, you'll follow this.

---

## Table of Contents

1. [What Is Rask?](#1-what-is-rask)
2. [The Big Idea: Safety You Don't Have to Think About](#2-the-big-idea-safety-you-dont-have-to-think-about)
3. [Ownership: Who's Responsible for What](#3-ownership-whos-responsible-for-what)
4. [Borrowing: Looking Without Taking](#4-borrowing-looking-without-taking)
5. [Pools and Handles: The Database Approach](#5-pools-and-handles-the-database-approach)
6. [Types: What Data Looks Like](#6-types-what-data-looks-like)
7. [Error Handling: Failures Are Expected](#7-error-handling-failures-are-expected)
8. [Functions and Closures](#8-functions-and-closures)
9. [Generics and Traits: Writing Reusable Code](#9-generics-and-traits-writing-reusable-code)
10. [Resource Types: Files, Sockets, and Must-Close Things](#10-resource-types-files-sockets-and-must-close-things)
11. [Concurrency: Doing Multiple Things](#11-concurrency-doing-multiple-things)
12. [Modules and Project Structure](#12-modules-and-project-structure)
13. [Compile-Time Execution](#13-compile-time-execution)
14. [C Interop and Unsafe](#14-c-interop-and-unsafe)
15. [The Metrics: How I Score the Design](#15-the-metrics-how-i-score-the-design)
16. [Design Decision FAQ](#16-design-decision-faq)

---

## 1. What Is Rask?

Rask is a systems programming language. That means it compiles to native code, gives you
control over memory, and targets the same space as C, C++, Rust, and Go.

**The pitch:** Rask gives you Rust-level safety without Rust-level complexity. You don't
write annotations to prove your code is safe. The language is designed so that entire
categories of bugs are structurally impossible—you just can't write them.

**What it's for:** Web servers, CLI tools, data processing pipelines, games, desktop apps,
embedded systems. Most real-world software.

**What it's not for:** Operating system kernels (need too much raw pointer work) or
nanosecond-budget real-time audio (need zero overhead everywhere, even for checks).

### The Core Philosophy

I keep coming back to one sentence: **feel simple, not safe.** Safety is a property of
Rask programs, not an experience of writing them. You think about your problem, not the
language.

Eight principles drive every design decision:

1. **Safety Without Annotation** — No lifetime parameters (`'a` in Rust), no borrow
   checker wrestling. You mark `mutate` and `take` on parameters—that's it. Safety
   emerges from how the language is structured, not from proving things to the compiler.

2. **Value Semantics** — All data is a value. Small things copy automatically, big things
   move. No hidden reference vs value distinction.

3. **No Storable References** — You can look at data temporarily, but you can't save a
   pointer to someone else's data in a struct. This one rule eliminates most memory bugs.

4. **Transparent Costs** — If something allocates memory, acquires a lock, or does I/O,
   you see it in the code. Small checks (like array bounds) can be invisible.

5. **Local Analysis Only** — The compiler never needs to look at your whole program to
   check one function. This means fast compilation and predictable error messages.

6. **Resource Types** — Things like files and sockets must be explicitly closed. The
   compiler won't let you forget.

7. **Compiler Knowledge Is Visible** — The IDE shows you what the compiler figured out
   (inferred types, where copies happen, where pauses happen) as ghost text.

8. **Machine-Readable Code** — Tools can analyze your code without understanding the whole
   program. Function signatures tell you everything.

---

## 2. The Big Idea: Safety You Don't Have to Think About

To understand why Rask exists, you need to understand three problems that every systems
language has to solve.

### Problem 1: Use-After-Free

Imagine you have a variable `name` that points to some text in memory. You give it to a
function that frees the memory. Then you try to read `name`. The memory is gone. Your
program crashes—or worse, silently reads garbage.

**C/C++:** This is your problem. Good luck.
**Rust:** The compiler tracks who "owns" every piece of data and refuses to compile if
you use something after giving it away. But this requires annotations (lifetime
parameters) that make signatures complex.
**Go:** A garbage collector handles it—memory is freed automatically when nobody needs it
anymore. But this adds unpredictable pauses and hides the cost of memory management.
**Rask:** Single ownership + move semantics (like Rust) but without the annotations. The
rules are simpler and more restrictive, but they cover 80%+ of real-world code.

### Problem 2: Data Races

Two threads modify the same data at the same time. One writes "Alice", the other writes
"Bob", and you end up with "Aob". This is a data race—undefined behavior in most
languages.

**C/C++:** Your problem. Use mutexes and hope you got it right.
**Rust:** The type system prevents you from sharing mutable data across threads.
**Go:** The race detector catches it at runtime, but only if your test hits the race.
**Rask:** Same approach as Rust—the type system prevents it—but with simpler mechanisms.
Shared data goes through `Shared<T>` (read-heavy) or `Mutex<T>` (write-heavy), both
using closures that can't leak references out.

### Problem 3: Resource Leaks

You open a file, start reading it, hit an error, and return early. The file never gets
closed. Eventually you run out of file handles and your server stops accepting
connections.

**C/C++:** Manual cleanup. Easy to mess up with early returns.
**Java/Python:** Garbage collector eventually closes it. "Eventually" might be too late.
**Rust:** RAII—resources clean up when they go out of scope.
**Go:** `defer` statements run at function exit.
**Rask:** Resource types must be consumed. The compiler literally won't compile your code
if a file handle might not be closed. The `ensure` keyword schedules cleanup that runs no
matter how the scope exits.

### Rask's Approach: Eliminate or Detect

Some bugs are prevented at compile time—the compiler refuses to build the program. Others
are detected at runtime with a clear panic at the exact line, not silent corruption. Both
are better than C's "silently do the wrong thing," but they're different:

| Bug Category | How | Compile or Runtime? |
|---|---|---|
| Use-after-free | Single ownership: source invalid after move | Compile error |
| Double free | Only one owner, drops happen automatically | Compile error |
| Dangling pointer | No storable references; handles validate on access | Compile + runtime panic |
| Data race | Can't share mutable data without sync primitives | Compile error |
| Null dereference | No null; use `Option<T>` (Some or None) | Compile error |
| Buffer overflow | Array bounds checked | Runtime panic |
| Resource leak | Resource types must be consumed; compiler enforces | Compile error |
| Integer overflow | Arithmetic panics on overflow (debug AND release) | Runtime panic |
| Uninitialized memory | All variables must be initialized | Compile error |

The runtime panics (bounds checks, overflow, stale handles) crash with a clear message at
the exact line. Your program stops, but it never silently corrupts data or continues with
garbage. That matters for real-time systems—Rask won't corrupt your state, but it can
halt on a checked error.

---

## 3. Ownership: Who's Responsible for What

Every value in Rask has exactly one owner. When the owner goes away, the value is cleaned
up. This is the most fundamental rule.

### Small Values Copy, Big Values Move

Here's the key insight: some things are cheap to copy (a number, a coordinate, a color),
and some things are expensive (a big string, a list of 10,000 items).

Rask draws the line at **16 bytes**:

```rask
// Small (≤16 bytes): copies automatically
const a = 42          // i32 = 4 bytes, copies
const b = a           // b gets a copy. a is still valid.
use(a)                // ✓ Fine

// Big (>16 bytes): moves
const name = "hello world"                 // string > 16 bytes (heap allocated)
const other = name                         // other takes ownership. name is GONE.
use(name)                                  // ✗ Compile error: name was moved
```

**Why 16 bytes?** It matches what most CPUs can pass in registers. Copying 16 bytes is
essentially free—it's what the hardware does anyway for function calls. This covers
integers, floats, booleans, small structs like `Point { x: f64, y: f64 }`, small enums,
and handles.

**Why not let the programmer choose the threshold?** Because changing it changes what
your code means. If you set it to 8 bytes, a `Point { x: f64, y: f64 }` would suddenly
move instead of copy. Code that worked would break. One fixed rule keeps things
predictable.

### Explicit Clone for Big Values

If you genuinely need two copies of a big value, say so explicitly:

```rask
const name = "hello"
const backup = name.clone()    // Allocates new memory, copies bytes
use(name)                      // ✓ Both are valid
use(backup)                    // ✓ Independent copies
```

The `.clone()` call makes the cost visible. You're asking for a heap allocation—you
should see it in the code.

### Passing to Functions: Borrow by Default

When you pass a value to a function, Rask **borrows** it by default—the function gets
read-only access, and you keep the value:

```rask
func display(data: Vec<i32>) {        // Default: borrows (read-only)
    println(data.len())
}

const numbers = Vec.from([1, 2, 3])
display(numbers)                       // numbers is borrowed, not moved
use(numbers)                           // ✓ Still valid
```

To actually transfer ownership, both sides must be explicit—`take` on the parameter and
`own` at the call site:

```rask
func process(take data: Vec<i32>) {    // Explicit: takes ownership
    // data is ours now
}

const numbers = Vec.from([1, 2, 3])
process(own numbers)                   // Explicit ownership transfer
use(numbers)                           // ✗ Compile error: numbers was moved
```

No surprises. The default is safe (borrow), and ownership transfer is always visible on
both ends.

### The `discard` Keyword

Sometimes you're done with a value but you're not at the end of its scope yet:

```rask
const data = load_big_data()
const summary = analyze(data)
discard data              // Signal: done with data, free it now
// ... more code using summary but not data ...
```

`discard` explicitly drops a value early. Using the value after `discard` is a compile
error.

---

## 4. Borrowing: Looking Without Taking

If every function took ownership of its arguments, programming would be miserable. You'd
have to clone everything before passing it anywhere. Instead, most functions just *look*
at data without taking it.

Rask calls this "borrowing"—but I think of it as **viewing**. You get a view of someone
else's data. The view is temporary.

### The One Rule: "Can It Grow?"

This is the most important concept in Rask's memory model. When you access data inside a
collection, the compiler asks one question:

> **Can the thing I'm looking into change size?**

- **No** (strings, struct fields, fixed arrays) → **Persistent view.** Your view lasts
  until the end of the block. The compiler knows the data won't move because the
  container can't resize.

- **Yes** (Vec, Map, Pool) → **Instant view.** Your view lasts only until the
  semicolon. The compiler knows the container might resize (moving its contents in
  memory), which would invalidate your view.

```rask
// Persistent view: string can't grow
const s = "hello world"
const slice = s[0..5]         // View valid until end of block
process(slice)                // ✓ Still valid
more_work(slice)              // ✓ Still valid

// Instant view: Vec can grow
const v = Vec.from([1, 2, 3])
v[0].process()                // View of v[0] lives until semicolon
                              // Now the view is gone
v.push(4)                     // ✓ Fine: no active views
```

**Why does this matter?** Imagine you're holding a reference to `v[0]`, and then
someone calls `v.push(4)`. The push might need to allocate a bigger array and copy
everything. Your reference now points to freed memory. By limiting collection views to
one expression, Rask makes this impossible.

### What "Persistent" and "Instant" Mean in Practice

**Persistent views** (from fixed-size things) are comfortable—use them across multiple
lines:

```rask
const point = Point { x: 1.0, y: 2.0 }
const x_ref = point.x        // View lasts until block end
if x_ref > 0.0 {
    log(x_ref)
}
```

**Instant views** (from growable things) are one-liners—chain what you need:

```rask
// ✓ Fine: everything in one expression
vec[i].field.method()

// ✗ Can't do this:
const item = vec[i]           // View dies at semicolon
item.process()                // ✗ View already expired

// ✓ Pattern: copy out the data you need
const health = pool[h].health // Copy the i32 out (small, copies)
if health <= 0 {
    handle_death()
}
```

### The Aliasing Rule

While someone is viewing data, nobody can modify it. While someone is modifying data,
nobody else can view it. This is "exclusive access for mutation":

```rask
const v = Vec.from([1, 2, 3])

// ✗ Can't do: read and write overlap
const first = v[0]
v.push(4)                    // ✗ Error: v is being viewed

// ✓ Works: views don't overlap
v[0].process()               // View ends at semicolon
v.push(4)                    // ✓ No view active
```

### Multi-Statement Collection Access

When you need to do multiple things with a collection element, use `with...as`:

```rask
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}
```

This borrows the element for the whole block. The pool is locked during this block—you
can't touch other elements in the same pool. When the block ends, the borrow ends.

### Field Projections: Partial Borrowing

Sometimes a function only needs one field of a struct. Field projections let the compiler
know, so other fields remain available:

```rask
func movement_system(state: GameState.{entities}, dt: f32) {
    // Only touches state.entities
}

func scoring_system(state: GameState.{score}, points: i32) {
    // Only touches state.score
}

// These can theoretically run in parallel—they touch different fields
movement_system(state.{entities}, dt)
scoring_system(state.{score}, 10)
```

**Why `.{entities}` with braces, not just `.entities`?** Because `state.entities` is
normal field access—it gives you the value. `state.{entities}` creates a *projection*—a
restricted view of the struct where only that field is accessible. The braces
disambiguate "I'm reading a field" from "I'm creating a partial view for borrowing
purposes." The compiler verifies you don't touch anything outside the projection.

---

## 5. Pools and Handles: The Database Approach

Here's where Rask diverges most from other languages. In most languages, when you need
objects that reference each other (a tree, a graph, a game entity system), you use
pointers or references. Rask says: no storable references. So how do you build a graph?

**Pools and handles. Think of them like database tables and primary keys.**

### The Mental Model

Imagine a database table called `entities`:

| ID | Health | Position | Velocity |
|---|---|---|---|
| 0 | 100 | (10, 20) | (1, 0) |
| 1 | 80 | (30, 40) | (0, -1) |
| 2 | 50 | (50, 60) | (-1, 1) |

You don't store a pointer to row 1. You store the ID `1`. When you want to access it,
you look it up by ID. If someone deleted row 1, the lookup tells you it's gone—you don't
crash by accessing freed memory.

That's exactly what `Pool<T>` and `Handle<T>` do.

### Using Pools

```rask
// Create a pool (like creating a table)
let entities = Pool<Entity>.new()

// Insert (like INSERT INTO)
const h1 = try entities.insert(Entity { health: 100, pos: Point.origin() })
const h2 = try entities.insert(Entity { health: 80, pos: Point.new(10, 20) })

// Access (like SELECT ... WHERE id = h1)
entities[h1].health -= 10     // Read/modify a field

// Remove (like DELETE WHERE id = h1)
entities.remove(h1)

// Stale handle detection
entities[h1].health            // ✗ Runtime panic: stale handle!
```

### How Handle Validation Works

Each handle contains three pieces:

```
Handle = {
    pool_id: u32,       // Which pool this belongs to
    index: u32,         // Slot number in the pool
    generation: u32,    // Version counter
}
```

Every time a slot is reused (something removed, something new inserted in the same slot),
the pool bumps the generation counter for that slot. When you access `pool[handle]`, it
checks:

1. **Pool ID matches?** This handle came from *this* pool, not some other pool.
2. **Generation matches?** The handle was created for the *current* occupant of this
   slot, not something that was removed.
3. **Index in bounds?** The slot number is valid.

If any check fails, you get a clear panic at the exact line where you used the stale
handle. Not a mysterious crash somewhere later.

### Handles Are Values, Not References

A handle is 12 bytes (4 + 4 + 4). That's under the 16-byte copy threshold, so **handles
copy automatically**:

```rask
const h1 = try pool.insert(entity)
const h2 = h1     // h2 is a copy. Both point to the same entity.
pool[h1].health   // ✓ Works
pool[h2].health   // ✓ Also works, same entity
```

This is like having two copies of a database primary key. Both refer to the same row.
Neither is "the" owner of the row—the pool owns the data.

### Handles Replace Pointers

Anywhere you'd use a pointer in C or a reference in Rust, you use a handle in Rask:

```rask
// Parent pointer? Store a handle.
struct TreeNode {
    value: i32
    parent: Handle<TreeNode>     // Not a pointer—a handle
    children: Vec<Handle<TreeNode>>
}

// Graph edges? Store handles.
struct GraphNode {
    data: string
    neighbors: Vec<Handle<GraphNode>>
}
```

### The Cost and the Benefit

**Cost:** Every `pool[handle]` access does a generation check. That's 1-2 nanoseconds.
In a tight loop doing millions of accesses per frame, that adds up.

**Benefit:** Use-after-free is impossible. Iterator invalidation is caught immediately.
Self-referential structures work naturally. No lifetime annotations anywhere.

**Mitigations for the cost:**
- The compiler coalesces checks: `pool[h].x = 1; pool[h].y = 2; pool[h].z = 3` becomes
  one check, not three.
- Frozen pools (`pool.freeze()`) skip all checks—you promise no inserts/removes happen.
- Copy out values you need: `const hp = pool[h].health` does one check, then `hp` is
  just a number.

### Cursor Iteration: Safe Modification During Loops

A common need: iterate over entities and remove some:

```rask
for h in pool.cursor() {
    pool[h].update()
    if pool[h].expired {
        pool.remove(h)    // ✓ Safe during cursor iteration
    }
}
```

The cursor handles removals gracefully—it adjusts so you never skip or double-visit
elements.

### Context Clauses: Automatic Pool Threading

If a function always needs access to a specific pool, context clauses thread it
automatically:

```rask
func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount    // Compiler resolves which Pool<Player> to use
}

const players = Pool<Player>.new()
const h = try players.insert(Player { health: 100 })
damage(h, 10)    // Compiler passes `players` automatically
```

The `with Pool<Player>` clause says "this function needs access to a player pool." The
compiler finds it in the caller's scope and passes it implicitly. The caller doesn't need
to thread it through manually.

---

## 6. Types: What Data Looks Like

### Primitives

These are always available, no imports needed:

| Type | What It Is | Size |
|---|---|---|
| `i8` through `i64` | Signed integers | 1-8 bytes |
| `u8` through `u64` | Unsigned integers | 1-8 bytes |
| `f32`, `f64` | Floating point | 4, 8 bytes |
| `bool` | True or false | 1 byte |
| `char` | Unicode character | 4 bytes |
| `string` | UTF-8 text | >16 bytes (heap) |
| `()` | Unit (nothing) | 0 bytes |

Integer and float conversions are explicit. Widening (small to big) uses `as`:

```rask
const x: i8 = 42
const y: i32 = x as i32     // ✓ Safe: i8 always fits in i32
```

Narrowing (big to small) uses explicit keyword operations—because data might be lost:

```rask
const big: i32 = 300
const small = big truncate to u8      // Wraps: 300 → 44
const clamped = big saturate to u8    // Clamps: 300 → 255
const checked = big try convert to u8 // Returns Option<u8>: None (300 > 255)
```

### Integer Overflow: Panic by Default

Unlike C (wraps silently) or Rust (wraps in release builds), Rask panics on overflow in
both debug AND release:

```rask
const x: u8 = 255
const y = x + 1    // Panic: "integer overflow: 255 + 1 exceeds u8 range"
```

**Why?** Silent wrapping hides bugs. Rust's approach (panic in debug, wrap in release)
means release builds have different behavior than debug builds—bugs that only appear in
production.

**When you actually want wrapping** (hash functions, checksums):

```rask
const h = Wrapping(5381u32)
const result = h * Wrapping(33) + Wrapping(byte as u32)   // Wraps on overflow
```

**When you want clamping** (audio levels, color values):

```rask
const volume = Saturating(200i16) + Saturating(100i16)  // Clamps to i16.MAX
```

The compiler is smart about eliminating unnecessary checks. If it can prove overflow
is impossible (like `for i in 0..100 { sum += i }`), it skips the check.

### Structs

Structs are named bundles of data. All fields must have names and types:

```rask
struct Point {
    x: f64
    y: f64
}

const p = Point { x: 1.0, y: 2.0 }
```

Structs are values. If they're ≤16 bytes and all fields copy, they copy automatically.
Otherwise they move.

**Methods** live in `extend` blocks, separate from the data definition:

```rask
extend Point {
    func distance(self, other: Point) -> f64 {
        const dx = self.x - other.x
        const dy = self.y - other.y
        return (dx * dx + dy * dy).sqrt()
    }

    func origin() -> Point {
        return Point { x: 0.0, y: 0.0 }
    }
}
```

`self` means "this instance." `origin()` has no `self`—it's a static method, called as
`Point.origin()`.

**Visibility:** Fields are package-visible by default (any file in the same package can
see them). Add `public` to expose them externally:

```rask
struct User {
    public name: string       // External code can see this
    password_hash: string     // Only same package can see this
}
```

If any field is non-public, external code can't construct the struct directly. They need
a factory function like `User.new(name, password)`.

### Enums (Tagged Unions)

An enum is a type that can be one of several variants. Each variant can carry data:

```rask
enum Shape {
    Circle(radius: f64)
    Rectangle(width: f64, height: f64)
    Triangle(a: f64, b: f64, c: f64)
}
```

You match on enums to figure out which variant you have:

```rask
match shape {
    Circle(r) => 3.14159 * r * r,
    Rectangle(w, h) => w * h,
    Triangle(a, b, c) => herons_formula(a, b, c),
}
```

The compiler verifies you handle every variant. If you add a new variant, every `match`
that doesn't handle it becomes a compile error.

**In expression context** (when assigned to a variable), match produces a value—the last
expression in each arm. **In statement context** (just doing side effects), each arm is
just code to run.

### Optionals: `T?`

Instead of null, Rask uses `Option<T>`—a value that's either `Some(value)` or `None`:

```rask
func find_user(id: i32) -> User? {      // User? = Option<User>
    if id == 0 { return none }
    return load_from_db(id)
}
```

**Optional chaining** (like Swift/Kotlin/TypeScript):

```rask
const theme = user?.profile?.settings?.theme ?? "default"
```

If any step is `None`, the whole chain becomes `None`. The `??` provides a fallback.

**Force unwrap** (when you're sure it's not None):

```rask
const user = get_user()!    // Panics if None
```

**Safe unwrap with `if`:**

```rask
if user? {
    // user is unwrapped here—it's a User, not User?
    process(user)
}
```

---

## 7. Error Handling: Failures Are Expected

Rask separates two kinds of failures:

| Kind | Mechanism | Examples |
|---|---|---|
| **Expected failures** (things that can reasonably go wrong) | Return `Result` | File not found, network down, invalid input |
| **Programmer errors** (bugs in the code) | Panic | Array out of bounds, overflow, violated invariant |

### Result Type: `T or E`

Functions that can fail return a Result:

```rask
func read_file(path: string) -> string or IoError {
    const file = try File.open(path)      // If this fails, return the error
    const content = try file.read_all()   // Same here
    return content                        // Auto-wrapped in Ok()
}
```

`T or E` is shorthand for `Result<T, E>`. The `try` keyword extracts the success value
or returns the error to the caller immediately.

**Without `try`, you'd write:**

```rask
match File.open(path) {
    Ok(file) => { ... }
    Err(e) => return Err(e),
}
```

`try` is just shorthand for that pattern. It's a prefix operator—goes before the
expression.

### Error Unions

When a function can fail with multiple error types:

```rask
func load_config() -> Config or (IoError | ParseError) {
    const text = try read_file("config.toml")    // IoError auto-widens to union
    const config = try parse(text)               // ParseError auto-widens too
    return config
}
```

The `|` means "or this kind of error." The compiler checks that each `try` expression's
error type is included in the return type's error union.

### Panic vs Error: The Design Choice

**Errors** are for things the caller can do something about. The user typed a bad
filename? Return an error—the caller can ask for a different filename.

**Panics** are for things that mean the program has a bug. Accessing array index 10
when the array has 5 elements? That's a logic error. There's nothing reasonable the
caller can do. Panic with a clear message.

```rask
const x = try file.read()       // Error: caller handles it
const y = array[10]              // Panic: programmer made a mistake
```

### Result Methods

| Method | What It Does |
|---|---|
| `result.on_err(default)` | Returns the value or a default (discards error) |
| `result.ok()` | Converts `Ok(v)` to `Some(v)`, `Err` to `None` |
| `result.map(f)` | Transforms the success value |
| `result.map_err(f)` | Transforms the error value |
| `result!` | Force-unwraps: panics if `Err` |
| `result! "context"` | Force-unwraps with a custom panic message |

### Auto-Ok Wrapping

When your function returns `T or E`, you can return a `T` directly—the compiler wraps it
in `Ok` for you:

```rask
func parse_int(s: string) -> i32 or ParseError {
    // ... validation ...
    return 42    // Compiler makes this Ok(42)
}
```

Functions returning `() or E` that reach the end without returning automatically return
`Ok(())`.

---

## 8. Functions and Closures

### Parameter Modes: Three Ways to Pass Data

Every parameter has a mode that says what the function can do with it:

| Mode | Syntax | What It Means | After the Call |
|---|---|---|---|
| **Borrow** (default) | `param: T` | Read-only access | Caller keeps the value |
| **Mutate** | `mutate param: T` | Can modify | Caller keeps (modified) value |
| **Take** | `take param: T` | Takes ownership | Caller loses the value |

```rask
func display(user: User) {              // Borrow: just looking
    println(user.name)
}

func rename(mutate user: User) {        // Mutate: changing it
    user.name = "New Name"
}

func archive(take user: User) {         // Take: consuming it
    database.insert(own user)
}
```

The mutation and ownership transfer are visible in the function signature AND at the call
site. No guessing.

### Self Parameter

Methods use the same modes for `self`:

```rask
extend File {
    func size(self) -> usize { ... }             // Read-only
    func write(mutate self, data: []u8) { ... }  // Modifies the file
    func close(take self) { ... }                // Consumes the file
}
```

### Closures: Functions That Capture Context

Closures are anonymous functions. In Rask, they come in three flavors based on how they
interact with surrounding variables:

**Storable closures** — capture by value (copy or move). Can be stored in structs,
returned, or sent across threads:

```rask
const multiplier = 3                       // i32 = 4 bytes, copies into closure
const triple = |x| x * multiplier
triple(10)    // 30
use(multiplier)                            // ✓ Still valid (was copied)

const name = "Alice"
const greet = || println("Hello, {name}")  // string > 16 bytes, MOVES into closure
greet()    // "Hello, Alice"
// name is INVALID here — it was moved
```

**Immediate closures** — don't capture anything, just access outer scope directly. Must be
called immediately (like in iterator chains):

```rask
const items = vec_of_things()
const result = items
    .filter(|i| i.active)       // Accesses items directly, no capture
    .map(|i| i.value * 2)
    .collect()
```

**Local-only closures** — capture some borrowed data. Can't outlive the scope of that
data:

```rask
const s = get_string()
const slice = s[0..5]
const f = || process(slice)    // Captures a view—can't escape this scope
f()                            // ✓ Fine: same scope
// return f                    // ✗ Would be a compile error
```

The compiler figures out which kind you need. You just write `|args| body` and the
compiler tells you if you're violating the rules.

### Return Semantics

Functions require explicit `return`. Blocks in expression context use implicit last
expression:

```rask
// Functions: explicit return
func double(x: i32) -> i32 {
    return x * 2
}

// Blocks in expression context: implicit last expression
const result = if x > 0 {
    x * 2     // This is the value of the if-block
} else {
    0         // This is the value of the else-block
}
```

**Why the difference?** `return` exits the entire function. But `if` and `match` blocks
just need to produce a value — they're expressions, not exit points. The last expression
in a block naturally becomes what that block "evaluates to."

---

## 9. Generics and Traits: Writing Reusable Code

### Generics: Code That Works With Many Types

```rask
func max<T: Comparable>(a: T, b: T) -> T {
    if a.compare(b) == Greater { return a }
    return b
}
```

`T` is a placeholder. `T: Comparable` means "T must have a `compare` method." The
compiler generates a separate copy of the function for each concrete type you call it
with — one for `i32`, one for `f64`, etc. This is called **monomorphization** and means
zero runtime overhead.

### Traits: Shared Behavior

A trait says "any type that has these methods can be used here." It's a contract:

```rask
trait Comparable {
    func compare(self, other: Self) -> Ordering
}
```

Any type with a matching `compare` method automatically satisfies this trait. You don't
have to declare "I implement Comparable"—the compiler checks structurally.

```rask
struct Score {
    value: i32
}

extend Score {
    func compare(self, other: Score) -> Ordering {
        return self.value.compare(other.value)
    }
}

max(Score { value: 10 }, Score { value: 20 })  // ✓ Works: Score has compare()
```

This is called **structural trait satisfaction**—if the methods match, the trait is
satisfied. Like how a USB cable fits any USB port without needing to register with a
central authority.

Some traits are marked `explicit`—these require an explicit `extend...with` block. This
protects against accidental satisfaction when a method happens to have the right signature
but different semantics.

### Operator Overloading

Operators desugar to trait methods. `a + b` becomes `a.add(b)`. Define `add` and you get
`+`:

```rask
extend Point {
    func add(self, other: Point) -> Point {
        return Point { x: self.x + other.x, y: self.y + other.y }
    }
}

const p3 = p1 + p2    // Calls p1.add(p2)
```

| Operator | Method | Trait | Copy Required? |
|---|---|---|---|
| `+` `-` `*` `/` `%` | `add`, `sub`, `mul`, `div`, `rem` | Arithmetic | Yes |
| `+=` `-=` `*=` `/=` `%=` | `add_assign`, `sub_assign`, etc. | Compound assign | No |
| `==` `!=` | `eq` | Equal | No |
| `<` `>` `<=` `>=` | `cmp` | Ordered | No |

**Arithmetic operators require Copy.** `a + b` looks cheap—like a register operation. If
it secretly heap-allocates, that violates transparent cost. So arithmetic operator traits
(`Add`, `Sub`, etc.) require the type to be Copy (≤16 bytes, no heap). This works for
primitives, `Point`, `Complex`, `Wrapping<T>`, SIMD vectors—all small value types.

**Compound assignment (`+=`) has no Copy requirement.** `a += b` mutates in place—it
doesn't create a new value. For heap types like a hypothetical `BigInt`, `+=` can grow the
internal buffer without allocating a temporary:

```rask
// BigInt can't use +, but += works
let total = BigInt.from("0")
total += amount                    // Mutates in place
const sum = a.add(b)               // Named method for creating new values
```

This matches the string design—`string` has no `+` operator either. You use `.concat()`
or `string_builder`. Same principle: if it allocates, the method name says so.

### Gradual Constraints: Types Optional for Private Code

Public functions must spell out all types and constraints. Private functions can skip them:

```rask
// Fully explicit (required for public functions)
public func find_best<T: Copy, U: Comparable>(items: Vec<T>, score: |T| -> U) -> T {
    // ...
}

// Partially explicit (OK for private functions)
func find_best(items, score) {
    // Compiler infers: items is Vec<T>, score is |T| -> U
    // Infers T: Copy and U: Comparable from how they're used
}
```

This is NOT dynamic typing. The compiler still checks everything. It just infers the
types from how you use the parameters. The IDE shows the inferred types as ghost text.

**The pipeline:** Prototype with no types → add types as you solidify → make it public
with full types. Safety at every step.

### Runtime Polymorphism: `any Trait`

Sometimes you need a collection of different types that share behavior—like a list of UI
widgets where each is a different type:

```rask
trait Widget {
    func draw(self)
    func size(self) -> (i32, i32)
}

let widgets: []any Widget = [button, textbox, slider]
for w in widgets {
    w.draw()    // Calls the right draw() for each type at runtime
}
```

`any Widget` means "I don't know the concrete type, but I know it can `draw()` and
`size()`." The compiler forgets the specific type and only remembers which methods are
available. At runtime, calling `w.draw()` does one extra lookup to find the right
function — a tiny cost.

**Trade-off:**
- Generics (`T: Widget`): No extra lookup, but every item in the list must be the same
  type. You can't mix buttons and sliders.
- `any Widget`: One pointer lookup per call, but items can be different types.

Use `any Trait` when you need mixed types in one collection (HTTP handlers, plugins, UI
widgets). Use generics when everything is the same type.

---

## 10. Resource Types: Files, Sockets, and Must-Close Things

Some values represent external resources—file handles, network sockets, database
connections. If you forget to close them, bad things happen (file handle leaks, connection
exhaustion).

### The `@resource` Annotation

```rask
@resource
struct File { ... }
```

`@resource` tells the compiler: this value MUST be consumed. You can't just let it go out
of scope—you must explicitly close, commit, or otherwise clean it up.

```rask
const file = try File.open("data.txt")
// ... use file ...
try file.close()    // ✓ Consumed: compiler is happy

// Without close:
const file = try File.open("data.txt")
// ... use file ...
// ✗ Compile error: resource not consumed
```

### The `ensure` Keyword: Guaranteed Cleanup

The problem with putting `file.close()` at the end of a function: what if you return
early? What if an error happens on line 3?

`ensure` schedules cleanup that runs no matter how the scope exits:

```rask
func process_file(path: string) -> Data or Error {
    const file = try File.open(path)
    ensure file.close()           // Scheduled now. Runs when scope exits.

    const header = try file.read_header()    // If this fails → ensure runs
    const body = try file.read_body()        // If this fails → ensure runs
    try validate(body)                       // If this fails → ensure runs

    return parse(body)
    // ensure runs here too (normal exit)
}
```

`ensure` is registered immediately but executed later. It's like Go's `defer` but
scoped to the block, not the function, and it satisfies the compiler's resource tracking.

**LIFO order:** Multiple `ensure` statements run in reverse order (last registered =
first to run):

```rask
const a = try open("a.txt")
ensure a.close()               // Runs second

const b = try open("b.txt")
ensure b.close()               // Runs first
```

### Transaction Pattern

`ensure` pairs beautifully with transactions:

```rask
const tx = try db.begin()
ensure tx.rollback()          // Safety net: rollback if anything fails

try tx.insert(record1)
try tx.insert(record2)

tx.commit()                   // ✓ Consumes tx. ensure is cancelled.
```

If `commit()` is reached, it consumes the transaction and `ensure` is cancelled (nothing
to rollback). If any `try` fails, we return early and `ensure` rolls back.

### Resource Types vs Unique Types

| | `@resource` | `@unique` |
|---|---|---|
| Can be dropped silently | No—must be consumed | Yes |
| Can be cloned | No | Yes (explicit `.clone()`) |
| Implicit copy | No | No |
| Use case | External resources (files, sockets) | Identity values, performance control |

`@unique` has two uses: preventing logic errors (like accidentally duplicating a unique
ID or auth token) and controlling performance (opting a small struct out of implicit
copying in hot paths where you want explicit moves instead). `@resource` is for values
where forgetting cleanup would leak a resource.

---

## 11. Concurrency: Doing Multiple Things

### No Function Coloring

In many languages (JavaScript, Rust, C#), async functions are different from regular
functions. You mark them with `async`, call them with `await`, and they can't be mixed
freely with synchronous code. This is called "function coloring"—async functions are a
different color than sync functions.

Rask doesn't do this. A function is a function. If it does I/O, the runtime handles
pausing and resuming automatically. You write the same code whether you're in async
context or not.

```rask
func fetch_data(url: string) -> Data or Error {
    const response = try http.get(url)    // Pauses here (if in async context)
    return try response.json()            // Pauses here too
}
```

No `async`. No `await`. The IDE shows pause points as ghost annotations so you know
where yields happen, but your code doesn't carry that burden.

### Three Ways to Do Work in Parallel

| Construct | What It Does | Overhead | Use For |
|---|---|---|---|
| `spawn { }` | Creates a green task (lightweight) | ~4KB per task | I/O-heavy work, thousands of concurrent operations |
| `spawn thread { }` | Sends work to a thread pool | OS thread cost | CPU-heavy computation |
| `spawn raw { }` | Creates a raw OS thread | Heaviest | Special cases (thread-local storage, specific scheduling) |

### Green Tasks: Lightweight Concurrency

Green tasks are like goroutines in Go. They're managed by a scheduler in the runtime, not
by the OS. You can have 100,000+ of them.

```rask
func main() {
    const scheduler = Multitasking.new()
    const listener = try TcpListener.bind("0.0.0.0:8080")

    loop {
        const conn = try listener.accept()
        spawn { handle_connection(conn) }.detach()
    }
}
```

The scheduler is **opt-in**—it doesn't run by default. A plain Rask program has zero
scheduler overhead. `Multitasking.new()` is what spins it up, and `spawn { }` only
works when a `Multitasking` value is in scope. Each incoming connection gets its own
task—cheap enough that you don't worry about it.

### Task Handles Are Affine

Every `spawn` returns a handle that you must deal with:

```rask
const h = spawn { compute_something() }
const result = try h.join()    // Wait for it, get the result

// Or explicitly detach (fire-and-forget):
spawn { log_event(event) }.detach()

// ✗ Compile error: handle not used
spawn { work() }    // ERROR: unused TaskHandle
```

This prevents silently losing track of spawned work. If you meant fire-and-forget, say
`.detach()` explicitly.

### Thread Pool: CPU Parallelism

Green tasks interleave on a few OS threads—they're concurrent but not truly parallel. For
CPU-heavy work (parsing, compression, number crunching), use the thread pool:

```rask
func main() {
    const scheduler = Multitasking.new()
    const pool = ThreadPool.new()

    const data = try fetch(url)                                // I/O: pauses task
    const result = try spawn thread { analyze(data) }.join()   // CPU: actual parallelism
    try save(result)                                           // I/O: pauses task
}
```

### Channels: Communication Between Tasks

```rask
let (tx, rx) = Channel<Message>.buffered(100)

const producer = spawn {
    for msg in messages {
        try tx.send(msg)     // Pauses if buffer full
    }
}

const consumer = spawn {
    while rx.recv() is Ok(msg) {
        process(msg)
    }
}

try join_all(producer, consumer)
```

Sending on a channel transfers ownership. The sender can't use the value after sending.

### Shared State

When you need shared mutable state across tasks, use synchronization primitives:

**`Shared<T>`** — for data that's read often, written rarely:

```rask
let config = Shared.new(AppConfig { timeout: 30 })

// Many readers (concurrent, non-blocking)
const timeout = config.read(|c| c.timeout)

// Exclusive writer (blocks readers during write)
config.write(|c| c.timeout = 60)
```

**`Mutex<T>`** — for data that's written often:

```rask
const queue = Mutex.new(Vec.new())
queue.lock(|q| q.push(item))
```

Both use closures instead of lock guards. This means you can't accidentally hold a lock
across an await point or forget to unlock. When the closure returns, the lock is released.

**Atomics** — for single values (counters, flags):

```rask
const counter = AtomicU64.new(0)
counter.fetch_add(1, Relaxed)    // Lock-free increment
```

### Select: Waiting on Multiple Channels

```rask
select {
    rx1 -> msg: handle_message(msg),
    rx2 -> event: handle_event(event),
    Timer.after(5.seconds) -> _: handle_timeout(),
}
```

Waits until one channel is ready, then runs that arm. If multiple are ready, picks
randomly to prevent starvation.

### Cancellation

Tasks check a cancellation flag cooperatively:

```rask
const h = spawn {
    const file = try File.open("data.txt")
    ensure file.close()       // ALWAYS runs, even on cancel

    loop {
        if cancelled() { break }
        do_work()
    }
}

sleep(5.seconds)
try h.cancel()    // Request cancellation
```

`ensure` blocks always run—cancellation doesn't skip cleanup.

---

## 12. Modules and Project Structure

### Package = Directory

Every directory with `.rk` files is a package. The directory name is the package name:

```
myapp/
  main.rk           // package: myapp
  config.rk          // package: myapp (same)
  handlers/
    auth.rk          // package: myapp.handlers
    api.rk           // package: myapp.handlers (same)
```

All files in a directory can see each other's non-public items. No file-private
visibility—if it's in the package, the whole package can use it.

### Imports

```rask
import io                      // Use as: io.print()
import http                    // Use as: http.get()
import mylib.Parser            // Use directly: Parser
import mylib.{Parser, Lexer}   // Multiple items
import mylib as ml             // Alias: ml.Parser
```

### Visibility

Two levels:

- **Package-visible** (default): Any file in the same package can see it.
- **Public**: Any external package can see it. Requires the `public` keyword.

```rask
func helper() { ... }                 // Package-visible only
public func api_endpoint() { ... }    // Visible to importers
```

### Build Configuration

Projects use a `rask.build` file (not TOML—it's Rask syntax):

```rask
package "myapp" "1.0.0" {
    dep "http" "^2.0"
    dep "json" "^1.5"

    scope "dev" {
        dep "mock-server" "^2.0"
    }

    feature "ssl" {
        dep "openssl" "^3.0"
    }
}
```

Build scripts are written as a function in the same file:

```rask
func build(ctx: BuildContext) -> () or Error {
    const schema = try fs.read_file("schema.json")
    try ctx.write_source("types.rk", generate_types(schema))
}
```

---

## 13. Compile-Time Execution

Rask can run code at compile time to compute constants, generate types, and embed files.

### `comptime` Blocks

```rask
const PRIMES = comptime {
    const v = Vec.new()
    for n in 2..100 {
        if is_prime(n) { v.push(n) }
    }
    v.freeze()    // Convert to fixed array at compile time
}
```

The block runs during compilation. The result is baked into the binary. At runtime,
`PRIMES` is just a fixed array—no computation needed.

### What You Can Do at Compile Time

- Arithmetic, loops, conditionals, function calls
- Build Vecs, Maps, strings (must `.freeze()` to use at runtime)
- Embed files: `comptime @embed_file("schema.json")`
- Type operations: `sizeof<Point>()`, `alignof<Point>()`

### What You Can't Do at Compile Time

- I/O (except embedding files)
- Use pools, concurrency, or unsafe code
- Infinite loops (there's a limit on iterations)

### Conditional Compilation

```rask
comptime if cfg.os == "linux" {
    const SOCKET_TYPE = EpollSocket
} else if cfg.os == "macos" {
    const SOCKET_TYPE = KqueueSocket
}
```

`cfg` provides platform info: `cfg.os`, `cfg.arch`, `cfg.profile`, `cfg.features`.

### Comptime vs Build Scripts

| | Comptime | Build Script |
|---|---|---|
| Runs during | Compilation | Before compilation |
| Can do I/O | Only `@embed_file` | Full I/O |
| Can call C | No | Yes |
| Use for | Constants, lookup tables, type selection | Code generation, external tools |

---

## 14. C Interop and Unsafe

### Calling C Code

```rask
import c "sqlite3.h" as sqlite

func main() {
    unsafe {
        const db = try sqlite.open("mydb.sqlite")
        // ...
    }
}
```

All C calls require `unsafe` because Rask can't verify C code follows its safety rules.

### When `unsafe` Is Required

- Dereferencing raw pointers (`*ptr`)
- Pointer arithmetic (`ptr.add(n)`)
- Calling C functions
- Transmuting types (reinterpreting bits)
- Inline assembly
- Accessing mutable statics

### Debug vs Release Safety

In debug mode, `unsafe` blocks get extra runtime checks (null pointer detection,
bounds checking on pointers, use-after-free detection). In release mode, these checks are
removed for performance.

This is pragmatic: catch bugs during development, run fast in production.

### Raw Pointer Types

| Type | Description |
|---|---|
| `*T` | Immutable raw pointer |
| `*mut T` | Mutable raw pointer |

Unlike handles, raw pointers have no validation. They can be null, dangling, or
misaligned. That's why they require `unsafe`.

---

## 15. The Metrics: How I Score the Design

Every design decision is measured against eight metrics. ALL must pass:

| Metric | Target | What It Measures |
|---|---|---|
| **Transparency (TC)** | ≥ 0.90 | Can you see the costs in the code? |
| **Mechanical Correctness (MC)** | ≥ 0.90 | How many bug categories are impossible? |
| **Use Case Coverage (UCC)** | ≥ 0.80 | What percentage of real software can you build? |
| **Predictability (PI)** | ≥ 0.85 | Can you predict behavior, memory use, performance? |
| **Ergonomic Delta (ED)** | ≤ 1.2x | How much longer is Rask vs the simplest language for each task? |
| **Syntactic Noise (SN)** | ≤ 0.30 | What fraction of tokens are ceremony vs logic? |
| **Runtime Overhead (RO)** | ≤ 1.10x hot | How much slower than C/Rust in hot paths? |
| **Compilation Speed (CS)** | ≥ 5x Rust | How fast does it compile? |

**Red flags I watch for:**
- If error handling is longer than the happy path → noise too high
- If Rask needs 3+ lines where Go needs 1 → ergonomics failing
- If a hot loop is 10%+ slower than C → runtime overhead too high
- If safety requires annotations → not invisible enough

**Test programs that must work naturally:**
1. HTTP JSON API server
2. grep clone
3. Text editor with undo
4. Game loop with entities
5. Embedded sensor processor

---

## 16. Design Decision FAQ

### Why no garbage collector?

GC introduces unpredictable pauses and hides memory costs. Rask's principle #4
(Transparent Costs) says you should see where memory is allocated and freed. With
ownership and move semantics, you get deterministic cleanup without a GC.

### Why no lifetime annotations (like Rust)?

Lifetimes are powerful but create complexity that ripples through your entire codebase.
One function needing a lifetime infects every function that calls it. Rask trades some
flexibility (no storable references) for zero annotation burden. Handles + pools cover
the cases where you'd need complex lifetimes in Rust.

### Why the 16-byte copy threshold?

It matches what CPUs pass in registers. Below 16 bytes, copying is literally free—it's
what the hardware does anyway. The threshold is fixed (not configurable) because changing
it would change program semantics. One rule, always.

### Why no storable references?

This is the keystone decision. By forbidding stored references:
- Use-after-free: eliminated (no dangling references)
- Iterator invalidation: eliminated (no references into collections)
- Lifetime annotations: eliminated (nothing to track)
- Data races on references: eliminated (no shared references across threads)

The cost is restructuring some patterns. Where you'd store a `&str` in Rust, you store a
`StringSlice` handle in Rask. Where you'd store a `&Node`, you store a `Handle<Node>`.

### Why `try` instead of `?` (like Rust)?

`try` reads left-to-right: "try to open the file." Rust's `?` is a postfix operator that
some find easy to miss. It's a readability preference—either works mechanically.

### Why panic on integer overflow in release builds?

Rust panics in debug but wraps in release. This means release builds have different
behavior. Bugs that only appear in production are the worst kind. Rask panics in both
modes—same behavior everywhere. Use `Wrapping<T>` when you genuinely want wrapping.

The compiler eliminates most overflow checks anyway through range analysis.

### Why closures capture by value instead of by reference?

If closures captured by reference, they'd need lifetime tracking—exactly the complexity
Rask eliminates by forbidding storable references. Capture by value means closures own
their data and can be stored, sent across threads, or returned freely.

The cost: mutating shared state through closures requires a Pool + Handle pattern instead
of direct mutation. I think that's a reasonable trade for not needing lifetimes.

### Why separate `Shared<T>` and `Mutex<T>` (instead of just `Mutex`)?

They optimize for different access patterns:
- `Shared<T>`: Many readers, rare writes. Readers don't block each other.
- `Mutex<T>`: Frequent writes. All access is exclusive.

Having both makes the programmer's intent clear and lets the runtime optimize accordingly.

### Why closure-based locking instead of lock guards?

Lock guards (Rust's approach) are a reference you hold. As long as the reference exists,
the lock is held. This makes it easy to accidentally hold a lock too long—or across an
await point, causing deadlocks.

Closures scope the lock precisely: `mutex.lock(|data| { ... })`. When the closure
returns, the lock is released. You can't hold it by accident. The compiler can also
detect direct nested locking and flag it as an error.

### Why `Multitasking.new()` instead of just having async everywhere?

Explicit resource declaration. Async runtimes aren't free—they create scheduler threads,
allocate task queues, set up I/O polling. `Multitasking.new()` makes this cost visible
and opt-in. A CLI tool that just needs thread parallelism uses `ThreadPool.new()` and
pays only for threads. A web server uses `Multitasking.new()` and gets the full
scheduler.

### Why are task handles affine (must be consumed)?

Go's `go func()` silently spawns work. If the goroutine panics, you might not know until
production. If it leaks, you might not notice until memory grows.

Rask forces you to either `.join()` (wait for it) or `.detach()` (explicitly say "I don't
care"). This catches forgotten spawns at compile time.

### Why `ensure` instead of RAII (automatic cleanup on scope exit)?

Rask does have RAII for non-resource types (automatic drop at scope exit). `ensure` adds
explicit, visible cleanup scheduling for resource types that MUST be consumed in a
specific way. It's more flexible than RAII—you can ensure different cleanup paths
(commit vs rollback) and the cleanup is visible in the code rather than hidden in a
destructor.

### Why expression-scoped borrows for collections?

If you hold a reference to `vec[0]` and then `vec.push(x)`, the push might reallocate
the vector's backing array. Your reference now points to freed memory.

Languages handle this differently:
- C++: Undefined behavior. Your problem.
- Rust: Borrow checker refuses to compile it (but the error messages are confusing).
- Rask: References to collection elements expire at the semicolon. Can't hold them.
  Clear rule, clear error.

### Why can't `+` allocate? (Why Copy-only operators?)

C++ lets `std::string a + b` silently heap-allocate. Then `a + b + c` creates an
intermediate string that's immediately thrown away. Expression templates were invented
specifically to work around this. Rask eliminates the problem instead.

Arithmetic operators (`+` `-` `*` `/`) require the type to be Copy (≤16 bytes). This means
`a + b` is always cheap—register-level work, no heap. Types that need heap allocation
(strings, big integers, matrices) use named methods: `.concat()`, `.add()`, `.mul()`. The
method name makes the allocation visible.

Compound assignment (`+=`) is different—it mutates in place and may not allocate at all. So
`+=` has no Copy requirement. A BigInt can implement `+=` even though it can't implement `+`.

### Why structural trait satisfaction (not nominal)?

**Nominal** means "you explicitly declare that Type implements Trait."
**Structural** means "if the methods match, it satisfies the trait."

Structural matching is simpler: add the right methods and things work. No ceremony. It
matches how developers think—"does this type have a compare method?"—rather than "did
someone remember to write `impl Comparable for Score`?"

The `explicit` keyword exists for traits where accidental satisfaction would be
dangerous. Best of both worlds.

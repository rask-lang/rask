<!-- depends: memory/ownership.md, types/error-types.md, types/optionals.md, control/ensure.md -->

# Canonical Patterns

For each common operation, there should be one idiomatic way to do it. When every project follows the same patterns, developers read unfamiliar code faster, tools can pattern-match on idioms, and newcomers learn one approach instead of five.

---

## Why These Patterns Matter

The same properties that make code clear to developers make it clear to machines: explicit intent, consistent patterns, local reasoning. Every property here was chosen for developer ergonomics first — the fact that it also helps automated tooling is a bonus.

### Local Analysis

Every function can be understood in isolation. Public signatures fully describe the interface. No cross-function inference, no whole-program analysis needed.

A tool can reason about one function without loading the entire codebase. Refactoring a single file doesn't require global analysis. Incremental checking is trivial.

### Rich Signatures

Function signatures carry a lot of information in Rask:

```rask
func process(config: Config, take data: Vec<u8>) -> ProcessResult or IoError
    with Pool<Node>
```

From this single line, a tool can determine:
- `config` is read-only (won't be modified)
- `data` ownership transfers (caller loses access)
- Can fail with `IoError` (and only `IoError`)
- Needs a `Pool<Node>` in scope
- Returns `ProcessResult` on success

Most languages require reading the function body to learn half of this. In Rask, the signature is a specification.

### Keyword-Based Semantics

Rask uses words where other languages use symbols:

| Concept | Rask | Alternative |
|---------|------|------------|
| Error propagation | `try expr` | `expr?` |
| Ownership transfer | `own value` | implicit move |
| Pattern check | `x is Some(v)` | `let Some(v) = x` |
| Result type | `T or E` | `Result<T, E>` |

Keywords are unambiguous tokens. `try` means one thing in Rask. `?` means different things in different languages. Tools that process multiple languages benefit from unambiguous tokens; developers benefit from readable code.

### Explicit Returns

Functions require explicit `return`. No ambiguity about what a function produces — find the `return` statements and you have the complete picture. Blocks use implicit last-expression, but functions don't.

### No Hidden Effects

No implicit async. No algebraic effects. No monkey patching. No operator overloading beyond the standard set. If a function does I/O, it shows up in the body. If it can fail, the return type says so.

---

## Construction

Build values with struct literals and `from_*` constructors.

```rask
// Struct literal — the default for known fields
const point = Point { x: 10, y: 20 }

// from_* — construction from a different source type
const path = Path.from_str("/usr/bin")
const config = Config.from_file("config.toml")

// .new() — zero-argument or minimal constructor
const buf = Buffer.new()
const map = Map.new()

// .with_* — builder-style for optional configuration
const pool = Pool.new().with_capacity(64)
const server = Server.new(8080).with_timeout(Duration.seconds(30))

// Collection literals
const names = Vec.from(["alice", "bob", "carol"])
const scores = Map.from([("alice", 100), ("bob", 85)])
```

**Anti-patterns:**
- Factory functions that hide which type is constructed — use `from_*` or struct literals instead.
- Overloading `.new()` with many optional parameters — use `.with_*` chaining.

See [stdlib/collections.md](stdlib/collections.md), [memory/pools.md](memory/pools.md).

---

## Conversion and Naming Conventions

Name encodes the cost. A developer — or a tool — knows what happens from the method name alone.

```rask
// as_* — cheap view, no allocation
const bytes = s.as_bytes()
const slice = vec.as_slice()

// to_* — allocates a new value, doesn't consume source
const s = number.to_string()
const lower = name.to_lowercase()

// into_* — consumes source, produces new type
const owned = view.into_string()
const vec = list.into_vec()
```

### Required Naming Patterns (Stdlib)

| Prefix/Suffix | Meaning | Returns | Examples |
|---------------|---------|---------|----------|
| `from_*` | Construction from source | `Self` or `Self or E` | `from_str(s)`, `from_bytes(b)` |
| `into_*` | Consuming conversion | new type (takes ownership) | `into_string()`, `into_vec()` |
| `as_*` | Cheap view or cast | reference or copy | `as_slice()`, `as_str()` |
| `to_*` | Non-consuming conversion | new type (may allocate) | `to_string()`, `to_lowercase()` |
| `is_*` | Boolean predicate | `bool` | `is_empty()`, `is_valid()` |
| `with_*` | Builder-style setter | `Self` | `with_capacity(n)` |
| `*_or(default)` | Unwrap with fallback | `T` | `unwrap_or(0)`, `env_or(k, d)` |
| `try_*` | Fallible version | `T or E` | `try_parse()`, `try_connect()` |

### Domain-Specific Patterns

| Pattern | Domain | Examples |
|---------|--------|---------|
| `read_*` / `write_*` | Binary I/O | `read_u32be()`, `write_all()` |
| `decode` / `encode` | Serialization | `json.decode<T>()`, `json.encode()` |

**Anti-patterns:**
- `to_*` that consumes the source — should be `into_*`.
- `as_*` that allocates — should be `to_*`.

Future stdlib additions must follow these patterns; `rask lint` enforces them. See [tooling/lint.md](tooling/lint.md).

---

## Error Handling

Propagate with `try`, handle with `match`. There's no third style.

```rask
// Propagation — pass the error up to the caller
func load_config(path: string) -> Config or IoError {
    const text = try fs.read_file(path)
    const config = try Config.from_str(text)
    return config
}

// Handling — react to the specific error
match fs.read_file(path) {
    Ok(data) => process(data),
    Err(e) => log("failed to read {path}: {e}"),
}

// Guard pattern — early return on error
func get_user(id: i64) -> User or NotFound {
    let user = db.find(id) is Ok else { return Err(NotFound {}) }
    return user
}
```

**Anti-patterns:**
- `unwrap()` in production code — crashes on error. Use `try` or `match`.
- Long `if result is Err(e)` chains — use `try` for propagation.
- Ignoring errors silently — always handle or propagate.

See [types/error-types.md](types/error-types.md).

---

## Resource Cleanup

`ensure` guarantees cleanup on all exit paths. One mechanism, no alternatives.

```rask
// File access pattern
const file = try fs.open(path)
ensure file.close()
const data = try file.read_text()

// Transaction pattern — explicit close + ensure fallback
const tx = try db.begin()
ensure tx.rollback()

try tx.execute("INSERT INTO users VALUES (?, ?)", [name, email])
tx.commit()  // consumes tx, ensure's rollback() becomes a no-op
```

**Anti-patterns:**
- Manual cleanup in every branch — `ensure` handles all paths automatically.
- RAII/destructor-style cleanup — Rask uses explicit `ensure`, not implicit drop.
- `finally` blocks — Rask doesn't have them; `ensure` is the mechanism.

See [control/ensure.md](control/ensure.md), [memory/resource-types.md](memory/resource-types.md).

---

## Option Handling

Four patterns, each for a different situation.

```rask
// Single check — do something if present
if opt is Some(v) {
    use(v)
}

// Fallback — provide a default
const name = opt ?? "anonymous"

// Guard — early return if absent
let v = opt is Some else { return None }

// Full handling — both branches matter
match opt {
    Some(v) => process(v),
    None => handle_missing(),
}
```

**Anti-patterns:**
- `unwrap()` without checking — crashes on None.
- Nested `if opt is Some` when `match` is clearer.

See [types/optionals.md](types/optionals.md).

---

## Collection Access

Read from collections with `get` (safe), index (panics), or iterate.

```rask
// Safe access — returns Option
const item = vec.get(i)

// Indexed access — panics on out of bounds
const first = vec[0]

// Slicing — sub-range
const middle = vec[1..3]

// Iteration — the default for processing all elements
for item in collection {
    process(item)
}

// Search
const found = users.find(|u| u.name == target)

// Transform
const names = users.map(|u| u.name).collect()

// Filter + transform
const active = users
    .filter(|u| u.is_active())
    .map(|u| u.name)
    .collect()
```

**Anti-patterns:**
- C-style index loops (`for i in 0..vec.len()`) when `for item in vec` works.
- Manual accumulation loops when `map`/`filter`/`fold` express intent clearly.

See [stdlib/collections.md](stdlib/collections.md), [stdlib/iteration.md](stdlib/iteration.md).

---

## String Operations

Strings are UTF-8. Use `format()` for building, methods for inspecting.

```rask
// Interpolation — the default for building strings
const msg = format("hello, {name}! you have {count} messages")

// StringBuilder — for loops or many concatenations
const sb = string_builder.new()
for item in items {
    sb.push_str(format("{item}\n"))
}
const result = sb.to_string()

// Searching
if line.contains("error"): handle_error(line)
if path.starts_with("/"): treat_as_absolute(path)

// Splitting
const parts = line.split(",")
const words = text.split_whitespace()

// Trimming
const clean = input.trim()
```

**Anti-patterns:**
- `+` for string concatenation in loops — use `string_builder`.
- Byte-level indexing when you mean character operations — use `.chars()`.

See [stdlib/strings.md](stdlib/strings.md), [stdlib/fmt.md](stdlib/fmt.md).

---

## Shared State

Message passing for communication, `Shared<T>` for shared data.

```rask
// Shared data — closure-based access, no lock leaks
const db = Shared.new(Database.new())

db.read(|d| {
    const user = d.users.get(id)
    respond(user)
})

db.write(|d| {
    d.users.insert(id, new_user)
})

// Message passing — channels between tasks
const ch = Channel.buffered(16)
spawn { ch.sender.send(compute_result()) }
const result = try ch.receiver.recv()
```

**Anti-patterns:**
- Global mutable state — use `Shared<T>` with explicit read/write scopes.
- Holding locks across await points — `Shared` closures prevent this by design.

See [concurrency/sync.md](concurrency/sync.md).

---

## Concurrency

`spawn` for tasks, `with multitasking` for the runtime. No async/await.

```rask
// Spawn and join
with multitasking {
    const handle = spawn { fetch(url) }
    const result = try handle.join()
}

// Fire-and-forget
spawn { log_event(event) }.detach()

// Parallel work with channels
with multitasking {
    const ch = Channel.buffered(10)

    for url in urls {
        spawn {
            const data = try fetch(url)
            try ch.sender.send(data)
        }
    }

    for _ in 0..urls.len() {
        const data = try ch.receiver.recv()
        process(data)
    }
}
```

**Anti-patterns:**
- Spawning without `with multitasking` scope — tasks need a runtime.
- Ignoring join handles — either `.join()` or `.detach()` explicitly.

See [concurrency/async.md](concurrency/async.md), [concurrency/sync.md](concurrency/sync.md).

---

## I/O

Explicit, no hidden effects. Every I/O operation is visible in the function body and return type.

```rask
// Read entire file
const text = try fs.read_file(path)

// Write entire file
try fs.write_file(path, data)

// Line-by-line reading
const lines = try fs.read_lines(path)
for line in lines {
    process(line)
}

// Resource file — open, use, close
const file = try fs.open(path)
ensure file.close()
const data = try file.read_text()

// Buffered I/O
const reader = BufReader.new(file)
while try reader.read_line() is Some(line) {
    process(line)
}
```

**Anti-patterns:**
- Opening a file without `ensure file.close()` — resource leak.
- Reading entire large files when line-by-line suffices.

See [stdlib/fs.md](stdlib/fs.md), [stdlib/io.md](stdlib/io.md).

---

## Pattern Matching

`if x is` for single checks, `match` for multiple branches.

```rask
// Single pattern check
if result is Ok(value) {
    use(value)
}

// Multiple branches
match event {
    Click(pos) => handle_click(pos),
    Key(k) => handle_key(k),
    Quit => break,
}

// Destructuring structs
if point is Point { x, y } {
    draw_at(x, y)
}

// Guard pattern
let conn = try_connect() is Ok else { return Err(ConnectFailed {}) }
```

**Anti-patterns:**
- If-else chains checking enum variants — use `match`.
- `match` with one arm and a wildcard — use `if x is`.

See [control/control-flow.md](control/control-flow.md), [types/enums.md](types/enums.md).

---

## Iteration

`for x in collection` is the only loop construct for traversal. Adapters for transformation.

```rask
// Basic iteration
for item in items {
    process(item)
}

// With index
for (i, item) in items.enumerate() {
    print("{i}: {item}")
}

// Range
for i in 0..10 {
    print(i)
}

// Chained adapters
const result = items
    .filter(|x| x.is_valid())
    .map(|x| x.value)
    .sum()
```

**Anti-patterns:**
- `while` with manual index increment — use `for i in 0..n`.
- Manual `collect` loops — use `.map()` / `.filter()` / `.fold()`.

See [stdlib/iteration.md](stdlib/iteration.md), [types/iterator-protocol.md](types/iterator-protocol.md).

---

## Testing

Tests are first-class blocks. No test framework needed.

```rask
test "user creation" {
    const user = User.new("alice", "alice@example.com")
    assert_eq(user.name, "alice")
    assert user.is_valid()
}

test "file cleanup" {
    const file = try fs.create("/tmp/test.txt")
    ensure fs.remove("/tmp/test.txt")

    try file.write_text("hello")
    const content = try fs.read_file("/tmp/test.txt")
    assert_eq(content, "hello")
}
```

**Anti-patterns:**
- External test frameworks — use built-in `test` blocks.
- Tests without assertions — every test should verify something.

See [stdlib/testing.md](stdlib/testing.md).

---

## Error Messages

Error messages should be actionable. A developer reading an error should know exactly what to change. A tool reading an error should be able to generate the fix.

Every error message has three parts:

1. **What went wrong** — The symptom, with source span
2. **How to fix it** — Concrete code change, not vague advice
3. **Why the rule exists** — One sentence explaining the constraint

```
error[E0042]: cannot use `data` after ownership transfer

  14 | process(own data)
     |         ~~~~~~~~ ownership transferred here
  15 | println(data.len())
     |         ^^^^ used after transfer

fix: clone before transfer
  14 | process(own data.clone())

why: `own` transfers ownership — the caller can no longer access the value.
```

**Guidelines:**
- **Concrete fixes over vague suggestions.** "Clone before transfer" with the exact line, not "consider cloning the value."
- **One primary fix.** Mention alternatives briefly after the main suggestion.
- **The `fix:` section is machine-parseable.** Tools can extract the line number and replacement text for automated fixes.
- **The `why:` section teaches.** Developers learn the rule; they don't just memorize the fix.
- **Every new error must include `fix:` and `why:` text.** Enforced in the compiler's `ToDiagnostic` implementations.

---

## Summary

| Operation | Canonical Pattern |
|-----------|------------------|
| Construct | Struct literal, `from_*`, `.new()`, `.with_*` |
| Convert | `as_*` (free), `to_*` (allocates), `into_*` (consumes) |
| Handle errors | `try` (propagate), `match` (handle) |
| Clean up resources | `ensure` |
| Handle options | `if x is Some`, `??`, guard, `match` |
| Access collections | `get` (safe), `[i]` (panic), `for` (iterate) |
| Build strings | `format()`, `string_builder` |
| Share state | `Shared<T>`, channels |
| Run concurrently | `spawn`, `with multitasking` |
| Do I/O | `fs.read_file`, `fs.open` + `ensure close` |
| Match patterns | `if x is` (single), `match` (multiple) |
| Iterate | `for x in`, adapters (`.map`, `.filter`) |
| Test | `test "name" { }` blocks |

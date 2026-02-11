<!-- id: ctrl.flow -->
<!-- status: decided -->
<!-- summary: Context-dependent expressions, explicit return, break values, block-scoped labels -->
<!-- depends: types/enums.md, types/error-types.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-interp/ -->

# Control Flow

Assignment context determines whether a construct produces a value or executes for side effects. Functions require explicit `return`. `loop` supports `break value` for value-returning loops.

## Expression vs Statement Context

| Rule | Description |
|------|-------------|
| **CF1: Assignment = expression** | `const x = match/if/loop` makes arms/branches produce values |
| **CF2: Standalone = statement** | Standalone `match/if/while/for` are side effects, produce `()` |
| **CF3: Block last expression** | In expression context, block's last expression is its value |

```rask
// Expression context — arms produce values
const color = match status {
    Active => "green",
    Failed => "red",
}

// Statement context — arms are side effects
match event {
    Click(pos) => handle_click(pos),
    Key(k) => handle_key(k),
}

// Block in expression context
const token = match c {
    '+' => { self.advance(); Token.Plus }   // Token.Plus is the value
    '-' => { self.advance(); Token.Minus }
}
```

## Conditional: if/else

| Rule | Description |
|------|-------------|
| **CF4: Bool condition** | Condition must be `bool` (no implicit conversion) |
| **CF5: Braces required** | Braces required for blocks (except inline syntax `if c: a else: b`) |
| **CF6: Branch type matching** | Expression context: both branches must have same type |
| **CF7: Omitted else** | Omitting `else` produces `()` (statement context only) |

```rask
// Inline expression
const sign = if x > 0: "+" else: "-"

// Block expression
const x = if c { a } else { b }

// Statement, no else needed
if error {
    log("failed")
    retry()
}
```

## Pattern Matching: is

| Rule | Description |
|------|-------------|
| **CF8: Binding scope** | `if expr is Pattern(v)` binds `v` inside block only |
| **CF9: Not exhaustive** | Unmatched patterns skip the block (not an error) |
| **CF10: Combined conditions** | Bindings from `is` available after `&&` in same condition |
| **CF11: Linear resources** | Non-Copy values moved into pattern; must handle both match/no-match paths |
| **CF12: Implicit unwrap** | `if expr is Variant` (no binding) unwraps single-payload variant, reusing outer name |

```rask
// Pattern match with explicit binding
if state is Connected(sock) {
    sock.send(data)
}

// Implicit unwrap for single-payload variants
if user is Some {
    process(user)  // user unwrapped, same name
}

if result is Ok {
    use(result)  // result unwrapped
}

// Loop while pattern matches
while reader.next() is Some(line) {
    process(line)
}

// Combined with boolean
if state is Connected(sock) && sock.is_ready() {
    sock.send(data)
}
```

## Guard Pattern: let...is...else

| Rule | Description |
|------|-------------|
| **CF13: Diverging else** | `else` block must diverge (`return`, `break`, `panic`, etc.) |
| **CF14: Binding escapes** | Successful match binds value to outer scope |
| **CF15: Linear in else** | Pattern fails, value available in `else` for cleanup |

```rask
// Early return on error
const value = result is Ok else { return Err(e) }
// value available here

// Break from loop on None
const item = queue.pop() is Some else { break }
// item available here
```

## Infinite Loop: loop

| Rule | Description |
|------|-------------|
| **CF15: Break value** | `break value` exits loop and produces a value (expression context) |
| **CF16: Type consistency** | All `break value` expressions must have same type |
| **CF17: Never type** | Loop without reachable exit has type `Never` |
| **CF18: Bare break forbidden** | If any `break value` exists, bare `break` is an error |

```rask
const input = loop {
    const x = read_input()
    if x.is_valid() { break x }
    println("Invalid, try again")
}
```

## Conditional Loop: while

| Rule | Description |
|------|-------------|
| **CF19: Bool condition** | Condition must be `bool` |
| **CF20: No break value** | `break value` not allowed (use `loop` for value-returning) |
| **CF21: Produces unit** | Always produces `()` (statement, not expression) |

```rask
while queue.len() > 0 {
    const task = queue.pop()
    process(task)
}
```

## Loop Labels

| Rule | Description |
|------|-------------|
| **CF22: Unique labels** | Labels must be unique within function scope |
| **CF23: Break label** | `break label` exits labeled loop |
| **CF24: Continue label** | `continue label` continues labeled loop |
| **CF25: Label value** | `break label value` exits labeled `loop` with value |

```rask
outer: for i in rows {
    for j in cols {
        if grid[i][j] == target {
            break outer
        }
    }
}

search: loop {
    for i in 0..n {
        if found(i) {
            break search Some(i)
        }
    }
    break None
}
```

## Return

| Rule | Description |
|------|-------------|
| **CF26: Exits function** | `return` immediately exits current function (not just block) |
| **CF27: Ensure trigger** | `return` triggers `ensure` cleanup before exiting |
| **CF28: Never type** | Type of `return` expression is `Never` |

```rask
func format_size(bytes: i64) -> string {
    if bytes < 1024 {
        return "{bytes} B"
    }
    return "{bytes / 1024} KB"
}

func process(file: File) -> Data or Error {
    ensure file.close()
    const data = try file.read()   // try may return early
    Ok(transform(data))
}
```

## Block Expressions

| Rule | Description |
|------|-------------|
| **CF29: Expression context** | Block in expression context produces last expression's value |
| **CF30: Statement context** | Block in statement context produces `()` |
| **CF31: Scope isolation** | Variables declared in block scoped to block |

```rask
// Expression context
const result = {
    const temp = compute()
    transform(temp)   // this is the block's value
}

// Statement context
{
    const temp = compute()
    process(temp)   // return value discarded
}
```

## Never Type

| Rule | Description |
|------|-------------|
| **CF32: Never coercion** | `Never` coerces to any type |

```rask
let x: i32 = if cond { 42 } else { panic("nope") }
// else branch is Never, coerces to i32
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `if` without else, used as expression | CF7 | Error unless consequent is `()` |
| `break value` in `while` or `for` | CF20 | Error: use `loop` instead |
| Unlabeled `break` outside loop | — | Error: break outside loop |
| `break label` with nonexistent label | CF22 | Error: undefined label |
| `return` in `ensure` body | CF27 | Error: cannot return from ensure |
| `break`/`continue` in `ensure` body | CF27 | Error: cannot break from ensure |
| Empty `if` branches | CF7 | Valid: `if c {} else {}` evaluates to `()` |
| Nested `ensure` in loop | CF27 | Each iteration registers new ensure |
| `loop` with `break value` and bare `break` | CF18 | Error: inconsistent exit types |
| `loop` without exit | CF17 | Type is `Never` (infinite loop) |
| Chained `else if` | — | Desugars to nested `if` in `else` block |

## Error Messages

**Missing else branch in expression context [CF6]:**
```
ERROR [ctrl.flow/CF6]: if expression requires else branch
   |
3  |  const x = if cond { 42 }
   |            ^^^^^^^^^^^^^^ no else branch

WHY: In expression context, both branches must produce a value.

FIX: Add an else branch:

  const x = if cond { 42 } else { 0 }
```

**break value in while loop [CF20]:**
```
ERROR [ctrl.flow/CF20]: cannot break with value from while loop
   |
5  |  while cond {
   |  ----------- while loop
6  |      break 42
   |      ^^^^^^^^ value not allowed here

WHY: while loops are statements and cannot produce values.

FIX: Use loop instead:

  loop {
      if !cond { break 42 }
      // loop body
  }
```

**Inconsistent break types [CF16]:**
```
ERROR [ctrl.flow/CF16]: break expressions have inconsistent types
   |
3  |  loop {
4  |      if a { break 42 }
   |             -------- i32
5  |      if b { break "x" }
   |             --------- string: expected i32

WHY: All break expressions in a loop must have the same type.

FIX: Ensure all break expressions match:

  loop {
      if a { break 42 }
      if b { break 0 }  // Changed to i32
  }
```

**else block doesn't diverge [CF13]:**
```
ERROR [ctrl.flow/CF13]: else block must diverge
   |
3  |  const x = opt is Some else { None }
   |                               ^^^^ doesn't diverge

WHY: let...is...else requires the else block to exit via return,
     break, continue, or panic.

FIX: Use if is instead:

  const x = if opt is Some(v) { v } else { default_value }
```

**Linear resource consumed only in one branch [CF11]:**
```
ERROR [ctrl.flow/CF15]: linear resource may not be consumed
   |
3  |  const file = open("data.txt")
   |               ---------------- resource created here
4  |  if cond {
5  |      file.close()
   |      ------------ consumed here
6  |  }
   |  - file may not be consumed if cond is false

WHY: Linear resources must be consumed on all paths.

FIX: Consume in both branches or use ensure:

  // Option 1: consume in both
  if cond {
      file.close()
  } else {
      file.close()
  }

  // Option 2: use ensure
  ensure file.close()
  if cond { /* ... */ }
```

---

## Appendix (non-normative)

### Rationale

**CF1/CF2 (context-dependent):** Most control flow is for side effects (logging, validation, mutation). Assignment context (`const x = match/if ...`) naturally signals value production; standalone constructs are side effects. This eliminates trailing semicolons without ambiguity.

**CF26 (explicit return):** `return` always means "exit the function" — no overloading. Inside a match arm or if block, `return` exits the **function**, not just the construct. This is consistent and predictable.

**CF15 (break value):** `loop` with `break value` provides clear syntax for value-returning loops. The alternative (while with mutation, implicit last expression) is ambiguous and error-prone.

**CF8/CF13 (pattern binding):** Two forms for two use cases:
- `if expr is Pattern(v) { ... }` — binding scoped to block, for conditional execution
- `const v = expr is Pattern else { ... }` — binding escapes to outer scope, for early exit

The diverging `else` requirement (CF13) ensures the binding is always valid after the statement.

**CF12 (implicit unwrap):** For single-payload variants like `Some(T)` or `Ok(T)`, omitting the binding in `if x is Some` unwraps using the outer variable name. Reduces friction for the common case. Multi-field variants require explicit destructuring.

**CF22-25 (labels):** Labels enable breaking/continuing outer loops without extra flags or state. The `label:` syntax is clear and unambiguous.

### Patterns & Guidance

**When to use which:**

| Use Case | Recommended | Why |
|----------|-------------|-----|
| Value-producing conditional | `const x = if c: a else: b` | Inline, clear |
| Value from multi-case enum | `const x = match e { ... }` | Exhaustive |
| Value from repeated checks | `const x = loop { break ... }` | Clear exit |
| Side effect conditional | `if c { f() }` | No value needed |
| Check other enum variant | `if x is Variant` | Pattern match |
| Early exit on error | `let v = x is Ok else { return }` | Guard |
| Loop over iterator | `for x in iter` | Standard |
| Loop until condition | `while cond { ... }` | Clear intent |
| Infinite loop | `loop { ... }` | Explicit |

**Pattern matching vs match:**

`if is` is for single-variant checks. `match` is for exhaustive handling.

```rask
// Check one variant - use if is
if state is Connected(sock) {
    sock.send(data)
}

// Handle all variants - use match
match state {
    Connected(sock) => sock.send(data),
    Disconnected => reconnect(),
    Error(e) => log(e),
}
```

**For loops:**

Fully specified in [loops.md](loops.md) and [iteration.md](../stdlib/iteration.md).

Key points:
- Produces `()` (statement, not expression)
- `break` and `continue` supported
- `break value` not allowed (use `loop` for value-returning)
- Labels supported: `label: for i in coll { ... }`

**Semicolons:**

Semicolons separate statements on the same line. Newlines also separate statements.

```rask
// Equivalent forms
do_thing()
do_other()

do_thing(); do_other()

// Semicolons separate statements within expression-context blocks
const token = match c {
    '+' => { self.advance(); Token.Plus }
}
```

**Linear resources in conditionals:**

Linear resources (files, connections, etc.) must be consumed on all paths. Use `ensure` for cleanup on all exit paths.

```rask
// Valid: linear consumed in both branches
if cond {
    file.close()
} else {
    file.close()
}

// Better: use ensure
ensure file.close()
if cond {
    process(file)
}
```

**Never type and divergence:**

Some expressions never complete normally:

| Expression | Type |
|------------|------|
| `return ...` | `Never` |
| `break` | `Never` |
| `break ...` | `Never` |
| `continue` | `Never` |
| `panic(...)` | `Never` |
| `loop { /* no exit */ }` | `Never` |

`Never` coerces to any type, allowing branches with different control flow:

```rask
let x: i32 = if cond { 42 } else { return }
// else branch is Never, coerces to i32
```

### Examples

**Value-producing constructs:**
```rask
// Match expression
const color = match status {
    Active => "green",
    Inactive => "gray",
    Failed => "red",
}

// Inline if
const sign = if x > 0: "+" else: "-"

// Match with block arms
const opts = match parse_args(args) {
    Ok(o) => o,
    Err(e) => {
        println("error: {e}")
        std.exit(1)
    }
}
```

**Loop with break value:**
```rask
const input = loop {
    const x = read_input()
    if x.is_valid() {
        break x
    }
    println("Invalid, try again")
}
```

**Search pattern:**
<!-- test: skip -->
```rask
func find_first<T: Eq>(items: Vec<T>, target: T) -> Option<usize> {
    let i = 0
    loop {
        if i >= items.len() {
            break None
        }
        if items[i] == target {
            break Some(i)
        }
        i += 1
    }
}
```

**Labeled break with value:**
```rask
func find_in_matrix<T: Eq>(matrix: Vec<Vec<T>>, target: T) -> Option<(usize, usize)> {
    search: loop {
        for i in 0..matrix.len() {
            for j in 0..matrix[i].len() {
                if matrix[i][j] == target {
                    break search Some((i, j))
                }
            }
        }
        break None
    }
}
```

**While with mutation:**
<!-- test: parse -->
```rask
func drain_queue(queue: Queue<Task>) {
    while queue.len() > 0 {
        const task = queue.pop()
        if task.is_cancelled() {
            continue
        }
        task.run()
    }
}
```

**Server loop (no value):**
<!-- test: parse -->
```rask
func run_server(server: Server) {
    loop {
        const conn = server.accept()
        handle(conn)
    }
}
```

**Pattern matching in conditions:**
```rask
// Single-variant check with binding
if state is Connected(sock) {
    sock.send(data)
}

// Loop while pattern matches
while reader.next() is Some(line) {
    process(line)
}

// With else
if result is Ok(value) {
    use(value)
} else {
    handle_error()
}

// Combined with boolean
if state is Connected(sock) && sock.is_ready() {
    sock.send(data)
}
```

**Guard pattern for early exit:**
```rask
func process_file(path: string) -> () or Error {
    const file = try open(path)
    ensure file.close()

    const line = file.read_line() is Some else { return Ok(()) }
    // line available here

    const value = parse(line) is Ok else { return Err("invalid") }
    // value available here

    Ok(())
}
```

**Negation:**
```rask
if !(state is Connected(_)) {
    reconnect()
}
```

Note: `!` applies to the whole `is` expression. There is no `is not` syntax.

### Control Flow and Ensure

`ensure` runs on ALL exit paths:

| Exit Path | Ensure Runs? |
|-----------|--------------|
| Normal block end | Yes |
| `return` | Yes |
| `break` | Yes |
| `break value` | Yes |
| `continue` | Yes |
| `try` propagation | Yes |
| `panic` | Yes |

See [ensure.md](ensure.md) for full specification.

### See Also

- [Enums](../types/enums.md) — Match syntax, exhaustiveness (`type.enums`)
- [Error Types](../types/error-types.md) — `try` propagation, linear resources (`type.errors`)
- [Loops](loops.md) — For loops, iteration (`ctrl.loops`)
- [Ensure Cleanup](../ecosystem/ensure.md) — Cleanup on all exit paths

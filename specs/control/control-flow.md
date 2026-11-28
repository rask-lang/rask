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
| **CF1: Assignment = expression** | `let x = match/if/loop` makes arms/branches produce values |
| **CF2: Standalone = statement** | Standalone `match/if/while/for` are side effects, produce `()` |
| **CF3: Block last expression** | In expression context, block's last expression is its value |

```rask
// Expression context — arms produce values
let color = match status {
    Active => "green",
    Failed => "red",
}

// Statement context — arms are side effects
match event {
    Click(pos) => handle_click(pos),
    Key(k) => handle_key(k),
}

// Block in expression context
let token = match c {
    '+' => { self.advance(); Token.Plus }   // Token.Plus is the value
    '-' => { self.advance(); Token.Minus }
}
```

## Conditional: if/else

| Rule | Description |
|------|-------------|
| **CF4: Bool condition** | Condition must be `bool` (no implicit conversion) |
| **CF5: Braces required** | Every body is a block in braces, one line or many: `if c { a } else { b }`. A colon never opens a body |
| **CF6: Branch type matching** | Expression context: both branches must have same type |
| **CF7: Omitted else** | Omitting `else` produces `()` (statement context only) |
| **CF7b: No bare struct literal in a condition** | A `{` in an `if`/`while` condition starts the body. To write a struct literal there, parenthesize it |

The `{` after a condition belongs to the block, so `Shape.Circle(r: 4)` in
condition position has nowhere to go. Parens settle it — inside them a `{` can
only be a literal:

```rask
if s == Shape.Dot { ... }                      // fine — unit variant, no braces
if (c == Shape.Circle(r: 4)) { ... }        // literal needs parens
let want = Shape.Circle(r: 4)             // or bind it first
if c == want { ... }
```

I follow Rust and Swift here. The alternative — deciding by whether the name
happens to be a unit variant — makes the parse depend on a declaration the
parser hasn't read yet.

```rask
// Inline expression
let sign = if x > 0 { "+" } else { "-" }

// Block expression
let x = if c { a } else { b }

// Statement, no else needed
if error {
    log("failed")
    retry()
}
```

## Pattern Matching: is

| Rule | Description |
|------|-------------|
| **CF8: Where a binding reaches** | A name bound by `is` or `? as` reaches the rest of its `&&` chain and the body the chain guards: the `if` branch, the `while` body, or the match arm under a guard. Nothing else sees it |
| **CF9: Not exhaustive** | Unmatched patterns skip the block (not an error) |
| **CF10: `&&` carries, `\|\|` and `!` don't** | `s is Circle(r) && r > 1.0` reads `r`. A name bound on one side of `\|\|` or under `!` has no value where it could be read, so binding it is an error (E0220). One pattern can bind across alternatives: `s is Circle(r) \| Square(r)` |
| **CF11: Linear resources** | Non-Copy values moved into pattern; must handle both match/no-match paths |
| **CF12: A bare variant tests** | `x is Variant` or `x is Enum.Variant` with no payload pattern tests the tag and binds nothing, whatever the variant carries. A payload is named by writing it: `x is Variant(v)`. `Variant(_)` and `Variant()` are errors whose fix is the bare name |
| **CF35: The else reads none of them** | The `else` of an `if` runs when the test failed, so it can't read a name the condition bound (E0221) |
| **CF36: A binding needs a reader** | A test that binds where nothing follows is an error (E0220): `let ok = a? as v` drops `v`. Keep the test without the name (`let ok = a?`), or bind in an `if` |
| **CF37: A resource ends its chain** | A test that binds a resource (`@resource`) is the last test of its `&&` chain (E0858). When a later test fails, the else runs, and the else can't see the name (CF35), so nothing could consume it. Bind it alone and test inside the body |

```rask
// Pattern match with explicit binding
if state is Connected(sock) {
    sock.send(data)!
}

// A bare variant only tests
if event is Tick {
    count += 1
}

// Optionals use the `?` predicate + `as` binding
if user? as u {
    process(u)
}

// Results use a type pattern — `?` is absence, not failure
if load() is Config as c {
    apply(c)
}

// Loop while status holds a value
while reader.next()? as line {
    process(line)
}

// Combined with boolean
if state is Connected(sock) && sock.is_ready() {
    sock.send(data)!
}

// A match guard's binding reaches its arm
match job {
    Retry(n) if n < 3 && lookup(n)? as plan => run(plan),
    _ => give_up(),
}
```

What reaches where, in one block:

```rask
if s is Circle(r) && r > 1.0 { use(r) }     // ✅ `&&` carries `r`
if s is Circle(r) || s is Dot { use(r) }    // ❌ E0220: `r` has no value when `Dot` held
if !(s is Circle(r)) { … }                  // ❌ E0220: the branch runs when it didn't bind
if s is Circle(r) { … } else { use(r) }     // ❌ E0221: the else runs when it didn't bind
let ok = a? as v                            // ❌ E0220: nothing reads `v`; write `a?`
```

A condition can run over several lines without parens. The `{` that opens the body ends it:

```rask
if s is Circle(r)
    && r > 1.0
    && r < 10.0 {
    use(r)
}
```

## Guard Pattern: let...else

| Rule | Description |
|------|-------------|
| **CF13: Diverging else** | `else` block must diverge (`return`, `break`, `panic`, etc.) |
| **CF14: Binding escapes** | `let Pattern = value else { … }` binds the pattern's names for the rest of the block. The left side is a pattern that can miss: a variant, a struct with a literal field, a nested test. A pattern that takes every value (a lowercase name, a tuple, a plain struct) is E0225: the `else` never runs. A type pattern on an optional or a result is E0226: `??` and `catch` do that on one line |
| **CF33: Linear in else** | Pattern fails, value available in `else` for cleanup |

The pattern sits where a `let` name sits, the same place `let (a, b) = pair` puts its tuple. Without `else` the pattern has to match every value (a tuple, a struct); with `else` it may miss:

```rask
// Early return on non-matching variant
let Connected(sock) = state else { return }
// sock available here

// Break from loop when task missing
let Ready(task) = event else { break }
// task available here

// A test inside a field
let Job(id, result: IoError as e) = job else { return }

// A struct with a literal field
let Point(x: 0, y) = p else { return }
```

What isn't a guard:

```rask
let n = find() else { return 0 }            // ❌ E0225: `n` takes the `none` too
let Point(x, y) = p else { return }         // ❌ E0225: every `Point` matches
let i64 as n = load() else { return -1 }    // ❌ E0226
let n = load() catch _ => return -1         // ✅ the guard on a result
let n = find() ?? return 0                  // ✅ the guard on an optional
```

**Why type patterns aren't guards.** On an optional or a result, the guard would say what `??` and `catch` already say on one line, with the exit written where it happens and the value bound. Two spellings of one thing is the confusion this design removes. A type test *inside* a pattern stays (`Job(id, result: IoError as e)`): no operator reaches into a field.

The old spelling, `let sock = state is Connected else { … }`, is an error whose fix is the line above. It put the name on the left and the thing it named on the right, and it only worked for a variant with one payload.

Optionals and results have their own surface: `??` supplies the absent branch and `catch e =>` the error branch (a value, or an exit like `?? break`), `try` propagates — and each binds the payload directly:

```rask
let item = queue.pop() ?? break
process(item)
```

## Infinite Loop: loop

| Rule | Description |
|------|-------------|
| **CF15: Break value** | `break value` exits loop and produces a value (expression context) |
| **CF16: Type consistency** | All `break value` expressions must have same type |
| **CF17: No exit, no finish** | A loop with no reachable exit doesn't finish (CF32) |
| **CF18: Bare break forbidden** | If any `break value` exists, bare `break` is an error |

```rask
let input = loop {
    let x = read_input()
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
    let task = queue.pop()
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
            break search i
        }
    }
    break none
}
```

## Return

| Rule | Description |
|------|-------------|
| **CF26: Exits function** | `return` immediately exits current function, closure or task block (not just block) |
| **CF27: Ensure trigger** | `return` triggers `ensure` cleanup before exiting |
| **CF28: Return doesn't finish** | A `return` expression doesn't finish (CF32) |

Closures are independent scopes for `return`. `return` inside a closure exits the **closure**, not the enclosing function. This matches function semantics — a closure is an anonymous function. Block-bodied closures require explicit `return`, same as functions.

```rask
func format_size(size: i64) -> string {
    if size < 1024 {
        return "{size} B"
    }
    return "{size / 1024} KB"
}

// return in closure exits the closure, not process_items
func process_items(items: Vec<Item>) -> Vec<string> {
    return items.map(|item| {
        if item.is_empty() { return "empty" }
        return item.format()
    }).to_vec()
}

func process(file: File) -> Data or Error {
    ensure file.close()
    let data = try file.read()   // try may return early
    return transform(data)
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
let result = {
    let temp = compute()
    transform(temp)   // this is the block's value
}

// Statement context
{
    let temp = compute()
    process(temp)   // return value discarded
}
```

## Expressions That Don't Finish

| Rule | Description |
|------|-------------|
| **CF32: Fits any slot** | An expression that doesn't finish (`return`, `break`, `continue`, a loop with no exit, a call to a `@noreturn` function) needs no value: an `if` arm, a `match` arm or a `??` fallback made of one fits whatever the other side is |
| **CF34: `@noreturn`** | A function marked `@noreturn` never comes back to its caller: `panic`, `todo`, `unreachable`, `os.exit`. It declares no result type. Every path through its body ends in something that doesn't finish, and it has no `return` (E0809, E0850) |

```rask
mut x: i32 = if cond { 42 } else { panic("nope") }
// the else arm doesn't finish, so the `if` is an i32
```

<!-- test: skip -->
```rask
@noreturn
func fail(msg: string) {
    println("fail: {msg}")
    os.exit(3)
}

let v = x ?? fail("none")
```

There's no type for "doesn't finish" to write. I had one, `Never`, and the only
places it was ever written were the two `exit` declarations. Saying it on the
function is what C does with `noreturn`, and it reads as what it is: a fact
about the function, not a kind of value.

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
| `loop` without exit | CF17 | Doesn't finish (infinite loop) |
| Chained `else if` | — | Desugars to nested `if` in `else` block |

## Error Messages

**Missing else branch in expression context [CF6]:**
```
ERROR [ctrl.flow/CF6]: if expression requires else branch
   |
3  |  let x = if cond { 42 }
   |            ^^^^^^^^^^^^^^ no else branch

WHY: In expression context, both branches must produce a value.

FIX: Add an else branch:

  let x = if cond { 42 } else { 0 }
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
3  |  let Tick(x) = event else { 0 }
   |                              ^ doesn't diverge

WHY: let...else requires the else block to exit via return,
     break, continue, or panic.

FIX: Use if is instead:

  let x = if event is Tick(v) { v } else { default_value }
```

**Linear resource consumed only in one branch [CF11]:**
```
ERROR [ctrl.flow/CF11]: linear resource may not be consumed
   |
3  |  let file = open("data.txt")
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
      file.close()!
  } else {
      file.close()!
  }

  // Option 2: use ensure
  ensure file.close()
  if cond { /* ... */ }
```

## Destructuring

| Rule | Description |
|------|-------------|
| **DS1: Const destructuring** | `let (a, b) = expr` binds tuple elements as immutable |
| **DS2: Mut destructuring** | `mut (a, b) = expr` binds tuple elements as rebindable |
| **DS3: For destructuring** | `for (k, v) in iter` destructures each iteration element |
| **DS4: Arity match** | Number of bindings must match tuple length (compile error otherwise) |
| **DS5: Nested** | `let (a, (b, c)) = expr` destructures nested tuples |
| **DS6: Wildcard** | `_` discards an element: `let (_, b) = pair` |
| **DS7: Guard pattern** | `let Paired(a, b) = expr else { return }` destructures a value that may not match. See CF13–CF14 |

<!-- test: parse -->
```rask
// Basic destructuring
let point = (10, 20)
let (x, y) = point

func use_destructuring() {
    // Mutable destructuring (mut requires function scope)
    mut (a, b) = get_pair()
    a = a + 1

    // For loop destructuring
    for (key, value) in map {
        println("{key}: {value}")
    }
}

// Wildcard
let (_, second) = pair

// Channel pattern
let (tx, rx) = Channel<Message>.buffered(100)
```

**Arity mismatch [DS4]:**
```
ERROR [ctrl.flow/DS4]: tuple destructuring arity mismatch
   |
3  |  let (a, b, c) = (1, 2)
   |        ^^^^^^^^^ expected 2 elements, found 3 bindings

WHY: Destructuring requires exactly one binding per tuple element.

FIX: Match the number of bindings, use _ to discard:

  let (a, b) = (1, 2)
```

---

## Appendix (non-normative)

### Rationale

**CF1/CF2 (context-dependent):** Most control flow is for side effects (logging, validation, mutation). Assignment context (`let x = match/if ...`) naturally signals value production; standalone constructs are side effects. This eliminates trailing semicolons without ambiguity.

**CF26 (explicit return):** `return` exits the innermost function or closure scope. Inside a match arm or if block within a function, `return` exits the **function**. Inside a closure body, `return` exits the **closure**. Closures are anonymous functions — same return semantics. A task block (`spawn { … }`) is a frame of its own too: `return v` ends the task with `v`, since the function that started it may already have returned (`conc.async/S7`). Block-bodied closures require explicit `return`, same as functions. Expression-bodied closures (`|x| x * 2`) implicitly return their expression.

**CF15 (break value):** `loop` with `break value` provides clear syntax for value-returning loops. The alternative (while with mutation, implicit last expression) is ambiguous and error-prone.

**CF8/CF13 (pattern binding):** Two forms for two use cases:
- `if expr is Pattern(v) { ... }` — binding scoped to block, for conditional execution
- `let Pattern(v) = expr else { ... }` — binding escapes to outer scope, for early exit

The diverging `else` requirement (CF13) ensures the binding is always valid after the statement.

**CF8/CF10 (`&&` carries, `||` doesn't):** A binding exists where the test that made it is known to have held. After `&&`, it held. After `||`, maybe the other side held instead, and under `!` the code runs exactly when it didn't. Both used to pass the checker and fail later, in lowering or at run time; now the binding is the error, with the fix written out.

**Why the pattern goes on the left.** `let sock = state is Connected else { … }` named `sock` on the left and the variant it came from on the right, and bound the payload by position, so it couldn't take a variant with two payloads or a struct. Putting the pattern where `let` puts names is how Rust and Swift write it, and it reuses the destructuring every `let` already has. Optionals stay with `??` and `catch`: those handle the miss inline, with the exit written out and the payload bound.

**CF12 (no implicit unwrap):** An earlier draft had `if x is Variant` rebind `x` to a single payload inside the branch. I dropped it for the same reason the error-model redesign dropped `if x is Some { use(x) }` (its P3): the name's type changes with nothing written to say so, and `while s is Next { s = step(s) }` can't be written at all. Naming the payload costs one word, `x is Variant(v)`. Optionals use dedicated operators (`?`, `? as v`, `??`); results use `catch`, `try`, and `is` with a type pattern.

**CF22-25 (labels):** Labels enable breaking/continuing outer loops without extra flags or state. The `label:` syntax is clear and unambiguous.

### Patterns & Guidance

**When to use which:**

| Use Case | Recommended | Why |
|----------|-------------|-----|
| Value-producing conditional | `let x = if c { a } else { b }` | Inline, clear |
| Value from multi-case enum | `let x = match e { ... }` | Exhaustive |
| Value from repeated checks | `let x = loop { break ... }` | Clear exit |
| Side effect conditional | `if c { f() }` | No value needed |
| Check other enum variant | `if x is Variant` | Pattern match |
| Early exit on other variant | `let Connected(sock) = state else { return }` | Guard |
| Early exit on error | `let v = try r` | Propagates |
| Loop over iterator | `for x in iter` | Standard |
| Loop until condition | `while cond { ... }` | Clear intent |
| Infinite loop | `loop { ... }` | Explicit |

**Pattern matching vs match:**

`if is` is for single-variant checks. `match` is for exhaustive handling.

```rask
// Check one variant - use if is
if state is Connected(sock) {
    sock.send(data)!
}

// Handle all variants - use match
match state {
    Connected(sock) => sock.send(data)!,
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
let token = match c {
    '+' => { self.advance(); Token.Plus }
}
```

**Linear resources in conditionals:**

Linear resources (files, connections, etc.) must be consumed on all paths. Use `ensure` for cleanup on all exit paths.

```rask
// Valid: linear consumed in both branches
if cond {
    file.close()!
} else {
    file.close()!
}

// Better: use ensure
ensure file.close()
if cond {
    process(file)!
}
```

**Expressions that don't finish:**

`return`, `break` (with or without a value), `continue`, a call to a
`@noreturn` function (`panic(...)`, `os.exit(...)`) and `loop { }` with no exit
never complete normally, so an arm made of one needs no value (CF32):

```rask
mut x: i32 = if cond { 42 } else { return }
// the else arm doesn't finish, so the `if` is an i32
```

### Examples

**Value-producing constructs:**
```rask
// Match expression
let color = match status {
    Active => "green",
    Inactive => "gray",
    Failed => "red",
}

// Inline if
let sign = if x > 0 { "+" } else { "-" }

// Match with block arms on error union
let opts = match parse_args(args) {
    Args as o => o,
    ParseError as e => {
        println("error: {e}")
        std.exit(1)
    }
}
```

**Loop with break value:**
```rask
let input = loop {
    let x = read_input()
    if x.is_valid() {
        break x
    }
    println("Invalid, try again")
}
```

**Search pattern:**
<!-- test: skip -->
```rask
func find_first<T: Equal>(items: Vec<T>, target: T) -> usize? {
    mut i = 0
    loop {
        if i >= items.len() {
            break none
        }
        if items[i] == target {
            break i
        }
        i += 1
    }
}
```

**Labeled break with value:**
```rask
func find_in_matrix<T: Equal>(matrix: Vec<Vec<T>>, target: T) -> (usize, usize)? {
    search: loop {
        for i in 0..matrix.len() {
            for j in 0..matrix[i].len() {
                if matrix[i][j] == target {
                    break search (i, j)
                }
            }
        }
        break none
    }
}
```

**While with mutation:**
<!-- test: parse -->
```rask
func drain_queue(queue: Queue<Task>) {
    while queue.len() > 0 {
        let task = queue.pop()
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
        let conn = server.accept()
        handle(conn)
    }
}
```

**Pattern matching in conditions:**
```rask
// Single-variant check with binding
if state is Connected(sock) {
    sock.send(data)!
}

// Loop while optional has a value
while reader.next()? as line {
    process(line)
}

// With else on an optional
if cached? as value {
    use(value)
} else {
    recompute()
}

// Combined with boolean
if state is Connected(sock) && sock.is_ready() {
    sock.send(data)!
}
```

**Guard pattern for early exit:**
```rask
func process_file(path: string) -> void or Error {
    let file = try open(path)
    ensure file.close()

    if file.read_line()? as line {
        let value = try parse(line)
        use(value)
    }
    return
}
```

**Negation:**
```rask
if !(state is Connected) {
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
- [Ensure Cleanup](ensure.md) — Cleanup on all exit paths

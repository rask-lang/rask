# Control Flow

## The Question
How do conditionals, loops, and control transfer work in Rask? What is the distinction between expressions and statements?

## Decision
Context-dependent expressions. The assignment context determines whether a construct produces a value or executes for side effects. Functions require explicit `return`. Labels use `label:` syntax.

## Rationale
Most control flow is for side effects (logging, validation, mutation), not value production. The assignment context (`const x = match/if ...`) naturally signals value production, while standalone constructs are side effects. This eliminates the need for trailing semicolons without introducing ambiguity. `return` always means "exit the function" — no overloading. The `loop` keyword with `deliver` provides clear syntax for value-returning loops.

## Specification

### Expression Context vs Statement Context

**The assignment context determines whether a construct produces a value or executes for side effects.**

**Expression context** — assigned to a variable, arms/branches produce values:
```rask
// Match expression — arms are values
const color = match status {
    Active => "green",
    Failed => "red",
}

// Inline if expression — branches are values
const sign = if x > 0: "+" else: "-"
```

**Statement context** — standalone, arms/branches are side effects:
```rask
// Match statement — arms are side effects
match event {
    Click(pos) => handle_click(pos),
    Key(k) => handle_key(k),
}

// If statement — branches are side effects
if error {
    log("failed")
    retry()
}
```

**Blocks in expression context** — last expression is the value:
```rask
const token = match c {
    '+' => { self.advance(); Token.Plus }   // Token.Plus is the value
    '-' => { self.advance(); Token.Minus }
    _ => Token.Unknown,
}
```

**Blocks in statement context** — produce `()`, no accidental value leakage:
```rask
while self.peek_char() is Some(c) {
    if c.is_digit() {
        num_str.push(c)
        self.advance()   // return value discarded — block produces ()
    } else {
        break
    }
}
```

**Functions** — require explicit `return` to produce values:
```rask
func format_size(bytes: i64) -> string {
    if bytes < 1024 {
        return "{bytes} B"
    }
    return "{bytes / 1024} KB"
}
```

| Context | Behavior |
|---------|----------|
| `const x = match/if ...` | Expression — arms/branches produce values |
| Standalone `match/if ...` | Statement — side effects, produces `()` |
| `return value` | Always exits the function |
| `deliver value` | Always exits the loop |
| Block `{ }` in expression | Last expression is the value |
| Block `{ }` in statement | Produces `()` |

### Semicolons

**Semicolons separate statements on the same line.** Newlines also separate statements.

```rask
// These are equivalent
do_thing()
do_other()

do_thing(); do_other()

// Semicolons also separate statements within expression-context blocks
const token = match c {
    '+' => { self.advance(); Token.Plus }
}
```

### Conditional: `if`/`else`

**Syntax:**
```rask
if condition { consequent } else { alternative }
if condition { consequent }  // else branch implicitly ()
```

**Rules:**
- Condition MUST be type `bool` (no implicit conversion)
- Parentheses around condition: allowed but not required
- Braces MUST be present (no single-statement form), except inline syntax
- In expression context: both branches MUST have same type
- When `else` omitted: statement context only (produces `()`)

| Pattern | Context | Type | Notes |
|---------|---------|------|-------|
| `const x = if c: a else: b` | Expression | `T` | Inline, both branches same type |
| `const x = if c { a } else { b }` | Expression | `T` | Block, last expression is value |
| `if c { side_effects() }` | Statement | `()` | No else needed |
| `if c { f() } else { g() }` | Statement | `()` | Both branches side effects |

**Ownership:**
- Condition is evaluated (read or consume per expression)
- Only one branch executes; ownership tracked per branch
- Linear resources: if consumed in one branch, MUST be consumed in other

```rask
// Valid: linear consumed in both branches
if cond {
    file.close()
} else {
    file.close()
}

// Invalid: linear consumed only in one branch
if cond {
    file.close()
}  // Error: file may not be consumed
```

### Pattern Matching in Conditions: `is`

The `is` keyword enables pattern matching within `if` and `while` conditions, with automatic binding of matched values.

**Syntax:**
```rask
if expr is Pattern(binding) { body }
while expr is Pattern(binding) { body }
```

**Semantics:**
- Expression is evaluated once
- If pattern matches, bindings are available in the block (smart unwrap)
- If pattern doesn't match, block is skipped (or loop exits)
- Works with any enum, not just Option

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
```

**Combined conditions:**
```rask
if state is Connected(sock) && sock.is_ready() {
    sock.send(data)
}
```

The binding `sock` is available after `&&` because `is` introduces it.

**Negation:**
```rask
if !(state is Connected(_)) {
    reconnect()
}
```

Note: `!` applies to the whole `is` expression. There is no `is not` syntax.

**Comparison with other constructs:**

| Use Case | Recommended | Alternative |
|----------|-------------|-------------|
| Check Option | `if opt?` | `if opt is Some(x)` |
| Check other enum variant | `if x is Variant(v)` | `match` |
| Exhaustive handling | `match` | — |
| Loop over iterator | `for x in iter` | `while iter.next() is Some(x)` |

**Rules:**
- `is` patterns are NOT exhaustive — unmatched cases skip the block
- Bindings are scoped to the block (like `if opt?`)
- Linear resources in bindings follow normal linear rules within the block
- `is` is an expression returning `bool`, but bindings only available in truthy branch

**Ownership:**
- Expression is evaluated once; if it produces a non-Copy value, it's moved into the pattern
- If pattern doesn't match and value is linear, it must still be handled

```rask
// Linear resource: must handle both paths
if file_result is Ok(file) {
    file.close()
} else {
    // file_result is Err here, still owned
}
```

### Extracting with `let ... is ... else`

When you need the binding to escape to the outer scope (for early returns), use `let` with `is` and a diverging `else`:

**Syntax:**
```rask
const binding = expr is Pattern else { diverge }
```

**Semantics:**
- `expr` is evaluated and matched against `Pattern`
- If match succeeds, payload is bound to `binding` in outer scope
- If match fails, `else` block executes (must diverge: `return`, `break`, `panic`, etc.)

```rask
// Early return on error
const value = result is Ok else { return Err(e) }
// value available here

// Break from loop on None
const item = queue.pop() is Some else { break }
// item available here

// Panic on unexpected state
const sock = state is Connected else { panic("not connected") }
// sock available here
```

**Multiple bindings:**
```rask
let (a, b) = result is Ok else { return Err(e) }
```

**Comparison with `if is`:**

| Syntax | Binding Scope | Use Case |
|--------|---------------|----------|
| `if x is P(v) { ... }` | Inside block | Conditional execution |
| `let v = x is P else { ... }` | Outer scope | Early exit / guard |

**Rules:**
- The `else` block MUST diverge (`return`, `break`, `continue`, `panic`, `deliver`)
- If `else` doesn't diverge, compiler error: "else block must diverge"
- Linear resources: if pattern doesn't match, the value is available in `else` for cleanup

```rask
// Linear: must handle the error case
const file = result is Ok else {
    // result is Err(e) here — e is the error, must handle
    return Err(e)
}
file.close()
```

### Conditional Chaining: `else if`

**Syntax:**
```rask
if c1 { a }
else if c2 { b }
else if c3 { c }
else { d }
```

**Desugaring:**
```rask
if c1 { a }
else { if c2 { b } else { if c3 { c } else { d } } }
```

No special syntax; `else if` is just `else` followed by `if`.

### Infinite Loop: `loop`

**Syntax:**
```rask
loop { body }
```

**Semantics:**
- Repeats forever until `break` or `deliver`
- `deliver value` exits and produces a value (expression)
- `break` exits without a value (loop evaluates to `()`)
- Type is determined by `deliver` expressions

```rask
const input = loop {
    const x = read_input()
    if x.is_valid() { deliver x }
    println("Invalid, try again")
}
```

| Exit | Loop Type | Meaning |
|------|-----------|---------|
| `deliver value` | Type of value | Exit with result |
| `break` | `()` | Exit without result |
| No exit reachable | `Never` | Infinite loop |

**Rules:**
- All `deliver` expressions MUST have the same type
- If both `deliver` and `break` are used, `deliver` determines type and `break` is error
- `deliver` only valid inside `loop` (not `while` or `for`)

### Conditional Loop: `while`

**Syntax:**
```rask
while condition { body }
```

**Semantics:**
- Evaluates condition before each iteration
- Executes body if condition is true
- Produces `()` (statement, not expression)

**Rules:**
- Condition MUST be type `bool`
- Parentheses around condition: allowed but not required
- `break` exits loop
- `continue` skips to next iteration
- `deliver` NOT allowed (use `loop` for value-returning)

```rask
while queue.len() > 0 {
    const task = queue.pop()
    process(task)
}
```

### For Loops

Fully specified in [Loops](loops.md) and [Iteration](../stdlib/iteration.md).

**Key points for control flow:**
- Produces `()` (statement, not expression)
- `break` and `continue` supported
- `deliver` NOT allowed (use `loop` for value-returning)
- Labels supported: `label: for i in coll { ... }`

### Loop Labels

**Syntax:**
```rask
label: loop { ... }
label: while cond { ... }
label: for i in coll { ... }
```

Labels enable breaking/continuing outer loops from nested contexts.

```rask
outer: for i in rows {
    for j in cols {
        if grid[i][j] == target {
            break outer
        }
    }
}
```

**Rules:**
- Labels are identifiers followed by `:`
- Labels MUST be unique within function scope
- `break label` exits labeled loop
- `continue label` continues labeled loop
- `deliver label value` exits labeled `loop` with value

| Statement | Behavior |
|-----------|----------|
| `break` | Exit innermost loop |
| `break label` | Exit labeled loop |
| `deliver value` | Exit innermost `loop` with value |
| `deliver label value` | Exit labeled `loop` with value |
| `continue` | Next iteration of innermost loop |
| `continue label` | Next iteration of labeled loop |

### Return

**Syntax:**
```rask
return           // Returns () from function
return value     // Returns value from function
```

**Semantics:**
- Immediately exits the current function
- Value must match function return type
- Triggers `ensure` cleanup (see [ensure.md](ensure.md))
- Type of `return` expression is `Never`

**Linear Resources:**
- All linear resources in scope MUST be consumed or ensured before `return`
- `ensure` satisfies this requirement
- See [Sum Types - Error Propagation](../types/enums.md#error-propagation-and-linear-resources)

```rask
func process(file: File) -> Data or Error {
    ensure file.close()

    const data = try file.read()   // try may return early; ensure handles cleanup
    Ok(transform(data))
}
```

### Function Returns

**Functions require explicit `return` to produce values. `return` always exits the function.**

```rask
func double(x: i32) -> i32 {
    return x * 2
}

func greet(name: string) {
    println("Hello, " + name)  // No return: function produces ()
}

func format_size(bytes: i64) -> string {
    if bytes < 1024 {
        return "{bytes} B"
    }
    return "{bytes / 1024} KB"
}
```

| Function Body | Return Value |
|---------------|--------------|
| `return value` | Exits function, returns `value` |
| No `return` statement | Returns `()` |
| Empty block `{}` | Returns `()` |

**Note:** `return` inside a match or if block exits the **function**, not just the match/if:

```rask
func token_string(self) -> string {
    // return in match arms exits the function
    match self.current {
        Number(n) => return "{n}",
        Plus => return "+",
        Minus => return "-",
    }
}
```

### Block Expressions

**Blocks in statement context produce `()`.** Blocks in expression context produce their last expression's value.

```rask
// Statement context — block produces ()
{
    const temp = compute()
    process(temp)   // return value discarded
}

// Expression context — last expression is the value
const result = {
    const temp = compute()
    transform(temp)   // this IS the block's value
}
```

**Scope:**
- Variables declared in block are scoped to block
- `ensure` registered in block runs at block exit
- Linear resources must be consumed before block exits

### Divergent Expressions (Never Type)

Some expressions never complete normally:

| Expression | Type |
|------------|------|
| `return ...` | `Never` |
| `break` | `Never` |
| `deliver ...` | `Never` |
| `continue` | `Never` |
| `panic(...)` | `Never` |
| `loop { /* no exit */ }` | `Never` |

`Never` coerces to any type:

```rask
let x: i32 = if cond { 42 } else { panic("nope") }
// else branch is Never, coerces to i32
```

### Control Flow and Ensure

`ensure` runs on ALL exit paths:

| Exit Path | Ensure Runs? |
|-----------|--------------|
| Normal block end | Yes |
| `return` | Yes |
| `break` | Yes |
| `deliver` | Yes |
| `continue` | Yes |
| `try` propagation | Yes |
| `panic` | Yes |

See [Ensure Cleanup](../ecosystem/ensure.md) for full specification.

### Edge Cases

| Case | Behavior |
|------|----------|
| `if` without else, used as expression | Error unless consequent is `()` |
| `deliver` in `while` or `for` | Error: use `loop` instead |
| Unlabeled `break` outside loop | Error: break outside loop |
| `break label` with nonexistent label | Error: undefined label |
| `return` in `ensure` body | Error: cannot return from ensure |
| `break`/`deliver`/`continue` in `ensure` body | Error: cannot break from ensure |
| Empty `if` branches | Valid: `if c {} else {}` evaluates to `()` |
| Nested `ensure` in loop | Each iteration registers new ensure |
| `loop` with both `deliver` and `break` | Error: inconsistent exit types |
| `loop` without exit | Type is `Never` (infinite loop) |

## Examples

### Value-Producing Match and If
```rask
// Match expression — arms produce values
const color = match status {
    Active => "green",
    Inactive => "gray",
    Failed => "red",
}

// Inline if expression
const sign = if x > 0: "+" else: "-"

// Match with block arms — last expression is value
const opts = match parse_args(args) {
    Ok(o) => o,
    Err(e) => {
        println("error: {e}")
        std.exit(1)
    }
}
```

### Loop with Deliver
```rask
const input = loop {
    const x = read_input()
    if x.is_valid() {
        deliver x
    }
    println("Invalid, try again")
}
```

### Search Pattern
<!-- test: skip -->
```rask
func find_first<T: Eq>(items: Vec<T>, target: T) -> Option<usize> {
    let i = 0
    loop {
        if i >= items.len() {
            deliver None
        }
        if items[i] == target {
            deliver Some(i)
        }
        i += 1
    }
}
```

### Labeled Deliver
```rask
func find_in_matrix<T: Eq>(matrix: Vec<Vec<T>>, target: T) -> Option<(usize, usize)> {
    search: loop {
        for i in 0..matrix.len() {
            for j in 0..matrix[i].len() {
                if matrix[i][j] == target {
                    deliver search Some((i, j))
                }
            }
        }
        deliver None
    }
}
```

### While with Mutation
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

### Server Loop (No Value)
<!-- test: parse -->
```rask
func run_server(server: Server) {
    loop {
        const conn = server.accept()
        handle(conn)
    }
}
```

## Integration Notes

- **Match:** Already specified in [enums.md](../types/enums.md); follows same expression semantics
- **For loops:** Specified in [loops.md](loops.md); statement, not expression
- **Ensure:** Cleanup runs on all control flow exits; see [ensure.md](ensure.md)
- **Linear resource types:** Must be consumed on all branches; ensure enables `try`/`return` safety
- **Compiler:** Control flow analysis is local (no whole-program); divergence tracked per-block

## Summary

| Keyword | Meaning | Scope |
|---------|---------|-------|
| `return` | Exit function with value | Functions |
| `break` | Exit loop (no value) | `loop`, `while`, `for` |
| `deliver` | Exit loop with value | `loop` only |
| `continue` | Next iteration | `loop`, `while`, `for` |

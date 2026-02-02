# Control Flow

## The Question
How do conditionals, loops, and control transfer work in Rask? What is the distinction between expressions and statements?

## Decision
Expression-oriented design. `if`, `match`, `loop`, and blocks are expressions that produce values. `while` and `for` are statements (produce unit). Semicolons distinguish expression position from statement position. Labels use `label:` syntax.

## Rationale
Expression-oriented design reduces ceremony for common patterns (assigning from conditionals, returning from loops). The `loop` keyword with `deliver` provides clear, intuitive syntax for value-returning infinite loops: `deliver` means "deliver this result" — clearer than Rust's `break value`. The `label:` syntax is universal across languages.

## Specification

### Expressions vs Statements

**Expressions** produce values; **statements** do not.

| Construct | Kind | Produces |
|-----------|------|----------|
| `if`/`else` | Expression | Value of chosen branch |
| `match` | Expression | Value of matched arm |
| `loop` | Expression | Value from `deliver` |
| `{ ... }` (block) | Expression | Value of final expression |
| `while` | Statement | Unit `()` |
| `for` | Statement | Unit `()` |
| `return` | Statement | Never (exits function) |
| `break`/`continue` | Statement | Never (exits loop) |
| `deliver` | Statement | Never (exits loop with value) |

### Semicolons

**Rule:** Trailing semicolon converts an expression to a statement (discards value).

```rask
const x = if cond { 1 } else { 2 }   // Expression position: no semicolon, value used
if cond { do_thing() };            // Statement position: semicolon discards unit
```

| Syntax | Meaning |
|--------|---------|
| `expr` at block end | Block evaluates to `expr` |
| `expr;` at block end | Block evaluates to `()` |
| `expr;` mid-block | Statement, value discarded |

### Conditional: `if`/`else`

**Syntax:**
```rask
if condition { consequent } else { alternative }
if condition { consequent }  // else branch implicitly ()
```

**Rules:**
- Condition MUST be type `bool` (no implicit conversion)
- Parentheses around condition: allowed but not required
- Braces MUST be present (no single-statement form)
- When used as expression: both branches MUST have same type
- When `else` omitted: consequent MUST be `()` (unless used as statement)

| Pattern | Type | Notes |
|---------|------|-------|
| `if c { T } else { T }` | `T` | Both branches same type |
| `if c { T } else { Never }` | `T` | Divergent branch (return/panic) |
| `if c { () }` | `()` | No else needed for unit |
| `if c { T }` (no else, T != ()) | Error | Missing else branch |

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
func process(file: File) -> Result<Data, Error> {
    ensure file.close()

    const data = file.read()?   // ? may return early; ensure handles cleanup
    Ok(transform(data))
}
```

### Implicit Return

The final expression in a function (without semicolon) is the return value.

```rask
func double(x: i32) -> i32 {
    x * 2           // Implicit return (no semicolon)
}

func greet(name: string) {
    println("Hello, " + name);  // Semicolon: returns ()
}
```

| Function End | Return Value |
|--------------|--------------|
| `expr` | Returns `expr` |
| `expr;` | Returns `()` |
| Empty block | Returns `()` |
| `return value` | Returns `value` (explicit) |

### Block Expressions

Blocks are expressions; they evaluate to their final expression.

```rask
const result = {
    const temp = compute()
    transform(temp)    // Block value (no semicolon)
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
| `?` propagation | Yes |
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

### Expression-Oriented If
```rask
const status = if count > 0 { "active" } else { "empty" }

const message = if user.is_admin() {
    format!("Welcome, {}", user.name)
} else if user.is_guest() {
    "Welcome, guest"
} else {
    format!("Hello, {}", user.name)
}
```

### Loop with Deliver
```rask
const input = loop {
    const x = read_input()
    if x.is_valid() { deliver x }
    println("Invalid, try again")
}
```

### Search Pattern
<!-- test: skip -->
```rask
func find_first<T: Eq>(items: Vec<T>, target: T) -> Option<usize> {
    let i = 0
    loop {
        if i >= items.len() { deliver None }
        if items[i] == target { deliver Some(i) }
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
- **Linear types:** Must be consumed on all branches; ensure enables `?`/`return` safety
- **Compiler:** Control flow analysis is local (no whole-program); divergence tracked per-block

## Summary

| Keyword | Meaning | Scope |
|---------|---------|-------|
| `return` | Exit function with value | Functions |
| `break` | Exit loop (no value) | `loop`, `while`, `for` |
| `deliver` | Exit loop with value | `loop` only |
| `continue` | Next iteration | `loop`, `while`, `for` |

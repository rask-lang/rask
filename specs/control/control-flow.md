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

```
let x = if cond { 1 } else { 2 }   // Expression position: no semicolon, value used
if cond { do_thing() };            // Statement position: semicolon discards unit
```

| Syntax | Meaning |
|--------|---------|
| `expr` at block end | Block evaluates to `expr` |
| `expr;` at block end | Block evaluates to `()` |
| `expr;` mid-block | Statement, value discarded |

### Conditional: `if`/`else`

**Syntax:**
```
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

```
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

### Conditional Chaining: `else if`

**Syntax:**
```
if c1 { a }
else if c2 { b }
else if c3 { c }
else { d }
```

**Desugaring:**
```
if c1 { a }
else { if c2 { b } else { if c3 { c } else { d } } }
```

No special syntax; `else if` is just `else` followed by `if`.

### Infinite Loop: `loop`

**Syntax:**
```
loop { body }
```

**Semantics:**
- Repeats forever until `break` or `deliver`
- `deliver value` exits and produces a value (expression)
- `break` exits without a value (loop evaluates to `()`)
- Type is determined by `deliver` expressions

```
let input = loop {
    let x = read_input()
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
```
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

```
while queue.len() > 0 {
    let task = queue.pop()
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
```
label: loop { ... }
label: while cond { ... }
label: for i in coll { ... }
```

Labels enable breaking/continuing outer loops from nested contexts.

```
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
```
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

```
fn process(file: File) -> Result<Data, Error> {
    ensure file.close()

    let data = file.read()?   // ? may return early; ensure handles cleanup
    Ok(transform(data))
}
```

### Implicit Return

The final expression in a function (without semicolon) is the return value.

```
fn double(x: i32) -> i32 {
    x * 2           // Implicit return (no semicolon)
}

fn greet(name: string) {
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

```
let result = {
    let temp = compute()
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

```
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
```
let status = if count > 0 { "active" } else { "empty" }

let message = if user.is_admin() {
    format!("Welcome, {}", user.name)
} else if user.is_guest() {
    "Welcome, guest"
} else {
    format!("Hello, {}", user.name)
}
```

### Loop with Deliver
```
let input = loop {
    let x = read_input()
    if x.is_valid() { deliver x }
    println("Invalid, try again")
}
```

### Search Pattern
```
fn find_first<T: Eq>(items: Vec<T>, target: T) -> Option<usize> {
    let i = 0
    loop {
        if i >= items.len() { deliver None }
        if items[i] == target { deliver Some(i) }
        i += 1
    }
}
```

### Labeled Deliver
```
fn find_in_matrix<T: Eq>(matrix: Vec<Vec<T>>, target: T) -> Option<(usize, usize)> {
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
```
fn drain_queue(mutate queue: Queue<Task>) {
    while queue.len() > 0 {
        let task = queue.pop()
        if task.is_cancelled() {
            continue
        }
        task.run()
    }
}
```

### Server Loop (No Value)
```
fn run_server(server: Server) {
    loop {
        let conn = server.accept()
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

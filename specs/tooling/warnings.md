<!-- depends: tooling/lint.md, structure/build.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-diagnostics/ -->

# Compiler Warnings

`rask check` emits warnings for code that compiles but looks wrong. This is different from `rask lint` which enforces conventions — warnings are correctness hints from the compiler itself.

```
rask check src/server.rk                 # type check with warnings
rask check src/server.rk --deny-warnings # warnings become errors (CI mode)
rask check src/server.rk --format json   # JSON output for IDE integration
```

---

## Philosophy

I drew a hard line between three categories:

| Category | Tool | Blocks compilation? | Purpose |
|----------|------|---------------------|---------|
| **Error** | `rask check` | Yes | Safety violation, type error, broken code |
| **Warning** | `rask check` | No (unless `--deny-warnings`) | Suspicious-but-valid code, likely bugs |
| **Lint** | `rask lint` | No | Convention enforcement, style, idioms |

**Errors** are non-negotiable — the program is wrong. Use-after-move, type mismatch, missing return. These block compilation because running broken code is worse than not running code.

**Warnings** flag code that compiles but probably isn't what you meant. Ignoring a `T or E` return value is legal, but it's almost always a bug. An unused variable isn't dangerous, but it's clutter that hides real problems.

**Lints** enforce how code should look, not whether it's correct. Naming conventions, idiomatic patterns, style rules. See [lint.md](lint.md).

The boundary: if the code is wrong, it's an error. If the code *might* hide a bug, it's a warning. If the code just violates convention, it's a lint.

---

## Default Warnings

On by default. Suppress with `@allow(warning_name)`.

| Code | ID | Check |
|------|----|-------|
| W0201 | `unused_import` | Import never referenced in this file |
| W0301 | `unused_result` | `T or E` return value not checked |
| W0901 | `unused_variable` | Binding never read after assignment |
| W0902 | `unreachable_code` | Code after `return` or `break` |
| W0903 | `deprecated` | Calling an item marked `@deprecated` |

### `unused_variable` (W0901)

Fires when a `const` or `let` binding is never read. Prefix with `_` to suppress.

```rask
func process(data: Vec<u8>) -> i32 {
    const count = data.len()
    const _unused = setup()
    return 42
}
```

```
warning[W0901]: unused variable `count`
  --> src/process.rk:2:11
   |
 2 |     const count = data.len()
   |           ^^^^^ this value is never read
   |
   = fix: prefix with `_` if intentional: `const _count = data.len()`
```

### `unused_import` (W0201)

Fires when an imported module or symbol is never used in the file.

```rask
import json
import http using Request

func handle(req: Request) -> Response {
    return Response.ok("hello")
}
```

```
warning[W0201]: unused import `json`
  --> src/server.rk:1:8
   |
 1 | import json
   |        ^^^^ never referenced in this file
   |
   = fix: remove the import
```

### `unused_result` (W0301)

Fires when a function returning `T or E` is called and the result is discarded. This is the most important warning in the language — Rask's error model depends on callers handling results.

```rask
func save(data: Data) -> () or IoError {
    file.write(data)
    return Ok(())
}
```

```
warning[W0301]: unused result of type `() or IoError`
  --> src/save.rk:2:5
   |
 2 |     file.write(data)
   |     ^^^^^^^^^^^^^^^^ this `T or E` result is discarded
   |
   = fix: use `try` to propagate, or handle the error explicitly
```

**Not triggered by:**
- Functions returning plain types (no error to miss)
- `T?` return values (intentional absence, less dangerous than silenced errors)
- Results assigned to a binding (that's `unused_variable`'s job)

I considered making this an error, but there are rare legitimate cases — fire-and-forget logging, best-effort cleanup. A warning you can `@deny` project-wide is the right level.

### `unreachable_code` (W0902)

Fires when code follows a `return`, `break`, or diverging expression.

```rask
func check(x: i32) -> bool {
    if x > 0 {
        return true
    } else {
        return false
    }
    println("unreachable")
    return false
}
```

```
warning[W0902]: unreachable code
  --> src/check.rk:7:5
   |
 7 |     println("unreachable")
   |     ^^^^^^^^^^^^^^^^^^^^^^ all prior branches return
```

### `deprecated` (W0903)

Fires when calling a function, method, or type marked with `@deprecated`. Includes the deprecation message.

```rask
@deprecated("use connect_with_options instead")
public func connect(host: string) -> Connection or Error {
    return connect_with_options(host, Options.default())
}

func main() -> () or Error {
    const conn = try connect("localhost")
    return Ok(())
}
```

```
warning[W0903]: use of deprecated item `connect`
  --> src/main.rk:7:22
   |
 7 |     const conn = try connect("localhost")
   |                      ^^^^^^^ deprecated
   |
   = note: use connect_with_options instead
```

---

## Opt-In Warnings

Off by default. Enable with `@warn(warning_name)` on items or project-wide in `rask.build`.

| Code | ID | Check |
|------|----|-------|
| W0904 | `implicit_copy` | Flags implicit copies of types at the ≤16-byte threshold |
| W0905 | `shadowing` | Variable shadows an outer binding in the same function |
| W0906 | `type_narrowing` | Pattern match could use a more specific type |

### `implicit_copy` (W0904)

For performance-critical code that needs to audit every copy. Fires on any implicit copy of types at or below the 16-byte copy threshold.

```rask
@warn(implicit_copy)
func hot_loop(points: Vec<Point>) {
    for p in points {
        const q = p
        process(q)
    }
}
```

```
warning[W0904]: implicit copy of `Point` (12 bytes)
  --> src/render.rk:4:19
   |
 4 |         const q = p
   |                   ^ implicit copy
```

Most code shouldn't care — implicit copy is a core ergonomic feature. But game loops and embedded code sometimes need to know exactly where copies happen. This is the audit tool for that. See [value-semantics.md](../memory/value-semantics.md#the-16-byte-threshold) for the design rationale.

### `shadowing` (W0905)

Fires when a binding shadows an outer binding in the same function.

```rask
@warn(shadowing)
func transform(x: i32) -> i32 {
    const result = x * 2
    if result > 100 {
        const result = 100
        return result
    }
    return result
}
```

```
warning[W0905]: `result` shadows binding from line 3
  --> src/math.rk:5:15
   |
 5 |         const result = 100
   |               ^^^^^^ shadows outer `result`
```

Shadowing is a deliberate language feature — I don't want to flag it by default. But some teams want to see it, especially in long functions where it can cause confusion.

### `type_narrowing` (W0906)

Hint when a pattern match scrutinee could benefit from a more specific type.

```rask
@warn(type_narrowing)
func process(value: any Printable) {
    match value {
        s: string => println(s),
        _ => println("other"),
    }
}
```

Informational, not a bug indicator. Useful during refactoring to find places where generic code became specific.

---

## Configuration

Three levels. More specific wins.

### Attribute-level

Apply `@allow`, `@warn`, or `@deny` to any item or block:

```rask
@allow(unused_variable)
func scratch() {
    const x = expensive_setup()
}

@deny(unused_result)
extend Server {
    func start(take self) -> () or Error {
        try self.listener.bind(self.addr)
        return Ok(())
    }
}

@warn(implicit_copy)
func render_frame(entities: Vec<Entity>) {
    for e in entities {
        draw(e)
    }
}
```

Attributes cascade into nested items — `@deny(unused_result)` on an `extend` block applies to all methods inside it.

### Package-level

Configure warnings for the entire project in `rask.build`:

```rask
package "my-server" "1.0.0" {
    dep "http" "^2.0"

    warnings {
        deny: ["unused_result"]
        allow: ["shadowing"]
        warn: ["implicit_copy"]
    }
}
```

| Key | Effect |
|-----|--------|
| `deny` | Promote these warnings to errors |
| `allow` | Suppress these warnings entirely |
| `warn` | Enable these opt-in warnings |

### CLI

```
rask check src/ --deny-warnings
```

Promotes all warnings to errors. Blunt instrument for CI — use `rask.build` for granular control.

### Precedence

Inline attributes override package config. Package config overrides defaults. `--deny-warnings` promotes anything that wasn't explicitly `@allow`'d — it won't break builds that have intentional suppressions.

```
@allow on item          →  always suppressed (even with --deny-warnings)
@deny on item           →  always an error
@warn on item           →  always a warning
package deny/allow/warn →  overrides default for whole project
--deny-warnings         →  promotes remaining warnings to errors
default                 →  on or off per warning
```

---

## Relationship to Lint

| | Compiler Warnings | Lint Rules |
|-|-------------------|------------|
| **Tool** | `rask check` | `rask lint` |
| **What it checks** | Correctness hints | Convention enforcement |
| **ID format** | `unused_result` / `W0301` | `naming/is` / `idiom/unwrap-production` |
| **Suppression** | `@allow(unused_result)` | `@allow(naming/is)` |
| **When it runs** | Every build | Pre-commit, CI |

Both share the `@allow` attribute and the diagnostic output format. They use different ID namespaces so there's no collision.

---

## Warning Codes

Codes follow the same numbering as error codes, prefixed with `W`:

| Range | Category |
|-------|----------|
| W02xx | Resolver (unused imports) |
| W03xx | Type checking (unused results, narrowing) |
| W09xx | General (unused variables, unreachable code, shadowing, copies) |

Both the code (`W0301`) and the name (`unused_result`) work in attributes. `@allow(W0301)` and `@allow(unused_result)` are equivalent.

---

## Future

- **`unused_field`** — struct field never read outside the defining module.
- **`unnecessary_clone`** — `.clone()` on a type that's already Copy (≤16 bytes).
- **`large_move`** — moving a type significantly above the 16-byte threshold without explicit `own`.

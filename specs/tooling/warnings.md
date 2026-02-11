<!-- id: tool.warnings -->
<!-- status: decided -->
<!-- summary: Compiler warnings for suspicious-but-valid code -->
<!-- depends: tooling/lint.md, structure/build.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-diagnostics/ -->

# Compiler Warnings

`rask check` emits warnings for code that compiles but looks wrong. Distinct from `rask lint` which enforces conventions — warnings are correctness hints from the compiler.

## Severity Boundaries

| Rule | Description |
|------|-------------|
| **SB1: Error** | Safety violation, type error, broken code — blocks compilation |
| **SB2: Warning** | Suspicious-but-valid code, likely bugs — doesn't block (unless `--deny-warnings`) |
| **SB3: Lint** | Convention enforcement, style, idioms — separate tool (`rask lint`) |

The boundary: wrong code is an error. Code that *might* hide a bug is a warning. Code that violates convention is a lint.

## Default Warnings

On by default. Suppress with `@allow(warning_name)`.

| Rule | Code | ID | Check |
|------|------|----|-------|
| **W1: unused_import** | W0201 | `unused_import` | Import never referenced in this file |
| **W2: unused_result** | W0301 | `unused_result` | `T or E` return value not checked |
| **W3: unused_variable** | W0901 | `unused_variable` | Binding never read after assignment |
| **W4: unreachable_code** | W0902 | `unreachable_code` | Code after `return` or `break` |
| **W5: deprecated** | W0903 | `deprecated` | Calling an item marked `@deprecated` |

<!-- test: skip -->
```rask
func process(data: Vec<u8>) -> i32 {
    const count = data.len()       // W3: unused variable
    const _unused = setup()        // OK: _ prefix suppresses
    return 42
}
```

**W2 (unused_result) exceptions:** Not triggered by plain return types (no error to miss), `T?` values (intentional absence), or results assigned to a binding (that's W3's job).

## Opt-In Warnings

Off by default. Enable with `@warn(warning_name)` on items or project-wide in `rask.build`.

| Rule | Code | ID | Check |
|------|------|----|-------|
| **W6: implicit_copy** | W0904 | `implicit_copy` | Implicit copy of types at the 16-byte threshold |
| **W7: shadowing** | W0905 | `shadowing` | Variable shadows an outer binding in the same function |
| **W8: type_narrowing** | W0906 | `type_narrowing` | Pattern match could use a more specific type |

<!-- test: skip -->
```rask
@warn(implicit_copy)
func hot_loop(points: Vec<Point>) {
    for p in points {
        const q = p          // W6: implicit copy of Point (12 bytes)
        process(q)
    }
}
```

## Configuration

Three levels. More specific wins.

| Rule | Description |
|------|-------------|
| **CF1: Attribute-level** | `@allow`, `@warn`, `@deny` on any item or block; cascades into nested items |
| **CF2: Package-level** | `warnings` section in `rask.build` sets project-wide defaults |
| **CF3: CLI-level** | `--deny-warnings` promotes all warnings to errors |

<!-- test: skip -->
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
```

Package-level configuration in `rask.build`:

<!-- test: skip -->
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
| `deny` | Promote warnings to errors |
| `allow` | Suppress warnings entirely |
| `warn` | Enable opt-in warnings |

## Precedence

| Rule | Description |
|------|-------------|
| **P1: Inline wins** | `@allow` on item always suppresses (even with `--deny-warnings`) |
| **P2: Deny on item** | `@deny` on item always promotes to error |
| **P3: Package overrides default** | `rask.build` config overrides per-warning defaults |
| **P4: CLI promotes remaining** | `--deny-warnings` promotes anything not explicitly `@allow`'d |

## Warning Codes

| Rule | Description |
|------|-------------|
| **WC1: Dual ID** | Both code (`W0301`) and name (`unused_result`) work in attributes |
| **WC2: Code ranges** | W02xx = resolver, W03xx = type checking, W09xx = general |

## Error Messages

```
WARNING [tool.warnings/W3]: unused variable `count`
   |
2  |     const count = data.len()
   |           ^^^^^ this value is never read

FIX: prefix with `_` if intentional: `const _count = data.len()`
```

```
WARNING [tool.warnings/W2]: unused result of type `() or IoError`
   |
2  |     file.write(data)
   |     ^^^^^^^^^^^^^^^^ this `T or E` result is discarded

FIX: use `try` to propagate, or handle the error explicitly
```

```
WARNING [tool.warnings/W4]: unreachable code
   |
7  |     println("unreachable")
   |     ^^^^^^^^^^^^^^^^^^^^^^ all prior branches return
```

```
WARNING [tool.warnings/W5]: use of deprecated item `connect`
   |
7  |     const conn = try connect("localhost")
   |                      ^^^^^^^ deprecated

NOTE: use connect_with_options instead
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `_` prefix on variable | W3 | Suppresses `unused_variable` |
| `T?` return discarded | W2 | Not flagged (intentional absence, less dangerous) |
| Result assigned to binding | W2 | Not flagged (W3 handles unused bindings) |
| `@deny` on item + `@allow` on nested | P1 | Nested `@allow` wins |
| `--deny-warnings` + `@allow` on item | P1 | `@allow` wins |
| Code after diverging `match` | W4 | Flagged if all arms diverge |

---

## Appendix (non-normative)

### Rationale

**SB1-SB3 (three categories):** A hard line between errors, warnings, and lints. Errors are non-negotiable — running broken code is worse than not running code. Warnings flag code that probably isn't what you meant. Lints enforce how code should look.

**W2 (unused_result):** The most important warning in the language. Rask's error model depends on callers handling results. I considered making this an error, but there are rare legitimate cases — fire-and-forget logging, best-effort cleanup. A warning you can `@deny` project-wide is the right level.

**W6 (implicit_copy) off by default:** Implicit copy is a core ergonomic feature. Most code shouldn't care. But game loops and embedded code sometimes need to audit every copy. See `mem.value-semantics` for the 16-byte threshold design.

**W7 (shadowing) off by default:** Shadowing is a deliberate language feature. Some teams want to see it, especially in long functions where it causes confusion, but flagging it by default would be noisy.

### Patterns & Guidance

**Compiler warnings vs lint rules:**

| | Compiler Warnings | Lint Rules |
|-|-------------------|------------|
| **Tool** | `rask check` | `rask lint` |
| **What it checks** | Correctness hints | Convention enforcement |
| **ID format** | `unused_result` / `W0301` | `naming/is` / `idiom/unwrap-production` |
| **Suppression** | `@allow(unused_result)` | `@allow(naming/is)` |
| **When it runs** | Every build | Pre-commit, CI |

Both share the `@allow` attribute and diagnostic output format. Different ID namespaces prevent collision.

### Future

- **`unused_field`** — struct field never read outside the defining module
- **`unnecessary_clone`** — `.clone()` on a type already Copy (16 bytes or less)
- **`large_move`** — moving a type significantly above 16-byte threshold without explicit `own`

### See Also

- `tool.lint` — convention enforcement (`rask lint`)
- `mem.value-semantics` — copy threshold design
- `struct.build` — package-level warning configuration

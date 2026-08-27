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
| **W9: torn_lock_update** | W0907 | `torn_lock_update` | `with` block over `Mutex`/`Shared` assigns 2+ fields of the locked value without `.staged()` (`conc.sync/ST1–ST4`) |
| **W10: ensure_order** | W0908 | `ensure_order` | An `ensure` for a resource is registered after an `ensure` for something derived from it, so LIFO tears the dependency down first (`mem.resource-types/EO1`) |
| **W11: mod_for_index** | W0909 | `mod_for_index` | `%` whose result is used as an index and whose left operand can be negative — `%` takes the dividend's sign (`type.operators/AR2`), so `(i - 1) % n` indexes out of range instead of wrapping. `.mod(n)` is the floored answer (AR3) |

<!-- test: skip -->
```rask
func process(data: Vec<u8>) -> i32 {
    let count = data.len()       // W3: unused variable
    let _unused = setup()        // OK: _ prefix suppresses
    return 42
}
```

**W2 (unused_result) exceptions:** Not triggered by plain return types (no error to miss), `T?` values (intentional absence), or results assigned to a binding (that's W3's job).

**W11 (mod_for_index) scope:** Only where the remainder *is* the index of a `[…]` access — a `%` whose result is a value is usually exactly what was meant. Silent when the left operand can't be negative: a non-negative literal, a `.len()`/`.count()` call, or a sum/product of those. A suggestion, so the fix is printed as code: `ring[(i - 1).mod(n)]`.

**W10 (ensure_order) scope:** Derivation is read off the calls themselves, function-locally: the dependency appeared as an argument to the call that produced the dependent, or it appears in the dependent's own cleanup call. Independent resources never warn regardless of order, and an alias (`let b = a`) is not a derivation. Anything smarter needs a false-positive budget first.

**W9 (torn_lock_update) implementation:** checked at the `with` binding in
`rask-types` rather than as a syntactic pass, because it must not fire under
`Local` — nothing there can observe a torn update and `staged()` is refused
outright (`conc.sync/ST3a`), so the suggested fix would be a compile error.
Suppression is `@allow(torn_lock_update)` on the enclosing *function*; a `test`
block can't carry it, because `TestDecl` has no attributes ([#1010](https://github.com/rask-lang/rask/issues/1010)).

**W9 (torn_lock_update) scope:** Fires on two or more field assignments to the locked binding in one `with` block. Mutating method calls (`q.push(a)` twice) don't trigger — method bodies are opaque, and flagging every call pair would drown the real signal. A panic between the flagged writes leaves survivors a broken invariant (`ctrl.panic/LK3–LK4`); `.staged()` makes the update panic-atomic.

## Opt-In Warnings

Off by default. Enable with `@warn(warning_name)` on items or project-wide in `build.rk`.

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
        let q = p          // W6: implicit copy of Point (12 bytes)
        process(q)
    }
}
```

## Configuration

Three levels. More specific wins.

| Rule | Description |
|------|-------------|
| **CF1: Attribute-level** | `@allow`, `@warn`, `@deny` on any item or block; cascades into nested items |
| **CF2: Package-level** | `warnings` section in `build.rk` sets project-wide defaults |
| **CF3: CLI-level** | `--deny-warnings` promotes all warnings to errors |

<!-- test: skip -->
```rask
@allow(unused_variable)
func scratch() {
    let x = expensive_setup()
}

@deny(unused_result)
extend Server {
    func start(take self) -> void or Error {
        try self.listener.bind(self.addr)
        return
    }
}
```

Package-level configuration in `build.rk`:

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
| **P3: Package overrides default** | `build.rk` config overrides per-warning defaults |
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
2  |     let count = data.len()
   |           ^^^^^ this value is never read

FIX: prefix with `_` if intentional: `let _count = data.len()`
```

```
WARNING [tool.warnings/W2]: unused result of type `void or IoError`
   |
2  |     file.write(data)
   |     ^^^^^^^^^^^^^^^^ this `T or E` result is discarded

FIX: use `try` to propagate, or handle the error explicitly
```

```
WARNING [tool.warnings/W9]: multi-field update under a lock without staged()
   |
3  |  with accounts as a {
4  |      a.checking -= amount
   |      ^^^^^^^^^^ first field written
5  |      a.savings += amount
   |      ^^^^^^^^^ second field — a panic between these leaves other tasks a half-done update

FIX: stage the update — commits as one move on clean exit, discards on panic:

    with accounts.staged() as a {
        a.checking -= amount
        a.savings += amount
    }

Or `@allow(torn_lock_update)` if partial state is harmless here.
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
7  |     let conn = try connect("localhost")
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

**W9 (torn_lock_update) on by default:** Rask has no lock poisoning — a panic mid-update releases the lock and survivors see whatever was written (`ctrl.panic/LK1–LK4`). The by-construction fix (`staged()`) only helps if the sites that need it get pointed at it, so the pointer can't be opt-in tooling. Started as a lint candidate (`idiom/staged-multi-write`); promoted to a default compiler warning when [#485](https://github.com/rask-lang/rask/issues/485) made the point that the torn-invariant story shouldn't rest on tools you have to turn on.

### Patterns & Guidance

**Compiler warnings vs lint rules:**

| | Compiler Warnings | Lint Rules |
|-|-------------------|------------|
| **Tool** | `rask check` | `rask lint` |
| **What it checks** | Correctness hints | Convention enforcement |
| **ID format** | `unused_result` / `W0301` | `naming/is` / `idiom/force-unwrap-production` |
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

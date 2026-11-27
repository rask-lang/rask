<!-- id: tool.warnings -->
<!-- status: decided -->
<!-- summary: Compiler warnings for suspicious-but-valid code -->
<!-- depends: tooling/lint.md, structure/build.md -->
<!-- implemented-by: compiler/crates/rask-check/, compiler/crates/rask-diagnostics/ -->

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
| **W1: unused_import** | W0201 | `unused_import` | Import nothing in its package refers to |
| **W2: moved** | | | Ignoring a `T or E` result is no longer a warning: it is a compile error, `type.errors/ER50` |
| **W3: unused_variable** | W0901 | `unused_variable` | Binding never read after assignment |
| **W4: unreachable_code** | W0902 | `unreachable_code` | Code after `return`, `break`, `continue`, a `@noreturn` call, or a branch that leaves on every path |
| **W5: deprecated** | W0903 | `deprecated` | Calling an item marked `@deprecated` |
| **W9: torn_lock_update** | W0907 | `torn_lock_update` | `with` block over a `Shared` assigns 2+ fields of the locked value without `.staged()` (`conc.sync/ST1–ST4`) |
| **W11: mod_for_index** | W0909 | `mod_for_index` | `%` whose result is used as an index and whose left operand can be negative — `%` takes the dividend's sign (`type.operators/AR2`), so `(i - 1) % n` indexes out of range instead of wrapping. `.mod(n)` is the floored answer (AR3) |
| **W13: blocking_loop** | `comp.effects/CW2` | `blocking_loop` | A call that blocks, in a loop, in a program that spawns (`comp.effects/CW2`). Suppressed on the function, for a loop whose waits can't overlap anyway: a terminal reading one line at a time |

<!-- test: skip -->
```rask
func process(data: Vec<u8>) -> i32 {
    let count = data.len()       // W3: unused variable
    let _unused = setup()        // OK: _ prefix suppresses
    return 42
}
```

**W1 (unused_import) scope:** An import counts as used when anything in its package names it: an expression, a type, a type argument, a pattern, an `implements` block, a bound. Package-wide rather than per file, because an import reaches the whole package today, and a warning that says "remove this" must never leave a build that no longer compiles. A glob import is left alone (IM6 already warns). Importing a package only for a conformance it declares is unused too: a conformance is visible through the dependency graph in `rask.toml`, not through `import` (`type.generics/XC4`). An import carries no attributes, so there's no `@allow` for it: delete the line.

**W4 (unreachable_code) scope:** The first statement after one control never gets past: `return`, `break`, `continue`, a call to a `@noreturn` function (`panic`, `exit`), a `loop` with no `break` anywhere inside, or an `if`/`match` whose every branch does one of those. One warning per block, at the first dead statement. Suppress with `@allow(unreachable_code)` on the function or test block — the usual case is a spawned task that panics and still needs a `return` to say what type it would have produced.

**W3 (unused_variable) scope:** `let` and `mut` locals. A plain `x = v` doesn't count as reading `x`; `x += v` does, since it reads the old value. Parameters and names bound by patterns aren't checked yet.

**W11 (mod_for_index) scope:** Only where the remainder *is* the index of a `[…]` access — a `%` whose result is a value is usually exactly what was meant. Silent when the left operand can't be negative: a non-negative literal, a `.len()`/`.count()` call, or a sum/product of those. A suggestion, so the fix is printed as code: `ring[(i - 1).mod(n)]`.

**W9 (torn_lock_update) implementation:** checked at the `with` binding in
`rask-check` rather than as a syntactic pass, because it must not fire under
`Local` — nothing there can observe a torn update and `staged()` is refused
outright (`conc.sync/ST3a`), so the suggested fix would be a compile error.
Suppression is `@allow(torn_lock_update)` on the enclosing *function*; a `test`
block can't carry it, because `TestDecl` has no attributes ([#1010](https://github.com/rask-lang/rask/issues/1010)).

**W9 (torn_lock_update) scope:** Fires on two or more field assignments to the locked binding in one `with` block. Mutating method calls (`q.push(a)` twice) don't trigger — method bodies are opaque, and flagging every call pair would drown the real signal. A panic between the flagged writes leaves survivors a broken invariant (`ctrl.panic/LK3–LK4`); `.staged()` makes the update panic-atomic.

## Opt-In Warnings

Off by default. W6–W8 are about one piece of code (auditing the copies in a hot loop), so they're enabled with `@warn(warning_name)` on items or project-wide in `rask.toml`. W12 is about the build: whether stdout is a terminal or a pipe is a fact about how the program runs, the same for every function in it, so it's enabled with `--warn blocking_print` on any command that compiles and has no attribute form.

Only W12 is built. A name nothing answers to is an error rather than a silent no-op: `--warn` with an unknown name is refused with a did-you-mean, and `@warn(...)` is E0855 for every name until W6–W8 exist (for `blocking_print` the fix points at the flag).

| Rule | Code | ID | Check |
|------|------|----|-------|
| **W6: implicit_copy** | W0904 | `implicit_copy` | Implicit copy of types at the 16-byte threshold |
| **W7: shadowing** | W0905 | `shadowing` | Variable shadows an outer binding in the same function |
| **W8: type_narrowing** | W0906 | `type_narrowing` | Pattern match could use a more specific type |
| **W12: blocking_print** | W0910 | `blocking_print` | Printing counts as blocking for `comp.effects/CW2`, so a `println` in a loop of a program that spawns gets the warning `fs.read_text` would. Off by default: a terminal that stops reading is rare, and the warning on every print buried the ones that matter |

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
| **CF2: Package-level** | A `[warnings]` table in `rask.toml` sets project-wide defaults. Not in the `rask-package` schema yet, so writing one is an unknown-key error today |
| **CF3: CLI-level** | `--warn <name>` turns on a build-wide opt-in warning (W12). `--deny-warnings` promotes all warnings to errors (not built yet) |

<!-- test: skip -->
```rask
@allow(unused_variable)
func scratch() {
    let x = expensive_setup()
}

@deny(unused_variable)
extend Server {
    func start(take self) -> void or Error {
        try self.listener.bind(self.addr)
        return
    }
}
```

Package-level configuration in `rask.toml`:

```toml
[warnings]
deny = ["unused_variable"]
allow = ["shadowing"]
warn = ["implicit_copy"]
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
| **P3: Package overrides default** | `rask.toml` config overrides per-warning defaults |
| **P4: CLI promotes remaining** | `--deny-warnings` promotes anything not explicitly `@allow`'d |

## Warning Codes

| Rule | Description |
|------|-------------|
| **WC1: Dual ID** | Both code (`W0901`) and name (`unused_variable`) work in attributes |
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
| `@deny` on item + `@allow` on nested | P1 | Nested `@allow` wins |
| `--deny-warnings` + `@allow` on item | P1 | `@allow` wins |
| Code after diverging `match` | W4 | Flagged if all arms diverge |

---

## Appendix (non-normative)

### Rationale

**SB1-SB3 (three categories):** A hard line between errors, warnings, and lints. Errors are non-negotiable — running broken code is worse than not running code. Warnings flag code that probably isn't what you meant. Lints enforce how code should look.

**W6 (implicit_copy) off by default:** Implicit copy is a core ergonomic feature. Most code shouldn't care. But game loops and embedded code sometimes need to audit every copy. See `mem.value-semantics` for the 16-byte threshold design.

**W7 (shadowing) off by default:** Shadowing is a deliberate language feature. Some teams want to see it, especially in long functions where it causes confusion, but flagging it by default would be noisy.

**W9 (torn_lock_update) on by default:** Rask has no lock poisoning — a panic mid-update releases the lock and survivors see whatever was written (`ctrl.panic/LK1–LK4`). The by-construction fix (`staged()`) only helps if the sites that need it get pointed at it, so the pointer can't be opt-in tooling. Started as a lint candidate (`idiom/staged-multi-write`); promoted to a default compiler warning when [#485](https://github.com/rask-lang/rask/issues/485) made the point that the torn-invariant story shouldn't rest on tools you have to turn on.

### Patterns & Guidance

**Compiler warnings vs lint rules:**

| | Compiler Warnings | Lint Rules |
|-|-------------------|------------|
| **Tool** | `rask check` | `rask lint` |
| **What it checks** | Correctness hints | Convention enforcement |
| **ID format** | `unused_variable` / `W0901` | `naming/is` / `idiom/force-unwrap-production` |
| **Suppression** | `@allow(unused_variable)` | `@allow(naming/is)` |
| **When it runs** | Every build | Pre-commit, CI |

Both share the `@allow` attribute and diagnostic output format. Different ID namespaces prevent collision.

### Future

- **`unused_field`** — struct field never read outside the defining module
- **`unnecessary_clone`** — `.clone()` on a type already Copy (16 bytes or less)
- **`large_move`** — moving a type significantly above the 16-byte threshold

### See Also

- `tool.lint` — convention enforcement (`rask lint`)
- `mem.value-semantics` — copy threshold design
- `struct.build` — package-level warning configuration

# `rask lint` — Convention Enforcement

`rask lint` checks that code follows Rask's naming conventions and idiomatic patterns. It operates on the AST after parsing and type checking — this isn't formatting (that's `rask fmt`), it's semantic checking.

```
rask lint src/server.rk              # lint one file
rask lint src/                       # lint all .rk files recursively
rask lint src/ --format json         # JSON output for IDE integration
rask lint src/ --rule naming/*       # only naming convention rules
```

---

## Rules

### Naming Conventions

These enforce the naming table from [canonical-patterns.md](../canonical-patterns.md#required-naming-patterns-stdlib). The compiler already has the type information needed to check these — the linter reads method signatures and validates that names match their behavior.

| Rule ID | Check | Severity | Scope |
|---------|-------|----------|-------|
| `naming/from` | `from_*` returns `Self` or `Self or E` | warning | extend blocks |
| `naming/into` | `into_*` has `take self` (consuming) | warning | extend blocks |
| `naming/as` | `as_*` doesn't allocate (heuristic: return type is a reference or primitive) | warning | extend blocks |
| `naming/to` | `to_*` returns a different type than `Self` | warning | extend blocks |
| `naming/is` | `is_*` returns `bool` | error | extend blocks, standalone funcs |
| `naming/with` | `with_*` returns `Self` | warning | extend blocks |
| `naming/try` | `try_*` returns `T or E` | error | extend blocks, standalone funcs |
| `naming/or_suffix` | `*_or(default)` returns unwrapped `T` (not `T?` or `T or E`) | warning | extend blocks |

**Example violations:**

```rask
extend User {
    // naming/is: `is_valid` returns i32, expected bool
    func is_valid(self) -> i32 {
        return self.score
    }

    // naming/into: `into_string` doesn't take self
    func into_string(self) -> string {
        return self.name.to_string()
    }

    // naming/try: `try_parse` returns string, expected T or E
    func try_parse(s: string) -> string {
        return s
    }
}
```

**Output:**

```
warning[naming/into]: `into_string` should take ownership of self
  --> src/user.rk:8:5
   |
 8 |     func into_string(self) -> string {
   |          ^^^^^^^^^^^ `into_*` methods consume the value
   |
   = fix: change `self` to `take self`, or rename to `to_string`

error[naming/is]: `is_valid` must return `bool`
  --> src/user.rk:3:5
   |
 3 |     func is_valid(self) -> i32 {
   |          ^^^^^^^^ returns `i32`, expected `bool`
   |
   = fix: change return type to `bool`, or rename to remove the `is_` prefix
```

### Idiomatic Patterns

These check for common mistakes that the canonical patterns address. See [canonical-patterns.md](../canonical-patterns.md).

| Rule ID | Check | Severity |
|---------|-------|----------|
| `idiom/unwrap-production` | `unwrap()` call outside `test` blocks | warning |
| `idiom/missing-ensure` | `@resource` type created without matching `ensure` in same scope | warning |

### Style

| Rule ID | Check | Severity |
|---------|-------|----------|
| `style/snake-case-func` | Function names are `snake_case` | warning |
| `style/pascal-case-type` | Type/enum/trait names are `PascalCase` | warning |
| `style/public-return-type` | Public functions have explicit return type annotations | error |

---

## Suppression

Suppress individual rules with `@allow`:

```rask
@allow(naming/is)
func is_custom_check() -> i32 {
    return 42
}
```

Suppress for an entire extend block:

```rask
@allow(naming/into)
extend LegacyAdapter {
    func into_string(self) -> string {
        return self.data.to_string()
    }
}
```

---

## JSON Output

`rask lint --format json` produces structured output matching the diagnostic format:

```json
{
  "version": 1,
  "file": "src/user.rk",
  "success": true,
  "diagnostics": [
    {
      "rule": "naming/is",
      "severity": "error",
      "message": "`is_valid` must return `bool`, found `i32`",
      "location": {
        "line": 3,
        "column": 10,
        "source_line": "    func is_valid(self) -> i32 {"
      },
      "fix": "change return type to `bool`, or rename to remove the `is_` prefix"
    }
  ],
  "error_count": 1,
  "warning_count": 0
}
```

---

## Rule Selection

Run specific rules with `--rule`:

```
rask lint src/ --rule naming/*          # all naming rules
rask lint src/ --rule naming/is         # just is_* checks
rask lint src/ --rule idiom/*           # idiomatic patterns
rask lint src/ --rule style/*           # style checks
```

Exclude rules with `--exclude`:

```
rask lint src/ --exclude idiom/unwrap-production
```

---

## Relationship to Other Tools

| Tool | What it checks |
|------|---------------|
| `rask fmt` | Whitespace, indentation, line breaks — purely visual |
| `rask lint` | Naming conventions, idiomatic patterns, style — semantic |
| `rask check` | Type safety, ownership, borrowing — correctness; also emits [compiler warnings](warnings.md) for suspicious-but-valid code |

`rask lint` sits between formatting and type checking. It doesn't affect correctness — the code compiles fine — but it enforces conventions that make code consistent across projects. Both lint rules and compiler warnings use the `@allow` attribute for suppression, but they use different ID namespaces (lint: `naming/is`, warnings: `unused_result`).

---

## Future

- **Custom rules** — project-level lint configuration in `rask.build`.
- **Auto-fix** — `rask lint --fix` for rules with unambiguous fixes (e.g., rename `is_valid` return type).
- **CI integration** — exit code 1 on errors, 0 on warnings-only.

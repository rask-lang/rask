<!-- id: tool.lint -->
<!-- status: decided -->
<!-- summary: Convention enforcement for naming, idioms, and style -->
<!-- depends: tooling/warnings.md -->

# Lint

`rask lint` checks naming conventions, idiomatic patterns, and style rules. Operates on the AST after parsing and type checking — not formatting (that's `rask fmt`), but semantic checking.

## Naming Conventions

Enforce the naming table from [canonical-patterns.md](../canonical-patterns.md#required-naming-patterns-stdlib). The linter reads method signatures and validates names match behavior.

| Rule | Check | Severity | Scope |
|------|-------|----------|-------|
| **N1: from** | `from_*` returns `Self` or `Self or E` | warning | extend blocks |
| **N2: into** | `into_*` has `take self` (consuming) | warning | extend blocks |
| **N3: as** | `as_*` doesn't allocate (heuristic: returns reference or primitive) | warning | extend blocks |
| **N4: to** | `to_*` returns a different type than `Self` | warning | extend blocks |
| **N5: is** | `is_*` returns `bool` | error | extend blocks, standalone funcs |
| **N6: with** | `with_*` returns `Self` | warning | extend blocks |
| **N7: try** | `try_*` returns `T or E` | error | extend blocks, standalone funcs |
| **N8: or_suffix** | `*_or(default)` returns unwrapped `T` (not `T?` or `T or E`) | warning | extend blocks |

<!-- test: skip -->
```rask
extend User {
    // N5 violation: is_valid returns i32, expected bool
    func is_valid(self) -> i32 {
        return self.score
    }

    // N2 violation: into_string doesn't take self
    func into_string(self) -> string {
        return self.name.to_string()
    }

    // N7 violation: try_parse returns string, expected T or E
    func try_parse(s: string) -> string {
        return s
    }
}
```

## Idiomatic Patterns

Common mistakes the canonical patterns address.

| Rule | Check | Severity |
|------|-------|----------|
| **I1: unwrap-production** | `unwrap()` call outside `test` blocks | warning |
| **I2: missing-ensure** | `@resource` type created without matching `ensure` in same scope | warning |

## Style

| Rule | Check | Severity |
|------|-------|----------|
| **ST1: snake-case-func** | Function names are `snake_case` | warning |
| **ST2: pascal-case-type** | Type/enum/trait names are `PascalCase` | warning |
| **ST3: public-return-type** | Public functions have explicit return type annotations | error |

## Suppression

| Rule | Description |
|------|-------------|
| **SU1: Item suppress** | `@allow(rule_id)` on any item suppresses that rule for the item |
| **SU2: Block suppress** | `@allow(rule_id)` on an `extend` block suppresses for all methods inside |

<!-- test: skip -->
```rask
@allow(naming/is)
func is_custom_check() -> i32 {
    return 42
}

@allow(naming/into)
extend LegacyAdapter {
    func into_string(self) -> string {
        return self.data.to_string()
    }
}
```

## Rule Selection

| Rule | Description |
|------|-------------|
| **RS1: Filter** | `--rule <pattern>` runs only matching rules (e.g., `naming/*`, `naming/is`) |
| **RS2: Exclude** | `--exclude <rule_id>` skips specific rules |

## Error Messages

```
ERROR [tool.lint/N5]: `is_valid` must return `bool`
   |
3  |     func is_valid(self) -> i32 {
   |          ^^^^^^^^ returns `i32`, expected `bool`

FIX: change return type to `bool`, or rename to remove the `is_` prefix
```

```
WARNING [tool.lint/N2]: `into_string` should take ownership of self
   |
8  |     func into_string(self) -> string {
   |          ^^^^^^^^^^^ `into_*` methods consume the value

FIX: change `self` to `take self`, or rename to `to_string`
```

## JSON Output

`rask lint --format json` produces structured diagnostics:

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

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `is_*` on standalone func | N5 | Still checked (not just extend blocks) |
| `try_*` on standalone func | N7 | Still checked |
| `@allow` on item overrides block | SU1 | Item-level wins over block-level |
| Private `from_*` method | N1 | Still checked — conventions apply regardless of visibility |
| `into_*` with `mutate self` | N2 | Violation — must be `take self` |

---

## Appendix (non-normative)

### Rationale

**N2 (into consumes):** `into_*` implies conversion that destroys the original. If it borrows `self`, the caller might think the original is consumed when it isn't. Flagging this prevents subtle ownership confusion.

**N5/N7 as errors, not warnings:** `is_*` returning non-bool and `try_*` not returning a Result are strong enough contract violations that they should block — callers rely on these naming conventions for correctness assumptions.

**ST3 (public return type):** Public API signatures are documentation. Forcing explicit return types makes the API surface readable without hovering or inference.

### Patterns & Guidance

**Lint vs warnings:** Lint rules (`naming/is`, `idiom/unwrap-production`) enforce conventions. Compiler warnings (`unused_result`, `unreachable_code`) flag likely bugs. Both use `@allow` for suppression but different ID namespaces.

**CI usage:**
```
rask lint src/                        # all rules
rask lint src/ --rule naming/*        # naming only
rask lint src/ --rule naming/is       # single rule
rask lint src/ --exclude idiom/unwrap-production
```

### Future

- **Custom rules** — project-level lint configuration in `build.rk`
- **Auto-fix** — `rask lint --fix` for rules with unambiguous fixes
- **CI integration** — exit code 1 on errors, 0 on warnings-only

### See Also

- `tool.warnings` — compiler warnings (`rask check`)
- `tool.describe` — module API schema
- [canonical-patterns.md](../canonical-patterns.md) — naming convention source

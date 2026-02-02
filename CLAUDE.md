# Rask Design Objectives

Keep docs short. In chat, explain things to me—I'm not a language architect expert.

## Goal

Systems language where **safety is invisible**. Eliminate abstraction tax, cover 80%+ of real use cases.

**Non-negotiable:** Feel simple, not safe. Safety is a property, not an experience.

## Core Principles

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O). Small costs (bounds checks) can be implicit.
2. **Mechanical Safety** — Safety by structure. Use-after-free, data races, null derefs impossible by construction.
3. **Practical Coverage** — Handle web services, CLI, data processing, embedded. Not limited to fixed-size programs.
4. **Ergonomic Simplicity** — Low ceremony. If Rask needs 3+ lines where Go needs 1, question the design.

See [METRICS.md](METRICS.md) for scoring methodology.

---

## Design Status

Start with [CORE_DESIGN.md](CORE_DESIGN.md). For specs: [specs/README.md](specs/README.md).

### Decided

| Area | Decision | Spec |
|------|----------|------|
| Ownership | Single owner, move semantics, 16-byte copy threshold | [memory/](specs/memory/) |
| Borrowing | Block-scoped (values) vs expression-scoped (collections) | [borrowing.md](specs/memory/borrowing.md) |
| Collections | Vec, Map, Pool+Handle for graphs | [collections.md](specs/stdlib/collections.md), [pools.md](specs/memory/pools.md) |
| Linear types | Must-consume, `ensure` cleanup | [linear-types.md](specs/memory/linear-types.md) |
| Types | Primitives, structs, enums, generics, traits, unions | [types/](specs/types/) |
| Errors | Result, `?` propagation, `T?` optionals | [error-types.md](specs/types/error-types.md) |
| Concurrency | spawn/join/detach, channels, no function coloring | [concurrency/](specs/concurrency/) |
| Comptime | Compile-time execution | [comptime.md](specs/control/comptime.md) |
| C interop | Unsafe blocks, raw pointers | [unsafe.md](specs/memory/unsafe.md) |

### Open

| Area | Status |
|------|--------|
| Stdlib I/O | Not specified (io, fs, net, http) |
| Build system | Skeleton only |
| Macros/attributes | Not specified |

See [TODO.md](TODO.md) for full list.

---

## Validation

Test programs that must work naturally:
1. HTTP JSON API server
2. grep clone
3. Text editor with undo
4. Game loop with entities
5. Embedded sensor processor

**Litmus test:** If Rask is longer/noisier than Go for core loops, fix the design.

---

## Rask Syntax Quick Reference

**Claude: Use this, not Rust syntax.**

| Concept | Rask ✓ | Rust ✗ |
|---------|--------|--------|
| Immutable | `const x = 1` | `let x = 1` |
| Mutable | `let x = 1` | `let mut x = 1` |
| Function | `func foo()` | `fn foo()` |
| Methods | `extend Type { }` | `impl Type { }` |
| Visibility | `public` | `pub` |
| Enum variant | `Token.Plus` | `Token::Plus` |
| Read-only param | `func f(read x: T)` | N/A |
| Take ownership | `func f(take x: T)` | implicit move |
| Pass owned | `f(own value)` | implicit move |
| Inline block | `if x > 0: return x` | N/A |
| Pattern match | `if x is Some(v)` | `if let Some(v) = x` |
| Guard pattern | `let v = x is Ok else { return }` | `let Ok(v) = x else { return }` |
| Loop with value | `deliver value` | `break value` |
| Statement end | Newline | `;` |

**Common patterns:**
```rask
const x = 42                              // immutable
let y = 0; y = 1                          // mutable + reassign

func add(a: i32, b: i32) -> i32 { a + b }

extend Point {
    func distance(self, other: Point) -> f64 { ... }
}

if x > 0: return x                        // inline (colon)
if x > 0 { process(); return x }          // multi-line (braces)

if result is Ok(v): use(v)                // pattern match
let v = opt is Some else { return None }  // guard pattern

match status {
    Active => handle(),
    Failed(e) => log(e),
}
```

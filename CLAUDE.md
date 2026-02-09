# Rask Design Objectives

Keep docs short. In chat, explain things to me—I'm not a language architect expert.

**Tool usage:**
- Use `Write` tool for creating test files, not `Bash` with cat/heredocs
- Avoid pipes (`|`), redirects (`2>&1`), and command chaining (`&&`) in Bash commands - they break permission matching
- Run commands separately instead of chaining them
- Create test scripts in `/tmp`, not the main project folder

# Rask Writing Style Guide

**Core principle:** Sound like a developer with a vision, not a committee or AI. Natural flow over perfect grammar.

add // SPDX-License-Identifier: (MIT OR Apache-2.0) to the top of all files

## Documentation (Markdown)

**Use "I" for design choices:**
- ✅ "I chose handles over pointers—indirection cost is explicit"
- ❌ "It was decided that handles should be used"

**Keep technical sections neutral:**
- ✅ "References cannot outlive their lexical scope"
- ❌ "I make sure references cannot outlive scope"

**Be direct about tradeoffs:**
- ✅ "This means more `.clone()` calls. I think that's better than lifetime annotations"
- ❌ "While this may result in additional clones, it provides benefits..."

**Remove filler:** "It should be noted", "In order to", "With regard to"

**Natural language OK:** Contractions, slight grammar quirks, Scandinavian English flow

## Code Comments (Rust)

**Neutral and direct - no "I":**
- ✅ `// Skip to next declaration after error`
- ❌ `// I skip to the next declaration`

**Remove:**
- Obvious docs: `/// Get current token`
- Restating code: `// Check for X` when obvious
- Statement markers: `// While loop`
- AI explanations

**Keep:**
- Section headers
- Non-obvious algorithms
- Important constraints (tightened)
- "Why" not "what"

**Tighten everything:**
- ✅ `/// Record error, return if should continue`
- ❌ `/// Record an error and return a boolean indicating whether we should continue`

## Summary

**Docs:** "I" for design, neutral for tech specs, be direct, natural flow over grammar
**Code:** Neutral/direct, remove obvious, tighten verbose, no "I"
**Overall:** Sound like a developer with vision, own tradeoffs, no corporate speak


## Rask Syntax Quick Reference

**Claude: Use this, not Rust syntax.**

| Concept | Rask ✓ | Rust ✗ |
|---------|--------|--------|
| String type | `string` (lowercase) | `String` |
| Immutable | `const x = 1` | `let x = 1` |
| Mutable | `let x = 1` | `let mut x = 1` |
| Function | `func foo()` | `fn foo()` |
| Methods | `extend Type { }` | `impl Type { }` |
| Visibility | `public` | `pub` |
| Enum variant | `Token.Plus` | `Token::Plus` |
| Mutable param | `func f(mutate x: T)` | `&mut` / `inout` |
| Take ownership | `func f(take x: T)` | implicit move |
| Pass owned | `f(own value)` | implicit move |
| Return value | `return expr` (explicit) | `expr` (implicit) |
| Pattern match | `if x is Some(v)` | `if let Some(v) = x` |
| Guard pattern | `let v = x is Ok else { return }` | `let Ok(v) = x else { return }` |
| Result type | `T or E` (= `Result<T, E>`) | `Result<T, E>` |
| Error propagation | `try expr` | `expr?` |
| Loop with value | `deliver value` | `break value` |
| Statement end | Newline | `;` |

**Common patterns:**
```rask
const x = 42                              // immutable
let y = 0; y = 1                          // mutable + reassign
const s = string.new()                    // string is lowercase (primitive)

func add(a: i32, b: i32) -> i32 {
    return a + b                          // functions require explicit return
}

extend Point {
    func distance(self, other: Point) -> f64 { ... }
}

// Expression context — match/if produce values
const color = match status {
    Active => "green",
    Failed => "red",
}
const sign = if x > 0: "+" else: "-"

// Statement context — side effects
if x > 0 {
    process(x)                            // no value produced
}
match event {
    Click(pos) => handle(pos),
    Key(k) => process(k),
}

if result is Ok(v): use(v)                // pattern match
let v = opt is Some else { return None }  // guard pattern
```

**Return semantics:**
- **Functions** (including `comptime func`): require explicit `return`
- **Blocks** in expression context: last expression is the value (implicit)
- **Why different?** `return` exits functions, blocks naturally produce values
```rask
// Functions need explicit return
func factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    return n * factorial(n - 1)  // ✓ explicit
}

// Blocks use implicit last expression
const squares = comptime {
    const arr = Vec.new()
    for i in 0..10 { arr.push(i * i) }
    arr  // ✓ implicit (return would exit function!)
}
```


## Goal

Systems language where **safety is invisible**. Eliminate abstraction tax, cover 80%+ of real use cases.

**Non-negotiable:** Feel simple, not safe. Safety is a property, not an experience.

## Core Principles

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O). Small costs (bounds checks) can be implicit.
2. **Mechanical Safety** — Safety by structure. Use-after-free, data races, null derefs impossible by construction.
3. **Practical Coverage** — Handle web services, CLI, data processing, embedded. Not limited to fixed-size programs.
4. **Ergonomic Simplicity** — Low ceremony. If Rask needs 3+ lines where Go needs 1, question the design.

See [METRICS.md](specs/METRICS.md) for scoring methodology.

---

## Design Status

Start with [CORE_DESIGN.md](specs/CORE_DESIGN.md). For specs: [specs/README.md](specs/README.md).

### Decided

| Area | Decision | Spec |
|------|----------|------|
| Ownership | Single owner, move semantics, 16-byte copy threshold | [memory/](specs/memory/) |
| Borrowing | Block-scoped (values) vs expression-scoped (collections) | [borrowing.md](specs/memory/borrowing.md) |
| Collections | Vec, Map, Pool+Handle for graphs | [collections.md](specs/stdlib/collections.md), [pools.md](specs/memory/pools.md) |
| Resource types | Must-consume (linear resources), `ensure` cleanup | [resource-types.md](specs/memory/resource-types.md) |
| Types | Primitives, structs, enums, generics, traits, unions | [types/](specs/types/) |
| Errors | `T or E` result, `try` propagation, `T?` optionals | [error-types.md](specs/types/error-types.md) |
| Concurrency | spawn/join/detach, channels, no function coloring | [concurrency/](specs/concurrency/) |
| Comptime | Compile-time execution | [comptime.md](specs/control/comptime.md) |
| C interop | Unsafe blocks, raw pointers | [unsafe.md](specs/memory/unsafe.md) |
| Rust interop | compile_rust() in build scripts, C ABI, cbindgen | [build.md](specs/structure/build.md) |

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

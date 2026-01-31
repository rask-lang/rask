# Rask Design TODO

Gaps and incomplete areas identified in the design documents.

---

## High Priority (Core Design)

### Control Flow
- [ ] `if`/`else` semantics
- [ ] `while` loops
- [ ] `loop` (infinite loop with break)
- [ ] `break`/`continue` with labels
- [ ] `return` semantics
- [ ] Expression vs statement distinction

### Primitives and Numeric Types
- [ ] Full list of primitive types
- [ ] Floating point semantics (NaN, infinity, comparison)
- [ ] Casting rules between numeric types
- [ ] Numeric traits beyond `Numeric`
- [ ] Boolean operations

---

## Medium Priority (Ecosystem)

### Testing Framework
- [ ] Test file convention (`*_test.rask`)
- [ ] Test function syntax (`#[test]`)
- [ ] Assertion functions/macros
- [ ] Test runner behavior
- [ ] Mocking/dependency injection patterns

### Build System (`rask.build`)
- [ ] Build script syntax and capabilities
- [ ] Relationship to comptime
- [ ] Dependency on external tools
- [ ] Asset bundling
- [ ] Code generation hooks

### Standard Library Outline
- [ ] Core module contents
- [ ] I/O module (`io`, `fs`)
- [ ] Networking (`net`)
- [ ] Time and duration
- [ ] What's built-in vs imported?

### Error Types
- [ ] Built-in `Error` type definition
- [ ] Custom error definition patterns
- [ ] Error trait requirements
- [ ] Error conversion/wrapping

### Operators
- [ ] Full operator precedence table
- [ ] Bitwise operators (`&`, `|`, `^`, `~`, `<<`, `>>`)
- [ ] Assignment operators (`+=`, `-=`, etc.)
- [ ] Logical operators (`&&`, `||`, `!`)
- [ ] Comparison operators

---

## Lower Priority (Details)

### Attributes/Annotations
- [ ] Attribute syntax (`#[...]`)
- [ ] Built-in attributes list
- [ ] Custom attribute support (if any)
- [ ] Conditional compilation attributes

### Macros
- [ ] `format!` macro specification
- [ ] Macro system design (if planned)
- [ ] Procedural vs declarative macros

### Validation: Test Programs
Walk through the 7 litmus test programs from CLAUDE.md:
- [ ] HTTP JSON API server
- [ ] grep clone
- [ ] Text editor (dynamic buffer, undo)
- [ ] Log aggregation (streaming)
- [ ] Sensor processor (fixed memory, real-time)
- [ ] Game loop (dynamic entities)
- [ ] Database (indexes, caching)

---

## Known Issues from Specs

### Concurrency (from sync-concurrency.md)
- [ ] Linear types + channels silent failure (RAII wrapper silences close errors)
- [ ] Nursery nesting rules unclear
- [ ] Thread pool and resource limits unspecified
- [ ] Channel drop with items ("best-effort" undefined)

### Async (from async-runtime.md)
- [ ] Sync nursery blocks async runtime

### Comptime (from compile-time-execution.md)
- [ ] Should comptime have limited heap allocation (Vec/Map)?
- [ ] Comptime memoization strategy
- [ ] Step-through debugger for comptime?
- [ ] Which stdlib functions are comptime-compatible?

---

## Cross-References to Add

These specs mention features that should link to other specs:

- [x] ~~String spec → Iteration spec~~ (fixed)
- [x] ~~Memory model → Structs spec~~ (specs/structs.md created)
- [x] ~~Generics → Unsafe spec~~ (specs/unsafe.md created)
- [ ] Module system → Build system spec (when created)

# Rask Compiler Implementation Plan

Tree-walk interpreter to validate the language design. Target: run the grep clone test program.

## Phase 1: Lexer + Parser Foundation

**Goal:** Parse basic expressions and statements into an AST.

### Lexer (`rask-lexer`)
- [ ] Whitespace and newline handling (newlines are statement terminators)
- [ ] Line comments (`//`) and block comments (`/* */`)
- [ ] Doc comments (`///`)
- [ ] Integer literals (dec, hex `0x`, bin `0b`, oct `0o`, with `_` separators)
- [ ] Float literals (with optional `f32`/`f64` suffix)
- [ ] String literals (regular `"..."` and multi-line `"""..."""`)
- [ ] String interpolation `"{expr}"`
- [ ] Character literals
- [ ] All keywords: `fn`, `let`, `const`, `struct`, `enum`, `trait`, `impl`, `pub`, `import`, `return`, `if`, `else`, `match`, `for`, `in`, `while`, `loop`, `break`, `continue`, `deliver`, `spawn`, `select`, `with`, `ensure`, `take`, `where`, `as`, `true`, `false`
- [ ] All operators: `+`, `-`, `*`, `/`, `%`, `==`, `!=`, `<`, `>`, `<=`, `>=`, `&&`, `||`, `!`, `?`, `??`, `..`, `=>`, `->`, `@`, `.`
- [ ] All delimiters: `{`, `}`, `(`, `)`, `[`, `]`, `:`, `;`, `,`
- [ ] Error recovery and good error messages

### Parser (`rask-parser`)
- [ ] Expression parsing (Pratt parser for precedence)
- [ ] Operator precedence table
- [ ] Literal expressions
- [ ] Binary and unary expressions
- [ ] Function calls
- [ ] Field access and indexing
- [ ] Block expressions
- [ ] If expressions (inline `:` and braced `{}`)
- [ ] Match expressions
- [ ] Let/const bindings
- [ ] Assignments
- [ ] Loops (for, while, loop)
- [ ] Function declarations
- [ ] Struct declarations
- [ ] Enum declarations
- [ ] Error recovery and good error messages

**Validation:** Parse simple programs, pretty-print AST back to source.

---

## Phase 2: Basic Interpreter

**Goal:** Execute simple programs without ownership/pools.

### Interpreter (`rask-interp`)
- [ ] Value representation for all primitives
- [ ] Environment with scope stack
- [ ] Expression evaluation
- [ ] Binary/unary operations
- [ ] Variable lookup and assignment
- [ ] Function calls
- [ ] Control flow (if, match, loops)
- [ ] Return, break, continue, deliver

### Builtins
- [ ] `print()`, `println()`
- [ ] `assert()`
- [ ] Basic type conversions

**Validation:** Fibonacci, factorial, FizzBuzz.

---

## Phase 3: Type System

**Goal:** Add type checking before interpretation.

### Type Checker (`rask-types`)
- [ ] Type representation
- [ ] Type inference for locals
- [ ] Struct type definitions
- [ ] Enum type definitions
- [ ] Function signature checking
- [ ] Binary operator type rules
- [ ] Generic type instantiation (basic)
- [ ] Type error diagnostics

**Validation:** Type errors caught at compile time, not runtime.

---

## Phase 4: Ownership + Move Semantics

**Goal:** Enforce single ownership and move semantics.

### Ownership Analysis
- [ ] Track variable liveness (not moved)
- [ ] Use-after-move detection
- [ ] Copy vs move based on type (16-byte threshold, Copy trait)
- [ ] Move on function call with `take`

### Borrow Checking (simplified)
- [ ] Block-scoped borrows for plain values
- [ ] Expression-scoped borrows for collections
- [ ] Mutable XOR shared rule
- [ ] Borrow error diagnostics

**Validation:** Use-after-move and borrow violations detected.

---

## Phase 5: Pool/Handle System

**Goal:** The core innovation - handle-based indirection.

### Pool Implementation
- [ ] `Pool<T>` data structure
- [ ] Generation counters
- [ ] `Handle<T>` as (index, generation) pair
- [ ] Insert, remove, access operations
- [ ] Generation mismatch = runtime error
- [ ] Weak handles (optional)

### Ambient Pool Scoping
- [ ] `with pool { }` syntax
- [ ] Handle auto-dereference within scope
- [ ] Pool registry for handle resolution

**Validation:** Graph/tree structures, stale handle detection.

---

## Phase 6: Error Handling

**Goal:** Result types and `?` propagation.

### Result/Option Types
- [ ] `Result<T, E>` type
- [ ] `Option<T>` type
- [ ] Union error types: `Result<T, E1 | E2>`
- [ ] `?` operator for early return
- [ ] Error widening through `?`
- [ ] `??` for default values
- [ ] `!` for force unwrap

### Ensure Blocks
- [ ] `ensure { cleanup }` parsing
- [ ] Deferred execution on any exit
- [ ] Multiple ensure blocks (LIFO order)

**Validation:** File handling with proper cleanup, error propagation.

---

## Phase 7: Grep Clone

**Goal:** First real program - validates the design.

### Required Features
- [ ] Command-line argument parsing
- [ ] File I/O with linear types
- [ ] String handling
- [ ] Pattern matching (basic substring or regex)
- [ ] Line iteration
- [ ] Error propagation throughout
- [ ] Proper resource cleanup

### Validation
- [ ] Compare output to real `grep`
- [ ] Measure lines of code vs Go equivalent
- [ ] Document design friction encountered

---

## Deferred (Post-MVP)

- [ ] Closures with capture rules
- [ ] Concurrency (spawn/join/channels)
- [ ] Comptime execution
- [ ] C interop
- [ ] Traits and dynamic dispatch
- [ ] Generics beyond basic instantiation
- [ ] REPL

---

## Reference

- Syntax: `../specs/SYNTAX.md`
- Memory model: `../specs/memory/`
- Type system: `../specs/types/`
- Control flow: `../specs/control/`

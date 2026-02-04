# Rask Compiler Development Prompt

I'm building a compiler for Rask, a systems language with ownership semantics.

## Current State
- Lexer: Complete (rask-lexer, uses logos)
- Parser: Complete (Pratt parser, all examples parse)
- AST: Complete
- Name Resolution: Complete (rask-resolve)
- Operator Desugaring: Complete (rask-desugar) - `a + b` → `a.add(b)`
- Type System Core: Complete (rask-types) - inference, unification, type table
- Trait Checking: Complete (rask-types/traits.rs) - structural satisfaction
- Comptime Execution: Complete (rask-comptime) - compile-time evaluation
- Ownership Analysis: Complete (rask-ownership) - move tracking, borrow scopes, Copy inference
- Interpreter: Stub (returns Unit)

## Architecture
Three phases: Tree-Walk Interpreter → Bytecode VM → Native Codegen

Phase 1 pipeline:
```
Source → Lexer → Parser → AST → Desugar → Resolve → Typecheck → [Ownership] → Interpreter
                                   ↓
                              Comptime (available)
```

## Implementation Order
1. ~~Name Resolution~~ ✓ - resolve identifiers to declarations
2. ~~Type System Core~~ ✓ - unification, inference for let/const
3. ~~Operator Desugaring~~ ✓ - `a + b` → `a.add(b)`
4. ~~Comptime Execution~~ ✓ - compile-time evaluation
5. ~~Trait Checking~~ ✓ - structural satisfaction
6. ~~Ownership Analysis~~ ✓ - move tracking, borrow scopes
7. **Basic Interpreter** - evaluate expressions, statements, calls
8. MILESTONE: simple_test.rask runs
9. Collections & Pools
10. MILESTONE: grep_clone.rask runs

## Key Files
- compiler/crates/rask-lexer/ - tokenization
- compiler/crates/rask-parser/ - parsing
- compiler/crates/rask-ast/ - AST definitions
- compiler/crates/rask-resolve/ - name resolution
- compiler/crates/rask-desugar/ - operator desugaring
- compiler/crates/rask-types/ - type checking + trait checking
- compiler/crates/rask-comptime/ - compile-time interpreter
- compiler/crates/rask-ownership/ - ownership and borrow checking
- compiler/crates/rask-interp/ - runtime interpreter (stub)
- compiler/crates/rask-cli/ - CLI commands (lex, parse, resolve, typecheck, ownership)
- examples/simple_test.rask - first validation target
- specs/ - language specifications

## CLI Commands
```bash
rask lex <file>       # Show tokens
rask parse <file>     # Show AST
rask resolve <file>   # Show symbols and resolutions
rask typecheck <file> # Run full pipeline through type checking
rask ownership <file> # Run ownership and borrow analysis
```

## Key Design Decisions
- Local analysis only (no whole-program analysis)
- Two borrow scopes: Persistent (block end) vs Instant (semicolon)
- 16-byte copy threshold for implicit copy
- Structural trait satisfaction (not nominal)
- Operators desugar to method calls before type checking
- Comptime restricted: no I/O, no pools, no concurrency

DONT USE CAT OR SIMILAR COMMANDS IN git commits

## This Session
Step 7: Basic Interpreter - the final step before simple_test.rask runs.

Read compiler/ARCHITECTURE.md for full details.

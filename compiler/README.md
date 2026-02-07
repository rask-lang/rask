# Rask Compiler

14 crates. Runs interpreted. No native codegen yet.

## Pipeline

```
.rk → Lexer → Parser → Resolver → Desugar → Type Checker → Interpreter
```

## Crates

**Frontend:**
- `rask-lexer` - Tokenization
- `rask-parser` - AST construction
- `rask-ast` - Shared AST types

**Analysis:**
- `rask-resolve` - Symbol resolution
- `rask-desugar` - Sugar expansion
- `rask-types` - Type checking and inference
- `rask-ownership` - Borrow checking (planned)
- `rask-comptime` - Compile-time execution

**Backend:**
- `rask-interp` - Tree-walking interpreter
- `rask-stdlib` - Runtime for Vec/Map/String/etc

**Tools:**
- `rask-cli` - Main binary
- `rask-lsp` - IDE language server
- `rask-diagnostics` - Error formatting
- `rask-spec-test` - Test harness

## Build

```bash
cargo build --release
./target/release/rask ../examples/hello_world.rk
```

## What Works

Interpreter runs grep, game loop, text editor. Core language features work: ownership, generics, traits, channels, linear resources.

## What Doesn't

No native codegen (100-1000x slower). Limited LSP. No package manager. Minimal stdlib.

# Rask Compiler

14 crates. Runs interpreted. No native codegen yet.

## Pipeline

```
.rk → Lexer → Parser → Desugar → Resolver → Type Checker → Comptime → Ownership → Interpreter
```

## Build

```bash
cargo build --release
```

## Usage

```bash
# Run a program
rask run examples/hello_world.rk

# Run tests in a file
rask test examples/test_example.rk

# Filter tests by name
rask test examples/test_example.rk -f "add"

# Run benchmarks
rask benchmark examples/test_example.rk

# Type check without running
rask typecheck examples/hello_world.rk

# Evaluate comptime blocks
rask comptime examples/17_comptime.rk

# Format source files
rask fmt examples/hello_world.rk

# Explain an error code
rask explain E0308

# Dump tokens or AST (debugging)
rask lex examples/hello_world.rk
rask parse examples/hello_world.rk

# JSON diagnostic output
rask run examples/hello_world.rk --json
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
- `rask-fmt` - Code formatter

## What Works

Interpreter runs grep, game loop, text editor, HTTP server. Core language features: ownership, generics, traits, channels, linear resources, concurrency.

## What Doesn't

No native codegen (100-1000x slower). Limited LSP. No package manager. Minimal stdlib.

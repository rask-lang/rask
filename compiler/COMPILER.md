# How the Rask Compiler Works

A walkthrough of the compiler code. Assumes no Rust or compiler background.

For deep dives, see the files in [`docs/`](docs/).

## What a compiler does

A compiler reads source code (text) and transforms it into something a computer
can execute. It does this in stages, each one understanding the code at a deeper
level:

```
Source text  →  Tokens  →  Tree structure  →  Meaning  →  Execution
  "1 + 2"    [1] [+] [2]   Add(1, 2)        "integer"    3
```

## The crate map

Each stage lives in its own crate (Rust's word for library) under
`compiler/crates/`:

```
compiler/crates/
├── rask-ast/            Data structures everything shares (tokens, AST nodes)
├── rask-lexer/          Stage 1: Text → tokens
├── rask-parser/         Stage 2: Tokens → tree (AST)
├── rask-desugar/        Stage 3: Simplify tree (operators → method calls)
├── rask-resolve/        Stage 4: Connect names to definitions
├── rask-types/          Stage 5: Figure out every expression's type
├── rask-ownership/      Stage 6: Verify memory safety
├── rask-hidden-params/  Stage 7: Desugar `using` into regular params
├── rask-mono/           Stage 8: Eliminate generics
├── rask-mir/            Stage 9: Flatten into basic blocks (MIR)
├── rask-codegen/        Stage 10: Generate machine code (Cranelift)
├── rask-interp/         Alternative to 8-10: interpret the AST directly
├── rask-cli/            The `rask` command—dispatches to everything
├── rask-diagnostics/    Error formatting (shared by all stages)
├── rask-stdlib/         Built-in type methods (print, Vec, etc.)
├── rask-comptime/       Compile-time evaluation
├── rask-fmt/            Code formatter
├── rask-lint/           Linter
├── rask-describe/       Public API display
├── rask-lsp/            Editor integration (Language Server Protocol)
├── rask-rt/             Runtime library (green threads, channels)
├── rask-spec-test/      Spec validation
└── rask-wasm/           WebAssembly (experimental)
```

## The two execution paths

When you run `rask run hello.rk`, the CLI runs each stage in sequence. You can
read this directly in `rask-cli/src/commands/run.rs`:

```rust
// 1. Lex
let lex_result = rask_lexer::Lexer::new(&source).tokenize();
// 2. Parse
let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
// 3. Desugar operators to method calls
rask_desugar::desugar(&mut parse_result.decls);
// 4. Resolve names to symbols
let resolved = rask_resolve::resolve(&parse_result.decls)?;
// 5. Type check
let typed = rask_types::typecheck(resolved, &parse_result.decls)?;
// 6. Verify memory safety
rask_ownership::check_ownership(&typed, &parse_result.decls);
// 7. Execute
rask_interp::Interpreter::with_args(args).run(&parse_result.decls)?;
```

After stage 6, the pipeline forks:

```
                    Source Code
                        │
                   ┌────▼────┐
                   │  Lexer  │
                   └────┬────┘
                   ┌────▼────┐
                   │ Parser  │
                   └────┬────┘
                   ┌────▼─────┐
                   │ Desugar  │
                   └────┬─────┘
                   ┌────▼─────┐
                   │ Resolver │
                   └────┬─────┘
                   ┌────▼──────┐
                   │ Typecheck │
                   └────┬──────┘
                   ┌────▼───────┐
                   │ Ownership  │
                   └────┬───────┘
                        │
           ┌────────────┴────────────┐
           │                         │
      ┌────▼──────┐          ┌───────▼────────┐
      │ Interpret │          │ Hidden Params  │
      │ (rask run)│          └───────┬────────┘
      └───────────┘          ┌───────▼────────┐
                             │ Monomorphize   │
                             └───────┬────────┘
                             ┌───────▼────────┐
                             │ MIR Lowering   │
                             └───────┬────────┘
                             ┌───────▼────────┐
                             │ Codegen + Link │
                             │(rask compile)  │
                             └────────────────┘
```

- **Interpretation** (`rask run`): Stages 1–6, then the tree-walk interpreter
  executes the AST directly.
- **Interpretation via native** (`rask run --native`): Full compilation to a
  temp binary, execute it, delete it.
- **Compilation** (`rask compile`): Full pipeline to native binary via
  Cranelift codegen + C runtime linking.
- **Build** (`rask build`): Multi-package compilation with `build.rk` scripts,
  dependency resolution, and profile support.

Each stage also has a CLI command for inspection: `rask lex`, `rask parse`,
`rask resolve`, `rask check`, `rask ownership`, `rask mono`, `rask mir`.

## Deep dives

| Document | What it covers |
|----------|----------------|
| [Pipeline Stages](docs/01-pipeline.md) | Each compilation stage: what it does, its input/output, key code |
| [Type System](docs/02-types.md) | Type inference, unification, constraints, how generics work |
| [Interpreter](docs/03-interpreter.md) | How the tree-walk interpreter executes programs |
| [Code Generation](docs/04-codegen.md) | MIR, Cranelift, linking, the C runtime |
| [Maintainer Guide](docs/05-maintainer.md) | Where to find things, how to add features, Rust cheat sheet |

## Shared concepts

Three data types thread through the entire compiler:

**NodeId** — Every AST node gets a unique integer ID. Later stages attach
information to nodes via `HashMap<NodeId, Something>` instead of modifying
the tree. The parser assigns IDs sequentially; the desugarer starts at
1,000,000 to avoid collisions.

**Span** — Every token and AST node records its byte offset range
(`start..end`) in the source. This is how error messages point to the right
character.

**Diagnostic** — All stages convert their errors into a unified `Diagnostic`
type with severity, message, source labels, and help text. The CLI formats
these for human or JSON output.

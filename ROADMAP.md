# Rask Roadmap

## What's been done

1. **Research & design** — figured out what the language should be, wrote all the specs
2. **Lexer** — turns source into tokens
3. **Parser** — tokens → AST
4. **Name resolver** — symbols, scopes
5. **Type checker** — inference, generics, trait bounds
6. **Ownership checker** — moves, borrows, linear resources
7. **Interpreter** — runs real programs (I/O, threads, channels, Map, Vec, Pool)
8. **Validation programs** — grep, text editor, game loop, HTTP server, sensor processor all run
9. **Tooling** — `rask test`, `rask fmt`, `rask lint`, `rask describe`, `rask explain`, LSP completions
10. **Stdlib specs** — collections, I/O, fs, net, json, time, cli, math, random, os

## What's left

- Check design feel, do the design challenges

### Code generation
Turn AST into actual binaries. Need:
- MIR lowering (AST → MIR)
- Cranelift backend (MIR → machine code)
- Runtime library (allocator, panic, collections, I/O, threads)
- Monomorphization (generic instantiation)

### Build system
Right now just compile files directly. Eventually want:
- Multi-file projects
- Dependencies
- Package manager

### Polish
- Name mangling scheme
- Memory layout docs (enum tags, closures, vtables)
- More tests
- Performance tuning

That's it. Most design work is done, just need to generate code.

# Rask Roadmap

## What's done

1. **Language design** — all specs written (70+ files), core semantics stable
2. **Frontend** — lexer, parser, resolver, type checker, ownership checker
3. **Interpreter** — runs real programs (I/O, threads, channels, closures, Map, Vec, Pool)
4. **Validation programs** — grep, text editor, game loop, HTTP server, sensor processor
5. **Tooling** — `rask test`, `rask fmt`, `rask lint`, `rask describe`, `rask explain`, LSP
6. **Stdlib specs** — collections, I/O, fs, net, json, time, cli, math, random, os
7. **Monomorphization** — reachability analysis, generic instantiation, layout computation
8. **MIR lowering** — full AST → MIR for all constructs (control flow, closures, error handling)
9. **Cranelift backend** — MIR → native code, links with C runtime, produces executables
10. **C runtime** — print, I/O, Vec, String, Map, Pool, resource tracking, args

## What compiles natively today

Hello world, string ops, structs, field access, for/while loops, closures (including mixed-type captures), Vec/Map/Pool operations, enum construction, multi-function programs, arithmetic, control flow.

## What's left

### Native codegen gaps

- **Stdlib module calls** — `cli.parse()`, `fs.read()`, `io.stdin()` work in interpreter but codegen doesn't resolve module-qualified names yet. C runtime has the backing functions.
- **Closure escape** — closures are stack-allocated; escaping closures (returned, passed to spawn) will dangle. Needs heap allocation or escape analysis.
- **Concurrency runtime** — spawn, join, channels, Shared<T> don't exist as native code. The interpreter implements them, the C runtime doesn't.

### Build system

Multi-file projects, dependency resolution, output directories. `build.rk` manifest format exists but isn't wired end-to-end.

### Deferred (post-v1.0)

- LLVM backend (Cranelift sufficient for now)
- Incremental compilation
- Package registry
- Macros, inline assembly, reflection

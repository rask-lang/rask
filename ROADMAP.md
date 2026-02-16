# Rask Roadmap

## What's done

1. **Language design** — all specs written (70+ files), core semantics stable
2. **Frontend** — lexer, parser, resolver, type checker, ownership checker
3. **Interpreter** — runs real programs (I/O, threads, channels, closures, Map, Vec, Pool)
4. **Validation programs** — grep, text editor, game loop, HTTP server, sensor processor (interpreter)
5. **Tooling** — `rask test`, `rask fmt`, `rask lint`, `rask describe`, `rask explain`, LSP
6. **Stdlib specs** — collections, I/O, fs, net, json, time, cli, math, random, os
7. **Monomorphization** — reachability analysis, generic instantiation, layout computation
8. **MIR lowering** — full AST → MIR for all constructs (control flow, closures, error handling)
9. **Cranelift backend** — MIR → native code, links with C runtime, produces executables
10. **C runtime** — print, I/O, Vec, String, Map, Pool, resource tracking, args
11. **Stdlib codegen** — cli, fs, io, std, string module calls dispatch to C runtime
12. **Closure escape analysis** — heap allocation for escaping closures, stack for non-escaping
13. **Concurrency runtime** — OS threads, buffered+unbuffered channels, Mutex, Shared, panic handler, allocator
14. **String interpolation** — compile-time desugaring to concat/to_string
15. **Enum codegen** — pattern matching with variant tag resolution, `.variants()` method

## What compiles natively today

Hello world, string ops, structs (field access + methods), for/while/for-in loops, closures (escape analysis, mixed-type captures), Vec/Map/Pool operations, enum construction + pattern matching, string interpolation, multi-function programs, arithmetic, control flow.

## What's next

1. **Try compiling validation programs natively** — grep, editor, game loop should be close. Attempt each, fix what breaks.
2. **Concurrency codegen wiring** — C runtime has threads/channels/mutex, but `spawn()` MIR lowering is a dummy. Need real MIR statements and dispatch table entries. Blocks HTTP server.
3. **SIMD** — no vector operations codegen'd. Blocks sensor processor.

### Build system

Multi-file projects, dependency resolution, output directories. `build.rk` manifest format exists but isn't wired end-to-end.

### Deferred (post-v1.0)

- LLVM backend (Cranelift sufficient for now)
- Incremental compilation
- Package registry
- Macros, inline assembly, reflection

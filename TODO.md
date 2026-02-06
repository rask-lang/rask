# Rask — Status & Roadmap

## What's Done

### Language Design (Specs)
All core language semantics are specified:
- **Types:** primitives, structs, enums, generics, traits, unions, optionals, error types, SIMD
- **Memory:** ownership, borrowing, value semantics, closures, pools/handles, resource types, atomics, unsafe
- **Control:** if/else, loops, match, ensure, comptime, explicit returns
- **Concurrency:** spawn/join/detach, channels, select, threading, multitasking, no function coloring
- **Structure:** modules, packages, targets, C interop, Rust interop (via C ABI + build system)
- **Stdlib specs:** collections, strings, iteration, bits, testing

### Compiler (13 crates)
- **Lexer** — tokenizes Rask source
- **Parser** — full AST for current syntax (const/let, try, func, match, enums, structs, etc.)
- **Name resolution** — scope tree, symbol table
- **Type checker** — type inference, missing return detection (works on simple programs, gaps on complex ones)
- **Ownership checker** — move tracking, borrow scopes (works on simple programs)
- **Interpreter** — runs real programs: I/O, threading, channels, linear resources, string methods, Vec operations
- **LSP** — skeleton exists

### Example Programs That Run
`hello_world`, `simple_grep`, `cli_calculator` (stdin), `file_copy`, `game_loop` + all test_*.rask files (channels, threading, linear resources, ensure, match, etc.)

---

## Roadmap

### Phase 1: Consolidate (Next)
Close the gap between "demos work" and "the tool is reliable."

- [x] Fix compiler warnings (dead code, unused imports) — new `rask-diagnostics` crate, unified error display
- [ ] Fix type checker gaps so more examples typecheck end-to-end (currently fails on `own`, complex enums)
  - [x] Scope stack + local variable tracking
  - [x] Pattern checking in match/if-let
  - [x] Missing return detection (basic)
  - [ ] Missing return: handle match/if where all branches return
  - [ ] `Owned<T>` coercion support
  - [ ] `.clone()` method recognition
  - [ ] Register stdlib types in name resolver
  - [ ] Fix `<type#N>` display in error messages
- [x] Add `fmt` / string interpolation to interpreter — `format()`, `{name}` interpolation, format specifiers
- [x] Spec `io` — Reader/Writer traits — see [io.md](specs/stdlib/io.md)
- [x] Spec `fs` — File operations — see [fs.md](specs/stdlib/fs.md)
- [x] Spec `fmt` — String formatting — see [fmt.md](specs/stdlib/fmt.md)

### Phase 2: Stdlib Core
Spec and implement the modules needed for the litmus test programs.

- [x] `path` — Path manipulation — see [path.md](specs/stdlib/path.md)
- [x] `json` — JSON parsing — see [json.md](specs/stdlib/json.md)
- [x] `cli` — Argument parsing — see [cli.md](specs/stdlib/cli.md)
- [x] `time` — Duration, Instant, timestamps — see [time.md](specs/stdlib/time.md)
- [x] `math` — Mathematical functions — see [math.md](specs/stdlib/math.md)
- [x] `random` — Random number generation — see [random.md](specs/stdlib/random.md)
- [x] `os` — Environment variables, process exit — see [os.md](specs/stdlib/os.md)

### Phase 3: Litmus Test Validation
Run through the 5 validation programs for real. Each one will surface design gaps.

- [ ] grep clone (needs: cli, fs, path, string matching)
- [ ] HTTP JSON API server (needs: net, http, json, fmt)
- [ ] Text editor with undo (needs: terminal, collections patterns)
- [ ] Game loop with entities (needs: time, pool patterns — partially done)
- [ ] Sensor processor (needs: fixed-size patterns, embedded constraints)

### Phase 4: Code Generation
Move from interpreter to compiled output.

- [ ] Choose backend (LLVM vs Cranelift)
- [ ] IR design — lower AST to backend IR
- [ ] Monomorphization — generate concrete instances of generics
- [ ] Basic code generation — primitives, functions, structs, control flow
- [ ] Runtime — allocator, panic handler, thread startup
- [ ] Self-hosting bootstrap path

### Phase 5: Ecosystem
- [ ] Build system (`rask.build`) — syntax, relationship to comptime
- [ ] Package manager — dependency resolution, registry
- [ ] LSP completion — type-aware completions, go-to-definition
- [ ] Test runner — `rask test` command
- [ ] Formatter — `rask fmt`

---

## Open Design Questions

### Small (Can decide anytime)
- [ ] `char` — needed as a type, or just `u32` + validation?
- [ ] `discard` keyword for wildcards on non-Copy types
- [ ] Panic vs Error guidelines (when to panic, when to return error)
- [ ] `Owned<T>` semantics — needs full spec (only mentioned in passing)

### Medium (Should decide before Phase 3)
- [ ] Parameter modes consolidation — `read`, `mutate`, `take` need one spec
- [ ] Shared state primitives — `Shared<T>` / `ReadWrite<T>` for read-heavy patterns
- [ ] Multi-element access sugar — `with pool[h] as entity { }` vs closure pattern
- [ ] Task-local storage syntax

### Deferred
- [ ] Macros / `format!` — after core language is solid
- [ ] Inline assembly (`asm!`)
- [ ] Pointer provenance rules
- [ ] Comptime memoization
- [ ] Comptime debugger
- [ ] Fuzzing / property-based testing
- [ ] Code coverage tooling
- [ ] Metrics validation (user studies, UCC quantification, SN calibration)

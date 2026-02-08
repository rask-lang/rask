# Rask — Status & Roadmap

## What's Done

### Language Design (Specs)
I've specified all core language semantics:
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
`hello_world`, `simple_grep`, `cli_calculator` (stdin), `file_copy`, `game_loop` + all test_*.rk files (channels, threading, linear resources, ensure, match, etc.)

---

## Roadmap

### Phase 1: Consolidate (Current Focus)
Close the gap between "demos work" and "actually reliable."

- [x] Fix compiler warnings (dead code, unused imports) — new `rask-diagnostics` crate, unified error display
- [ ] Fix type checker gaps—right now it fails on `own` keyword and some complex enum patterns
  - [x] Scope stack + local variable tracking
  - [x] Pattern checking in match/if-let
  - [x] Missing return detection (basic)
  - [x] Missing return: handle match/if where all branches return
  - [x] `Owned<T>` coercion support
  - [x] `.clone()` method recognition
  - [x] Register stdlib types in name resolver
  - [x] Fix `<type#N>` display in error messages
  - [x] Generic functions with trait bounds (`func foo<T: Trait>()`)
  - [x] Return analysis through if/match/unsafe branches
  - [x] Integer literal constrained by type annotation (fresh type var inference)
  - [x] fs module methods recognized (`read_lines`, `canonicalize`, `copy`, etc.)
  - [x] File `write_line` method recognized
  - [x] Closure aliasing: skip closure params (not captured vars)
  - [x] Auto-Ok wrapping for `T or E` return types
  - [ ] Generic struct field resolution — blocks `09_generics` (5 errors)
  - [ ] `Owned<T>` coercion in recursive enum fields — blocks `cli_calculator` (7 errors)
- [ ] Fix parser gaps
  - [x] Closure types in type positions: `f: |i32| -> i32`
  - [x] Struct-style enum variants: `Move { x: i32, y: i32 }`
  - [x] Struct variant patterns: `Enum.Variant { field }` in match
  - [x] Struct variant construction: `Enum.Variant { field: val }` in expressions
  - [x] `read` parameter mode
  - [x] `read` keyword as method name (`db.read()`)
  - [x] Newline after `=>` in match arms
  - [ ] Const generics: `<const N: usize>` — blocks `sensor_processor`
- [ ] Fix ownership checker gaps
  - [x] False borrow errors in chained closure params (`.filter(|n| ...).map(|n| ...)`)
- [ ] Fix resolver gaps
  - [x] Generic type constructors `Type<T>.method()` → base name fallback
  - [x] Generic function/struct/enum declarations → strip `<...>` from name
  - [x] Qualified struct variant literals `Enum.Variant { ... }`
  - [x] `null` builtin constant
  - [x] `HttpResponse`/`HttpRequest`/`TcpListener`/`TcpConnection` net types
- [x] Add `fmt` / string interpolation to interpreter — `format()`, `{name}` interpolation, format specifiers
- [x] Spec `io` — Reader/Writer traits — see [io.md](specs/stdlib/io.md)
- [x] Spec `fs` — File operations — see [fs.md](specs/stdlib/fs.md)
- [x] Spec `fmt` — String formatting — see [fmt.md](specs/stdlib/fmt.md)

### Phase 2: Stdlib Core
Spec and implement the modules needed for validation programs.

- [x] `path` — Path manipulation — see [path.md](specs/stdlib/path.md)
- [x] `json` — JSON parsing — see [json.md](specs/stdlib/json.md)
- [x] `cli` — Argument parsing — see [cli.md](specs/stdlib/cli.md)
- [x] `time` — Duration, Instant, timestamps — see [time.md](specs/stdlib/time.md)
- [x] `math` — Mathematical functions — see [math.md](specs/stdlib/math.md)
- [x] `random` — Random number generation — see [random.md](specs/stdlib/random.md)
- [x] `os` — Environment variables, process exit — see [os.md](specs/stdlib/os.md)

### Phase 3: Validation Programs (IN PROGRESS - 2026-02-07)
Run the 5 validation programs for real. Each one surfaces design gaps that need fixing.

**Interpreter Enhancements Completed:**
- String/Enum `.eq()` for `==` operator, Vec `.eq()`/`.ne()`
- Tuple expression evaluation (`match (a, b)`)
- Vec methods: `insert(idx)`, `remove(idx)`, `collect()`, `to_vec()`, `chunks(size)`
- Deep struct clone (recursively clones Vec/Pool fields)
- Expression interpolation in strings (`{x + 1}`, `{obj.field / 100}`)
- `file.write_line()` method
- LetTuple/ConstTuple destructuring
- Deep nested field assignment (`entities[h].position.x = value`)
- Projection parameter support (`func f(entities: GameState.{entities})`)
- **Map<K,V>** — full implementation with 11 methods (insert, get, remove, contains, keys, values, len, is_empty, clear, iter, clone)
- **Vec iterator adapters** — 18 methods: filter, map, flat_map, fold, reduce, enumerate, zip, limit (renamed from `take`), flatten, sort, sort_by, any, all, find, position, dedup, sum, min, max
- **Clone method** — universal `.clone()` support for all types (Vec, Map, Pool, String, Struct, Enum, primitives)
- **String push_str** — concatenate strings
- **value_cmp()** — comparison helper for sorting (Int, Float, String, Bool, Char)

**Status:**
- [x] **grep clone** — ✅ **FULLY WORKING** (tested: pattern matching, -i, -n flags, file I/O)
  - Fixed: missing `return` statements, CLI `--` delimiter handling, type annotations
- [x] **Text editor** — ✅ **FULLY WORKING** (tested: insert, delete, undo, save)
  - Fixed: missing `return` statements, Vec.pop() returns Option, enum variant construction
  - Minor: undo message displays incorrectly but functionality works
- [x] **Game loop** — ✅ **FULLY WORKING** (tested: entities, collision, spawning, scoring)
  - Fixed: Pool iteration, projection parameters, Rust syntax (.collect, .map closure), tuple enum patterns
  - Slow: ~60ms/frame in interpreter, but functionally correct
- [x] **HTTP JSON API server** — `net` module ✅, `json.decode<T>` ✅, `Shared<T>` ✅, `with multitasking` ✅ (aliased to threading), `Map.from` ✅, string slicing ✅
- [ ] **Sensor processor** — **BLOCKED** on: const generics `<const N: usize>`, fixed arrays `[T; N]`, SIMD `f32x8`, `u128` type, `@no_alloc` enforcement

**Additional Interpreter Enhancements (2026-02-07):**
- Pool direct iteration (`for h in pool` = `for h in pool.cursor()`)
- Vec.pop() returns Option (was returning raw value)
- Implicit Ok() wrapping for `return ()` in `() or E` functions

**Design Gaps Found:**
- String interpolation doesn't support complex expressions (`{vec[i].field}`, `{x.method()}`)
- Tuple patterns can't use qualified enum paths (`(Enum.Variant, ...)` - parser limitation)
- Closures not implemented (`.map(|x| ...)` syntax)
- CLI module passes `--` delimiter as an argument (needs spec)
- Examples had Rust syntax remnants (`.collect<Vec<_>>()`, `.map(|x| ...)`, implicit returns)
- ~~Vec.`take(n)` method name conflicts with `take` keyword~~ — **FIXED:** renamed to `limit(n)`

### Phase 4: Code Generation
Move from interpreter to actual compiled output.

- [x] Choose backend (LLVM vs Cranelift) - I use Cranelift for now
- [ ] IR design — lower AST to backend IR
- [ ] Monomorphization — generate concrete instances of generics
- [ ] Basic code generation — primitives, functions, structs, control flow
- [ ] Runtime — allocator, panic handler, thread startup
- [ ] Self-hosting bootstrap path

### Phase 5: Ecosystem
Tools that make it actually usable:
- [ ] Build system (`rask.build`) — syntax, relationship to comptime
- [ ] Package manager — dependency resolution, registry
- [x] LSP completion — type-aware completions, go-to-definition
- [x] Test runner — `rask test` command
- [x] Formatter — `rask fmt`
- [ ] `rask describe` — structured module summaries (JSON) for tool consumption
- [ ] `rask explain` — compiler-generated function explanations from analysis
- [ ] Naming convention linter — enforce `is_*`, `into_*`, `as_*`, `to_*`, `from_*`, `with_*` patterns
- [ ] Structured error fixes — machine-parseable `fix:` blocks in diagnostics

---

## Open Design Questions

### Small (Can decide later)
- [ ] Decide: `char` as a type, or just `u32` + validation?
- [ ] Decide: `discard` keyword for wildcards on non-Copy types
- [ ] Write guidelines: when to panic vs return error
- [x] Spec `Owned<T>` semantics — see [owned.md](specs/memory/owned.md)

### Medium (Should decide before Phase 3)
- [x] Consolidate parameter modes — see [parameters.md](specs/memory/parameters.md) (borrow/read/take)
  - [ ] Type checker: enforce `read` parameter mode (`ParamMode::Read`)
- [x] Design shared state primitives — `Shared<T>`, see [sync.md](specs/concurrency/sync.md)
- [x] Decide multi-element access syntax — `with...as` binding + closure pattern
- [ ] Design task-local storage syntax

### Machine Readability (see [specs/machine-readability.md](specs/machine-readability.md))
- [ ] Formalize "one obvious way" principle — document canonical patterns for each operation
- [ ] Naming convention enforcement — linter rule for stdlib method naming
- [ ] Structured error fixes — `fix:` / `why:` sections in all error messages
- [ ] `rask describe` JSON schema — define output format for tool consumption

### Deferred
- [ ] Capability-based security for dependencies (restrict filesystem/network access)
- [ ] Macros / `format!` — wait until core language is solid
- [ ] Inline assembly (`asm!`)
- [ ] Pointer provenance rules
- [ ] Comptime memoization
- [ ] Comptime debugger
- [ ] Fuzzing / property-based testing
- [ ] Code coverage tooling
- [ ] Metrics validation (actual user studies)

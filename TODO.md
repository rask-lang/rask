# Rask — Status & Roadmap

## What's Done

### Language Design (Specs)
I've specified all core language semantics:
- **Types:** primitives, structs, enums, generics, traits, unions, optionals, error types, SIMD
- **Memory:** ownership, borrowing, value semantics, closures, pools/handles, resource types, atomics, unsafe
- **Control:** if/else, loops, match, ensure, comptime, explicit returns
- **Concurrency:** spawn/join/detach, channels, select, ThreadPool, Multitasking, no function coloring, async runtime implementation spec
- **Structure:** modules, packages, targets, C interop, Rust interop (via C ABI + build system)
- **Stdlib specs:** collections, strings, iteration, bits, testing

### Compiler (13 crates)
- **Lexer** — tokenizes Rask source
- **Parser** — full AST for current syntax (const/let, try, func, match, enums, structs, etc.)
- **Name resolution** — scope tree, symbol table
- **Type checker** — type inference, missing return detection, generic struct fields, `@no_alloc` enforcement
- **Ownership checker** — move tracking, borrow scopes (works on simple programs)
- **Interpreter** — runs real programs: I/O, threading, channels, linear resources, string methods, Vec operations
- **LSP** — skeleton exists

### Example Programs That Run
`hello_world`, `simple_grep`, `cli_calculator` (stdin), `file_copy`, `game_loop` + all test_*.rk files (channels, threading, linear resources, ensure, match, etc.)

---

## Current State (2026-02-12)

**Language design:** ✅ Complete and stable. All core semantics decided, 70+ spec files covering types, memory, control, concurrency, stdlib.

**Frontend (Phases 1-4):** ✅ Complete. Lexer, parser, resolver, type checker, ownership checker all work. All validation programs pass checks.

**Interpreter:** ✅ Fully functional. 15+ stdlib modules, 4/5 validation programs run (grep, editor, game loop, HTTP server; sensor typechecks).

**What's blocking compiler implementation (Phase 5):**
1. **Name mangling scheme** — Must design symbol naming rules before emitting object files
2. **Memory layout documentation** — Should specify enum/closure/vtable layouts for consistency
3. **Test infrastructure** — Need systematic validation strategy (unit tests, integration tests, end-to-end)

**What's NOT blocking (despite TODO listings):**
- MIR structure: ✅ Specified (codegen.md)
- Monomorphization: ✅ Algorithm defined (M1-M5 rules)
- Runtime library API: ✅ Designed (RT1-RT3)
- Stdlib implementations: ✅ Exist in interpreter (3,000+ LOC Rust)
- Build system: Can start simple (single-file compilation)
- Self-hosting: Not needed for v1.0

**Critical path forward:** Design mangling + layouts → Implement MIR lowering → Build Cranelift backend → Create rask-rt runtime → Compile hello world → Expand to validation programs.

---

## Roadmap

### Phase 1: Consolidate (COMPLETE)
Closed the gap between "demos work" and "actually reliable."

- [x] Fix compiler warnings (dead code, unused imports) — new `rask-diagnostics` crate, unified error display
- [x] Fix type checker gaps—right now it fails on `own` keyword and some complex enum patterns
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
  - [x] Generic struct field resolution — `09_generics` passes type checker
  - [x] `Owned<T>` coercion in recursive enum fields — `cli_calculator` passes type checker
- [x] Fix parser gaps
  - [x] Closure types in type positions: `f: |i32| -> i32`
  - [x] Struct-style enum variants: `Move { x: i32, y: i32 }`
  - [x] Struct variant patterns: `Enum.Variant { field }` in match
  - [x] Struct variant construction: `Enum.Variant { field: val }` in expressions
  - [x] `read` parameter mode
  - [x] `read` keyword as method name (`db.read()`)
  - [x] Newline after `=>` in match arms
  - [x] Const generics: `<comptime N: usize>` — parser supports this, resolver needs to register params
- [x] Fix ownership checker gaps
  - [x] False borrow errors in chained closure params (`.filter(|n| ...).map(|n| ...)`)
- [x] Fix resolver gaps
  - [x] Generic type constructors `Type<T>.method()` → base name fallback
  - [x] Generic function/struct/enum declarations → strip `<...>` from name
  - [x] Qualified struct variant literals `Enum.Variant { ... }`
  - [x] `null` builtin constant
  - [x] `HttpResponse`/`HttpRequest`/`TcpListener`/`TcpConnection` net types
  - [x] Register comptime generic params (`N`) in scope
  - [x] Type-level constants (`u64.MAX`)
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

### Phase 3: Validation Programs (COMPLETE - 2026-02-10)
All 5 validation programs pass type checking. 4 of 5 run in the interpreter.

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
- [x] **HTTP JSON API server** — `net` module ✅, `json.decode<T>` ✅, `Shared<T>` ✅, `Multitasking` ✅, `Map.from` ✅, string slicing ✅
- [x] **Sensor processor** — ✅ **PASSES TYPE CHECK** (resolver, type checker, SIMD, @no_alloc all fixed)
  - Fixed: comptime generic params in scope, `u64.MAX` type constants, generic struct field access, array size tracking
  - SIMD `f32x8` type: load, splat, element-wise ops, sum
  - `@no_alloc` enforcement: flags Vec.new(), Map.new(), string.new(), format() in annotated functions

**Additional Interpreter Enhancements (2026-02-07):**
- Pool direct iteration (`for h in pool` = `for h in pool.cursor()`)
- Vec.pop() returns Option (was returning raw value)
- Implicit Ok() wrapping for `return ()` in `() or E` functions

**Design Gaps Found and Fixed:**
- ~~String interpolation doesn't support complex expressions~~ — **FIXED:** uses real parser for `{vec[i].field}`, `{x.method()}`
- ~~Tuple patterns can't use qualified enum paths~~ — **FIXED:** `(Enum.Variant, ...)` now works in patterns
- ~~Closures not implemented~~ — **FIXED:** full closure support with captured environments
- CLI module `--` delimiter — already handled correctly (documented)
- Examples had Rust syntax remnants (`.collect<Vec<_>>()`, `.map(|x| ...)`, implicit returns)
- ~~Vec.`take(n)` method name conflicts with `take` keyword~~ — **FIXED:** renamed to `limit(n)`


### Phase 4: Complete Frontend (COMPLETE - 2026-02-11)
Every specced language feature parses, resolves, type-checks, and ownership-checks.

**Parser polish**
- [x] Raw string literals (`r"..."`, `r#"..."#`)
- [x] Trait composition (`trait T: Other`) — super-traits in parser, resolver, type checker

**Union types** — [union-types.md](specs/types/union-types.md)
- [x] Parse `A | B` union type syntax (error-position only: `T or (A | B)`)
- [x] `Type::Union` variant with canonical form (sorted, deduped)
- [x] Type checker: union parsing, subset widening for `try` propagation, pattern exhaustiveness

**`select` statement** — [select.md](specs/concurrency/select.md)
- [x] AST nodes: `SelectArm`, `SelectArmKind` (Recv/Send/Default)
- [x] Parser: `select { }` / `select_priority { }` with recv (`->`), send (`<-`), default (`_`) arms
- [x] Type checker: channel type validation, arm body type compatibility

**`using` context clauses** — [context-clauses.md](specs/memory/context-clauses.md)
- [x] `ContextClause` AST node with name/type/frozen fields
- [x] Parser: `using [frozen] [name:] Type` on function signatures (before or after return type)
- [x] Resolver: named context bindings registered as scoped variables
- [x] Channel + spawn methods already formalized in type checker builtins

**Linear resource verification** — [resource-types.md](specs/memory/resource-types.md)
- [x] `is_resource: bool` on `TypeDef::Struct`, propagated from `@resource` attr
- [x] Ownership checker tracks resource bindings, `ensure` registration, `take self` consumption
- [x] `ResourceNotConsumed` error emitted at function exit for unconsumed resources

**Ownership checker hardening**
- [x] Projection borrows — `ActiveBorrow.projection` field, non-overlapping field projections don't conflict
- [x] `extract_projection()` strips `.{fields}` from type strings, feeds ownership checker
- [x] Closure capture mode inference — `collect_free_vars()` scans closure body, creates shared borrows

### Phase 5: Code Generation
Move from interpreter to actual compiled output.

**Critical blockers (must design before implementing):**
- [x] **Name mangling scheme** — Symbol naming for monomorphized functions, modules, generics (e.g., `Vec<i32>.push` → `collections_Vec_i32_push`)
- [x] **Memory layout documentation** — Specify enum tag placement, closure capture struct format, vtable structure, Result<T,E> encoding
- [x] **Test infrastructure** — Unit tests for MIR passes, integration tests for compile+run, validation program test suite

**Ready to implement (design complete):**
- [x] Choose backend (LLVM vs Cranelift) — Using Cranelift for dev builds
- [x] MIR structure — Defined in `codegen.md`: statements, terminators, types
- [x] Monomorphization algorithm — Specified (M1-M5 rules in `codegen.md`)
- [x] Runtime library API — Defined (RT1-RT3 in `codegen.md`): allocator, panic, collections, I/O, concurrency
- [x] Create `rask-mono` and `rask-mir` crate scaffolds — Data structures defined, compiles
- [ ] **Implement Monomorphization and MIR Lowering** (44 tasks):

  **Foundation (6 tasks):**
  - [ ] Study AST structure: read expr.rs, stmt.rs, decl.rs to understand all node types
  - [ ] Study TypedProgram structure: understand how type checker outputs are organized
  - [ ] Design type size/alignment computation: define functions for primitive and aggregate types
  - [ ] Implement struct layout computation: field ordering by alignment, padding calculation
  - [ ] Implement enum layout computation: tag size/placement, variant payload layout
  - [ ] Write layout computation tests: verify sizes match spec, test padding insertion

  **Monomorphization (8 tasks):**
  - [ ] Design AST cloning: implement deep clone for Decl/Expr/Stmt with type substitution
  - [ ] Implement type substitution visitor: replace type parameters throughout AST
  - [ ] Write instantiation tests: verify generic functions instantiate correctly
  - [ ] Design reachability walker: breadth-first traversal of call graph from main()
  - [ ] Implement function call discovery: find all Call expressions, extract type args
  - [ ] Implement generic instantiation deduplication: track (func_id, type_args) pairs
  - [ ] Wire up monomorphize(): connect reachability → instantiation → layouts
  - [ ] Write monomorphization integration tests: test on small programs with generics

  **MIR Basics (10 tasks):**
  - [ ] Design Type → MirType conversion: handle all type variants, error on generics
  - [ ] Implement MirType conversion with layout lookups
  - [ ] Implement literal lowering: Int/Float/String/Bool/Char → MirConst
  - [ ] Implement variable reference lowering: Ident → lookup local
  - [ ] Implement binary op lowering: lower operands, emit method Call
  - [ ] Implement unary op lowering: similar to binary ops
  - [ ] Implement simple call lowering: lower args, emit Call statement
  - [ ] Implement let/const lowering: allocate local, assign initializer
  - [ ] Implement return lowering: lower value, emit Return terminator
  - [ ] Write simple lowering tests: verify basic expressions produce correct MIR

  **Control Flow (6 tasks):**
  - [ ] Implement if-expression lowering: branch, then/else blocks, merge
  - [ ] Implement match-expression lowering: extract tag, switch, payload extraction
  - [ ] Write control flow tests: verify CFG structure for if/match
  - [ ] Implement while loop lowering: check/body/exit blocks
  - [ ] Implement for loop lowering: desugar to while with iterator
  - [ ] Implement loop/break/continue: infinite loop with exit handling

  **Error Handling (3 tasks):**
  - [ ] Implement try lowering: call, tag check, Ok/Err paths with cleanup
  - [ ] Implement ensure block lowering: push cleanup block, track stack
  - [ ] Write error handling tests: verify cleanup chain execution

  **Aggregates (4 tasks):**
  - [ ] Implement struct literal lowering: allocate, store fields
  - [ ] Implement enum literal lowering: store tag and payload
  - [ ] Implement array literal lowering: store elements sequentially
  - [ ] Implement field access lowering: Field rvalue with offset

  **Closures (4 tasks):**
  - [ ] Implement closure capture analysis: find free variables in closure body
  - [ ] Implement closure environment generation: create struct for captured vars
  - [ ] Implement closure function generation: clone body, add env parameter
  - [ ] Implement closure creation lowering: allocate env, store captures

  **Integration (3 tasks):**
  - [ ] Add rask mir command: pretty-print MIR for debugging
  - [ ] Integrate into build command: add mono and MIR lowering phases
  - [ ] Write end-to-end tests: compile hello_world.rk and verify MIR
  - [ ] Test on validation programs: grep, game_loop, editor - verify all lower correctly

- [ ] Implement Cranelift backend — MIR → machine code
- [ ] Build `rask-rt` runtime library — Rust implementation of allocator, panic, Vec, Map, Pool, string, I/O

**Deferred (not blocking v1.0):**
- [ ] Self-hosting bootstrap path — Compiler can stay Rust-based initially
- [ ] LLVM backend — Cranelift sufficient for initial release
- [ ] Advanced build system — Can use simple file compilation initially

### Phase 6: Ecosystem
Most core tooling is done. Remaining items can be built incrementally.

**Already complete:**
- [x] LSP completion — type-aware completions, go-to-definition
- [x] Test runner — `rask test` command
- [x] Formatter — `rask fmt`
- [x] `rask describe` — implement command (schema spec done: [specs/tooling/describe-schema.md](specs/tooling/describe-schema.md))
- [x] `rask explain` — real explanations + examples for all 43 error codes
- [x] `rask lint` — implement command (spec done: [specs/tooling/lint.md](specs/tooling/lint.md))
- [x] Structured error fixes — `fix:` / `why:` fields in all diagnostics

**Can defer:**
- [ ] Build system (`rask.build`) — Start with simple `rask compile file1.rk file2.rk -o binary`, add advanced features later
- [ ] Package manager — Use directory-based imports initially, add registry/versioning later

---

## Open Design Questions

### Critical (blocks Phase 5 start)
- [ ] **Name mangling scheme** — How to encode `Vec<Map<string, i32>>.push()` in symbol names? Need simple, readable format (Go-style vs Rust-style compression)
- [ ] **Memory layouts** — Document enum tag placement (before/after payload?), closure capture format, vtable structure, Result encoding

### Important (needed during Phase 5)
- [ ] **Runtime simplification strategy** — Should initial compiler target full M:N scheduler with reactor (complex), or start with OS threads per spawn (simple) and upgrade later?
- [ ] `using` block expressions (`using ThreadPool(workers: 4) { ... }`) — parser dispatches `With` but examples use `using`

### Quality improvements (doesn't block, improves ergonomics)
- [ ] `ensure` ordering lint — wrong LIFO order hides C-level UB behind safe-looking cleanup code
- [ ] `pool.remove_with(h, |val| { ... })` stdlib helper — cascading @resource cleanup is a 4-step dance today
- [ ] Style guideline: max 3 context clauses per function — lint, not language rule

### After codegen works (evaluate with real usage)
- [ ] **Package granularity decision** — folder = package (current, Go-style nested hierarchy) vs file = package (Zig-style flat with many files). Defer until validation programs exist to evaluate which feels better. Key tension: nested folders vs flat with descriptive filenames.
- [ ] Field projections for `ThreadPool.spawn` closures — can't do disjoint field access across threads without destructuring
- [ ] Design task-local storage syntax
- [ ] Design `Projectable` trait — let custom containers define `with...as` behavior
- [ ] String interop convenience — `as_c_str()`, `string.from_c()` methods

### Deferred (post-v1.0, no urgency)

**Advanced compilation:**
- [ ] LLVM backend — Cranelift sufficient initially, add for release optimization later
- [ ] Incremental compilation — Semantic hashing specified, implement when compile times become an issue
- [ ] Cross-compilation — C interop with per-target `c_type` sizes, header re-parsing

**Advanced tooling:**
- [ ] Comptime debugger — Step through comptime execution
- [ ] Fuzzing / property-based testing — Automated test generation
- [ ] Code coverage tooling — Track test coverage
- [ ] Metrics validation — Actual user studies for METRICS.md goals

**Language extensions (maybe):**
- [ ] `std.reflect` — Comptime reflection stdlib (local-analysis-safe) — see [reflect.md](specs/stdlib/reflect.md)
- [ ] Macros / `format!` — Wait until core language is solid
- [ ] Inline assembly (`asm!`) — For lowest-level code
- [ ] Pointer provenance rules — Formal memory model refinement
- [ ] Comptime memoization — Cache comptime computation results

**Ecosystem (maybe):**
- [ ] `compile_cpp()` build script support — Similar to `compile_rust()`
- [ ] Auto Rask wrapper generation from Rust cbindgen output
- [ ] Capability-based security for dependencies (restrict filesystem/network access)

### Resolved
- [x] Decide: `char` as a type — first-class Unicode scalar value, see [primitives.md](specs/types/primitives.md)
- [x] Decide: `discard` keyword — explicit drop for non-Copy types, see [ownership.md](specs/memory/ownership.md)
- [x] Write guidelines: panic vs error — panic for bugs, errors for expected failures, see [error-types.md](specs/types/error-types.md)
- [x] Spec `Owned<T>` semantics — see [owned.md](specs/memory/owned.md)
- [x] Consolidate parameter modes — see [parameters.md](specs/memory/parameters.md) (default=read-only/mutate/take)
- [x] Design shared state primitives — `Shared<T>`, see [sync.md](specs/concurrency/sync.md)
- [x] Decide multi-element access syntax — `with...as` binding + closure pattern
- [x] Formalize "one obvious way" principle — see [specs/canonical-patterns.md](specs/canonical-patterns.md)
- [x] Naming convention enforcement — see [specs/tooling/lint.md](specs/tooling/lint.md)
- [x] Structured error fixes — `fix:` / `why:` fields added to all compiler diagnostics
- [x] `rask describe` JSON schema — see [specs/tooling/describe-schema.md](specs/tooling/describe-schema.md)

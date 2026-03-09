# Rask — TODO

Open work, grouped by theme. Each chunk is roughly independent.

---

## 1. Enum & Pattern Matching Codegen

Enums compile as tagged unions but advanced patterns don't work natively yet.

- [x] **MIR: enum payload destructuring** — Match arms that destructure enum payloads (e.g., `Circle(radius)`) leave payload variables unresolved. Blocks 10_enums_advanced.
- [x] **Codegen: unknown type layouts** — Monomorphizer doesn't resolve enum types referenced inside structs (e.g., `EntityType` in game_loop). Defaults to (8, 8) which causes wrong field offsets and silent runtime crashes.

## 2. Closure & Indirect Call Codegen

Closures work as inline lambdas (spawn, iterators) but can't be passed as function parameters.

- [x] **Codegen: closure-as-parameter calling** — Functions taking closure params (`func apply(f: Func)`) generate calls to `f` but codegen can't resolve the indirect call. Blocks 11_closures.

## 3. Aggregate / Struct Return Codegen

Returning or passing structs through function boundaries sometimes generates wrong Cranelift IR.

- [x] **Codegen: aggregate return/arg count mismatches** — Pool.alloc() and some return paths generate wrong Cranelift IR argument counts. Blocks 14_borrowing_patterns, 15_memory_management.

## 4. Comptime Execution

The interpreter-based comptime system works for simple cases but doesn't bridge into native codegen.

- [x] **MIR: comptime module constants** — `comptime { ... }` at module level doesn't inject results into MIR scope. `SQUARES`, `PRIMES` etc. unresolved. Blocks 17_comptime.
- [x] **Conditional compilation** — `comptime if cfg.os/arch/features` (CC1-CC2). Dead branch elimination via AST rewrite before desugar; resolver also has cfg-aware fallback.

## 5. Collection Iteration & Runtime Crashes

Several examples compile and link but crash at runtime with no output.

- [x] **Runtime: silent crashes in collection iteration** — 03_collections (needs `.join()` and `is Some(score)`), 12_iterators (needs `.iter().map().collect()`), 13_string_operations (unknown crash). pool_test still crashes silently.

## 6. Ownership Checker Gaps

Ownership checking works for common patterns but has gaps in error reporting and explicit `own` handling.

- [x] **Ownership error messages lack source location** — `rask-cli/src/commands/build.rs:628` prints `error.kind` only, no file/line/span. The `OwnershipError` struct has a `span` field but it's never rendered. Needs file-to-span mapping like the type checker has.
- [x] **Ownership checker ignores `own` at call sites** — `ArgMode::Own` is parsed but `rask-ownership` never inspects it. Call sites with `own kv` don't mark `kv` as moved.

## 7. Multi-Package / Build System

Single-file compilation works. Multi-file packages have symbol visibility issues.

- [ ] **Cross-package public symbol export** — `public` types/funcs not visible via `import pkg` in workspace path deps. `lsm.DbError` resolves to `no such field on __module_lsm`. Blocks multi-package examples.
- [x] **Parser: empty guard else block** — `expr is Ok else {}` with empty braces causes brace mismatch.

## 8. Build Infrastructure & Security

Hardening for the package ecosystem. Not blocking any examples.

- [ ] **Build script sandbox** — Cross-platform sandbox for dep build scripts (SB1-SB7).
- [x] **Package signing** — Ed25519 TOFU signing on publish/fetch (SG1-SG7, KM1-KM3, LK8).
- [ ] **Build exec gating** — `exec()`/`exec_output()` require `build_exec` capability (PM9-PM10).
- [x] **Unsafe report command** — CLI command to report all unsafe operations by category.

---

## Design Questions

- [ ] Package granularity — folder = package (Go-style) vs file = package (Zig-style)
- [ ] Task-local storage syntax
- [ ] String interop — `as_c_str()`, `string.from_c()`
- [ ] `pool.remove_with(h, |val| { ... })` — cascading @resource cleanup helper
- [ ] Style guideline: max 3 context clauses per function
- [ ] **Trait satisfaction on methods** — Annotate individual methods with which trait they satisfy instead of `extend Type with Trait { }`.

## Post-v1.0

- [ ] LLVM backend
- [ ] Incremental compilation (semantic hashing)
- [ ] Cross-compilation toolchain support (XT1-XT8)
- [ ] Comptime debugger
- [ ] Fuzzing / property-based testing
- [ ] Code coverage
- [ ] `std.reflect` — comptime reflection
- [ ] Inline assembly
- [ ] Pointer provenance rules
- [ ] `compile_cpp()` build script support
- [ ] Auto Rask wrapper generation from cbindgen

---

## Done

<details>
<summary>Completed items (click to expand)</summary>

### Codegen
- [x] Type layout topological sort — forward-referenced enums in structs get correct sizes
- [x] Aggregate return/arg count — Pool.alloc/insert return paths work correctly
- [x] ThreadPool.spawn / Thread.spawn MIR routing
- [x] Sensor processor native compilation — f64 struct field access fixed
- [x] CleanupReturn deduplication
- [x] Non-closure `map_err` variant constructors
- [x] Unsafe block codegen
- [x] Result return from internal functions
- [x] Struct constructor + threads
- [x] HTTP server native compilation
- [x] Shared/Channel/spawn codegen
- [x] String interpolation with inline arithmetic
- [x] f64 chained struct field access
- [x] `format()` removed as redundant (string interpolation covers it)
- [x] None lowering — proper tagged union allocation
- [x] Vec.from([...]) — uses `rask_vec_from_static`
- [x] Dispatch gaps — Pool_is_empty, Pool_contains, Pool_cursor, Thread_detach, f64_powf, f64_powi, string_parse
- [x] Map string key comparison — content-based FNV hash and strcmp
- [x] Map.get().unwrap() crash — `rask_map_get_unwrap` panics on missing key

### MIR
- [x] Module-level constants
- [x] `i64.MAX` / `i32.MIN` type-associated constants
- [x] For-loop tuple destructuring
- [x] Comptime module constants — MIR resolves globals, scalar deref, branch quota reset

### Type Checker
- [x] Vec.len() returned i64 instead of u64
- [x] Module-level const didn't coerce integer literals

### Closures
- [x] Closure-as-parameter calling — function-type params registered in closure_locals

### Enum / Pattern Matching
- [x] Enum payload destructuring — Pattern::Struct handler for named-field match arms
- [x] Collection iteration crashes — None allocation, Vec.from dispatch, get().unwrap() tag checks

### Ownership
- [x] `mutate self` treated as borrowed
- [x] if/else branches don't save/restore state
- [x] Ownership error messages lack source location in `rask build`
- [x] `own` at call sites marks variable as moved

### Build System
- [x] Build pipeline end-to-end
- [x] Output directories
- [x] `rask add`/`remove`
- [x] Watch mode
- [x] Lock file system
- [x] Capability inference
- [x] `rask clean`, `rask targets`, `rask init`, `rask fetch`
- [x] Semver parsing, feature resolution, build scripts
- [x] Directory-based imports
- [x] Remote package registry, `rask publish`, `rask yank`
- [x] Vendoring, dependency auditing, workspace support
- [x] `string_builder` definition
- [x] `JsonParser` in multi-file builds

### Language Features
- [x] Type aliases
- [x] `todo()` / `unreachable()`
- [x] Tuple spec, destructuring spec
- [x] `format!` rejected (string interpolation covers it)

### Design Decisions
- [x] Serialization / encoding spec
- [x] Granular unsafe operations — keep blanket unsafe blocks
- [x] Capability-based security for dependencies

### Build Infrastructure
- [x] Unsafe report command — `rask unsafe <file>` reports operations by category

### Parser
- [x] Empty guard else block — `expr is Ok else {}` already works correctly

</details>

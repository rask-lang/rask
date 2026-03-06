# Rask — TODO

## Codegen

- [x] **ThreadPool.spawn / Thread.spawn MIR routing** — Already handled via `is_type_constructor_name` detecting uppercase type names and routing through dispatch table.
- [ ] **Sensor processor native compilation** — Cranelift f64 struct field access generates loads with wrong address type (uses f64 value as pointer). Blocks `compute_averages`.
- [x] CleanupReturn deduplication — shared Cranelift blocks per unique cleanup chain
- [x] Non-closure `map_err` variant constructors — handles both bare (`MyError`) and qualified (`ConfigError.Io`) names
- [x] **Unsafe block codegen** — Unsafe context enforced by type checker. Raw pointer primitives (read, write, add, sub, offset, etc.) fully implemented with dispatch and C runtime.
- [x] **Result return from internal functions** — `copy_aggregate` properly copies into caller stack slots. `return Ok(42)` from `-> T or E` works.
- [x] **Struct constructor + threads** — Aggregate handling (`copy_aggregate` + `stack_slot_map`) prevents callee stack pointer dangling.
- [x] **HTTP server native compilation** — C HTTP parser/response writer in runtime. Rask stdlib wrappers in `http.rk` compile alongside user code (injected at mono/codegen level). Request parsing (method, path, headers, body) and response writing (status, Content-Length, body) verified with curl.
- [x] **Shared/Channel/spawn codegen** — Fixed garbage allocation sizes from complex generic type codegen. `Shared.new()`, `Channel.buffered()`, and green `spawn` work natively.
- [x] **String interpolation with inline arithmetic** — Fixed: binary op MIR lowering now uses `binop_result_type()` instead of hardcoding `MirType::Bool`.

## Build System & Packages

- [x] **Build pipeline end-to-end** — `rask build` does full pipeline: package discovery → type check → ownership → mono → codegen → link. Output to `build/<profile>/`. Build scripts via interpreter. Compilation caching (XC1-XC5). Parallel compilation (PP1-PP3).
- [x] **Output directories** — `build/<profile>/` for native, `build/<target>/<profile>/` for cross. Auto `.gitignore`. Binary naming from manifest (OD1-OD5).
- [x] **`rask add`/`remove`** — Adds/removes deps in build.rk. Handles `--dev`, `--feature`, `--path`. Preserves formatting (AD1-AD4, RM1-RM2).
- [x] **Watch mode** — `rask watch [check|build|test|run|lint]`. 100ms debounce, auto-clear, error persistence (WA1-WA8).
- [x] **Lock file system** — SHA-256 checksums, capability tracking, deterministic output, `rask update` (LK1-LK7).
- [x] **Capability inference** — Import-based capability inference, `allow:` enforcement, lock file tracking (PM1-PM8).
- [x] **`rask clean`** — Remove build artifacts, `--all` for global cache (OD6).
- [x] **`rask targets`** — List cross-compilation targets with tier info (XT9).
- [x] **`rask init`** — New project scaffolding with build.rk, main.rk, .gitignore, git init.
- [x] **`rask fetch`** — Dependency validation: version constraints, path dep existence, capability checks, lock file update.
- [x] **Semver parsing** — `^`, `~`, `=`, `>=` constraints. Version ordering. Constraint matching. Resolution algorithm (VR1-VR3, D1, MV1).
- [x] **Feature resolution** — Additive and exclusive feature groups. Default selection, conflict detection, dependency activation (F1-F6, FG1-FG6).
- [x] **Build scripts** — `func build()` in build.rk runs via interpreter. Build state caching (LC1-LC2, BL1-BL3).
- [x] **Directory-based imports** — Multi-file packages, package discovery, cross-package symbol lookup (PO1-PO3, IM1-IM8).
- [x] **Remote package registry** — `rask fetch` downloads from `packages.rask-lang.dev`, semver resolution, SHA-256 verified cache, transitive deps, lock file with `registry+` sources (RG1-RG4).
- [x] **`rask publish`** — Pre-checks (build), required metadata validation (description+license), reproducible tarballs (sorted, zero timestamps), 10MB size limit, `--dry-run`, auth via `RASK_REGISTRY_TOKEN` or `~/.rask/credentials`, uploads to registry (PB1-PB7).
- [x] **`rask yank`** — Hide published versions from new resolution. Existing lock files unaffected. Auth token required.
- [x] **Vendoring** — `rask vendor` copies registry deps to `vendor/` with checksums. `vendor_dir: "vendor"` in build.rk enables vendor-first resolution. Offline builds supported (VD1-VD5).
- [x] **Dependency auditing** — `rask audit` checks locked versions against advisory database. Supports `--db` for offline JSON, `--ignore` for known risks, non-zero exit for CI gates (AU1-AU5).
- [x] **Workspace support** — `members: ["app", "lib"]` in root build.rk. Single `rask.lock` at workspace root. Members discovered independently, path deps between them (WS1-WS3).
- [ ] **Cross-package public symbol export** — `public` types/funcs not visible via `import pkg` in workspace path deps. `lsm.DbError` resolves to `no such field on __module_lsm`. Blocks multi-package examples.
- [ ] **Parser: empty guard else block** — `expr is Ok else {}` with empty braces causes brace mismatch. Parser loses track of enclosing scope, reports `Expected 'func', found 'try'` on the next statement. Non-empty body `else { return }` works fine.
- [x] **`string_builder` not defined** — `string_builder.new()` used in examples (markdown_renderer, lsm_database) resolves to `undefined symbol: string_builder`. Not in stdlib or compiler builtins. Needs implementation or stdlib definition.
- [x] **`JsonParser` not resolved in multi-file builds** — `rask build` on packages reports `undefined symbol: JsonParser` even though `stdlib/json.rk` defines it. Stdlib types not properly injected into multi-file package compilation.
- [x] **Vec.len() returned i64 instead of u64** — `resolve_vec_method` in `rask-types/src/checker/resolve.rs` unified `.len()` and `.capacity()` with `Type::I64` instead of `Type::U64`. Caused `expected u64, found i64` in any code doing `let x: usize = vec.len()`. Array `.len()` was already correct.
- [x] **Module-level const didn't coerce integer literals** — `DeclKind::Const` in `declarations.rs` used `infer_expr` instead of `infer_expr_expecting`. `const X: usize = 1024` wouldn't coerce the literal to `usize`.
- [x] **Ownership checker: `mutate self` treated as borrowed** — `mutate` params were registered as `Borrowed { Exclusive }`, preventing any reads within the function body. Fixed to `Owned` (the caller holds the borrow; within the function we own the reference).
- [x] **Ownership checker: if/else branches don't save/restore state** — Checker processes `then` and `else` sequentially. A move in the `then` branch marks the binding as `Moved`, so the `else` branch sees a false `UseAfterMove`. Root cause: `ExprKind::If` handler at `rask-ownership/src/lib.rs:534` doesn't snapshot/restore `self.bindings` per branch. Fix: save bindings before `then`, restore before `else`, merge afterward (both-moved → moved, one-moved → owned). Blocks: `memtable.rk` `put()` which moves `kv` in both if/else branches.
- [ ] **Ownership error messages lack source location** — `rask-cli/src/commands/build.rs:628` prints `error.kind` only, no file/line/span. The `OwnershipError` struct has a `span` field but it's never rendered. Needs file-to-span mapping like the type checker has.
- [ ] **Ownership checker ignores `own` at call sites** — `ArgMode::Own` is parsed but `rask-ownership` never inspects it. Call sites with `own kv` don't mark `kv` as moved. Currently moves happen implicitly via `handle_assignment` on the RHS, but explicit `own` should also mark the source as moved and validate it's still owned.
- [x] **MIR lowering: module-level constants not visible** — `const BLOOM_BITS: i32 = 256` etc. at file scope cause `UnresolvedVariable` during MIR lowering. Fix: pass `DeclKind::Const` decls through to MIR, inject as locals with `try_eval_const_init` before function body.
- [x] **MIR lowering: `i64` as variable** — `i64.MAX` / `i32.MIN` etc. not recognized. Fix: `primitive_type_constant()` resolves type-associated constants in `ExprKind::Field`.
- [x] **MIR lowering: for-loop tuple destructuring** — `for (name, value) in collection.iter()` only bound first element. Fix: generic for-loop and iter-chain paths now extract fields for `ForBinding::Tuple`.
- [x] **Codegen: `format()` was redundant** — Examples used `format("template {}", arg)` but Rask has string interpolation (`"template {arg}"`). Converted examples, removed unused MIR lowering.
- [x] **Codegen: None lowering** — Bare `None` was lowered as integer constant 1, causing segfault when tag-checked. Fixed: allocates proper tagged union.
- [x] **Codegen: Vec.from([...])** — Was calling `rask_vec_clone` with stack array pointer. Fixed: `lower_vec_from_array` uses `rask_vec_from_static`.
- [x] **Codegen: dispatch gaps** — Added Pool_is_empty, Pool_contains, Pool_cursor, Thread_detach, f64_powf, f64_powi, string_parse.
- [ ] **Codegen: closure-as-parameter calling** — Functions taking closure params (`func apply(f: Func)`) generate calls to `f` but codegen can't resolve the indirect call. Blocks 11_closures.
- [ ] **Codegen: f64 chained struct field access** — Loading an f64 field produces an f64 Cranelift value which then gets used as a base address for the next field load. Root cause: MIR field chains where intermediate loads return typed values instead of addresses. Blocks sensor_processor `compute_averages`.
- [ ] **Codegen: aggregate return/arg count mismatches** — Pool.alloc() and some return paths generate wrong Cranelift IR argument counts. Blocks 14_borrowing_patterns, 15_memory_management.
- [ ] **Codegen: unknown type layouts** — Monomorphizer doesn't resolve enum types referenced inside structs (e.g., `EntityType` in game_loop). Defaults to (8, 8) which causes wrong field offsets and silent runtime crashes.
- [ ] **MIR: enum payload destructuring** — Match arms that destructure enum payloads (e.g., `Circle(radius)`) leave payload variables unresolved. Blocks 10_enums_advanced.
- [ ] **MIR: comptime module constants** — `comptime { ... }` at module level doesn't inject results into MIR scope. `SQUARES`, `PRIMES` etc. unresolved. Blocks 17_comptime.
- [ ] **Runtime: silent crashes in collection iteration** — 03_collections, 12_iterators, 13_string_operations compile+link but exit(1) with no output. Likely Vec for-each iterator codegen producing wrong loop bounds or element access.
- [ ] **Conditional compilation** — `comptime if cfg.os/arch/features` (CC1-CC2).
- [ ] **Build script sandbox** — Cross-platform sandbox for dep build scripts (SB1-SB7).
- [ ] **Package signing** — Ed25519 TOFU signing on publish/fetch (SG1-SG7, KM1-KM3, LK8).
- [ ] **Build exec gating** — `exec()`/`exec_output()` require `build_exec` capability (PM9-PM10).

## Language Features

- [x] **Type aliases** — `type UserId = u64` transparent aliases. Parser, resolver, type checker. See [type-aliases.md](specs/types/type-aliases.md)
- [x] **`todo()` / `unreachable()`** — Development panic builtins returning `!` (Never). Interpreter + native codegen (desugars to `panic()` in MIR). See [error-types.md](specs/types/error-types.md)
- [x] **Tuple spec** — Formalized existing tuple support. See [tuples.md](specs/types/tuples.md)
- [x] **Destructuring spec** — Formalized existing destructuring. See [control-flow.md](specs/control/control-flow.md)
- [x] **Macros / `format!`** — `format()` removed as redundant — string interpolation `"hello {name}"` covers all use cases. General macro system rejected (`rejected-features.md`)
## Design — Decided

- [x] **Serialization / encoding** — `comptime for` + field access, auto-derived `Encode`/`Decode` marker traits, field annotations (`@rename`, `@skip`, `@default`). See [encoding.md](specs/stdlib/encoding.md)

## Design Questions

- [ ] Package granularity — folder = package (Go-style) vs file = package (Zig-style)
- [ ] Task-local storage syntax
- [ ] String interop — `as_c_str()`, `string.from_c()`
- [ ] `pool.remove_with(h, |val| { ... })` — cascading @resource cleanup helper
- [ ] Style guideline: max 3 context clauses per function
- [ ] **Trait satisfaction on methods** — Instead of `extend Type with Trait { }`, annotate individual methods: `func compare(self, other: T) -> Ordering for Comparable`. One `extend` block per type, methods self-declare which `explicit trait` they satisfy. Needs design for: default method overrides, multiple trait satisfaction per method, interaction with structural matching
- [x] **Granular unsafe operations** — Decided: keep blanket unsafe blocks. Compiler tracks operation categories internally (UnsafeCategory enum). Added oversized-block lint + expression-form `unsafe <expr>`. See mem.unsafe appendix for rationale
- [ ] **Unsafe report command** — CLI command to report all unsafe operations by category, using UnsafeCategory data the checker already collects. Name TBD (not `rask audit` — that's dependency auditing)

## Post-v1.0

- [ ] LLVM backend
- [ ] Incremental compilation (semantic hashing)
- [ ] Cross-compilation toolchain support — `--target` flag wired to Cranelift, needs cross-linker detection (XT1-XT8)
- [ ] Comptime debugger
- [ ] Fuzzing / property-based testing
- [ ] Code coverage
- [ ] `std.reflect` — comptime reflection. See [reflect.md](specs/stdlib/reflect.md)
- [ ] Inline assembly
- [ ] Pointer provenance rules
- [ ] `compile_cpp()` build script support
- [ ] Auto Rask wrapper generation from cbindgen
- [x] Capability-based security for dependencies

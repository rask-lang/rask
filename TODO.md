# Rask — TODO

## Codegen

- [x] **ThreadPool.spawn / Thread.spawn MIR routing** — Already handled via `is_type_constructor_name` detecting uppercase type names and routing through dispatch table.
- [x] **Sensor processor native compilation** — Runs natively with threads, timing, shared Vec. Float averages limited by untyped Vec codegen.
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
- [ ] **Conditional compilation** — `comptime if cfg.os/arch/features` (CC1-CC2).
- [ ] **Build script sandbox** — Cross-platform sandbox for dep build scripts (SB1-SB7).
- [ ] **Package signing** — Ed25519 TOFU signing on publish/fetch (SG1-SG7, KM1-KM3, LK8).
- [ ] **Build exec gating** — `exec()`/`exec_output()` require `build_exec` capability (PM9-PM10).

## Language Features

- [x] **Type aliases** — `type UserId = u64` transparent aliases. Parser, resolver, type checker. See [type-aliases.md](specs/types/type-aliases.md)
- [x] **`todo()` / `unreachable()`** — Development panic builtins returning `!` (Never). Interpreter + native codegen (desugars to `panic()` in MIR). See [error-types.md](specs/types/error-types.md)
- [x] **Tuple spec** — Formalized existing tuple support. See [tuples.md](specs/types/tuples.md)
- [x] **Destructuring spec** — Formalized existing destructuring. See [control-flow.md](specs/control/control-flow.md)
- [x] **Macros / `format!`** — `format()` is a compiler-known function (`std.fmt/CM1`, `struct.modules/BF2`). General macro system rejected (`rejected-features.md`)
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

# Compiler vs Spec Audit

Systematic comparison of what the specs require vs what the compiler implements (both interpreter and codegen). Organized by severity.

---

## Critical Gaps (spec features with zero implementation)

### 1. `ensure` cleanup — does nothing (EN1–EN7)

Parser accepts `ensure` syntax. Both interpreter and codegen **ignore it entirely**.

- **Interpreter:** `StmtKind::Ensure { .. } => Ok(Value::Unit)` — no-op
- **MIR:** No lowering for ensure blocks
- **Codegen:** `EnsurePush`/`EnsurePop` are no-ops; cleanup chains not materialized on normal exits

This means:
- No LIFO cleanup on scope exit (EN2)
- No cleanup on `return`, `break`, `continue`, `try` propagation (EN3)
- No per-iteration cleanup in loops (EN7)
- No linear resource consumption commitment (L1–L3)
- `ensure file.close()` does nothing — resources leak silently

**Impact:** Every spec example showing `ensure` for resource cleanup is broken. The HTTP server spec, file handling, connection cleanup — all affected.

### 2. `@binary` structs — completely unimplemented (type.binary B1–G4)

Zero parser support, zero codegen, zero stdlib. The entire binary struct feature:
- No `@binary` attribute recognition
- No bit-width field specifiers (`version: 4`, `u16be`, `u16le`)
- No generated `.parse()` / `.build()` methods
- No compile-time validation of layouts

**Impact:** Binary protocol parsing (TCP headers, file formats, wire protocols) has no path.

### 3. Error origin tracking — not implemented (ER15, ER16)

Spec requires all errors to capture `(file, line)` at first propagation via a `.origin` field (~16 bytes per error). No evidence in codebase — no origin capture, no `.origin` field on error types.

**Impact:** Error diagnostics in production have no call-site information.

### 4. `Cell<T>` type — doesn't exist (CE1–CE6)

Spec defines `Cell<T>` as a heap-allocated single-value container with `with`-based access. Not in stdlib stubs, not in builtins, not in runtime.

**Impact:** No way to share a mutable value across closures without Pool+Handle ceremony.

### 5. `discard` statement — not implemented (D1–D3)

No `discard` keyword in parser or AST. Spec requires explicit discard to invalidate bindings, with warnings on Copy types (D2) and compile error on `@resource` types (D3).

**Impact:** No way to explicitly drop values early and communicate intent.

### 6. `comptime for` + field reflection — not implemented (CT48–CT54)

- `comptime for` not recognized as distinct from regular `for`
- No loop unrolling at compile time
- No `value.(comptime_expr)` dynamic field access syntax
- No `reflect.fields<T>()` or `reflect.variants<T>()` API

**Impact:** Encoding/decoding spec (which relies on `comptime for` over struct fields) is blocked. Serialization patterns don't work.

### 7. C header auto-parsing — not implemented (CI1)

`import c "header.h"` syntax doesn't parse. Only manual `extern "C"` bindings work.

**Impact:** C interop requires manual binding declarations for every function.

---

## Major Gaps (partially implemented, key behaviors missing)

### 8. `for mutate` iteration — parsed but not enforced (LP11–LP16)

Parser accepts `for mutate item in vec { ... }`. Type checker doesn't validate:
- No structural mutation check inside body (LP14 — `vec.push()` during `for mutate` not rejected)
- No enforcement that `item` can't be passed to `take` parameters (LP16)
- MIR doesn't generate different access patterns for mutable vs immutable iteration

### 9. Pool API — missing several spec-required features

Missing from both interp and codegen:
- `pool.try_insert(x)` returning `Result<Handle<T>, InsertError<T>>` (PL8)
- `WeakHandle<T>` with `.valid()` and `.upgrade()` (weak handles spec)
- `pool.snapshot()` for concurrent read/write (PL9)
- `pool.drain()` and `pool.entries()` iterators
- Performance escape hatches: `pool.with_valid()`, `pool.get_unchecked()`
- `Pool<@resource>` runtime panic when non-empty at scope exit (R5)

### 10. Concurrency — missing Phase A surface area

Spec's own Phase A requirements (runtime-strategy.md) not met:

| Missing | Spec rule |
|---------|-----------|
| `try_send()` on channels | CH1 |
| `close()` on Sender/Receiver | CH4 |
| `Shared<T>.try_read()` / `.try_write()` | R3 |
| `join_all(handles)` | M1 |
| `select_first(handles)` | M2 |
| `TaskGroup<T>` struct + methods | M3 |
| `cancelled()` runtime check | CN1 |
| `Timer.after(duration)` | Channels |
| `ensure` cleanup on cancellation | CN2 |

### 11. Disjoint field borrowing — unclear enforcement (F1–F4)

Spec requires two borrows on different fields of the same struct to not conflict (F2). Borrow checker has `root_ident_name()` extraction but field-level granularity tracking isn't clearly implemented. Closures capturing individual fields (F4) not verified.

### 12. `@unique` move-only types — enforcement unclear (U1–U4)

`@unique` forces move semantics even for ≤16-byte types. Transitive propagation (U4: struct containing unique field is automatically unique) has no visible enforcement.

### 13. Scope-limited closures — escape not detected (SL1–SL2)

Closures capturing block-scoped borrows should be scope-limited and can't escape (SL2). No error for returning or storing such closures.

### 14. `private` field enforcement — not type-checked (V5)

Parser accepts `private` on struct fields. Type checker doesn't prevent construction or field access from outside the `extend` block.

---

## Minor Gaps (edge cases, polish, non-critical paths)

### 15. Single-element tuple `(T,)` — not parsed (TU4)

Spec says `(T,)` with trailing comma is a 1-tuple, `(T)` is a parenthesized expression. Parser doesn't handle this — confirmed in TODO.md.

### 16. Labeled break with value — MIR doesn't allocate result slots (CF25)

Parser accepts `break 'label value`. MIR lowering always sets `result_local: None` for labeled loops, so the value is lost.

### 17. Cyclic type alias detection — silent (T6)

`type A = B; type B = A` silently returns `None` instead of emitting a clear compile error with the cycle path.

### 18. Enum `.discriminant()` builtin — not exposed (E9)

Spec requires `func discriminant(e: T) -> u16 where T: Enum`. Not in stdlib stubs.

### 19. `.variants()` error on payload enums — not checked (E10)

Should be a compile error to call `.variants()` on enums with payloads. Not validated.

### 20. Iterator trait — not user-visible (type.iterators)

The `trait Iterator<Item>` isn't registered in stdlib for user code to write generic bounds like `T: Iterator<Item>`. Custom iterators can't be constrained.

### 21. Error auto-delegation for `@message` wrapper variants (ER25)

Variants with a single Error payload and no `@message` should auto-delegate to `inner.message()`. Desugar doesn't implement this.

### 22. Step range validation (SP1–SP3)

No compile-time or runtime check for zero step (`step(0)`) or wrong-direction step (`(0..10).step(-1)`).

### 23. Comptime safety limits (CT27–CT35)

Backwards branch quota and recursion depth limits not enforced during compile-time evaluation.

### 24. Context clause auto-resolution — opaque (CC1–CC10)

`using` clauses parsed, but the hidden parameter threading mechanism for auto-resolution isn't clearly wired through all passes.

### 25. Inline expression access for sync primitives (E5)

`Shared<T>.read()` and `.lock()` chains should be expression-scoped. Borrow checker doesn't enforce this boundary.

---

## Codegen-Specific Gaps

### 26. SIMD — stub only

`f32x8`/`i32x4` defined in MirType but passed as pointers like structs. No actual SIMD instructions generated. Interpreter simulates element-wise.

### 27. Debug info — partial DWARF

Source locations tracked but DWARF generation is minimal. Not fully integrated.

### 28. Single target — x86-64 only

No ARM, no WASM codegen paths. Cranelift supports them, but the compiler doesn't configure for anything else.

---

## Stdlib Gaps (interp has more coverage than codegen)

### Completely missing subsystems (0% implemented)

**Encoding (std.encoding)** — No stub file. No `Encode`/`Decode` traits, no auto-derive, no field annotations (`@rename`, `@skip`, `@default`, `@tag`). Blocked on `comptime for` + reflection (gap #6 above).

**Formatting (std.fmt)** — No stub file. No `format(template, ...args)` with compile-time checking, no format specifiers (`{:?}`, `{:x}`, `{:>10}`, `{:.3}`), no `Displayable`/`Debug` traits, no named interpolation in `println()`. Current state: basic `print()`/`println()` with string args only.

**Testing (std.testing)** — No stub file. No `test` block execution infrastructure, no `check` (soft assert, A2), no `skip()`/`expect_fail()` (T12–T13), no `benchmark` blocks (B1–B2), no doc test extraction (T14–T15), no subtests (T10), no parallel execution (T7), no seeded random (T8).

### Significantly incomplete (20–50% implemented)

**I/O (std.io ~20%)** — No `Reader`/`Writer` traits. No `BufReader`/`BufWriter`. No `Stdin`/`Stdout`/`Stderr` as linear resources. No `Buffer` type. No `io.copy()`. File has `read_all()` but doesn't formally implement traits.

**Strings (std.strings ~40%)** — Core string type works, but missing: `string_builder` type, `string_view` type (lightweight indices), `StringPool` type, `cstring` type and `c"literal"` syntax, `from_utf8()` validation, `char_count()`, `is_ascii()` with caching.

**OS (std.os ~50%)** — Env/args/exit/platform work. Missing: `Command` builder for subprocess spawning, `Process` as `@resource` with `wait()`/`kill_and_wait()`, `Signal` enum, `os.on_signal()` handler, `os.set_env()`/`os.remove_env()`.

### Mostly complete with notable gaps (60–85%)

**Collections (std.collections ~85%)** — Core Vec/Map/Pool work. Missing: `try_push()`/`try_insert()` error variants, `AllocError` enum, `vec.with()` block syntax, `vec.modify_many()`, `SliceDescriptor<T>` type, `vec.shrink_to_fit()`.

**Time (std.time ~75%)** — `Duration` and `Instant` work. Missing: `SystemTime` type entirely (only `Instant` exists), `Duration.from_secs_f64()`, arithmetic operators on Duration.

**FS (std.fs ~75%)** — Read/write/append/list work. Missing: `OpenOptions` builder pattern, `Metadata` struct (`is_file`, `is_dir`, `size`, `modified`), `DirEntry` struct, `File` doesn't implement Reader/Writer traits.

**Net (std.net ~70%)** — TCP listener/connection work. Missing: `UdpSocket` entirely, `net.resolve()` DNS resolution.

**HTTP (std.http ~65%)** — Basic server/client work. Missing: `Request.query_param()`/`query_params()`, `HttpClient` builder, `Responder` as `@resource` linear handle, `http.listen_and_serve()`.

**JSON (std.json ~70%)** — Parse/stringify work. Missing: typed `encode()`/`decode()` depends on missing Encode/Decode traits, field annotations depend on encoding spec.

**CLI (std.cli ~60%)** — Quick API works. Missing: `cli.Parser` builder pattern, auto-generated `--help`/`--version`, `CliError` enum.

### Bits (std.bits)

Binary parsing utilities specified but not implemented (tied to `@binary` gap above).

---

## What's Well-Implemented

For balance — these areas are solid:

- **Ownership/move semantics** (O1–O4): Use-after-move detection works
- **Basic borrowing** (A1–A3): Read/exclusive conflicts caught
- **Parameter modes** (PM1–PM3): borrow/mutate/take all work
- **Traits + trait objects** (TR1–TR16): Full vtable dispatch, implicit coercion
- **Enums + pattern matching** (E1–E8, PM1–PM6): Exhaustiveness, guards, destructuring
- **Optionals** (`T?`): Full OPT1–OPT13 compliance
- **Error types** (`T or E`): ER1–ER14 mostly working, `try`/`try...else`, `@message`
- **Generics + monomorphization**: G1–G7, full specialization pipeline
- **Closures**: Stack/heap allocation, captures, nested closures
- **String SSO + RC**: Runtime is sophisticated (16-byte SSO, refcount elision for statics)
- **Collections**: Vec, Map, Pool core operations all work
- **Concurrency basics**: spawn/join/detach, channels, Shared<T>, Mutex, atomics
- **JSON**: Full parse/stringify/encode/decode
- **HTTP/TCP**: Server + client both work
- **File I/O**: Read, write, append, directory listing

---

## Suggested Priority

1. **`ensure` cleanup** — everything else depends on safe resource cleanup
2. **Error origin tracking** — fundamental to error handling ergonomics
3. **`comptime for` + reflection** — blocks encoding/serialization patterns
4. **Pool weak handles + `try_insert`** — needed for real graph/entity patterns
5. **`for mutate` enforcement** — correctness hole
6. **Concurrency Phase A surface** — spec commits to this for Phase A
7. **`@binary` structs** — blocks a whole use case category
8. **`Cell<T>`** — ergonomic gap for closure patterns
9. **`discard`** — small but affects intent communication
10. **Private field enforcement** — correctness hole

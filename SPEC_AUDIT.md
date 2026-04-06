# Compiler vs Spec Audit

Systematic comparison of what the specs require vs what the compiler implements (both interpreter and codegen). Organized by severity.

---

## Critical Gaps (spec features with zero implementation)

### 1. ~~`ensure` cleanup — does nothing (EN1–EN7)~~ PARTIALLY FIXED

**Interpreter:** Fully working. `exec_stmts()` collects ensures and runs them in LIFO order on any block exit (normal, error, return). Consumption cancellation (C1) tracked via `ensure_receiver_consumed()`. Error handling (ER1–ER5) implemented with `else` handler support.

**MIR/Codegen:** Ensure bodies now lower into cleanup blocks. `CleanupReturn` terminators at `return`, `try` error, and implicit function exit chain through cleanup blocks in LIFO order. Inliner converts `CleanupReturn` to inline cleanup + goto when functions are inlined.

Working:
- LIFO cleanup on function exit (EN2) ✓
- Cleanup on `return` (EX2) ✓
- Cleanup on `try` error propagation (EX3) ✓
- Cleanup on implicit function exit (EX1) ✓
- Cleanup on `break`/`continue` (EX4) ✓
- Per-iteration cleanup in loops (EN7) ✓
- Multiple ensures with correct LIFO ordering ✓
- Function inlining preserves cleanup semantics ✓
- Loop-scoped ensures: `ensure_depth` on LoopContext, inline cleanup at break/continue/iteration-end, stack truncated after loop ✓

- `else` handler error routing (ER2): cleanup blocks form proper sub-CFGs with Branch terminators. Codegen processes full cleanup sub-CFGs. Inliner redirects Unreachable sentinels to merge blocks ✓
- Error type inferred from body call's Result type for handler binding ✓

- Consumption cancellation (C1/C2): C runtime resource tracker (`rask_resource_register/consume/is_consumed`). MIR lowerer extracts ensure receiver, emits ResourceRegister. `take self` method calls detected from monomorphized decls emit ResourceConsume. Cleanup blocks check consumption and skip if consumed ✓

Remaining:
- Linear resource consumption commitment (L1–L3) — ownership checker tracks it, codegen doesn't enforce

### 2. ~~`@binary` structs (type.binary B1–G4)~~ FIXED

Parser recognizes `@binary` attribute, bit-width field specifiers, and endianness annotations. Generated `.parse()` / `.build()` methods. Compile-time layout validation.

### 3. ~~Error origin tracking — not implemented (ER15, ER16)~~ FIXED

**Interpreter:** `Value::Enum` carries `origin: Option<Arc<str>>`. `try` sets origin to `"file.rk:line"` at first propagation only (first-wins per ER15). `.origin()` universal method — enums return stored origin, other types return `"<no origin>"`. SourceInfo (file name + LineMap) passed from CLI.

**Codegen:** Result layout changed to `[tag:8][origin_file:8][origin_line:8][payload]` (+16 bytes per Result). MIR `lower_try` constructs full Result.Err with origin line from LineMap on err path; conditional branch preserves source origin if already set (first-propagation semantics). `.origin()` calls `rask_result_origin` C runtime helper. `rask_set_origin_file()` called at start of `rask_main` to register the source file name — codegen now returns `"file.rk:line"` matching interpreter output.

### 4. ~~`Cell<T>` type (CE1–CE6)~~ FIXED

`Cell<T>` implemented as a heap-allocated single-value container with `with`-based access. Stdlib stubs, builtins, and runtime support.

### 5. ~~`discard` statement — not implemented (D1–D3)~~ FIXED

Full pipeline: lexer → parser → AST → type checker → ownership checker → interpreter → MIR → formatter. D1 (use-after-discard error), D2 (Copy type warning), D3 (@resource compile error) all enforced.

### 6. ~~`comptime for` + field reflection (CT48–CT54)~~ FIXED

`comptime for` with loop unrolling, `value.(comptime_expr)` dynamic field access, `reflect.fields<T>()` / `reflect.variants<T>()` API. Encoding/serialization patterns unblocked.

### 7. ~~C header auto-parsing (CI1)~~ PARTIALLY FIXED

`import c "header.h"` syntax now parses. AST node (`CImportDecl`), parser, formatter all support the full syntax including `as` aliases, multi-header `{ }` blocks, and `hiding { }` clauses. Match arm coverage across all compiler passes.

Remaining: actual C header parser backend (translating C declarations to Rask AST). Manual `extern "C"` bindings still work as before.

---

## Major Gaps (partially implemented, key behaviors missing)

### 8. ~~`for mutate` iteration (LP11–LP16)~~ FIXED

Parser accepts `for mutate item in vec { ... }`. Ownership checker enforces LP14 (structural mutation rejected) and LP16 (`own item` rejected). MIR in-place mutation codegen implemented.

### 9. ~~Pool API~~ FIXED

`try_insert`, `WeakHandle<T>` with `.valid()` and `.upgrade()`, `pool.snapshot()`, `pool.drain()`, `pool.entries()`, performance escape hatches, `Pool<@resource>` panic on non-empty drop.

### 10. ~~Concurrency Phase A~~ FIXED

`try_send()`, `close()`, `Shared<T>.try_read()/.try_write()`, `join_all(handles)`, `select_first(handles)`, `TaskGroup<T>`, `cancelled()`, `Timer.after(duration)`, `ensure` cleanup on cancellation.

### 11. ~~Disjoint field borrowing — unclear enforcement (F1–F4)~~ PARTIALLY FIXED

F1–F3 (direct field borrows) fully implemented: `extract_root_and_fields()`, `ActiveBorrow.projection`, `overlaps()` all work — disjoint field borrows coexist correctly. F4 (closure field-level captures) now implemented: `collect_free_vars` tracks field projections so closures capturing `state.score` register a field-level borrow, not a whole-object borrow on `state`. Closure captures of disjoint fields no longer conflict.

### 12. ~~`@unique` move-only types — enforcement unclear (U1–U4)~~ FIXED

`is_unique` flag on TypeDef::Struct, parsed from `@unique` attribute. Ownership checker's `is_copy()` returns false for unique types. `MoveReason::Unique` wired through diagnostics. U4 transitive propagation via fixed-point iteration in `propagate_uniqueness()` — structs containing unique fields are automatically marked unique.

### 13. ~~Scope-limited closures — escape not detected (SL1–SL2)~~ FIXED

Closures capturing non-Copy `const` bindings (block-scoped borrows) are now scope-limited. Ownership checker tracks borrow bindings and closure scope limits. Two escape paths detected: (1) `return f` where `f` is scope-limited → E0813 error; (2) assigning scope-limited closure to outer-scope variable → E0813 at block exit. Valid in-scope use (calling the closure, passing it to functions) works fine.

### 14. ~~`private` field enforcement — not type-checked (V5)~~ FIXED

Private fields now checked in struct literals and field access. Extend-block context carried through HasField constraints.

---

## Minor Gaps (edge cases, polish, non-critical paths)

### 15. ~~Single-element tuple `(T,)` — not parsed (TU4)~~ FIXED

Parser now distinguishes `(T)` (parenthesized) from `(T,)` (single-element tuple) for both expressions and types.

### 16. ~~Labeled break with value — MIR doesn't allocate result slots (CF25)~~ FIXED

Loop expressions now allocate `result_local` so `break value` stores correctly. Works for both statement and expression loops.

### 17. ~~Cyclic type alias detection — silent (T6)~~ FIXED

Cycle detection at registration time with clear error showing the cycle path.

### 18. ~~Enum `.discriminant()` builtin — not exposed (E9)~~ FIXED

`.discriminant()` method on enum values returns `u16` variant index. Type checker, interpreter, and MIR lowering all support it.

### 19. ~~`.variants()` error on payload enums — not checked (E10)~~ ALREADY FIXED

Both type checker and interpreter reject `.variants()` on enums with payload fields. Was implemented before audit.

### 20. ~~Iterator trait — not user-visible (type.iterators)~~ FIXED

`Iterator` registered as a builtin trait in `get_builtin_trait_methods` with `next(mutate self) -> Item?`. Parser now supports generic trait bounds (`T: Iterator<i64>`) with `>>` splitting for nested generics. Trait lookup strips generic args so `Iterator<i64>` resolves to the `Iterator` trait definition. Users can write `func consume<T: Iterator<string>>(iter: T)` and have bounds checked.

### 21. ~~Error auto-delegation for `@message` wrapper variants (ER25)~~ FIXED

Desugar now only auto-delegates for single-field variants whose type name ends with "Error" (ER25). Variants with fields but no `@message` and no auto-delegatable payload trigger an ER26 coverage error. Pipeline reports desugar errors before proceeding.

### 22. ~~Step range validation (SP1–SP3)~~ FIXED

SP3 (zero step) produces a compile error. SP1/SP2 (direction mismatch) now produce compile-time warnings when start, end, and step are all integer literals — e.g., `(10..0).step(2)` warns that the positive step on a descending range will produce zero iterations. Handles desugared negative literals (`-1` → `(1).neg()`).

### 23. ~~Comptime safety limits (CT27–CT35)~~ FIXED

Default backwards branch quota set to 1,000 (CT35, was incorrectly 10,000). Call depth tracking added with 256-frame limit (CT29) — stack overflow detected separately from branch quota with clear error. `@comptime_quota(N)` attribute override not yet implemented.

### 24. ~~Context clause auto-resolution (CC1–CC10)~~ FIXED

Hidden parameter pass restructured into modules. CC4 scope resolution (local > param > self.field > using clause), CC7 private inference from handle field access, CC8 ambiguity detection, CC9/CC10 closure context rules. TypedProgram.node_types passed for type-aware resolution.

### 25. ~~Inline expression access for sync primitives (E5)~~ FIXED

Bare `.read()/.write()/.lock()` without field chain is a compile error. Cannot store sync access result in variable. DL4 deadlock detection for multiple sync accesses in one expression. Expression tree walk finds nested sync accesses.

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

### Mostly complete with notable gaps (60–85%)

**Collections (std.collections ~90%)** — Core Vec/Map/Pool work. `try_push()`/`try_insert()` error variants implemented. Missing: `AllocError` enum, `vec.with()` block syntax, `SliceDescriptor<T>` type.

**Time (std.time ~85%)** — `Duration` and `Instant` work, including `from_secs_f64()`. Missing: `SystemTime` type (only `Instant` exists), arithmetic operators on Duration.

**FS (std.fs ~90%)** — Read/write/append/list/copy/rename/remove/mkdir/metadata work. File implements Reader/Writer traits. Missing: `OpenOptions` builder pattern, `DirEntry` struct.

**Net (std.net ~70%)** — TCP listener/connection work. Missing: `UdpSocket` entirely, `net.resolve()` DNS resolution.

**HTTP (std.http ~85%)** — Server + client work. `Request.query_param()`/`query_params()`, `HttpClient` builder, `Responder` as `@resource`, `http.listen_and_serve()` all implemented.

**JSON (std.json ~70%)** — Parse/stringify work. Missing: typed `encode()`/`decode()` depends on Encode/Decode traits, field annotations depend on encoding spec.

**CLI (std.cli ~60%)** — Quick API works. Missing: `cli.Parser` builder pattern, auto-generated `--help`/`--version`, `CliError` enum.

**Encoding (std.encoding ~40%)** — Stub file exists with trait definitions. Auto-derive and field annotations depend on comptime for.

**Formatting (std.fmt ~40%)** — Stub file exists. Basic format specifiers specified. Full compile-time template checking not yet implemented.

**Testing (std.testing ~85%)** — `test` and `benchmark` blocks execute via `rask test`. `check` (soft assert), `skip()`/`expect_fail()`, subtests, parallel execution implemented. Missing: doc test extraction.

**Bits (std.bits ~40%)** — `bits.rk` stub with network byte order aliases, `BinaryBuilder`, `ParseError`. Per-integer bit methods (`popcount`, `leading_zeros`, etc.) not yet registered as type methods.

---

## What's Well-Implemented

For balance — these areas are solid:

- **Ownership/move semantics** (O1–O4): Use-after-move detection works
- **Basic borrowing** (A1–A3): Read/exclusive conflicts caught
- **Disjoint field borrowing** (F1–F4): Field-level projections, closure captures
- **Parameter modes** (PM1–PM3): borrow/mutate/take all work
- **Traits + trait objects** (TR1–TR16): Full vtable dispatch, implicit coercion
- **Enums + pattern matching** (E1–E8, PM1–PM6): Exhaustiveness, guards, destructuring
- **Optionals** (`T?`): Full OPT1–OPT13 compliance
- **Error types** (`T or E`): ER1–ER16 working, `try`/`try...else`, `@message`, origin tracking
- **Generics + monomorphization**: G1–G7, full specialization pipeline
- **Closures**: Stack/heap allocation, captures, nested closures, scope-limited escape detection
- **String SSO + RC**: Runtime is sophisticated (16-byte SSO, refcount elision for statics)
- **Collections**: Vec, Map, Pool core operations all work, including try variants and weak handles
- **Concurrency**: spawn/join/detach, channels, Shared<T>, Mutex, atomics, TaskGroup, select, cancellation
- **Context clauses** (CC1–CC10): Full auto-resolution, propagation, inference, ambiguity detection
- **Sync inline access** (E5): Expression-scoped locks with bare-access and DL4 deadlock detection
- **Comptime**: Conditional compilation, `comptime for`, field reflection, safety limits
- **`ensure` cleanup**: LIFO ordering, all exit paths, consumption cancellation, inlining
- **`@binary` structs**: Parse/build, bit-width fields, endianness
- **`Cell<T>`**: Heap-allocated mutable container
- **`@unique` types**: Move-only enforcement with transitive propagation
- **I/O traits**: Reader/Writer abstraction, BufReader/BufWriter, Stdin/Stdout/Stderr, io.copy()
- **Strings**: string_builder, string_view, cstring, from_utf8(), char_count(), is_ascii()
- **OS**: Command builder, Process @resource, Signal enum, on_signal(), set_env/remove_env
- **Testing**: test/benchmark blocks, check (soft assert), skip/expect_fail, subtests, parallel
- **JSON**: Full parse/stringify/encode/decode
- **HTTP/TCP**: Server + client both work, request parsing, response formatting
- **File I/O**: Read, write, append, directory listing, copy, rename, metadata

---

## Remaining Priority

1. **C header parser backend** — AST plumbing done, need actual C declaration parser
2. **Codegen SIMD** — actual vector instructions instead of scalar fallback
3. **Codegen multi-target** — ARM, WASM
4. **Linear resource commitment** (L1–L3) — codegen enforcement
5. **Stdlib doc test extraction** — T14–T15
6. **Stdlib net** — UdpSocket, DNS resolution

# Parallel Work Streams — Phase 2

Three streams that address the remaining gaps with proper solutions, not patches.

| Stream | Primary Crates/Dirs | Depends On |
|--------|---------------------|------------|
| 1. MIR Type Fidelity | `rask-mir`, `rask-codegen` (builder.rs print only) | Nothing |
| 2. Closure Pipeline | `rask-mir` (stmt/operand), `rask-codegen` (builder stmt handlers) | Nothing |
| 3. Stdlib + Build Integration | `rask-cli`, `rask-codegen` (module/dispatch), `runtime/` | Nothing |

Merge order: any. Streams 1+2 touch different parts of MIR (types vs statements). Stream 3 is in CLI/runtime, no MIR changes.

---

## Stream 1: MIR Type Fidelity

**Directories:** `compiler/crates/rask-mir/src/`, `compiler/crates/rask-codegen/src/builder.rs`
**Goal:** Preserve semantic types through MIR so codegen can make correct decisions.

### Problem

MIR erases type identity. `string` becomes `FatPtr`, indistinguishable from other
pointers. The conversion path is lossy:

```
Type::String → format!("{}") → "string" → resolve_type_str() → MirType::FatPtr
```

This causes `print(string_variable)` to dispatch to `rask_print_i64` instead of
`rask_print_string`. But it's a symptom of a deeper issue — codegen can't make
type-directed decisions because MIR threw the information away.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to fix the
MIR type system so it preserves semantic type information through lowering.

**The root cause:**

MIR types are defined in `compiler/crates/rask-mir/src/types.rs`. The `MirType`
enum has `Ptr` and `FatPtr` but no `String` variant. When the MIR lowerer converts
from the type checker's `Type` enum (in `rask-types`), it goes through a lossy path:

  Type::String → format!("{}", ty) → "string" → resolve_type_str() → MirType::FatPtr

This happens in `compiler/crates/rask-mir/src/lower/mod.rs` (~line 108).

**What to fix:**

1. **Add `MirType::String`** to the MirType enum in `types.rs`.
   - String is a pointer to heap-allocated data, but it's semantically distinct
   - Codegen can lower it to the same Cranelift type (I64/pointer) but needs to know
     it's a string for dispatch decisions

2. **Replace `resolve_type_str` with direct Type → MirType conversion.**
   The current path formats a Type to a string, then pattern-matches the string.
   This is fragile and lossy. Instead:
   - Add a method that takes a reference to the type checker's `Type` enum directly
   - Match on `Type::String => MirType::String`, `Type::Bool => MirType::Bool`, etc.
   - Keep `resolve_type_str` as a fallback for string-based lookups (monomorphized
     names, type annotations in source) but prefer the direct path wherever possible
   - The `node_types` HashMap in MirContext maps AST node IDs to type checker Types —
     use this to get accurate types for expressions

3. **Preserve types through variable bindings.**
   In `lower/stmt.rs` around line 168, `lower_binding` infers the variable type from
   the initializer expression. When the initializer is a string literal, the inferred
   type is currently `FatPtr`. With the MirType::String fix, it should become
   `MirType::String`, and this should propagate automatically.

4. **Fix print dispatch in codegen.**
   In `compiler/crates/rask-codegen/src/builder.rs` around line 415, the function
   `runtime_print_for_operand` checks local variable types to pick the right print
   function. Currently, string variables fall through to `rask_print_i64` because
   their type is `FatPtr`. After the MirType::String change:
   - Match `MirType::String => "rask_print_string"`
   - Keep `MirType::FatPtr` and `MirType::Ptr` as `rask_print_i64` (they're genuinely
     opaque pointers)

5. **Update MirType → Cranelift type mapping.**
   In `compiler/crates/rask-codegen/src/types.rs`, add `MirType::String => types::I64`
   (strings are passed as pointers at the machine level). The semantic distinction is
   for dispatch, not for register allocation.

6. **Update Display/Debug impls** for MirType to show "string" for the new variant.

7. **Update all match arms** that handle MirType across both crates. The compiler will
   tell you where — add the new variant to every match. In most cases, String behaves
   like Ptr (it's a pointer), except in dispatch decisions.

**What NOT to fix (out of scope):**
- Don't add typed arguments to MirStmt::Call — that's a bigger refactor
- Don't change the monomorphizer — it works with string names and that's fine
- Don't touch the C runtime or dispatch.rs — those are Stream 3

**Testing:**
- Create `/tmp/test_mir_string.rk`:
  ```rask
  func main() {
      const s = "hello world"
      print(s)
      print("\n")
  }
  ```
  Compile with `rask compile /tmp/test_mir_string.rk -o /tmp/test_mir_string`
  Run `/tmp/test_mir_string` — should print "hello world" (not garbage)

- Create `/tmp/test_mir_types.rk`:
  ```rask
  func greet(name: string) {
      print("Hello, ")
      print(name)
      print("\n")
  }
  func main() {
      greet("world")
  }
  ```
  Verify `rask mir /tmp/test_mir_types.rk` shows `string` type on locals, not `fatptr`.

- Run existing codegen tests: `cargo test -p rask-codegen`
- Run existing MIR tests: `cargo test -p rask-mir`

**Design consideration:** You might be tempted to add more semantic types (Vec, Map,
etc.). Don't — those are genuinely type-erased at the C runtime level. String is
special because it has dedicated print/IO functions. Other collections are dispatched
by name, not by type.

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 2: Closure Pipeline

**Directories:** `compiler/crates/rask-mir/src/`, `compiler/crates/rask-codegen/src/`
**Goal:** Complete the MIR lowering for closures so they compile to working machine code.

### Problem

The closure pipeline has a gap in the middle:

```
AST (Closure expr)  ✅ complete
    ↓
Monomorphization    ✅ clones closure AST, substitutes types
    ↓
MIR Lowering        ❌ STUB — returns null pointer (expr.rs ~line 584)
    ↓
Codegen             ✅ closures.rs has all infrastructure (allocate_env, create_closure,
                       call_closure, load_capture) — just needs MIR to feed it
```

### Prompt

```
You're working on the Rask programming language compiler. Your task is to implement
closure lowering in MIR so that closures compile to working native code.

**Current state:**

- AST: `ExprKind::Closure { params, ret_ty, body }` — fully parsed
- Codegen infrastructure: `compiler/crates/rask-codegen/src/closures.rs` has everything
  ready — environment allocation, capture loading, closure construction, indirect calls.
  A closure is 16 bytes: `[func_ptr: i64, env_ptr: i64]`.
- MIR lowering: `compiler/crates/rask-mir/src/lower/expr.rs` ~line 584 is a STUB that
  lowers closure parameters, lowers the body (discarding result), then returns 0 as a
  Ptr. No function synthesis, no capture analysis, no environment construction.

**What to implement:**

1. **Add MIR statements for closures** in `stmt.rs`:

   ```rust
   // Create a closure — allocate env, store captures, build (func_ptr, env_ptr) pair
   ClosureCreate {
       dst: LocalId,
       func_name: String,              // name of synthesized closure function
       captures: Vec<(LocalId, u32)>,  // (source_local, offset_in_env)
       env_size: u32,
   },

   // Call a closure — indirect call through func_ptr with env_ptr as first arg
   ClosureCall {
       dst: Option<LocalId>,
       closure: LocalId,               // the 16-byte closure struct
       args: Vec<MirOperand>,          // user-visible arguments
   },
   ```

   Also add to `MirRValue` in `operand.rs`:
   ```rust
   // Load a captured variable from the closure environment pointer
   LoadCapture { env: LocalId, offset: u32, ty: MirType },
   ```

2. **Implement free variable analysis** in the MIR lowerer.
   When lowering a `Closure` expression:
   - Walk the closure body and collect all `Ident` references
   - Filter against the closure's own parameters
   - Filter against known function names (global scope)
   - What remains are captured variables — look them up in `self.locals`
   - Assign each capture an offset in the environment struct (8-byte aligned)

3. **Synthesize a closure function** in the MIR lowerer.
   When lowering a `Closure` expression:
   - Generate a unique name: `__closure_{parent_func}_{counter}`
   - Create a new `MirFunction` with:
     - First parameter: `__env: MirType::Ptr` (pointer to environment struct)
     - Remaining parameters: the closure's declared params
     - Body: lower the closure body, but for captured variables, emit
       `LoadCapture { env: __env_local, offset, ty }` instead of reading the local
   - Add this function to a collection that gets returned alongside the parent function
     (the caller in `codegen.rs` needs to codegen these too)

4. **Emit ClosureCreate** at the closure expression site:
   ```
   ClosureCreate {
       dst: closure_local,
       func_name: "__closure_main_0",
       captures: [(x_local, 0), (y_local, 8)],
       env_size: 16,
   }
   ```

5. **Handle ClosureCall** in MIR.
   When a closure variable is called (it's a `Call` where the callee is a local, not
   a function name), emit `ClosureCall` instead of a regular `Call`.

   How to detect this: if the call target name matches a local variable (not a known
   function), it's a closure call. The lowerer has `self.locals` to check.

6. **Handle new statements in codegen** (`builder.rs`).
   Add match arms in `lower_stmt` for:

   - `ClosureCreate`: Use `closures::allocate_env()` to create the environment,
     store each captured variable at its offset, then use `closures::create_closure()`
     to build the 16-byte struct with (func_ptr, env_ptr).

   - `ClosureCall`: Use `closures::load_func_ptr()` and `closures::load_env_ptr()`
     to extract from the closure struct, then `closures::call_indirect()` with
     env_ptr prepended to the argument list.

   Add a match arm in `lower_rvalue` for:
   - `LoadCapture`: Use `closures::load_capture()` to read from the env pointer.

7. **Wire synthesized functions through the pipeline.**
   The MIR lowerer currently returns one `MirFunction` per input function. Change it
   to return additional synthesized closure functions. In `codegen.rs` (the CLI command),
   these extra functions need to be declared and codegenned alongside regular functions.

   Look at how `cmd_compile` in `compiler/crates/rask-cli/src/commands/codegen.rs`
   iterates `mir_functions` — the synthesized closure functions need to be in that list.

**Testing:**

Create test .rk files in `/tmp/`:

- `/tmp/test_closure_basic.rk` — closure with no captures:
  ```rask
  func apply(f: |i32| -> i32, x: i32) -> i32 {
      return f(x)
  }
  func main() {
      const double = |x: i32| -> i32 { return x * 2 }
      print(apply(double, 21))
      print("\n")
  }
  ```
  Expected output: `42`

- `/tmp/test_closure_capture.rk` — closure capturing a variable:
  ```rask
  func apply(f: |i32| -> i32, x: i32) -> i32 {
      return f(x)
  }
  func main() {
      const offset = 10
      const add_offset = |x: i32| -> i32 { return x + offset }
      print(apply(add_offset, 32))
      print("\n")
  }
  ```
  Expected output: `42`

- `/tmp/test_closure_multi_capture.rk` — multiple captures:
  ```rask
  func main() {
      const a = 10
      const b = 20
      const sum = || -> i32 { return a + b }
      print(sum())
      print("\n")
  }
  ```
  Expected output: `30`

For each: compile with `rask compile`, run the binary, verify output.
Also run `cargo test -p rask-codegen` and `cargo test -p rask-mir`.

**Scope limits:**
- Only stack-allocated environments (closures that escape will dangle — known limitation)
- Only value captures (no mutable captures / capture by reference)
- Don't implement higher-order closures (closures returning closures) — that needs
  heap allocation

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 3: Stdlib + Build Integration

**Directories:** `compiler/crates/rask-cli/src/commands/`, `compiler/crates/rask-codegen/src/module.rs`, `compiler/crates/rask-codegen/src/dispatch.rs`, `compiler/runtime/`
**Goal:** Make compiled programs that use Vec, String, Map, and Pool actually link and run.

### Problem

Three issues prevent stdlib usage in compiled programs:

1. **`declare_stdlib_functions()` is never called.** The compile pipeline in
   `codegen.rs` calls `declare_runtime_functions()` (print, I/O, exit) but never
   calls `declare_stdlib_functions()` (Vec, String, Map, Pool). Stdlib calls in
   compiled programs will fail with "undefined symbol" at link time.

2. **Link step only compiles `runtime.c`.** The linker runs
   `cc runtime.c obj.o -o bin`. But runtime.c has TWO sets of implementations:
   - Old i64-based ones (inline in runtime.c) matching dispatch.rs signatures
   - New typed-pointer ones (in vec.c, string.c, map.c, pool.c) with different signatures
   The new .c files are never compiled into the binary. Duplicate symbols will
   appear if both are included.

3. **Dispatch uses bare names.** MIR produces calls like `push`, `len`, `get` without
   qualifying which type they belong to. `dispatch.rs` maps `push` → `rask_vec_push`,
   but if Map or String also has `push`, they'd collide.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to wire up
the stdlib so compiled Rask programs can use Vec, String, Map, and Pool.

**Read first:**
- `compiler/crates/rask-cli/src/commands/codegen.rs` — the compile pipeline
- `compiler/crates/rask-cli/src/commands/link.rs` — linking
- `compiler/crates/rask-codegen/src/module.rs` — runtime function declarations
- `compiler/crates/rask-codegen/src/dispatch.rs` — stdlib dispatch table
- `compiler/runtime/runtime.c` — C runtime (has old i64-based Vec/String/Map/Pool)
- `compiler/runtime/vec.c`, `string.c`, `map.c`, `pool.c` — new typed implementations
- `compiler/runtime/rask_runtime.h` — new API declarations

**Problem 1: Stdlib not declared in compile pipeline**

In `codegen.rs` around line 333, the compile pipeline calls:
```rust
codegen.declare_runtime_functions()?;  // print, I/O, exit
codegen.declare_functions(&mono, &mir_functions)?;
codegen.register_strings(&mir_functions)?;
```

But never calls `codegen.declare_stdlib_functions()`. This method exists in `module.rs`
and works (it's tested), it's just not wired into production. Fix:

- Add `codegen.declare_stdlib_functions()?;` after `declare_runtime_functions()`
- Do the same in `build.rs` if it has a similar pipeline

**Problem 2: Dual runtime implementations**

`runtime.c` contains OLD inline implementations of Vec/String/Map/Pool that use raw
i64 as the opaque pointer type. These match what `dispatch.rs` expects. Separately,
`vec.c`, `string.c`, `map.c`, `pool.c` have NEW typed implementations with different
signatures (take `RaskVec*` instead of `int64_t`, take `elem_size` parameter, etc.).

The link step (`link.rs`) only compiles `runtime.c`. The new .c files are unused in
compilation. This needs to be reconciled.

**The proper solution:**

Keep the old i64-based implementations in runtime.c for now. They match dispatch.rs
and they work. The new typed .c files are better-engineered but have incompatible
signatures with what codegen generates. Reconciling the two APIs is a larger project.

For this stream:
1. Verify the old implementations in runtime.c cover everything dispatch.rs references
2. If any dispatch.rs entries reference functions that DON'T exist in runtime.c, add
   them to runtime.c using the i64 convention
3. Add a comment at the top of the old implementations explaining the situation:
   separate typed implementations exist in vec.c etc., and the plan is to migrate
   codegen to use them once MIR has richer type information

**Problem 3: Bare name dispatch**

`dispatch.rs` maps bare names like `push` → `rask_vec_push`. This works only because
Vec is the only stdlib type with a `push` method that monomorphization emits. But it's
fragile.

Check what names the monomorphizer actually emits for stdlib method calls by examining
`compiler/crates/rask-mono/src/`. The fix depends on what format those names take:
- If mono emits `Vec_i32_push` → dispatch should match on `Vec_` prefix
- If mono emits bare `push` → dispatch needs a type-context mechanism (bigger change)

For now: verify the current bare-name scheme works for the validation programs. Add
a comment in dispatch.rs documenting which names are ambiguous and what the plan is.
Don't redesign the dispatch table — just make it work and document the limitations.

**Problem 4: Link all necessary runtime files**

If you find that the old runtime.c implementations are incomplete (missing functions
that dispatch.rs expects), you have two choices:
a. Add the missing functions to runtime.c (preferred — keeps linking simple)
b. Change link.rs to compile multiple .c files

If you go with (b), update `link_executable()` to:
```
cc runtime.c alloc.c vec.c string.c map.c pool.c obj.o -o bin
```
But beware of duplicate symbol errors from the old implementations in runtime.c.

**Testing:**

Create test .rk files in `/tmp/` and test end-to-end:

- `/tmp/test_stdlib_vec.rk`:
  ```rask
  func main() {
      const v = Vec.new()
      v.push(10)
      v.push(20)
      v.push(30)
      print(v.len())
      print("\n")
      print(v.get(1))
      print("\n")
  }
  ```
  Compile: `rask compile /tmp/test_stdlib_vec.rk -o /tmp/test_stdlib_vec`
  Expected: `3\n20\n`

- `/tmp/test_stdlib_string.rk`:
  ```rask
  func main() {
      const s = string.new()
      const greeting = string.concat("hello", " world")
      print(greeting)
      print("\n")
      print(string.len(greeting))
      print("\n")
  }
  ```

- `/tmp/test_stdlib_map.rk`:
  ```rask
  func main() {
      const m = Map.new()
      m.insert("key", 42)
      print(m.len())
      print("\n")
  }
  ```

For each: compile, run, verify output. If linking fails with "undefined symbol",
that tells you exactly which runtime function is missing.

Also run existing integration tests:
  `cargo test -p rask-cli --test compile_run`

**Build command fix:**

Check `compiler/crates/rask-cli/src/commands/build.rs`. It has hardcoded output paths
(`"output.o"` and `"output"`) — all packages write to the same file. Fix this to use
the package name or input filename, matching how `cmd_compile` works.

Read CLAUDE.md for project conventions before starting.
```

---

## Execution Notes

All 3 streams can start immediately. No ordering dependencies.

**File overlap risk:**
- Streams 1 and 2 both touch `rask-mir`, but different files: Stream 1 modifies `types.rs`
  and `lower/mod.rs`; Stream 2 modifies `stmt.rs`, `operand.rs`, and `lower/expr.rs`.
  Merge should be clean.
- Streams 2 and 3 both touch `rask-codegen`, but different files: Stream 2 modifies
  `builder.rs` (new statement handlers); Stream 3 modifies `module.rs` and `dispatch.rs`.
- Stream 3 is the only one touching `rask-cli` and `runtime/`.

**After all 3 merge:** compile the grep validation program natively as the integration test.

# Parallel Work Streams

Four independent streams that touch **different crates/directories** — no merge conflicts.

| Stream | Crate/Directory | Depends On |
|--------|----------------|------------|
| 1. C Runtime Library | `compiler/runtime/` | Nothing |
| 2. Codegen Completion | `compiler/crates/rask-codegen/` | ABI from #1 (can stub) |
| 3. Green Tasks Runtime | `compiler/crates/rask-rt/` | Nothing |
| 4. Build Pipeline | `compiler/crates/rask-cli/` | Nothing |

---

## Stream 1: C Runtime Library

**Directory:** `compiler/runtime/`
**Goal:** Implement all C-callable functions that compiled Rask programs link against.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to extend
the C runtime library at `compiler/runtime/runtime.c` (and add new files if needed).

Currently runtime.c has: print functions, exit/panic, and POSIX I/O wrappers
(open/close/read/write). Compiled Rask programs link against this.

Implement C runtime functions for:

**1. Heap Allocator**
- `rask_alloc(size: i64) -> ptr` — malloc wrapper, aborts on OOM
- `rask_realloc(ptr: ptr, new_size: i64) -> ptr` — realloc, aborts on OOM
- `rask_dealloc(ptr: ptr)` — free wrapper

**2. Vec<T> (type-erased, caller knows element size)**
- `rask_vec_new(elem_size: i64) -> ptr` — allocate Vec header {data, len, cap, elem_size}
- `rask_vec_push(vec: ptr, elem: ptr)` — copy elem_size bytes, grow if needed (double capacity)
- `rask_vec_pop(vec: ptr, out: ptr) -> i8` — copy last element to out, return 1 if ok, 0 if empty
- `rask_vec_get(vec: ptr, index: i64) -> ptr` — return pointer to element (bounds-check, panic on OOB)
- `rask_vec_len(vec: ptr) -> i64`
- `rask_vec_cap(vec: ptr) -> i64`
- `rask_vec_clear(vec: ptr)` — set len=0 (don't free)
- `rask_vec_free(vec: ptr)` — free data + header

Vec header struct: `{ void* data; int64_t len; int64_t cap; int64_t elem_size; }`

**3. String Operations (heap-allocated, UTF-8)**
- `rask_string_new() -> ptr` — allocate empty string {data, len, cap}
- `rask_string_from(src: ptr, len: i64) -> ptr` — copy from pointer
- `rask_string_len(s: ptr) -> i64`
- `rask_string_push_str(s: ptr, other: ptr)` — append (grow if needed)
- `rask_string_eq(a: ptr, b: ptr) -> i8` — byte equality
- `rask_string_clone(s: ptr) -> ptr` — deep copy
- `rask_string_free(s: ptr)` — free data + header
- `rask_string_as_ptr(s: ptr) -> ptr` — return data pointer (for I/O)
- `rask_string_slice(s: ptr, start: i64, end: i64) -> ptr` — new string from range

String header struct: `{ char* data; int64_t len; int64_t cap; }`

**4. Map<K,V> (string keys for now, type-erased values)**
- `rask_map_new(val_size: i64) -> ptr` — open-addressing hash map
- `rask_map_insert(map: ptr, key: ptr, val: ptr)` — key is null-terminated string
- `rask_map_get(map: ptr, key: ptr) -> ptr` — return pointer to value, or NULL
- `rask_map_remove(map: ptr, key: ptr) -> i8` — return 1 if found
- `rask_map_contains(map: ptr, key: ptr) -> i8`
- `rask_map_len(map: ptr) -> i64`
- `rask_map_free(map: ptr)` — free all entries + structure

**5. Pool<T> (handle-based arena)**
- `rask_pool_new(elem_size: i64) -> ptr` — pool with generation counters
- `rask_pool_alloc(pool: ptr, elem: ptr) -> i64` — returns handle (packed index+generation)
- `rask_pool_get(pool: ptr, handle: i64) -> ptr` — validated access (panic on stale handle)
- `rask_pool_remove(pool: ptr, handle: i64)` — invalidate handle, add to freelist
- `rask_pool_len(pool: ptr) -> i64`
- `rask_pool_free(pool: ptr)`

Handle encoding: lower 32 bits = index, upper 32 bits = generation.

**6. CLI args**
- `rask_args_count() -> i64`
- `rask_args_get(index: i64) -> ptr` — return pointer to null-terminated string

Update main() to store argc/argv for rask_args_* to use.

**Design constraints:**
- All functions use C ABI (no C++ name mangling)
- Pointer type = void* passed as int64_t from Cranelift
- Panic on invariant violations (OOB, stale handle) — don't return error codes
- Keep it simple: no custom allocators, no thread safety (add later)
- Every function needs a clear header comment with its signature

**Testing:** Create `/tmp/test_runtime.c` that #includes runtime.c and exercises each
function with assertions. Compile and run with `gcc -o /tmp/test_runtime /tmp/test_runtime.c && /tmp/test_runtime`.

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 2: Codegen Completion

**Directory:** `compiler/crates/rask-codegen/`
**Goal:** Handle all currently-skipped MIR statements and add stdlib function dispatch.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to complete
the Cranelift code generation backend in `compiler/crates/rask-codegen/`.

Read the existing code first:
- `src/module.rs` — module setup, runtime imports, orchestration
- `src/builder.rs` — MIR → Cranelift IR translation (the core)
- `src/types.rs` — type mapping
- `src/tests.rs` — existing tests

**Current gaps (all in builder.rs):**

1. **EnsurePush / EnsurePop (cleanup blocks)**
   Currently skipped (empty match arms ~line 241). Implement:
   - EnsurePush: record the cleanup block ID on a stack
   - EnsurePop: pop the stack
   - On function return paths, emit jumps through the cleanup chain before returning
   - This implements RAII — ensure blocks run on scope exit

2. **ResourceRegister / ResourceConsume / ResourceScopeCheck**
   Currently skipped (~line 241). These enforce linear resource types:
   - ResourceRegister: record that a resource exists at current scope
   - ResourceConsume: mark it consumed (ok to exit scope)
   - ResourceScopeCheck: verify all resources consumed (emit panic if not)

   For now: emit runtime checks that call `rask_assert_fail` if a resource
   leaks. Track via a bitmask in a local variable.

3. **PoolCheckedAccess**
   Currently returns error (~line 249). Implement:
   - Emit a call to `rask_pool_get(pool_ptr, handle)` (runtime function)
   - Store result pointer in dst local
   - Declare `rask_pool_get` as an import in module.rs

4. **CleanupReturn**
   Currently treated as normal return (~line 545). Implement:
   - Walk the cleanup chain, jumping to each cleanup block in LIFO order
   - After all cleanup blocks execute, perform the actual return

5. **Stdlib method dispatch**
   When MIR has `Call { func: "Vec_i32_push", args: [vec, elem] }`, codegen
   needs to map this to the C runtime function `rask_vec_push`.

   Implement a name-mapping layer:
   - Parse monomorphized names like `Vec_i32_push`, `string_len`, `Map_string_i32_get`
   - Map to runtime function names: `rask_vec_push`, `rask_string_len`, `rask_map_get`
   - Auto-declare these as imports in the Cranelift module
   - Handle the type-erased calling convention (pass elem_size where needed)

6. **Closure environments**
   Closures in MIR have a captured environment struct. Implement:
   - Stack-allocate the environment struct
   - Store captured variables into it
   - When calling a closure, pass the env pointer as first arg
   - When inside a closure body, load captures from the env pointer

**For each feature:**
- Add tests in `src/tests.rs` following the existing pattern
- Test both the happy path and error cases

**Runtime function signatures to declare** (these will exist in runtime.c):
```
rask_vec_new(elem_size: i64) -> i64 (ptr)
rask_vec_push(vec: i64, elem: i64) -> void
rask_vec_get(vec: i64, index: i64) -> i64 (ptr)
rask_vec_len(vec: i64) -> i64
rask_vec_free(vec: i64) -> void
rask_string_new() -> i64
rask_string_len(s: i64) -> i64
rask_string_push_str(s: i64, other: i64) -> void
rask_string_eq(a: i64, b: i64) -> i8
rask_string_clone(s: i64) -> i64
rask_string_free(s: i64) -> void
rask_map_new(val_size: i64) -> i64
rask_map_insert(map: i64, key: i64, val: i64) -> void
rask_map_get(map: i64, key: i64) -> i64
rask_map_len(map: i64) -> i64
rask_pool_new(elem_size: i64) -> i64
rask_pool_alloc(pool: i64, elem: i64) -> i64
rask_pool_get(pool: i64, handle: i64) -> i64
rask_pool_remove(pool: i64, handle: i64) -> void
rask_alloc(size: i64) -> i64
rask_dealloc(ptr: i64) -> void
```

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 3: Green Tasks Runtime (Phase B)

**Directory:** `compiler/crates/rask-rt/`
**Goal:** Implement green task runtime with M:N scheduling and async I/O.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to implement
Phase B of the concurrency runtime in `compiler/crates/rask-rt/`.

Read the existing Phase A code first — it has OS threads, channels, select, mutex,
shared state, and cancellation. Phase B adds green tasks on top.

Also read these specs for design context:
- `specs/concurrency/runtime-strategy.md` — Phase A/B strategy
- `specs/concurrency/async-runtime.md` — green task design
- `specs/concurrency/io-context.md` — I/O context design

**Implement:**

1. **Task type** (`src/task.rs`)
   - `Task` = stackless coroutine (state machine enum)
   - States: Ready, Running, Waiting(WaitReason), Completed(Result)
   - `WaitReason`: Channel, Timer, IO, Join
   - Task ID (u64), priority (u8), creation timestamp

2. **Work-stealing scheduler** (`src/scheduler.rs`)
   - N worker threads (default = num_cpus)
   - Each worker has a local deque (chase-lev work-stealing deque)
   - Global injection queue for new tasks
   - Steal order: local deque → random other worker → global queue
   - Worker parking when no work available (condvar wakeup)

3. **I/O event loop** (`src/io_loop.rs`)
   - Linux: epoll (wrap with `libc` crate)
   - Single I/O thread that polls and wakes tasks
   - Register interest: `io_register(fd, interest, task_id)`
   - Deregister on completion
   - Timer wheel for sleep/timeout (hierarchical timing wheel, 4 levels)

4. **Async I/O wrappers** (`src/async_io.rs`)
   - `async_read(fd, buf) -> Result<usize>` — register for read, yield, resume
   - `async_write(fd, buf) -> Result<usize>` — register for write, yield, resume
   - `async_accept(listener_fd) -> Result<fd>` — register, yield, resume
   - `async_connect(addr) -> Result<fd>`
   - These return immediately, task gets rescheduled when I/O is ready

5. **Task API** (`src/runtime.rs`)
   - `Runtime::new(config) -> Runtime`
   - `runtime.spawn(task) -> TaskHandle`
   - `runtime.block_on(task) -> T` — run until root task completes
   - `TaskHandle::join() -> T`
   - `TaskHandle::detach()`
   - `TaskHandle::cancel()`

6. **Integration with Phase A**
   - `spawn()` in Multitasking context → creates green task (not OS thread)
   - Channel send/recv → yield point (task suspends if would block)
   - `Thread.spawn()` still creates OS thread (escape hatch)
   - Select works across green tasks + OS threads

**Design constraints:**
- No function coloring — green tasks use explicit yield points, not async/await
- Tasks are non-preemptive — yield at I/O, channel ops, sleep, explicit yield
- Keep Phase A API unchanged — Phase B is an alternative runtime, selected via `using Multitasking`
- Use `libc` crate for epoll/kqueue, not tokio/mio
- Target Linux first (epoll), macOS later (kqueue)

**Testing:**
- Unit tests for work-stealing deque
- Unit tests for timer wheel
- Integration test: spawn 1000 tasks, each incrementing a shared counter
- Integration test: echo server with green tasks (spawn accept loop + per-connection handler)
- Benchmark: green task spawn/join latency vs OS thread

Read CLAUDE.md for project conventions before starting.
```

---

## Stream 4: Build Pipeline (compile + link + run)

**Directory:** `compiler/crates/rask-cli/`
**Goal:** Make `rask compile` and `rask run` work end-to-end for native execution.

### Prompt

```
You're working on the Rask programming language compiler. Your task is to make
`rask compile file.rk -o binary` and `rask run file.rk` produce working native
executables by wiring up the full pipeline.

Read the existing CLI code in `compiler/crates/rask-cli/src/`. The pipeline stages
already exist as separate crates — you need to connect them:

1. Lexer → Parser → Resolver → Type Checker → Ownership → Desugar → Hidden Params →
   Monomorphization → MIR Lowering → Codegen → Object file → Link → Executable

**Currently broken/missing pieces:**

1. **`rask compile` command**
   Wire up the full pipeline:
   - Run all frontend passes (lex → parse → resolve → typecheck → ownership)
   - Run desugar + hidden params
   - Run monomorphization (produces MonoProgram with layouts)
   - Run MIR lowering (produces MirProgram)
   - Run codegen (produces object file bytes)
   - Write object file to temp location
   - Compile `compiler/runtime/runtime.c` with cc crate (or shell out to gcc/clang)
   - Link object file + runtime into executable with cc crate
   - Output to `-o` path (default: input filename without extension)

2. **`rask run` command**
   Currently uses the interpreter. Add a `--native` flag:
   - `rask run file.rk` → interpreter (existing behavior, keep this as default)
   - `rask run --native file.rk` → compile to temp file, execute it, delete temp
   - Forward command-line args after `--` to the compiled program
   - Capture and display exit code

3. **`rask build` command**
   For multi-file projects (future):
   - Find all .rk files in current directory
   - Compile each, link together
   - For now: just make single-file compilation work reliably

4. **Error handling**
   - If any pipeline stage fails, show the error and exit 1
   - If gcc/clang isn't found, show a clear message
   - If linking fails (missing symbols), show which runtime functions are missing

5. **Cross-platform linking**
   - Linux: `cc -o output object.o runtime.c -lm`
   - macOS: same but may need `-Wl,-no_pie`
   - Detect cc/gcc/clang availability

**Testing:**
- Create test .rk files in `/tmp/` that exercise the compile pipeline:
  - `/tmp/test_compile_arithmetic.rk`: `func main() { println(1 + 2) }`
  - `/tmp/test_compile_strings.rk`: `func main() { println("hello") }`
  - `/tmp/test_compile_control.rk`: if/else, while loop
  - `/tmp/test_compile_functions.rk`: multi-function program
- For each: compile, run, verify output matches expected
- Add these as integration tests in the CLI crate

**Use the `cc` crate** for invoking the C compiler if it's already a dependency,
otherwise shell out to `cc` (the system C compiler).

Read CLAUDE.md for project conventions before starting. Look at how existing
commands (check, run, lint) are structured in the CLI crate and follow the same pattern.
```

---

## Execution Order

All 4 streams can start immediately. Streams 1 and 4 have the strongest coupling
(linking needs the C runtime), but Stream 4 can start by wiring the pipeline with
the existing minimal runtime.c.

Once Streams 1 + 2 + 4 converge: compile the grep validation program end-to-end.

<!-- id: conc.strategy -->
<!-- status: decided -->
<!-- summary: Phased runtime implementation — OS threads first, M:N scheduler later -->
<!-- depends: concurrency/async.md, concurrency/runtime.md, compiler/codegen.md -->

# Runtime Implementation Strategy

OS threads first. Full M:N scheduler later. Same programmer-facing semantics either way.

## Decision

| Rule | Description |
|------|-------------|
| **RS1: Two-phase approach** | Phase A targets OS threads (1:1). Phase B upgrades to M:N green tasks. Both implement `conc.async` semantics identically |
| **RS2: Semantic parity** | `spawn`, `join`, `detach`, `cancel`, channels, `select` — all work in both phases. Programs don't change |
| **RS3: Performance boundary** | Phase A handles ~10k concurrent tasks. Phase B targets 100k+ (per `conc.runtime/PC3`) |
| **RS4: No feature gating** | Phase A implements everything in `conc.async` — no deferred features. Only implementation strategy differs |

**Why not jump straight to M:N?** Building the `fiber_switch` assembly routines, work-stealing scheduler, pluggable reactor, and preemption machinery simultaneously is a recipe for debugging four things at once. OS threads let us validate the full concurrency API with a thin C runtime first.

## Phase A: OS Threads (1:1)

| Rule | Description |
|------|-------------|
| **A1: Thread per spawn** | `spawn(|| {})` creates an OS thread via `pthread_create` (`thread.c`) |
| **A2: Blocking I/O** | All I/O blocks the calling thread. No reactor, no parking |
| **A3: Real channels** | Channels use a ring buffer + mutex/condvar (`channel.c`). Blocking send/receive |
| **A4: Affine handles** | `TaskHandle` wraps a refcounted `TaskState*`. Runtime panic on drop (same as interpreter) |
| **A5: Block installs process-global slot** | `using Multitasking { ... }` fills the process-global runtime slot (`conc.runtime/R1`) even in Phase A — implementations ignore the slot's contents and block threads for I/O, but the CC1/CC2 scope check and C1 single-active-block invariant are enforced |
| **A6: ThreadPool real** | `ThreadPool` uses a real bounded thread pool |

### What `using Multitasking` does in Phase A

```rask
using Multitasking {
    let h = spawn(|| { work() })
    try h.join()
}
```

Compiles to (pseudocode):

```c
// Compiler output
RaskTaskHandle h = rask_spawn(work_fn, arg_ptr);  // thread.c: pthread_create
int64_t result = rask_join(h);                     // thread.c: pthread_join
```

The `using Multitasking` block installs the process-global runtime slot on entry, inserts `rask_block_wait()` on exit to drain all non-detached handles, and clears the slot. No hidden parameters anywhere — `spawn` and stdlib I/O read the slot directly.

### Runtime files

All C files live in `compiler/runtime/`.

| File | Provides | Notes |
|------|----------|-------|
| `green.c` | `spawn`, `join`, parking, timers | Linux: the M:N scheduler — see below |
| `fiber.c` | Fiber stacks and the context switch | x86_64 and aarch64, ELF and Mach-O |
| `green_threads.c` | The same entry points on OS threads | Off Linux (no reactor backend yet) and under `RASK_NO_GREEN` |
| `thread.c` | `Thread.spawn`, `rask_sleep_ns` | pthreads; `Thread.spawn` stays an OS thread by design |
| `channel.c` | `rask_channel_*` | Ring buffer + mutex/condvar; capacity=0 for unbuffered rendezvous |
| `sync.c` | `rask_mutex_*`, `rask_shared_*` | The `Mutex` and `Readers` strategies of `Shared<T, S>` |
| `sim.h` | The wait wrappers every task-to-task wait goes through | Park a fiber, park a sim task, or call pthreads |
| `atomic.c` | `rask_atomic_*` | `Atomic<T>` load/store/CAS |

## Where Phase B stands

On Linux, `using Multitasking(workers: n)` runs tasks as stackful fibers on n worker threads. A task that waits in a join, a channel operation, a `Shared` lock or a sleep parks and gives its worker to another task. `tests/soak_gate.sh` holds five programs to `workers + 1` threads, including a 2^14-task join tree and a producer/consumer pair on one worker; `tests/tsan_gate.sh` runs the concurrency suite under ThreadSanitizer with every switch annotated.

Not yet:

- **I/O parking.** Stdlib I/O still makes the blocking syscall, so a task blocked on a socket holds its worker. The reactor engines (`io_epoll_engine.c`, `io_uring_engine.c`) exist and workers poll them; nothing submits to them yet.
- **Worker compensation for blocking FFI** (`conc.phase-b/FFI3`) — not built, so a long C call holds its worker too.
- **Preemption** (`conc.runtime/P1-P3`). Switching is cooperative: a task that computes without waiting keeps its worker until it finishes.
- **macOS.** `green.c` needs a kqueue backend; until then macOS runs `green_threads.c`. The aarch64 switch is assembled for both ELF and Mach-O and has not run yet.
- **Sim on fibers.** Sim mode still runs one OS thread per task with a baton.
- **`select` parking.** A `select` with nothing ready yields and polls again rather than parking on its arms (#1342).
- **Stack overflow is an abort, not a panic.** Running into a fiber's guard page prints which task overflowed and aborts.

## Phase B: M:N Stackful Fibers

| Rule | Description |
|------|-------------|
| **B1: Full runtime.md** | Implements everything in `conc.runtime` — work-stealing scheduler, pluggable reactor, stackful fibers, signal-based preemption |
| **B2: Stackful fiber codegen** | No state-machine transform. Function bodies compile the same as in Phase A. Parking happens via `fiber_switch` calls inside stdlib I/O functions |
| **B3: Swap runtime internals** | `rask_spawn` switches from `pthread_create` to fiber allocation + queue push. API unchanged |
| **B4: Trigger** | Upgrade when: (a) Cranelift backend handles full control flow, (b) `fiber_switch` assembly routines are ready for the supported targets, and (c) real programs hit the ~10k thread ceiling |

### Migration path

No source changes. The C runtime files swap internals:

| Function | Phase A | Phase B |
|----------|-------------------|---------|
| `rask_spawn` | `pthread_create` (`thread.c`) | Allocate fiber stack from pool, `Task` struct, push to worker queue |
| `rask_join` | `pthread_join` + `TaskState` (`thread.c`) | Park fiber via `fiber_switch` or block thread (J1) |
| I/O calls | Blocking syscall | Non-blocking + reactor registration + fiber_switch on EAGAIN |
| `rask_channel_send` | Ring buffer + mutex (`channel.c`) | Ring buffer + waker (runtime.md/CH2); parks fiber when full |
| `rask_sleep` | `clock_nanosleep` (`thread.c`) | Timer wheel registration (runtime.md/TM3) |

### New compiler requirements for Phase B

| Requirement | Description | Spec reference |
|-------------|-------------|---------------|
| Preemption safe-point instrumentation | Insert a flag check in every function prologue | `conc.runtime/P3` |
| Cross-crate "reaches spawn" metadata | Per-public-function bit for CC2 scope check | `conc.phase-b/SC1` |
| Process-global slot install/uninstall | Already done in Phase A | `conc.strategy/A5` |

No state-machine codegen pass, no pause-point enumeration, no wide ABIs for indirect calls. The stackful-fiber model keeps Phase B's compiler additions minimal.

## What doesn't change between phases

| Aspect | Stays the same |
|--------|---------------|
| Programmer syntax | `spawn(|| {})`, `.join()`, `.detach()`, channels, `select` |
| Error types | `JoinError`, `SendError`, `ReceiveError`, `TimedOut` |
| Affine handle rules | Must consume via join/detach/cancel |
| `using` block scoping | Block exit waits for non-detached tasks |
| Channel semantics | Buffered/unbuffered, close-on-drop, backpressure |
| Context clauses | `using Multitasking`, `using ThreadPool` |

## Error Messages

```
ERROR [conc.strategy/RS3]: too many concurrent tasks
   |
   | 10,247 OS threads active (Phase A limit: ~10,000)
   |
WHY: Phase A uses OS threads. Each spawn() creates a real thread.

FIX: Reduce concurrent tasks, or wait for Phase B (green tasks).
```

## Edge Cases

| Case | Phase A | Phase B |
|------|---------|---------|
| 100k concurrent spawns | OS thread limit (~10k), panics | Works (120 bytes/task) |
| I/O in tight loop | Blocks thread (acceptable for <10k tasks) | Parks task, runs others |
| `join()` in async context | Blocks calling thread | Parks calling task |
| Nested `using Multitasking` | Compile error (same in both) | Compile error |
| `cancelled()` check | Works (AtomicBool) | Works (same mechanism) |

---

## Appendix (non-normative)

### Rationale

**RS1 (two-phase):** The interpreter already proves OS threads work for semantics validation. The compiled version needs a working backend before it can run fibers with context switches. Building them in sequence avoids coupling backend bugs with runtime bugs.

**RS4 (no feature gating):** Deferring features creates two languages. If Phase A skips `select` or channels, programs written against Phase A won't exercise the full API. Then Phase B ships with untested surface area.

**A5 (slot install even in Phase A):** Installing the process-global slot in Phase A validates the CC1/CC2 scope check and the block lifecycle (R1-R2). Phase A implementations ignore the slot's contents for I/O behavior (they always block), but the slot is still present so Phase B can drop in without changing any lowering.

### Implementation order within Phase A

1. `rask_spawn` + `rask_join` + `rask_detach` (minimal concurrency)
2. `rask_channel_*` (producer-consumer patterns)
3. `rask_cancel` + ensure hooks (resource safety)
4. `rask_select` (multiplexing)
5. `rask_sleep` + `rask_timeout` (timers)
6. `rask_mutex` + `rask_shared` (shared state)

Each step is independently testable. Step 1 alone enables `spawn(|| {}).detach()` and `try h.join()`.

### Risk: Phase A "good enough" trap

If Phase A handles most real programs, there's temptation to never build Phase B. Guard against this by:
- Documenting the ~10k thread limit prominently
- Including a validation program that requires >10k concurrent connections (HTTP server benchmark)
- Tracking Phase B as a blocking requirement for v1.0

### See Also

- `conc.async` — Programmer-facing concurrency semantics
- `conc.runtime` — Full M:N runtime specification (Phase B target)
- `conc.io-context` — I/O context detection and async/sync dispatch
- `comp.codegen/RT1-RT3` — Runtime library requirements

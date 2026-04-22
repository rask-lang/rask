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
| **RS3: Performance boundary** | Phase A handles ~10k concurrent tasks. Phase B targets 100k+ (per `conc.runtime/P3`) |
| **RS4: No feature gating** | Phase A implements everything in `conc.async` — no deferred features. Only implementation strategy differs |

**Why not jump straight to M:N?** Building the `fiber_switch` assembly routines, work-stealing scheduler, pluggable reactor, and preemption machinery simultaneously is a recipe for debugging four things at once. OS threads let us validate the full concurrency API with a thin C runtime first.

## Phase A: OS Threads (1:1)

| Rule | Description |
|------|-------------|
| **A1: Thread per spawn** | `spawn(|| {})` creates an OS thread via `pthread_create` (`thread.c`) |
| **A2: Blocking I/O** | All I/O blocks the calling thread. No reactor, no parking |
| **A3: Real channels** | Channels use a ring buffer + mutex/condvar (`channel.c`). Blocking send/recv |
| **A4: Affine handles** | `TaskHandle` wraps a refcounted `TaskState*`. Runtime panic on drop (same as interpreter) |
| **A5: Block installs process-global slot** | `using Multitasking { ... }` fills the process-global runtime slot (`conc.runtime/R1`) even in Phase A — implementations ignore the slot's contents and block threads for I/O, but the CC1/CC2 scope check and C1 single-active-block invariant are enforced |
| **A6: ThreadPool real** | `ThreadPool` uses a real bounded thread pool |

### What `using Multitasking` does in Phase A

```rask
using Multitasking {
    const h = spawn(|| { work() })
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

### Phase A runtime files

All C files live in `compiler/runtime/`.

| File | Provides | Notes |
|------|----------|-------|
| `thread.c` | `rask_spawn`, `rask_join`, `rask_detach`, `rask_cancel`, `rask_sleep` | pthreads, refcounted `TaskState` |
| `channel.c` | `rask_channel_*` | Ring buffer + mutex/condvar; capacity=0 for unbuffered rendezvous |
| `sync.c` | `rask_mutex_*`, `rask_shared_*` | `Mutex<T>` and `Shared<T>` wrappers |
| `atomic.c` | `rask_atomic_*` | `Atomic<T>` load/store/CAS |
| `green.c` | (stub) | Phase B target — work-stealing scheduler, not active |

### What this validates

- Full `conc.async` API surface (S1-S4, H1-H4, C1-C4, CH1-CH4, CN1-CN3)
- Process-global runtime slot install/uninstall (`conc.strategy/A5`, `conc.runtime/R1-R2`)
- Affine handle enforcement (runtime)
- Channel semantics (buffered, unbuffered, close-on-drop)
- `select` statement compilation
- `ensure` hooks on cancellation
- Error propagation through task boundaries

### What this defers

- Stackful fibers (runtime.md/T1-T3) — `green.c` stubbed, `fiber_switch` assembly not written
- Work-stealing scheduler (runtime.md/S1-S4) — `green.c` has the skeleton
- Reactor / epoll / io_uring (runtime.md/R1-R3) — `io_epoll_engine.c` and `io_uring_engine.c` exist but aren't wired
- Transparent I/O pausing (tasks block their OS thread instead)
- Timer wheel (uses `clock_nanosleep` for now)
- 100k+ concurrent task scalability

## Phase B: M:N Stackful Fibers

| Rule | Description |
|------|-------------|
| **B1: Full runtime.md** | Implements everything in `conc.runtime` — work-stealing scheduler, pluggable reactor, stackful fibers, signal-based preemption |
| **B2: Stackful fiber codegen** | No state-machine transform. Function bodies compile the same as in Phase A. Parking happens via `fiber_switch` calls inside stdlib I/O functions |
| **B3: Swap runtime internals** | `rask_spawn` switches from `pthread_create` to fiber allocation + queue push. API unchanged |
| **B4: Trigger** | Upgrade when: (a) Cranelift backend handles full control flow, (b) `fiber_switch` assembly routines are ready for the supported targets, and (c) real programs hit the ~10k thread ceiling |

### Migration path

No source changes. The C runtime files swap internals:

| Function | Phase A (current) | Phase B |
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
| Error types | `JoinError`, `SendError`, `RecvError`, `TimedOut` |
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
- `conc.hidden-params` — Hidden parameter compiler pass
- `comp.codegen/RT1-RT3` — Runtime library requirements

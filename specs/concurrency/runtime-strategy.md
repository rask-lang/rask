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

**Why not jump straight to M:N?** Building a closure-to-state-machine compiler transform, work-stealing scheduler, and reactor simultaneously is a recipe for debugging three things at once. OS threads let us validate the full concurrency API with a thin C runtime.

## Phase A: OS Threads (1:1)

| Rule | Description |
|------|-------------|
| **A1: Thread per spawn** | `spawn(|| {})` creates an OS thread via `pthread_create` (`thread.c`) |
| **A2: Blocking I/O** | All I/O blocks the calling thread. No reactor, no parking |
| **A3: Real channels** | Channels use a ring buffer + mutex/condvar (`channel.c`). Blocking send/recv |
| **A4: Affine handles** | `TaskHandle` wraps a refcounted `TaskState*`. Runtime panic on drop (same as interpreter) |
| **A5: Context parameter threaded** | `using Multitasking` still desugars to hidden `__ctx` parameter — but context is a marker, not a scheduler handle |
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

The `using Multitasking` block is a scope marker — the compiler uses it to permit `spawn()` calls and to insert `rask_block_wait()` on block exit for any non-detached handles. No scheduler handle is threaded as a hidden parameter at runtime; the context clause affects compilation only.

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
- Hidden parameter desugaring (`conc.strategy/A5`)
- Affine handle enforcement (runtime)
- Channel semantics (buffered, unbuffered, close-on-drop)
- `select` statement compilation
- `ensure` hooks on cancellation
- Error propagation through task boundaries

### What this defers

- Green tasks / stackless state machines (runtime.md/T1-T3) — `green.c` is a stub
- Work-stealing scheduler (runtime.md/S1-S4) — `green.c` has the skeleton
- Reactor / epoll / io_uring (runtime.md/R1-R3) — `io_epoll_engine.c` and `io_uring_engine.c` exist but aren't wired
- Transparent I/O pausing (tasks block their OS thread instead)
- Timer wheel (uses `clock_nanosleep` for now)
- 100k+ concurrent task scalability

## Phase B: M:N Green Tasks

| Rule | Description |
|------|-------------|
| **B1: Full runtime.md** | Implements everything in `conc.runtime` — scheduler, reactor, state machines |
| **B2: State machine transform** | Compiler pass converts closures to stackless state machines at pause points |
| **B3: Swap runtime internals** | `rask_spawn` switches from `pthread_create` to scheduler queue push. API unchanged |
| **B4: Trigger** | Upgrade when: (a) Cranelift backend handles full control flow, and (b) real programs hit the ~10k thread ceiling |

### Migration path

No source changes. The C runtime files swap internals:

| Function | Phase A (current) | Phase B |
|----------|-------------------|---------|
| `rask_spawn` | `pthread_create` (`thread.c`) | Allocate `Task`, push to worker queue |
| `rask_join` | `pthread_join` + `TaskState` (`thread.c`) | Park task or block thread (J1) |
| I/O calls | Blocking syscall | Non-blocking + reactor registration |
| `rask_channel_send` | Ring buffer + mutex (`channel.c`) | Lock-free ring buffer + waker (runtime.md/CH2) |
| `rask_sleep` | `clock_nanosleep` (`thread.c`) | Timer wheel registration (runtime.md/TM3) |

### New compiler requirements for Phase B

| Requirement | Description | Spec reference |
|-------------|-------------|---------------|
| State machine transform | Closure → enum with `poll()` method | `conc.runtime/T3` |
| Pause point detection | Identify I/O calls, channel ops, sleep | `conc.io-context` (new spec) |
| Context parameter insertion | Already done in Phase A | `conc.strategy/A5` |

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

**RS1 (two-phase):** The interpreter already proves OS threads work for semantics validation. The compiled version needs a working backend before it can do state machine transforms. Building them in sequence avoids coupling backend bugs with runtime bugs.

**RS4 (no feature gating):** Deferring features creates two languages. If Phase A skips `select` or channels, programs written against Phase A won't exercise the full API. Then Phase B ships with untested surface area.

**A5 (hidden parameter even in Phase A):** Threading `__ctx` even when it's just a marker validates the desugaring pass. If we skip it in Phase A and add it in Phase B, we're debugging desugaring and runtime simultaneously.

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

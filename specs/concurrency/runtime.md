<!-- id: conc.runtime -->
<!-- status: decided -->
<!-- summary: Async runtime implementation model - M:N scheduler, reactor, task state machines -->
<!-- depends: concurrency/async.md, memory/context-clauses.md, memory/resource-types.md -->

# Async Runtime Implementation Model

This document specifies the runtime mechanisms that implement the async semantics described in [async.md](async.md). Where async.md focuses on what programmers write, this spec explains how the runtime executes it.

**Target audience:** Compiler engineers, runtime implementers, performance engineers

**Relationship to async.md:** async.md defines the rules (S1-S4, H1-H4, C1-C4, etc.). This spec explains the data structures, algorithms, and protocols that enforce those rules.

---

## Overview

Rask's async runtime is an **M:N green task scheduler** with transparent I/O pausing. Key properties:

- **Green tasks**: Lightweight tasks (stackless state machines) multiplexed on OS threads
- **Work-stealing scheduler**: Per-thread FIFO queues with random victim stealing for load balance
- **Event loop**: Central reactor (epoll/kqueue) for I/O multiplexing
- **Context-aware I/O**: Stdlib functions detect `using Multitasking` context via hidden parameters
- **No function coloring**: Same function works in async and sync contexts
- **Affine handles**: Must join or detach (runtime panic if dropped)

**Current interpreter:** Uses OS threads for spawn(), not green tasks. No M:N scheduler or event loop. Full runtime planned for compiled version. See [§15](#implementation-notes-for-interpreter) for details.

---

## Task Representation

### Task Structure (T1)

```rust
Task<T> {
    state: AtomicU8,                    // TaskState enum (see T2)
    result: Mutex<Option<Result<T, JoinError>>>,  // Completion value
    waker: Mutex<Option<Waker>>,        // Reactor wake-up handle
    cancel_flag: AtomicBool,            // Cooperative cancellation (CN1)
    ensure_hooks: Mutex<Vec<EnsureHook>>, // Resource cleanup (mem.resources/R4)
    future: Pin<Box<dyn Future<Output = T>>>, // Compiler-generated state machine
    spawn_location: (&'static str, u32), // (file, line) for debug traces
}
```

**Field purposes:**

| Field | Purpose | Memory Cost |
|-------|---------|-------------|
| `state` | Atomic task lifecycle tracking | 1 byte |
| `result` | Storage for completion value or error | 16 bytes (Mutex + Option) |
| `waker` | Event loop writes this when I/O ready | 16 bytes |
| `cancel_flag` | Set by cancel(), checked at safe points | 1 byte |
| `ensure_hooks` | Cleanup functions run on unwind | 24 bytes (Vec overhead) |
| `future` | Compiled closure state machine | Variable (closure captures) |
| `spawn_location` | Debug info for stack traces | 16 bytes |

**Total base cost:** ~120 bytes + closure captures

**Example:** `spawn(\|\| { process_request(conn) })` captures `conn` (typically 16-32 bytes), so total task cost ~150 bytes. 100k concurrent tasks = 15MB (acceptable).

### Task State Machine (T2)

```
           spawn()
             ↓
         ┌─[Ready]──────┐
         │              │
    schedule()      steal()
         │              │
         ↓              ↓
     [Running] ──────────────→ [Waiting] ← I/O blocks
         │                          │
         │                     waker.wake()
         │                          │
         │                          ↓
         ├──────────────────────→ [Ready]
         │
    completion/panic
         │
         ↓
    [Complete]

    cancel() at any state → [Cancelled] → run ensures → [Complete]
```

**State transitions (atomic):**

| From | To | Trigger | Who |
|------|-----|---------|-----|
| — | Ready | spawn() | Spawner thread |
| Ready | Running | schedule() | Worker thread |
| Running | Waiting | I/O call blocks | Current worker |
| Waiting | Ready | Reactor wakes task | Reactor thread |
| Running | Complete | Task returns | Current worker |
| Running | Complete | Task panics | Current worker (after unwind) |
| Any | Cancelled | cancel() called | Canceller thread |
| Cancelled | Complete | Ensures finished | Current worker |

**Invariants:**
- State transitions are atomic and monotonic (no backwards transitions except Waiting → Ready)
- Result is written before state transitions to Complete (Release ordering)
- Only one thread polls a task at a time (queue ownership ensures this)

### Closure to State Machine Transform (T3)

**Compiler transformation:** Closure → state machine at pause points (I/O calls, channel ops).

**Example:**
```rask
spawn(|| {
    const file = try File.open("data.txt")
    const data = try file.read_all()
    process(data)
})
```

**Desugars to state machine:**
```rust
enum State {
    Start { },
    AwaitingOpen { open_future: IoFuture<File> },
    AwaitingRead { file: File, read_future: IoFuture<Vec<u8>> },
    Processing { data: Vec<u8> },
    Complete { result: Result<(), Error> },
}

impl Future for State {
    fn poll(&mut self, cx: &mut Context) -> Poll<Result<(), Error>> {
        loop {
            match self {
                Start => {
                    let fut = File::open("data.txt");
                    *self = AwaitingOpen { open_future: fut };
                }
                AwaitingOpen { open_future } => {
                    match open_future.poll(cx) {
                        Poll::Ready(Ok(file)) => {
                            let fut = file.read_all();
                            *self = AwaitingRead { file, read_future: fut };
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                AwaitingRead { file, read_future } => {
                    match read_future.poll(cx) {
                        Poll::Ready(Ok(data)) => {
                            *self = Processing { data };
                        }
                        Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                        Poll::Pending => return Poll::Pending,
                    }
                }
                Processing { data } => {
                    let result = process(data);
                    return Poll::Ready(Ok(result));
                }
                Complete { result } => unreachable!(),
            }
        }
    }
}
```

**Current interpreter:** No state machine transform. Closures execute on real OS thread stacks. Full transform planned for compiled version.

---

## Scheduler Algorithm

### Worker Thread Architecture (S1)

```
Runtime {
    workers: Vec<WorkerThread>,             // N OS threads (typically = num_cores)
    reactor: Arc<Reactor>,                  // Central I/O event loop
    global_queue: Arc<InjectorQueue<Task>>, // Overflow + external spawn
}

WorkerThread {
    local_queue: Worker<Task>,       // Bounded FIFO (1024 entries)
    stealers: Vec<Stealer<Task>>,    // Other workers' steal interfaces
    runtime: Arc<Runtime>,           // Back-reference for reactor access
}
```

**Initialization (C1 - `using Multitasking` block entry):**
1. Create Runtime instance
2. Spawn N worker threads (N = num_cores or `Multitasking(workers: N)`)
3. Set context parameter `__multitasking_ctx` to Runtime handle
4. Workers enter main loop (see S2)

**Shutdown (C4 - block exit waits for tasks):**
1. Track active tasks via `Arc<Task>` ref count
2. Block exit waits until all non-detached tasks complete
3. Send shutdown signal to workers
4. Workers drain local queues, then exit
5. Reactor thread shuts down

### Worker Main Loop (S2)

```rust
fn worker_loop(ctx: RuntimeContext) {
    loop {
        // 1. Try local queue (fast path)
        if let Some(task) = local_queue.pop() {
            poll_task(task, ctx);
            continue;
        }

        // 2. Try stealing from random victim
        if let Some(task) = try_steal_from_random() {
            poll_task(task, ctx);
            continue;
        }

        // 3. Try global injection queue
        if let Some(task) = global_queue.pop() {
            local_queue.push(task);
            continue;
        }

        // 4. Poll reactor with timeout
        if let Some(task) = reactor.poll_ready_tasks(timeout: 1ms) {
            local_queue.push(task);
            continue;
        }

        // 5. Check shutdown
        if ctx.should_shutdown() {
            break;
        }
    }
}

fn poll_task(task: Arc<Task>, ctx: RuntimeContext) {
    task.state.store(Running, SeqCst);

    let waker = waker_for_task(task.clone());
    let mut cx = Context::from_waker(&waker);

    match task.future.poll(&mut cx) {
        Poll::Ready(result) => {
            task.result.lock().unwrap().replace(result);
            task.state.store(Complete, Release);  // Release ensures result visible
            task.notify_waiters();
        }
        Poll::Pending => {
            task.state.store(Waiting, SeqCst);
            // Task parked, waker registered with reactor
        }
    }
}
```

**Performance:**
- Local queue pop: ~10ns (lock-free fast path)
- Steal attempt: ~200ns (CAS on victim's deque)
- Global queue: ~50ns (lock-free injection queue)
- Reactor poll: ~1µs (epoll_wait syscall)
- Task poll: ~50ns (state machine transition)

### Work Stealing Protocol (S3)

**Algorithm:**
1. Select random victim worker (not self)
2. Try to steal from victim's local queue tail (FIFO maintains order)
3. Steal half of victim's queue in one batch (amortize synchronization cost)
4. Push stolen tasks to own local queue
5. If steal fails, retry with different victim (up to N attempts)
6. If all steals fail, fall back to global queue or reactor

**Rationale:** Random victim selection is simpler than tokio's sophisticated targeting while still achieving good load balance. Stealing half the queue (batch) reduces synchronization overhead compared to single-task steals.

**Tradeoff accepted:** Potential cache misses from cross-thread task migration. I think this is acceptable because I/O-bound tasks (the common case) have little hot cache state. Can reconsider if profiling shows NUMA issues.

**Why not Go's approach (no stealing)?** Poor load balance when work is uneven. Example: One connection spawns 1000 tasks, others spawn 10 each. Without stealing, that worker is swamped while others idle.

### Spawn Flow (S4 - realizes conc.async/S1, S4)

```rust
func spawn<T>(closure: || -> T) -> TaskHandle<T> {
    // Check context (S1: spawn requires using Multitasking)
    const ctx = CONTEXT.get() else {
        panic!("spawn() requires 'using Multitasking' context")
    }

    // Transform closure to state machine (T3)
    const future = closure_to_state_machine(closure)

    // Allocate Task on heap
    const task = Arc::new(Task {
        state: AtomicU8::new(Ready),
        result: Mutex::new(None),
        waker: Mutex::new(None),
        cancel_flag: AtomicBool::new(false),
        ensure_hooks: Mutex::new(Vec::new()),
        future: Box::pin(future),
        spawn_location: (file!(), line!()),
    })

    // Push to current worker's local queue
    if local_queue.len() < 1024 {
        local_queue.push(task.clone())
    } else {
        // Queue full: push half to global, push new task to local
        const half = local_queue.drain(512)
        global_queue.push_batch(half)
        local_queue.push(task.clone())
    }

    // Return affine handle (S4, H1)
    return TaskHandle {
        task: task,
        consumed: false,
    }
}
```

**Cost:** ~100ns (Arc allocation + queue push)

**Current interpreter:** Creates OS thread via `std::thread::spawn`, not green task. No state machine. Returns `ThreadHandle` wrapping `JoinHandle`.

---

## I/O Integration

### Context Detection via Hidden Parameters (IO1, IO2)

I chose hidden parameters over thread-locals because it's explicit in the type system and consistent with `using Pool<T>` (mem.context-clauses).

**Mechanism:**

`using Multitasking { }` desugars to threading a hidden `RuntimeContext` parameter through all function calls within the block:

```rask
using Multitasking {
    const file = try File.open("data.txt")
}

// Desugars to:
with_runtime_context(RuntimeContext::new(), || {
    const file = try File.open("data.txt", __ctx: RuntimeContext)
})
```

**Stdlib I/O functions accept optional context parameter:**

```rust
func File::open(path: string, __ctx?: RuntimeContext) -> File or Error {
    if __ctx is Some(ctx) {
        // Async path: register with reactor, park task
        return ctx.runtime.register_io(
            || blocking_open(path),  // Syscall to execute when ready
            Interest::Readable,
        )
    } else {
        // Sync path: blocking syscall
        return blocking_open(path)
    }
}
```

**Key insight:** Same function, two paths. Context parameter presence determines behavior. This realizes conc.async/IO1 (transparent pausing) and IO2 (sync fallback) without function coloring.

**Compared to thread-locals:** This approach is cleaner because:
1. Context is explicit in function signatures (type-safe)
2. No hidden global state
3. Consistent with existing `using` clause mechanism (CC1-CC5 in mem.context-clauses)
4. Enables future extensions (multiple runtime contexts, migration)

**Tradeoff:** All stdlib I/O functions need to accept this parameter. I think that's acceptable because it's hidden from programmers (desugaring) and makes the contract explicit.

### Async I/O Flow (IO3)

**Example:** `const file = try File.open("data.txt")` in `using Multitasking` block

**Flow:**
1. `File.open` receives `__ctx: RuntimeContext`
2. Detects async context (parameter is Some)
3. Initiates non-blocking open syscall (O_NONBLOCK)
4. If syscall returns EAGAIN (not ready):
   - Register FD with reactor (`reactor.register(fd, Interest::Readable, current_task_waker)`)
   - Return `Poll::Pending` from future
   - Scheduler marks task as Waiting, switches to next task
5. OS signals FD ready → reactor's epoll_wait returns
6. Reactor looks up Waker for this FD, calls `waker.wake()`
7. Waker pushes task back to ready queue (state: Waiting → Ready)
8. Scheduler polls task again
9. I/O completes immediately (non-blocking read succeeds)
10. Task continues execution

**Latency:** ~1µs from I/O ready to task resume (one context switch)

**Current interpreter:** No async I/O. `File.open` always blocks the OS thread.

---

## Event Loop (Reactor)

### Reactor Structure (R1)

```rust
Reactor {
    poller: mio::Poll,                    // epoll (Linux) / kqueue (macOS) wrapper
    registrations: HashMap<RawFd, Waker>, // FD → task waker mapping
    wakeup_pipe: Pipe,                    // Wake idle workers (eventfd on Linux)
    ready_tasks: Mutex<VecDeque<Arc<Task>>>, // Tasks woken by I/O
}
```

**Why single central reactor:**
- Simpler than per-thread reactors (tokio's multi-reactor model)
- Fewer syscalls (one epoll_wait vs N)
- Easier to reason about correctness
- Sufficient for ~100k concurrent I/O operations/sec

**Bottleneck:** At very high I/O rates (>100k ops/sec), reactor becomes contention point. I think that's acceptable for initial implementation. Can upgrade to per-thread reactors if profiling shows this is a bottleneck in real workloads.

**Alternative considered (per-thread reactors):** Each worker owns an epoll/kqueue. Scales better but requires task→reactor affinity (tasks can't migrate between workers). Adds complexity for edge cases (task woken on different thread). Decided against for simplicity.

### Reactor Integration with Scheduler (R2)

**Worker loop integration:**

Workers poll reactor when no local work is available (S2 step 4). Reactor has dedicated thread OR workers poll in turns:

**Option chosen: Dedicated reactor thread** for consistent I/O latency:

```rust
fn reactor_loop(reactor: Arc<Reactor>) {
    loop {
        // Block until I/O ready or timeout
        let events = reactor.poller.poll(timeout: 1ms);

        for event in events {
            let fd = event.fd();
            if let Some(waker) = reactor.registrations.get(fd) {
                waker.wake();  // Pushes task to ready queue
            }
        }

        if reactor.should_shutdown() {
            break;
        }
    }
}
```

**Waker implementation:**

```rust
impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.task.state.store(Ready, SeqCst);

        // Push to random worker's queue (load balance)
        let worker = pick_random_worker();
        worker.local_queue.push(self.task.clone());

        // Wake worker if idle (via eventfd)
        worker.notify();
    }
}
```

**Cost:** ~500ns to wake task (atomic store + queue push + eventfd write)

### Registration Protocol (R3)

**I/O call registers interest:**

```rust
func TcpConnection::read(self, buf: &mut [u8], __ctx?: RuntimeContext) -> usize or Error {
    if __ctx is Some(ctx) {
        // Non-blocking read
        match blocking_read_nonblocking(self.fd, buf) {
            Ok(n) => return Ok(n),
            Err(EAGAIN) => {
                // Register with reactor
                ctx.runtime.reactor.register(
                    self.fd,
                    Interest::Readable,
                    current_task_waker(),
                );

                // Park task
                return Poll::Pending;
            }
            Err(e) => return Err(e),
        }
    } else {
        // Blocking read
        return blocking_read(self.fd, buf)
    }
}
```

**Deregistration:** When I/O completes or FD closed, remove from `registrations` map.

**Edge-triggered vs level-triggered:** Use level-triggered for simplicity (epoll EPOLLIN, kqueue NOTE_READ). Spurious wakeups are harmless (task polls again, gets EAGAIN, re-registers).

---

## Hidden Parameter Debuggability

### The Challenge (HP1)

The hidden `__ctx: RuntimeContext` parameter achieves "no function coloring" but creates debuggability issues:

**Problems:**
1. **Stack traces show invisible parameters** - `File.open(path, __ctx)` but user wrote `File.open(path)`
2. **Mental model mismatch** - Function signatures appear different than they are
3. **Error messages confusing** - "missing parameter __ctx" when user didn't write that parameter
4. **Third-party tools blind** - gdb/lldb don't understand hidden parameters

**Trade-off accepted:** Some "magic" in exchange for no function coloring. Make the magic as visible as possible through tooling.

### Tooling Requirements (HP2)

To make hidden parameters acceptable, Rask tooling MUST provide excellent support:

#### 1. Debugger Integration (HP2.1)

**Stack trace rendering:**
- Hide `__ctx` parameter by default in stack traces
- Show "async frame" markers instead:
```
Frame 0: process_request(conn)  ⟨async⟩
Frame 1: handle_connection(addr)  ⟨async⟩
Frame 2: main()
```

**Verbose mode (opt-in):**
```
Frame 0: process_request(conn, __ctx: RuntimeContext)
Frame 1: handle_connection(addr, __ctx: RuntimeContext)
Frame 2: main()
```

**GDB/LLDB integration:**
- Debug symbols mark hidden parameters with special attribute
- Custom pretty-printers for stack frames
- `info async` command shows task tree

**Implementation:** DWARF debug info extension + debugger plugin

#### 2. LSP/IDE Support (HP2.2)

**Hover on function call:**
```rask
using Multitasking {
    const file = try File.open("data.txt")
    //               ^^^^^^^^^^
}
```

Hover shows:
```
func File.open(path: string) -> File or Error

Context: Multitasking (async)
⟨pauses task on I/O⟩

Note: In async context, this function may pause the current task.
I/O operations register with reactor and yield to scheduler.
```

**Outside async context:**
```rask
func main() {
    const file = try File.open("data.txt")
    //               ^^^^^^^^^^
}
```

Hover shows:
```
func File.open(path: string) -> File or Error

Context: Sync (blocking)
⟨blocks thread on I/O⟩

Note: In sync context, this function blocks the thread.
Consider using Multitasking { } for concurrent I/O.
```

**Signature help:** Show both contexts in signature popup
```
File.open(path: string) -> File or Error
  Context-aware: blocks in sync context, pauses in async context
```

**Go-to-definition:** Jump to function, show both implementations:
```rask
func File.open(path: string, __ctx?: RuntimeContext) -> File or Error {
    if __ctx is Some(ctx) {
        // Async path (pauses task)
        ...
    } else {
        // Sync path (blocks thread)
        ...
    }
}
```

#### 3. Compiler Diagnostics (HP2.3)

**Explicit context flag (--explicit-context):**
```
$ rask check --explicit-context main.rk
```

Makes hidden parameters visible in error messages:
```
error: function File.open requires RuntimeContext parameter
  --> main.rk:10:11
   |
10 |     const file = try File.open("data.txt")
   |                      ^^^^^^^^^^
   |
note: missing 'using Multitasking' context
help: wrap in 'using Multitasking { }' block to provide context
```

**Standard mode:**
```
error: function File.open requires async context
  --> main.rk:10:11
   |
10 |     const file = try File.open("data.txt")
   |                      ^^^^^^^^^^
   |
help: wrap in 'using Multitasking { }' block for async I/O
```

#### 4. Linter Rules (HP2.4)

**Warn on I/O in tight loops:**
```rask
using Multitasking {
    for i in 0..1000 {
        const file = try File.open("data_{i}.txt")  // Warning
        process(file)
    }
}

warning: I/O operation in tight loop may cause scheduler thrashing
  --> main.rk:15:22
   |
15 |         const file = try File.open("data_{i}.txt")
   |                          ^^^^^^^^^
   |
note: each I/O call pauses task and switches context
help: consider batching operations or using ThreadPool for parallel file access
```

**Suggest ThreadPool for CPU work:**
```rask
using Multitasking {
    spawn(|| {
        for i in 0..1000000 {  // Warning
            compute_heavy(i)
        }
    })
}

warning: long-running CPU computation in async context
  --> main.rk:10:9
   |
10 |         for i in 0..1000000 {
   |         ^^^
   |
note: async tasks should yield frequently (I/O or cancellation checks)
help: consider using ThreadPool for CPU-bound work:
      using ThreadPool {
          ThreadPool.spawn(|| { ... })
      }
```

#### 5. Documentation (HP2.5)

**Spec requirement:** The language specification MUST explicitly document hidden parameters:

**In reference manual:**
> ### Context Parameters
>
> Functions that interact with runtime systems (I/O, async, pools) accept hidden context parameters. These parameters are automatically provided by `using` blocks and are invisible in function calls.
>
> Example: `File.open(path: string)` has hidden signature:
> ```rask
> func File.open(path: string, __ctx?: RuntimeContext) -> File or Error
> ```
>
> The `__ctx` parameter is provided by `using Multitasking { }` blocks.

**In tutorial:**
> Rask uses "context parameters" to achieve zero-cost abstraction over sync/async. You don't see these parameters in your code, but they control how functions behave. Your IDE and debugger understand these hidden parameters and show helpful hints.

### Implementation Checklist (HP3)

For Rask v1.0, the following MUST be implemented:

- [ ] Debugger support (stack trace hiding, async frame markers)
- [ ] LSP hover hints (show both sync/async contexts)
- [ ] Compiler flag `--explicit-context` for debugging
- [ ] Linter rules (I/O in loops, CPU in async)
- [ ] Documentation (reference manual, tutorial, spec)

**Partial completion unacceptable:** Without tooling, hidden parameters are just confusing magic. The language cannot ship without these tools.

---

## Handle Implementation

### TaskHandle Structure (H1 - realizes conc.async/H1-H4)

```rust
TaskHandle<T> {
    task: Arc<Task<T>>,   // Shared reference to task
    consumed: bool,       // Affine tracking
}

impl<T> Drop for TaskHandle<T> {
    fn drop(&mut self) {
        if !self.consumed {
            panic!("TaskHandle dropped without join() or detach() (conc.async/H1)");
        }
    }
}
```

**Affine enforcement:** Drop panics if handle not consumed. This realizes H1 (must join or detach).

**Why runtime check, not compile-time?** Current type system doesn't track linear resources statically. Compiler support planned for compiled version (similar to mem.resources/R1-R5). Runtime panic is sufficient for interpreter.

### Join Operation (H2 - realizes conc.async/H2, J1)

```rust
func TaskHandle::join(mut self) -> T or JoinError {
    self.consumed = true;  // Mark consumed

    // Context-dependent waiting (J1)
    if __ctx is Some(ctx) {
        // Async context: yield to scheduler while waiting
        loop {
            if self.task.state.load(SeqCst) == Complete {
                break;
            }

            // Park current task, scheduler runs others
            ctx.runtime.park_current_task();
        }
    } else {
        // Sync context: block thread on condvar
        let mut state = self.task.state.load(SeqCst);
        while state != Complete {
            let (lock, cvar) = &self.task.completion_notify;
            let _guard = cvar.wait(lock.lock().unwrap());
            state = self.task.state.load(SeqCst);
        }
    }

    // Take result (written before Complete per T2 invariant)
    return self.task.result.lock().unwrap().take().unwrap()
}
```

**Cost:**
- Ready task: ~20ns (atomic read + Mutex lock + take)
- Waiting task: ~1µs (park + context switch + wake)

**Blocking join from async context:** If join() called from within `using Multitasking` but task not ready, current task parks (yields). This allows other tasks to run. Prevents deadlock (unless circular dependency).

### Detach Operation (H3 - realizes conc.async/H3)

```rust
func TaskHandle::detach(mut self) {
    self.consumed = true;
    drop(self.task);  // Drop Arc, decrement ref count
    // Task continues running independently
    // Block exit still waits if task hasn't completed (C4)
}
```

**Fire-and-forget pattern:** Caller doesn't care about result. Task runs to completion, result discarded.

**Block exit behavior (C4):** Even detached tasks are tracked via Arc ref count. `using Multitasking` block exit waits until all tasks complete (ref count reaches 0). This ensures tasks don't outlive the runtime.

### Cancel Operation (H4 - realizes conc.async/H4, CN1-CN3)

```rust
func TaskHandle::cancel(mut self) -> T or JoinError {
    self.consumed = true;

    // Set cancel flag (CN1: cooperative)
    self.task.cancel_flag.store(true, Release);

    // Wait for task to exit (same as join)
    // Task checks flag at safe points (CN3)
    return self.join()
}
```

**Cooperative cancellation (CN1):** Task continues until next check point. Not preemptive. This ensures no torn state (atomicity at pause points).

**Check points (CN3):**
- I/O entry (`File.open`, `TcpConnection.read`, etc.)
- Channel operations (`send`, `recv`)
- Explicit `cancelled()` function call

**Ensure blocks run (CN2):** Before task completes, ensure hooks execute LIFO (see §7).

---

## Cancellation Protocol

### Cancel Flag and Check Points (CN1, CN3)

**Flag location:** `task.cancel_flag: AtomicBool` (1 byte)

**Check points in stdlib:**

```rust
func File::read(self, buf: &mut [u8], __ctx?: RuntimeContext) -> usize or Error {
    if __ctx is Some(ctx) {
        // Check cancel flag before I/O
        if ctx.current_task().cancel_flag.load(Relaxed) {
            return Err(JoinError::Cancelled)
        }

        // Proceed with I/O...
    }
    // ...
}

func Channel::send<T>(self, value: T, __ctx?: RuntimeContext) -> () or Error {
    if __ctx is Some(ctx) {
        if ctx.current_task().cancel_flag.load(Relaxed) {
            return Err(JoinError::Cancelled)
        }
    }

    // Proceed with send...
}

public func cancelled(__ctx?: RuntimeContext) -> bool {
    if __ctx is Some(ctx) {
        return ctx.current_task().cancel_flag.load(Relaxed)
    }
    return false
}
```

**Programmer check pattern:**

```rask
using Multitasking {
    spawn(|| {
        loop {
            if cancelled() {
                return Err(Cancelled)
            }

            // Do work
            process_chunk()
        }
    })
}
```

**Memory ordering:** Release write (set flag), Relaxed read (check). Flag visibility doesn't need sequential consistency—task checks at discrete points, not continuously. If task misses one check, it'll catch the next.

### Ensure Hook Execution (CN2 - realizes mem.resources/R4)

**Hook storage:**

```rust
struct EnsureHook {
    callback: Box<dyn FnOnce() -> Result<(), Error>>,
    location: (&'static str, u32),  // (file, line) for debug
}

// In Task<T>:
ensure_hooks: Mutex<Vec<EnsureHook>>  // LIFO order
```

**Registration (during `ensure` block entry):**

```rask
ensure {
    file.close()
}

// Desugars to:
__ctx.current_task().register_ensure_hook(|| file.close())
// ... rest of function ...
```

**Execution (task unwind on cancel/panic/return):**

```rust
fn unwind_task(task: &Task) {
    let hooks = task.ensure_hooks.lock().unwrap();

    // Execute LIFO (reverse registration order)
    while let Some(hook) = hooks.pop() {
        match (hook.callback)() {
            Ok(()) => continue,
            Err(e) => {
                // Log error, don't propagate (ensure failures don't abort unwind)
                log::error!("ensure hook failed at {}:{}: {}", hook.location.0, hook.location.1, e);
            }
        }
    }
}
```

**Guarantee (CN2):** Ensure blocks run even on cancellation or panic. Ensures resources cleaned up (file handles closed, locks released, etc.).

**Error handling:** If ensure block fails, error is logged but doesn't prevent other ensures from running. This matches Rust's drop semantics (drop can't propagate panics).

---

## Compile-Time Affine Checking

### Motivation (AC1)

**Current approach:** Runtime panic in `TaskHandle::drop` if handle not consumed.

**Problem:** This violates Rask's "mechanical safety" principle. Safety should be compile-time (by construction), not runtime (by detection).

**Goal:** Enforce at compile time that all TaskHandles are consumed via join/detach/cancel before going out of scope.

### Linear Type System (AC2)

**Approach:** Mark `TaskHandle<T>` as a **linear type** (must be used exactly once).

**Type system rule:**
```
If a value of linear type enters a scope, it must be consumed before exiting that scope.
```

**Implementation:** Flow-sensitive control-flow analysis tracks linear values through all execution paths.

### Control Flow Examples (AC3)

**Simple case (easy):**
```rask
const h = spawn(|| { work() })
// ERROR: handle not consumed
// help: call h.join(), h.detach(), or h.cancel()
```

**Branching (requires flow analysis):**
```rask
const h = spawn(|| { work() })
if condition {
    h.join()  // Consumed here
} else {
    h.detach()  // Consumed here
}
// OK: consumed in all branches
```

**Early return (error):**
```rask
func process() {
    const h = spawn(|| { work() })
    if error {
        return  // ERROR: handle not consumed on this path
    }
    h.join()
}
```

**Loop (error):**
```rask
for item in items {
    const h = spawn(|| { process(item) })
    // ERROR: handle goes out of scope without consuming
}

// Fix: consume in loop
for item in items {
    spawn(|| { process(item) }).detach()  // OK
}
```

### Compiler Analysis (AC4)

**Algorithm:**

1. **Build control-flow graph (CFG)** for each function
2. **Track linear values** created in function (spawn calls)
3. **For each linear value:**
   - Follow all paths through CFG
   - Check if value consumed on every path
   - Error if any path doesn't consume
4. **Define "consumed":**
   - `h.join()` - consumes h
   - `h.detach()` - consumes h
   - `h.cancel()` - consumes h
   - Passing to function with `take h: TaskHandle<T>` - consumes h
   - Returning from function - consumes h (caller's responsibility)

**Complexity:**
- Per-function analysis (no cross-function tracking needed)
- O(basic_blocks × linear_values) typically very fast
- No lifetime inference (simpler than Rust's borrow checker)

**Example error message:**
```
error[E0509]: linear value `h` not consumed
  --> src/main.rk:15:11
   |
15 |     const h = spawn(|| { work() })
   |           ^ handle must be consumed (join/detach/cancel)
16 |     if error_occurred {
17 |         return
   |         ------ handle dropped here without consuming
   |
help: add `h.detach()` before return, or move `h.join()` after if block
```

### Integration with Type System (AC5)

**Marker trait:**
```rask
// In stdlib
trait Linear {}

// TaskHandle implements Linear
extend TaskHandle<T> : Linear {
    // All methods either:
    // 1. Take `self` (consuming, like join/detach/cancel), or
    // 2. Take `&self` (non-consuming, like is_complete)
}
```

**Compiler recognizes `Linear` trait:**
- Values of Linear types tracked through control flow
- Compiler errors if Linear value dropped without consuming

**Other linear types (future):**
- Resource types (File, TcpConnection) - must close
- Lock guards (if we add non-closure-based locks) - must drop
- Ownership tokens (advanced use cases)

### Compilation Speed Impact (AC6)

**Analysis cost:**
- Each function analyzed once (no global analysis)
- O(n) in function size (linear scan of CFG)
- Similar to definite assignment analysis (C#, Swift, Kotlin)

**Comparison to Rust:**
- Rust: Borrow checker is O(n²) in worst case (lifetime constraints)
- Rask: Affine checking is O(n) (just track consumption, no lifetimes)

**Target:** This should NOT violate the "5× faster than Rust" compilation goal. Affine checking is much simpler than borrow checking.

**Measurement needed:** Benchmark on large codebases (10k+ functions) to validate.

### Phased Rollout (AC7)

**Phase 1 (interpreter, current):** Runtime panic
- Simple to implement
- Validates semantics
- Catches bugs (just at runtime)

**Phase 2 (compiler):** Static analysis with compile errors
- Implement linear type tracking
- Flow-sensitive CFG analysis
- Error messages with helpful suggestions

**Phase 3 (production):** Remove runtime checks
- Compiler guarantees safety
- No Drop check needed (already verified)
- Slight performance win (~5ns per drop)

**Hybrid approach (defense-in-depth):**
- Keep both compile-time and runtime checks initially
- Compile error catches most bugs
- Runtime panic catches edge cases (FFI, reflection, compiler bugs)
- Gain confidence in static analysis over time

### Related Work (AC8)

**Languages with similar systems:**
- **Rust:** `#[must_use]` attribute + compiler warnings
  - Rask is stricter: compile *error*, not warning
  - Rask tracks through control flow (more sophisticated)

- **Rust:** Linear types for Future (must .await)
  - Same principle: value must be consumed
  - Rask applies to TaskHandle

- **Swift:** Definite assignment analysis
  - Same CFG-based approach
  - Proves variables initialized before use
  - Rask: proves handles consumed before drop

- **Kotlin:** Exhaustive `when` checks
  - Compiler verifies all branches covered
  - Similar flow analysis

**Novelty:** Applying linear types to concurrency primitive (TaskHandle) at language level, not library level.

---

## Timer Support

### Overview (TM1)

Timers are fundamental async primitives. Every async runtime needs:
- `sleep(duration)` - pause task for fixed duration
- `interval(duration)` - periodic ticks
- `timeout(duration, operation)` - bound operation time
- Integration with `select` for timeout branches

**Current status:** Not implemented (neither in spec nor interpreter).

**Requirement:** Must be specified for v1.0. Cannot claim "practical coverage" without timers.

### API Design (TM2)

```rask
// In stdlib async module
public func sleep(duration: Duration) -> ()
public func timeout<T>(duration: Duration, operation: || -> T) -> T or TimedOut

// Timer struct for periodic ticks
public struct Timer {
    func interval(duration: Duration) -> TimerReceiver
    func after(duration: Duration) -> TimerReceiver
}

public struct TimerReceiver {
    func recv() -> () or RecvError  // Blocks until timer fires
}
```

**Usage examples:**

```rask
using Multitasking {
    // Sleep
    spawn(|| {
        print("Starting...\n")
        sleep(Duration.seconds(5))
        print("5 seconds later!\n")
    }).detach()

    // Timeout
    const result = timeout(Duration.seconds(10), || {
        try fetch_from_slow_api()
    })
    match result {
        Ok(data) => process(data),
        Err(TimedOut) => print("Operation timed out\n"),
    }

    // Interval
    const ticker = Timer.interval(Duration.milliseconds(100))
    spawn(|| {
        loop {
            try ticker.recv()
            update_stats()
        }
    }).detach()

    // Select integration
    const rx = channel.receiver
    const timer = Timer.after(Duration.seconds(30))
    result = select {
        rx -> msg: handle_message(msg),
        timer -> _: handle_timeout(),
    }
}
```

### Implementation Architecture (TM3)

**Approach:** Hierarchical timing wheel (separate from reactor).

**Why not reactor-integrated?**
- Reactor handles I/O events (file descriptors)
- Timers are pure time-based (no FDs)
- Mixing concerns complicates reactor logic
- Timer wheel is well-studied, efficient

**Timing wheel structure:**

```rust
TimerWheel {
    wheels: [Vec<TimerEntry>; 4],  // Hours, minutes, seconds, milliseconds
    resolution: Duration,           // Smallest tick (1ms)
    current_tick: AtomicU64,        // Monotonic tick counter
}

struct TimerEntry {
    deadline: u64,       // Absolute tick count
    waker: Waker,        // Task to wake
    repeating: bool,     // For intervals
    interval: Duration,  // For intervals
}
```

**Tick precision:**
- 1ms resolution (adequate for most use cases)
- Can upgrade to µs if profiling shows need
- Trade-off: Higher resolution = more CPU overhead

**Registration flow:**

1. `sleep(duration)` called
2. Calculate absolute deadline: `current_tick + duration_in_ticks`
3. Insert into appropriate wheel slot
4. Register waker for current task
5. Return Poll::Pending (task parks)

**Timer thread (dedicated):**

```rust
fn timer_thread(wheel: Arc<TimerWheel>) {
    let mut last_tick = Instant::now();

    loop {
        // Sleep for one resolution unit
        sleep(Duration::from_millis(1));

        // Advance tick
        let now = Instant::now();
        let elapsed = now.duration_since(last_tick);
        let ticks = elapsed.as_millis() as u64;

        for _ in 0..ticks {
            wheel.current_tick.fetch_add(1, SeqCst);

            // Collect expired timers
            let expired = wheel.pop_expired(wheel.current_tick.load(SeqCst));

            // Wake tasks
            for entry in expired {
                entry.waker.wake();

                // Re-insert if repeating
                if entry.repeating {
                    let next_deadline = wheel.current_tick.load(SeqCst) + entry.interval_ticks;
                    wheel.insert(TimerEntry { deadline: next_deadline, ..entry });
                }
            }
        }

        last_tick = now;

        if wheel.should_shutdown() {
            break;
        }
    }
}
```

**Cost per timer:**
- Registration: ~200ns (calculate deadline, insert into wheel)
- Fire: ~500ns (wake task, remove from wheel)
- Memory: ~40 bytes per timer

**Accuracy:**
- Best case: ±1ms (tick resolution)
- Typical: ±2-5ms (scheduler latency)
- Worst case: ±10ms (system load)
- Not suitable for real-time (use OS timers for that)

### Integration with Select (TM4)

`select` already sketches timer support (from select.md):

```rask
result = select {
    rx1 -> v: handle_v(v),
    Timer.after(5.seconds) -> _: timed_out(),
}
```

**Implementation:**
- `Timer.after()` returns `TimerReceiver` (channel-like)
- `select` registers interest in timer receiver
- When timer fires, waker wakes select task
- `select` polls timer receiver, gets result

**Uniform interface:** Timers look like channels from select's perspective.

### Interpreter Implementation (TM5)

**Current:** None.

**Proposed:** Use `std::thread::sleep` for blocking sleep:

```rust
func sleep(duration: Duration) {
    std::thread::sleep(duration.into());
}
```

**No timer wheel needed** in interpreter (OS threads can just block).

**Timeout via thread + channel:**

```rust
func timeout<T>(duration: Duration, operation: || -> T) -> T or TimedOut {
    let (tx, rx) = channel::<Result<T, ()>>(1);

    // Spawn operation
    let op_thread = thread::spawn(move || {
        let result = operation();
        tx.send(Ok(result)).ok();
    });

    // Spawn timeout watchdog
    let timeout_tx = tx.clone();
    let watchdog = thread::spawn(move || {
        thread::sleep(duration);
        timeout_tx.send(Err(())).ok();
    });

    // Wait for first result
    match rx.recv() {
        Ok(Ok(value)) => {
            watchdog.join().ok();  // Cancel watchdog
            Ok(value)
        }
        _ => {
            op_thread.join().ok();  // Best-effort cancel
            Err(TimedOut)
        }
    }
}
```

### Open Questions (TM6)

**Adjusting timers:**
- Should timers be cancellable?
- Should `Timer.after()` return a handle with `.cancel()` method?

**Recommendation:** Start without cancel. Add if users request it.

**Timer drift:**
- For long-running intervals, accumulate error
- Should we adjust deadlines to prevent drift?

**Recommendation:** Yes, adjust deadlines (calculate next as `last + interval`, not `now + interval`).

**High-resolution timers:**
- Support µs or ns resolution?

**Recommendation:** Start with 1ms. Add high-res API if profiling shows need (separate timer wheel with different resolution).

---

## Channels

### Channel Structure (CH1-CH4)

```rust
Channel<T> {
    buffer: Mutex<RingBuffer<T>>,           // Circular buffer (bounded capacity)
    capacity: usize,                        // 0 = unbuffered, N = buffered
    senders: AtomicUsize,                   // Track sender count for close detection
    receivers: AtomicUsize,                 // Track receiver count
    send_wakers: Mutex<VecDeque<Waker>>,    // Blocked senders (buffer full)
    recv_wakers: Mutex<VecDeque<Waker>>,    // Blocked receivers (buffer empty)
}
```

**Unbuffered (capacity = 0):** Synchronous rendezvous. Send blocks until receiver ready.

**Buffered (capacity = N):** Asynchronous up to N items. Send blocks when buffer full.

**Memory cost:** ~64 bytes + `sizeof(T) * capacity`

**Example:** `Channel<Request>::buffered(1024)` for request type (32 bytes) = 64 + 32*1024 = ~33KB.

### Send Flow (CH2, CH4)

```rust
func Sender::send(self, value: T, __ctx?: RuntimeContext) -> () or SendError {
    if __ctx is Some(ctx) {
        // Check cancel flag first (CN3)
        if ctx.current_task().cancel_flag.load(Relaxed) {
            return Err(SendError::Cancelled)
        }
    }

    let mut buf = self.channel.buffer.lock().unwrap();

    if buf.len() < self.channel.capacity {
        // Buffer has space
        buf.push(value);

        // Wake one blocked receiver if any
        if let Some(waker) = self.channel.recv_wakers.lock().unwrap().pop_front() {
            waker.wake();
        }

        return Ok(())
    } else if __ctx is Some(ctx) {
        // Buffer full, async context: park task
        drop(buf);  // Release lock before parking

        let waker = current_task_waker(ctx);
        self.channel.send_wakers.lock().unwrap().push_back(waker);

        return Poll::Pending;  // Scheduler marks task as Waiting
    } else {
        // Buffer full, sync context: block thread
        let (lock, cvar) = &self.channel.send_notify;
        let _guard = cvar.wait(buf);  // Wait on condvar

        // Retry (recursive call)
        return self.send(value, __ctx)
    }
}
```

**Backpressure:** Naturally emerges from bounded buffer. Fast sender blocks when buffer full until receiver drains.

**Waker queue fairness:** FIFO order (VecDeque) ensures senders/receivers wake in arrival order.

### Receive Flow (CH3)

```rust
func Receiver::recv(self, __ctx?: RuntimeContext) -> T or RecvError {
    if __ctx is Some(ctx) {
        if ctx.current_task().cancel_flag.load(Relaxed) {
            return Err(RecvError::Cancelled)
        }
    }

    let mut buf = self.channel.buffer.lock().unwrap();

    if buf.len() > 0 {
        // Buffer has items
        let value = buf.pop();

        // Wake one blocked sender if any
        if let Some(waker) = self.channel.send_wakers.lock().unwrap().pop_front() {
            waker.wake();
        }

        return Ok(value)
    } else if self.channel.senders.load(Relaxed) == 0 {
        // All senders dropped (CH3: close on drop)
        return Err(RecvError::Closed)
    } else if __ctx is Some(ctx) {
        // Buffer empty, async context: park task
        drop(buf);

        let waker = current_task_waker(ctx);
        self.channel.recv_wakers.lock().unwrap().push_back(waker);

        return Poll::Pending;
    } else {
        // Buffer empty, sync context: block thread
        let (lock, cvar) = &self.channel.recv_notify;
        let _guard = cvar.wait(buf);

        return self.recv(__ctx)
    }
}
```

**Close detection (CH3):** When last sender drops, `senders` count reaches 0. Receiver returns `Err(Closed)`.

**Non-linear handles (CH1):** Senders and receivers use Arc internally. Can be dropped without explicit close. Refcount tracks lifetime.

### Performance Characteristics (CH5)

| Operation | Cost | Scenario |
|-----------|------|----------|
| send (space available) | ~50ns | Mutex lock + push + unlock |
| send (buffer full, async) | ~1µs | Park + context switch + wake |
| recv (items available) | ~50ns | Mutex lock + pop + unlock |
| recv (buffer empty, async) | ~1µs | Park + context switch + wake |
| channel creation | ~200ns | Allocate + initialize atomics |

**Lock contention:** Under heavy load (many senders/receivers), Mutex becomes bottleneck. I chose lock-protected for simplicity. Can upgrade to lock-free ring buffer if profiling shows contention is a real problem. Rust's crossbeam crate provides battle-tested lock-free channels, could integrate if needed.

**Current interpreter:** Uses `std::sync::mpsc::SyncSender` (synchronous channels). No async parking, just thread blocking. Works correctly but not green-task-aware.

---

## ThreadPool (Separate from Multitasking)

### ThreadPool Structure (S2 - realizes conc.async/S2)

ThreadPool is simpler than Multitasking because it's CPU-bound (no I/O reactor needed).

```rust
ThreadPool {
    workers: Vec<JoinHandle<()>>,           // N OS threads
    work_queue: ArrayQueue<Job>,            // Lock-free bounded queue
    shutdown: AtomicBool,
}

type Job = Box<dyn FnOnce() -> Value + Send>;
```

**Differences from Multitasking:**

| Aspect | Multitasking | ThreadPool |
|--------|-------------|-----------|
| Task type | Green tasks (stackless) | Closures (run-to-completion) |
| Scheduler | Work-stealing per-thread queues | Single global queue |
| I/O | Reactor-integrated | No I/O support |
| Parking | Tasks can park/resume | Jobs run to completion |
| Use case | I/O-bound, high concurrency | CPU-bound, parallel compute |

### ThreadPool Spawn Flow (TP1)

```rust
func ThreadPool::spawn<T>(closure: || -> T, __ctx?: ThreadPoolContext) -> ThreadPoolHandle<T> {
    const ctx = __ctx else {
        panic!("ThreadPool.spawn() requires 'using ThreadPool' context")
    }

    // Package closure as Box<FnOnce>
    const result_slot = Arc::new(Mutex::new(None));
    const result_clone = result_slot.clone();

    const job = Box::new(move || {
        let result = closure();
        *result_clone.lock().unwrap() = Some(result);
    });

    // Push to global queue
    ctx.thread_pool.work_queue.push(job);

    // Wake one worker
    ctx.thread_pool.notify_one();

    // Return handle
    return ThreadPoolHandle {
        result_slot: result_slot,
        consumed: false,
    }
}
```

**Cost:** ~150ns (Arc allocation + box closure + queue push)

### Worker Loop (TP2)

```rust
fn thread_pool_worker(pool: Arc<ThreadPool>) {
    loop {
        // Block on queue (condvar)
        let job = pool.work_queue.pop_blocking();

        // Execute to completion
        job();

        // Check shutdown
        if pool.shutdown.load(Relaxed) {
            break;
        }
    }
}
```

**Simplicity rationale:** No reactor, no stealing, no parking. Jobs are CPU-bound and run quickly to completion. Global queue is sufficient because CPU work is uniform (unlike I/O tasks which have variable latency).

**Current interpreter:** This implementation is correct and complete. ThreadPool works as specified.

---

## Performance Characteristics

### Operation Costs (P1)

| Operation | Latency | Explanation |
|-----------|---------|-------------|
| `spawn()` | ~100ns | Allocate Task, push to queue |
| Task context switch | ~50ns | State machine poll, queue pop |
| Work steal | ~200ns | Random victim, CAS on deque |
| `join()` (task ready) | ~20ns | Atomic load + take result |
| `join()` (task waiting) | ~1µs | Park + context switch + wake |
| Reactor poll (no events) | ~1µs | epoll_wait timeout |
| Reactor poll (events) | ~500ns | epoll_wait + waker.wake() |
| Channel send (buffered, space) | ~50ns | Mutex + push |
| Channel send (full, park) | ~1µs | Park + switch + wake |
| I/O registration | ~500ns | Reactor hashmap insert + syscall |
| ThreadPool spawn | ~150ns | Box + queue push |

### Memory Costs (P2)

| Structure | Size | Notes |
|-----------|------|-------|
| Task | 120 bytes | Base (no captures) |
| Task with captures | 120 + N bytes | N = closure capture size |
| TaskHandle | 16 bytes | Arc + bool |
| Channel | 64 bytes | + capacity * sizeof(T) |
| Sender/Receiver | 16 bytes each | Arc to channel |
| Runtime | ~8KB | Workers + reactor + queues |

**Example calculations:**

- 100k concurrent tasks (no captures): 100k * 120 = 12MB
- 1M tasks: 120MB (starts to matter)
- 10k channels (capacity 100, 32-byte items): 10k * (64 + 100*32) = ~32MB

### Scalability Limits (P3)

**Vertical scaling (single machine):**

| Metric | Limit | Bottleneck |
|--------|-------|-----------|
| Concurrent tasks | 1M+ | Memory (120 bytes/task = 120MB) |
| I/O ops/sec | ~100k | Single reactor epoll_wait |
| Spawn rate | ~10M/sec | Task allocation rate |
| Channel throughput | ~10M msg/sec | Mutex contention at high concurrency |

**When to worry:**
- >100k concurrent tasks: Check memory usage (120MB per 1M tasks)
- >100k I/O ops/sec: Reactor becomes bottleneck, consider per-thread reactors
- >1M msg/sec on single channel: Lock-free ring buffer upgrade

**Current interpreter:** Much lower limits (OS thread limits, typically ~10k threads max).

---

## Performance Roadmap

### Evolution Plan for Scalability

The current spec describes a **prototype implementation** suitable for validation and medium-scale services. Production deployments targeting high-scale infrastructure require evolution:

#### Phase 1: Prototype (Current Spec)
**Target:** Validate design, support 80% of typical services

**Architecture:**
- Single central reactor
- Random-victim work stealing
- Lock-protected channels

**Scalability:**
- Concurrent tasks: 1M+ (memory-bound)
- **I/O ops/sec: ~100k** (reactor bottleneck)
- Spawn rate: ~10M/sec
- Channel throughput: ~10M msg/sec (single channel)

**Suitable for:**
- Web applications (HTTP APIs, services)
- CLI tools with background tasks
- Game servers (<100k connections)
- Data processing pipelines

**Not suitable for:**
- High-scale infrastructure (proxies, load balancers)
- High-frequency systems (trading, real-time bidding)
- Database engines (Redis, PostgreSQL scale)

**Documentation requirement:** Specs and user docs MUST clearly state the 100k ops/sec limit. Don't promise "practical coverage" without qualification.

#### Phase 2: Production Scale
**Target:** 1M+ I/O ops/sec, scale to infrastructure workloads

**Architecture changes:**
1. **Per-thread reactors** (like tokio)
   - Each worker owns epoll/kqueue instance
   - Task→reactor affinity (tasks pinned to thread)
   - Removes reactor contention bottleneck
   - Tradeoff: More complex, no cross-thread I/O wakeups

2. **NUMA-aware work stealing**
   - Prefer stealing from same socket
   - Reduces cross-socket cache misses
   - Significant for >32 core systems

3. **Lock-free channels** (optional)
   - Use crossbeam unbounded/bounded channels
   - Removes mutex contention at high throughput
   - Only if profiling shows lock contention is real bottleneck

**Design sketch (per-thread reactor):**
```rust
WorkerThread {
    local_queue: Worker<Task>,
    reactor: mio::Poll,                    // Owned reactor
    registrations: HashMap<RawFd, TaskId>, // Local FD mappings
    stealers: Vec<Stealer<Task>>,
}

// I/O registration now thread-local:
func register_io(fd: RawFd, interest: Interest) {
    let worker = current_worker();
    worker.reactor.register(fd, interest);  // No cross-thread coordination
}
```

**Migration path:**
- Introduce `Multitasking(reactor: PerThread)` option
- Default to `Single` (Phase 1), opt-in to `PerThread`
- Measure real workloads, validate improvement

#### Phase 3: Optimization & Tuning
**Target:** Fine-tune for specific workload classes

**Optimizations:**
1. **Reactor polling strategies**
   - Busy-polling for latency-critical (io_uring on Linux)
   - Adaptive timeouts based on I/O rate
   - Batched event processing

2. **Task pool recycling**
   - Reuse Task allocations (object pool)
   - Reduces allocator pressure
   - Typical saving: 50ns per spawn

3. **Specializations**
   - `CurrentThread` executor (no work-stealing, latency-sensitive)
   - `Dedicated` executor (one task per thread, game engine style)
   - `Priority` queues (high/low priority tasks)

4. **Compiler optimizations**
   - Inline state machine transitions
   - Devirtualize future poll calls
   - Reduce state machine size (merge states)

**Guideline:** Don't optimize until Phase 2 deployed and profiled with real workloads.

---

## Concurrency Safety

### Invariants (CS1)

**Task state machine:**
1. State transitions are atomic (AtomicU8 with SeqCst)
2. Result written before state transitions to Complete (Release ordering)
3. Only one thread polls a task at a time (queue ownership)
4. Waker can be called from any thread (Send + Sync)

**Channels:**
1. Buffer access protected by Mutex (mutual exclusion)
2. Waker queues protected by separate Mutex (avoid deadlock)
3. Sender/receiver counts atomic (drop detection thread-safe)

**Reactor:**
1. Registration map protected by Mutex or RwLock
2. Waker.wake() can be called from reactor thread safely (Send)

### Lock and Atomic Usage (CS2)

| Field | Synchronization | Ordering | Rationale |
|-------|----------------|----------|-----------|
| Task.state | AtomicU8 | SeqCst | Simplicity over performance; ensures all threads see consistent lifecycle |
| Task.result | Mutex | N/A | Rarely contended (written once, read once) |
| Task.waker | Mutex | N/A | Written once (registration), read once (wake) |
| Task.cancel_flag | AtomicBool | Release/Relaxed | Write visible before check; checks are frequent so Relaxed reads OK |
| Channel.buffer | Mutex | N/A | Send/recv need mutual exclusion (push/pop modify buffer) |
| Channel senders/receivers | AtomicUsize | Relaxed | Only need atomicity for count, not ordering |
| Reactor.registrations | Mutex/RwLock | N/A | Reads common (poll), writes rare (register) - RwLock better but Mutex simpler |

**Why SeqCst for Task.state?** Simplest correct ordering. Task state transitions must be globally consistent (Running → Waiting must be visible to all threads immediately). Can optimize to Release/Acquire later if profiling shows bottleneck.

**Why Relaxed for cancel_flag reads?** Flag checked at discrete points (I/O entry, channel ops), not continuously. If task misses one check, it catches the next. Relaxed sufficient for this usage pattern.

### Thread Safety (CS3)

**Send + Sync types:**

| Type | Send | Sync | Rationale |
|------|------|------|-----------|
| Task<T> | Yes | Yes | Arc-wrapped, atomics + mutexes internally |
| TaskHandle<T> | Yes | Yes | Wraps Arc<Task>, safe to move/share |
| Sender<T> | Yes | Yes | Arc<Channel>, safe to clone and send |
| Receiver<T> | Yes | Yes | Arc<Channel>, safe to clone and send |
| Waker | Yes | Yes | Designed to be sent to reactor thread |

**Memory ordering guarantees:**

```
Thread A (worker):             Thread B (reactor):
task.state = Waiting (Release)
  ↓
atomic store visible
                               ← waker.wake()
                                 task.state.load() == Waiting
                                 task.state = Ready (SeqCst)
  ↓
task.state.load() == Ready
task resumes
```

**Happens-before relationships:**
1. Spawn → Schedule: Task pushed to queue before any worker pops it
2. Complete → Join: Result written (Release) before state set to Complete; join() sees state (Acquire) then reads result
3. Cancel → Check: Flag set (Release) before task checks (Relaxed - but visibility eventually guaranteed)

---

## Integration with Other Specs

### Borrowing Constraints (I1 - mem.borrowing)

**No cross-task reference escape (mem.borrowing/B5):**

Rask's borrowing rules prevent references from outliving lexical scope. This means:

```rask
// Illegal: can't capture reference in task
const vec = Vec.new()
spawn(|| {
    vec.push(1)  // Error: vec reference can't escape to task
})

// Legal: capture value (move semantics)
const vec = Vec.new()
spawn(own || {  // 'own' captures by move
    vec.push(1)  // OK: task owns vec
})

// Legal: capture handle (copyable)
const pool = Pool.new()
const h = pool.add(Entity { hp: 100 })
spawn(|| {
    pool[h].hp -= 10  // OK: handle is Copy, pool context available
})
```

**Why this matters:** Green tasks can migrate between threads (work stealing). If tasks could hold references, those references might become invalid after migration. By forbidding reference capture, Rask ensures tasks are truly independent and safely migratable.

### Resource Type Integration (I2 - mem.resources)

**Ensure hooks on cancellation (mem.resources/R4):**

Resource types (File, TcpConnection, etc.) use `ensure` blocks for cleanup. These must run even on task cancellation:

```rask
spawn(|| {
    const file = try File.open("data.txt")
    ensure { file.close() }  // Registers cleanup hook

    // If task cancelled here, ensure still runs
    const data = try file.read_all()
    process(data)
})
```

**Runtime protocol:**
1. `ensure { }` block entry registers hook in `task.ensure_hooks`
2. On cancel, task checks flag, returns `Err(Cancelled)`
3. Task unwind executes hooks LIFO (§7)
4. File closed even though task didn't reach end

**Guarantee (CN2 + mem.resources/R4):** Resources cleaned up even on abnormal exit.

### Pool Handle Passing (I3 - mem.pools)

**Handles are Send + Sync (mem.pools/PH1):**

Pool handles are copyable opaque IDs (pool_id, index, generation). They can safely cross task boundaries:

```rask
using Pool<Entity>, Multitasking {
    const h = pool.add(Entity { hp: 100 })

    spawn(|| {
        pool[h].hp -= 10  // OK: handle is Copy, pool context threaded
    }).detach()
}
```

**Why this works:**
1. Handle is `Copy` (no ownership issues)
2. `pool` context threaded through `using Pool<Entity>` (mem.context-clauses/CC4)
3. Pool validation checks handle on access (generation check)
4. Pool internal Arc<Mutex<Vec>> is thread-safe

**Constraint:** Pool itself must be in scope (context clause ensures this).

### Comptime Restrictions (I4 - control.comptime)

**No async at comptime (control.comptime/CT33):**

```rask
const data = comptime {
    spawn(|| { fetch() })  // Compile error: spawn not allowed at comptime
}
```

**Rationale:** Comptime execution is deterministic and bounded. Async introduces non-determinism (timing, scheduling order) and unbounded execution. Forbidden to keep comptime predictable.

**Also forbidden at comptime:**
- Channels
- `using Multitasking`
- I/O (File, TcpConnection)
- Pools (control.comptime/CT20)

---

## Edge Cases

### Spawn Outside Context (E1 - realizes conc.async/S1)

```rask
func main() {
    spawn(|| { work() })  // Panic: no 'using Multitasking' context
}
```

**Error:** Runtime panic (interpreter) or compile error (compiled version with static context tracking).

**Message:** `"spawn() requires 'using Multitasking' context"`

### Handle Dropped Without Consume (E2 - realizes conc.async/H1)

```rask
func main() {
    using Multitasking {
        spawn(|| { work() })  // Handle not consumed
    }  // Panic on handle drop
}
```

**Error:** Runtime panic in TaskHandle::drop

**Message:** `"TaskHandle dropped without join() or detach() (conc.async/H1)"`

**Fix:** Always consume handles:
```rask
spawn(|| { work() }).detach()  // Or .join()
```

### Detached Task Outlives Runtime (E3 - realizes conc.async/C4)

```rask
using Multitasking {
    spawn(|| { long_work() }).detach()
}  // Block exits but runtime waits
```

**Behavior:** Block exit waits for all tasks (even detached) to complete. Arc ref count tracking ensures tasks don't outlive runtime.

**Why:** Tasks hold references to runtime (reactor, queues). If runtime destroyed while tasks running, tasks would crash. Waiting ensures clean shutdown.

**Timeout:** Can add timeout to block exit (future extension).

### Cancel Already-Complete Task (E4)

```rask
const h = spawn(|| { quick_work() })
h.join()  // Task completes
h.cancel()  // Error: handle already consumed
```

**Error:** Compile error (handle moved by join).

**Alternative:**
```rask
const h = spawn(|| { quick_work() })
h.cancel()  // Sets flag, waits for completion
```

**Behavior:** If task already complete when cancel() called, flag set but no-op. cancel() returns result immediately.

### Nested Spawn (E5)

```rask
using Multitasking {
    spawn(|| {
        spawn(|| { inner_work() }).detach()  // OK: nested spawn
        outer_work()
    }).detach()
}
```

**Behavior:** Works fine. Inner spawn pushes to current worker's queue. Context parameter threaded through.

**Caveat:** Inner task's lifetime independent of outer. If outer completes first, inner continues.

### Deadlock Detection (E6 - debug mode)

```rask
using Multitasking {
    const h1 = spawn(|| { h2.join() })
    const h2 = spawn(|| { h1.join() })
    h1.join()  // Deadlock: circular dependency
}
```

**Debug mode:** Runtime maintains task dependency graph (join edges). On join timeout (configurable, default 30s), detect cycles and panic with cycle trace:

```
Deadlock detected:
  Task A (spawned at main.rk:10) waits for Task B
  Task B (spawned at main.rk:11) waits for Task A
  Cycle: A → B → A
```

**Release mode:** No detection (overhead). Deadlock hangs forever (programmer responsibility).

### Reactor Thread Panics (E7)

If reactor thread panics (bug in reactor code), all tasks waiting on I/O become permanently stuck.

**Mitigation:**
1. Reactor thread has panic handler that logs stack trace
2. Runtime enters "poisoned" state
3. All I/O operations return `Err(RuntimePoisoned)`
4. Workers drain non-I/O tasks, then shut down

**User impact:** Partial availability (CPU tasks continue, I/O tasks fail).

### Worker Thread Panics (E8)

If worker thread panics while polling task:

**Behavior:**
1. Panic caught by worker loop's catch_unwind
2. Task marked as Complete with `Err(Panicked(message))`
3. Worker logs error, restarts loop
4. Other workers continue unaffected

**Why not restart worker?** Simpler to keep worker alive, just log error. Worker thread pool is fixed-size (N = cores).

---

## Debuggability

### Stack Trace Preservation (D1)

**Spawn location tracking:**

Each Task stores spawn location:
```rust
Task {
    spawn_location: (&'static str, u32),  // (file, line)
}
```

**Panic handler walks task tree:**

```
Task panicked at src/handler.rk:42: "connection failed"
  Spawned from src/server.rk:28 in handle_request
  Spawned from src/main.rk:15 in main
```

**Cost:** 16 bytes per task. I think that's acceptable for debuggability. Can be disabled in release mode if profiling shows memory is tight.

### Deadlock Detection (D2 - debug mode)

**Dependency graph:**

Runtime maintains `task_dependencies: HashMap<TaskId, Vec<TaskId>>` tracking join edges.

**On join timeout:**
1. Detect cycles in dependency graph (Tarjan's algorithm)
2. Print cycle with spawn locations
3. Panic with `DeadlockDetected` error

**Overhead:** ~8 bytes per join + cycle detection on timeout. Disabled in release mode.

### Runtime Metrics (D3)

**Exposed via stdlib:**

```rask
public func async.stats() -> RuntimeStats {
    return RuntimeStats {
        tasks_running: usize,
        tasks_ready: usize,
        tasks_waiting: usize,
        tasks_completed: u64,

        spawn_count: u64,
        join_count: u64,
        detach_count: u64,
        cancel_count: u64,

        queue_depths: Vec<usize>,  // Per-worker
        steal_attempts: u64,
        steal_successes: u64,

        reactor_events: u64,
        reactor_polls: u64,
        reactor_timeouts: u64,

        channel_sends: u64,
        channel_recvs: u64,
    }
}
```

**Use case:** Observability, profiling, tuning (adjust worker count, queue sizes).

### IDE Integration (D4 - ghost annotations)

**Pause points annotated (conc.async line 129):**

```rask
using Multitasking {
    const file = try File.open("data.txt")  // ⟨pauses⟩ ghost annotation
    const data = try file.read_all()        // ⟨pauses⟩
    process(data)                           // (no annotation - CPU work)
}
```

**Hover on spawn:**
- Estimated task lifetime (based on profiling)
- Capture list (what variables captured, sizes)

**Linter warnings:**
- Long-running compute without ThreadPool: "Consider using ThreadPool for CPU-bound work"
- Spawn in loop without bound: "Potential spawn bomb (unbounded spawns)"

---

## Implementation Notes for Interpreter

**Current interpreter** (compiler/crates/rask-interp) uses a **simplified threading model**:

| Feature | Spec (ideal runtime) | Interpreter (current) |
|---------|---------------------|----------------------|
| spawn() | Green tasks (stackless) | OS threads (std::thread::spawn) |
| Scheduler | M:N work-stealing | 1:1 thread-per-task |
| Event loop | Central reactor (epoll/kqueue) | None (blocking I/O) |
| Channels | Async with parking | Sync (std::mpsc::SyncSender) |
| Context detection | Hidden parameter | Thread-local (TODO) |
| Affine handles | Compile error + runtime panic | Runtime panic only |
| Cancellation | Cooperative flag + ensure | Not implemented |

**Why the gap?**

Interpreter is an MVP for validating language semantics, not full runtime. Building M:N scheduler, reactor, and state machine transform is a multi-month project best suited for compiled version.

**What works:**
- ThreadPool (correct implementation, uses OS thread pool)
- Channels (work correctly but are synchronous blocking)
- Thread.spawn (correct, creates OS threads)
- Basic spawn/join/detach (using OS threads)

**What doesn't work:**
- Scalability: 100k concurrent connections would create 100k OS threads (crash)
- Transparent I/O pausing: I/O blocks the entire OS thread
- Cancellation: No cancel flag, no ensure hook execution
- True "no function coloring": Same code but not truly async (just threaded)

**Path to full runtime:**

Planned for compiled version:
1. Closure → state machine transform (compiler pass)
2. M:N scheduler implementation (work-stealing queues)
3. Reactor (mio integration)
4. Context parameter desugaring (similar to Pool<T>)
5. Static affine handle checking (linear types in type system)

Interpreter remains as-is (OS threads) for semantics validation and examples.

---

## Design Rationale

### Why These Choices (DR1)

**Work-stealing FIFO over pure FIFO:**
- Pure FIFO (Go-style) is simpler but has poor load balance
- Example: One worker gets 1000 tasks, others get 10 each
- Without stealing, that worker is swamped while others idle
- Work-stealing adds ~200ns per steal but solves imbalance
- Random victim is simpler than tokio's sophisticated targeting

**Single reactor over per-thread reactors:**
- Per-thread scales better (no contention) but adds complexity
- Task→reactor affinity required (tasks can't migrate)
- Single reactor bottlenecks at ~100k I/O ops/sec
- I think 100k ops/sec is sufficient for initial implementation
- Can upgrade later if real workloads show bottleneck

**Bounded queues with overflow over unbounded:**
- Unbounded queues can grow without bound (memory bloat)
- Bounded (1024 entries) is predictable: 100k tasks = 100k/1024 ~= 100 queue chunks
- Overflow to global handles bursts gracefully
- Backpressure on spawn is unacceptable (surprising behavior)

**Stackless over stackful tasks:**
- Stackful is simpler (no compiler transform) but expensive
- 8KB stack * 100k tasks = 800MB (unacceptable)
- Stackless: 120 bytes * 100k = 12MB (acceptable)
- Requires compiler but that's planned anyway

**Lock-protected channels over lock-free:**
- Lock-free is faster under contention but much more complex
- Lock-protected is simple, predictable, correct
- Mutex contention only matters at >1M msg/sec on single channel
- I think that's rare; most programs use multiple channels
- Can upgrade to lock-free (crossbeam) if profiling shows need

**Hidden parameter over thread-local:**
- Thread-local is faster (~1ns) and simpler
- Hidden parameter is cleaner (explicit in type system)
- Consistent with existing `using Pool<T>` mechanism
- Enables future extensions (task migration, multiple runtimes)
- User chose this approach despite complexity

### Tradeoffs Accepted (DR2)

**Simplicity over maximum performance:**

I prioritize understandable, correct implementations over exotic optimizations. Rask's goal is "simple enough" not "maximally fast."

| Choice | Tradeoff | Reconsider if... |
|--------|----------|-----------------|
| Single reactor | Bottleneck at ~100k I/O ops/sec | Profiling shows this limit hit in real apps |
| Random steal victim | Poor cache locality | NUMA benchmarks show cross-socket stealing hurts |
| SeqCst for task state | Slower than Release/Acquire | Profiling shows atomic overhead significant |
| Lock-protected channels | Contention at >1M msg/sec | Apps routinely exceed this rate |
| FIFO queue (not priority) | Potential starvation | Long-running tasks block short ones |

**Philosophy:** Start simple. Iterate based on real-world feedback. Don't optimize for hypothetical workloads.

### Comparison with Other Runtimes (DR3)

| Aspect | Rask (this spec) | tokio (Rust) | Go runtime |
|--------|------------------|--------------|-----------|
| Task repr | Stackless state machine | Stackless (async/await) | Stackful (2KB) |
| Scheduler | Per-thread FIFO + random steal | Multi-layer sharded queues | Per-P FIFO, no steal |
| Queue type | Bounded (1024) with overflow | Unbounded | Unbounded |
| Reactor | Single central | Per-thread (io-uring optional) | Single netpoller |
| Context detection | Hidden parameter | Explicit async fn | Implicit (runtime checks) |
| Affine handles | Must join/detach (panic) | Must .await (compile error) | No enforcement (leak OK) |
| Cancellation | Cooperative flag | Drop future (immediate) | Context.Cancel (cooperative) |
| Channel impl | Lock-protected ring buffer | Lock-free crossbeam | Lock-free hchan |
| Work distribution | Good (stealing) | Excellent (sophisticated) | Fair (no stealing) |
| Complexity | Low | High (7 queue types) | Medium |
| Target use case | 80% of workloads | High-performance async | General concurrency |

**Summary:**
- Rask is simpler than tokio (fewer queue types, single reactor)
- More structured than Go (affine handles, no goroutine leaks)
- Balances ergonomics (no function coloring) with transparency (explicit contexts)

---

## Open Questions

These items need future resolution:

### Timer Support (OQ1)

**Status:** Needs specification (moved to proper section below)

**See:** Timer Support section added before Channels for full specification.

### Task Priorities (OQ2)

**Question:** Should tasks have priorities?

**Options:**
1. No priorities (YAGNI, simpler)
2. Static priorities (high/normal/low, separate queues)
3. Dynamic priorities (adjust based on runtime behavior)

**Recommendation:** Start with no priorities (option 1). Add as `spawn_priority(priority, || {})` later if needed.

**Spec impact:** If added, need priority queue structure, starvation prevention, API design.

### Work Queue Capacity Tuning (OQ3)

**Question:** Fixed 1024-entry queues or tunable?

**Current:** Fixed 1024 per worker thread.

**Alternative:** `using Multitasking(queue_size: 2048)` allows tuning.

**Recommendation:** Make tunable. Different workloads have different spawn burst patterns.

**Spec impact:** Minor (add parameter to Multitasking context).

### Reactor Thread Dedicated vs Polled (OQ4)

**Question:** Dedicated reactor thread or workers poll in turns?

**Current spec:** Dedicated thread (consistent latency).

**Alternative:** Workers poll reactor when idle (saves one thread).

**Recommendation:** Dedicated for simplicity. One extra thread (~8KB stack) is negligible.

**Spec impact:** None (implementation detail).

### Full Backtrace or File:Line Only (OQ5)

**Question:** Store full backtrace on spawn or just file:line?

**Current spec:** File:line (16 bytes per task).

**Alternative:** Full backtrace (40+ bytes, captures call chain).

**Recommendation:** File:line for release, full backtrace for debug mode.

**Spec impact:** Memory cost increases to ~160 bytes/task in debug mode.

---

## Summary

This spec defines the **M:N green task runtime** that realizes Rask's async semantics (conc.async).

**Key mechanisms:**
- Tasks: Stackless state machines (120 bytes)
- Scheduler: Work-stealing FIFO queues
- Reactor: Single central epoll/kqueue (Phase 1 prototype)
- I/O: Context-aware via hidden parameters (with tooling requirements)
- Handles: Affine (compile-time checking via linear types)
- Cancellation: Cooperative flag + ensure hooks
- Channels: Lock-protected ring buffers
- Timers: Hierarchical timing wheel (sleep, timeout, intervals)

**New sections added (from critical review):**
1. **Performance Roadmap** - Phase 1 (100k ops/sec prototype), Phase 2 (1M+ ops/sec production), Phase 3 (optimization)
2. **Compile-Time Affine Checking** - Linear type system with flow analysis for TaskHandle consumption
3. **Hidden Parameter Debuggability** - Tooling requirements (debugger, LSP, linter) for making hidden `__ctx` parameter acceptable
4. **Timer Support** - Full specification for sleep, timeout, and interval timers

**Design philosophy:** Simple, correct, "fast enough" for 80% of use cases. Iterate based on real-world feedback. **Prototype-first approach** with clear evolution plan.

**Current interpreter:** Uses OS threads (1:1 model), no M:N scheduler. Full runtime planned for compiled version.

**Critical requirements for v1.0:**
- ✅ Reactor bottleneck documented (100k ops/sec limit in Phase 1)
- ⚠️ Static affine checking (linear types in compiler)
- ⚠️ Debugger/LSP tooling (for hidden parameters)
- ⚠️ Timer implementation (sleep, timeout, intervals)

**Related specs:**
- [async.md](async.md) - Programmer-facing semantics
- [memory/pools.md](../memory/pools.md) - Handle validation (similar detail level)
- [memory/context-clauses.md](../memory/context-clauses.md) - Context parameter threading
- [memory/resource-types.md](../memory/resource-types.md) - Ensure hook integration

**Implementation roadmap:**

**Phase 1 (Prototype - Current Spec):**
- Single reactor (100k ops/sec)
- Runtime affine checks (panics)
- Basic tooling (compiler errors only)

**Phase 2 (Production - Planned):**
- Per-thread reactors (1M+ ops/sec)
- Static affine checking (linear types)
- Full tooling suite (debugger, LSP, linter)
- Timer wheel implementation

**Phase 3 (Optimization - Future):**
- NUMA-aware stealing
- Lock-free channels (if needed)
- High-resolution timers
- Specialized executors

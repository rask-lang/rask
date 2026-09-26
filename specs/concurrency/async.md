<!-- id: conc.async -->
<!-- status: decided -->
<!-- summary: Green tasks with must-use handles, no async/await split, explicit resource declaration -->
<!-- depends: memory/ownership.md, memory/resource-types.md -->

# Execution Model

Green tasks with must-use handles. No async/await split — the same function works whether called from a green task or sync code. Explicit resource declaration.

## Spawn Constructs

| Rule | Description |
|------|-------------|
| **S1: Green task** | `spawn(|| {})` creates a green task; must run with an active `using Multitasking` block in the process |
| **S2: Pooled thread** | `ThreadPool.spawn(|| {})` runs on thread pool; must run with an active `using ThreadPool` block |
| **S3: Raw thread** | `Thread.spawn(|| {})` creates OS thread; no runtime required |
| **S4: Must-use handle** | All spawn forms return handles that must be joined or detached — dropping one is a compile error |
| **S5: The task works on copies** | Every spawn form gives the task a copy of what its closure captured. A borrow can't cross (E0862) and a write the task never reads back is an error (E0896) — see [mem.closures](../memory/closures.md#spawn) for both. The value both sides need is a `Shared` reached through a clone, or the closure's return value |

Spawn functions do not appear in signatures. No function declares `using Multitasking` — the compiler infers which functions (transitively) need a runtime and checks callers against the current lexical scope. See [Runtime Scope](#runtime-scope) below.

```rask
func main() -> void or Error {
    using Multitasking {
        let listener = try TcpListener.bind("0.0.0.0:8080")

        loop {
            let conn = try listener.accept()
            spawn(|| { handle_connection(conn) }).detach()
        }
    }
}

func handle_connection(conn: TcpConnection) -> void or Error {
    let request = try conn.read()
    let user = try fetch_user(request.id)
    try conn.write(user.to_json())
}
```

## Must-Use Handles

| Rule | Description |
|------|-------------|
| **H1: Must consume** | Every spawn form returns `Handle<T>`, which must be joined or detached — compile error if unused |
| **H2: Join** | `h.join()` waits for result, returns `T or JoinError`, consumes handle |
| **H3: Detach** | `h.detach()` opts out of tracking (fire-and-forget), consumes handle |
| **H4: Cancel** | `h.cancel()` requests cooperative cancellation, waits for exit, returns `T or JoinError`: what the body returned, or its panic |
| **H5: One handle type** | A green task, a pooled job and an OS thread all hand back the same `Handle<T>`. What ran the work is the spawn call's business; the caller only ever waits, lets go, or asks it to stop |

<!-- test: skip -->
```rask
// Propagate errors
let h = spawn(|| { compute() })
let result = try h.join()

// Panic on task failure
let h = spawn(|| { work() })
h.join()!

// Handle explicitly
let h = spawn(|| { fallible_work() })
match h.join() {
    T as val                   => process(val),
    JoinError.Panicked(msg)    => println("task panicked: {msg}"),
}

spawn(|| { background_work() }).detach()

spawn(|| { work() })  // ERROR [conc.async/H1]: unused Handle
```

### Handle API

<!-- test: skip -->
```rask
@resource
struct Handle<T> { }

extend Handle<T> {
    func join(take self) -> T or JoinError
    func detach(take self)
    func cancel(take self) -> T or JoinError
}

enum JoinError {
    Panicked(string),  // task panicked with message
}
```

I had `TaskHandle` and `ThreadHandle` for a long time. They did the same three
things, and the split meant every piece of code that holds handles had to pick
one or be written twice. So there is one.

## Multiple Tasks

| Rule | Description |
|------|-------------|
| **M1: Join each** | A fixed set of tasks is joined handle by handle |
| **M2: Handle group** | `Handles<T>` holds handles for a count known only at run time: `new()`, `add(h)`, `join_all()`, `detach()`. `join_all` gives one `T or JoinError` per handle in add order; `detach` lets them all run on. The group is linear like the handles it holds: joined or detached exactly once |
| **M3: Plain Rask** | `Handles<T>` is written in Rask (`stdlib/async.rk`): a linked list built from an enum and `Heap`. A group of any other linear type is written the same way |

<!-- test: skip -->
```rask
let h1 = spawn(|| { work1() })
let h2 = spawn(|| { work2() })
let a = try h1.join()
let b = try h2.join()

mut pages = Handles<Page>.new()
ensure pages.detach()
for url in urls {
    pages.add(spawn(|| { return fetch(url) }))
}
let results = pages.join_all()
```

The group has no `spawn` of its own. `add` takes a handle from any spawn form,
so one method covers tasks, threads and pool jobs, and the spawn stays visible
at the call site.

I dropped the free `join_all(a, b)` and `select_first(a, b)`. A call that takes
any number of handles and hands back a tuple of their results can't be declared
in Rask, and the `Vec<Handle<T>>` version couldn't be called, since a `Vec`
can't hold a linear value. Joining two handles is two lines, and a loop is
what `Handles` is for. Racing tasks for the first result is still open; a
channel both send to covers it today.

## Runtime Scope

`using Multitasking(config) { ... }` is a block that opts the program into concurrency. It does not appear on function signatures — only as a block, typically near the top of `main`.

| Rule | Description |
|------|-------------|
| **C1: Single active runtime** | At most one `using Multitasking` block is active in the process at any time. Entering a second while one is active is an error |
| **C2: Process-global visibility** | While the block is active, every thread in the process can `spawn()` — the runtime lives in a process-global slot |
| **C3: Block-scoped lifetime** | The runtime starts on block entry and shuts down on block exit. No refcounting, no persistence across blocks |
| **C4: Drain on exit** | Normal block exit waits for all tasks (including detached ones) to finish before returning. Panic-unwinding the block signals cancellation to remaining tasks without draining them (see edge cases) |
| **C5: Sequential blocks OK** | After one block exits cleanly, another may be opened (new runtime, possibly different config). Non-overlapping only |
| **C6: Libraries don't install runtimes** | Only application code opens `using Multitasking`. Libraries call `spawn()` assuming the caller already did. Violation triggers C1's nesting error |

`workers: n` is how many tasks may be *running*, not a cap on threads. A task
blocked in `join` isn't running anything, so it doesn't hold a slot — without
that, `workers: 1` plus one nested spawn+join had nobody left to run the inner
task and hung.

All three runtimes read it that way: the green scheduler starts a replacement
worker for as long as a join lasts, and the two thread-backed ones (off Linux,
and the interpreter) hold n slots that a task takes before it runs and gives up
while it waits. Reusing a blocked worker's thread needs the fiber switch; until
then a nested join costs a thread per level, and the green scheduler reports a
deadlock rather than growing past 32 of them.

`using ThreadPool(config) { ... }` works the same way for CPU-bound pools. The two can be combined with `using Multitasking, ThreadPool { }` (installs both; teardown in reverse order on block exit).

<!-- test: parse -->
```rask
func main() {
    using Multitasking(workers: 4) {
        // all spawn() calls below, on any thread, use this runtime
        // body
    }
    // block exit: all spawned tasks drained, runtime shut down
}
```

### Checking for a runtime

"Is there a runtime?" is a **runtime check with a static fast path.** `spawn` reads the process-global runtime slot and panics if it's empty — that's the check that always holds. On top of it, the compiler proves the common cases ahead of time and reports them as compile errors, so most missing-scope bugs never reach a running program. Dynamic dispatch is where the static path gives up.

The static path works from inference: the compiler figures out which functions transitively reach `spawn` through their call graph. That inference is **internal compiler metadata** — users write no annotations on signatures.

| Rule | Description |
|------|-------------|
| **CC1: Direct spawn check** | A lexical `spawn()` call outside any `using Multitasking` block, in a function nothing calls — the entry point, a `test` block, a `@test` function → compile error at the `spawn` |
| **CC2: Inferred-requirement check** | A call to any function inferred as requiring the runtime, lexically outside any block → compile error at the call |
| **CC3: Runtime check** | The check the other two are an optimization of. Where the call target isn't statically known — a closure stored and called across block boundaries, trait-object dispatch, FFI calling in — `spawn` finds the slot empty and panics with a clear message |

CC1 and CC2 split by *who has to open the block*, not by how far away the `spawn` is. A function that spawns and leaves the scope to its caller is the normal shape — `http.serve` spawns per connection — so blaming its definition would make it unwritable. The error goes to the call site, which is where the block belongs. CC1 is what's left: a root nothing calls, so there is no call site to point at.

So the honest summary: direct calls and ordinary call chains are caught at compile time; anything reached through a stored closure, an `any Trait`, or an FFI entry point is caught on the first `spawn`, at runtime. A program can't spawn without a runtime either way — what varies is whether you find out before or after you run it.

Inference is invisible in source: writing or reading a function's body never involves Multitasking annotations. Users see the compile error at the **call site** ("calling `X` requires a `using Multitasking` scope; `X` needs it because it calls `spawn` at `f.rk:42`"), not at the definition.

## I/O Model

| Rule | Description |
|------|-------------|
| **IO1: Transparent pausing** | Stdlib I/O pauses the task, not the thread — no `.await` needed |
| **IO2: Sync fallback** | Outside any `using Multitasking` block, I/O blocks the calling thread |

```rask
func process_file(path: string) -> Data or Error {
    let file = try File.open(path)
    let contents = try file.read_bytes()
    parse(contents)
}
```

I/O flow: function calls stdlib → stdlib issues non-blocking syscall → scheduler parks task → other tasks run → I/O completes → task wakes. IDEs show pause points as ghost annotations: `⟨pauses⟩`.

## Join Semantics

| Rule | Description |
|------|-------------|
| **J1: Context-dependent** | `.join()` pauses the green task (scheduler runs others) or blocks the thread (sync mode) |

| Calling from | `.join()` behavior |
|--------------|-------------------|
| Green task | Pauses task (scheduler runs others) |
| Sync mode | Blocks thread |

**Error handling:**
```rask
try h.join()          // propagate JoinError
h.join()!             // panic if task panicked
match h.join() { }    // explicit handling
```

## Cancellation

| Rule | Description |
|------|-------------|
| **CN1: Cooperative** | Cancellation sets a flag; the work checks `cancelled()`. Same for a task, a pooled job and an OS thread: `cancelled()` reads the flag of whichever one is running it |
| **CN2: Ensure runs** | `ensure` blocks always run, even on cancellation |
| **CN3: A cancel ends a wait** | A task parked in a channel `receive` or `send`, a `sleep`, or a socket call wakes when it's cancelled, and the call returns `Cancelled`: `ReceiveError.Cancelled`, `SendError.Cancelled`, `SysError.Cancelled`, `IoError.Cancelled`. A call that doesn't need to wait completes: a value already in the channel is received. Joins and lock waits keep waiting, since what they wait for ends by its own code anyway |
| **CN4: No kill at pause points** | Cancellation never terminates a task at a suspension point. A cancelled task always resumes and exits through its own control flow — the flag check or the `Cancelled` error return. Preemption pauses tasks, never kills them |
| **CN5: The body's ending is the answer** | `cancel()` and `join()` return what the body returned, or its panic. Cancellation is not a third way to end: a body that stops early says so in its own return type |

CN4 is what keeps invisible suspension safe around locks: a lock held across a pause is released only by the holder's own block exit or panic unwind — there is no third "died while suspended" path (`ctrl.panic/LK4`).

<!-- test: skip -->
```rask
let h = spawn(|| {
    let file = try File.open("data.txt")
    ensure file.close()

    mut done = 0
    while !cancelled() {
        do_work()
        done += 1
    }
    return done
})

sleep(5.seconds)
let finished = try h.cancel()   // how far it got
```

A cancel reaches a task that is waiting, not only one that polls: the wait
ends with `Cancelled`, and the task carries on through its own code. Without
that, `cancel()` on a task parked in a receive waited forever.

`JoinError.Cancelled` used to exist, and `cancel()` answered with it whatever
the body did. That threw away a value the task had already produced, and when
that value is linear (a `File`, a `Handle`), nothing could close it. A body
that sees `cancelled()` returns like any other; if the caller needs to tell
"stopped early" from "finished", the body's return type says so.

## Channels

| Rule | Description |
|------|-------------|
| **CH1: Non-linear** | `Sender<T>` and `Receiver<T>` can go out of scope without explicit close |
| **CH2: Buffered/unbuffered** | `Channel<T>.unbuffered()` (sync) or `Channel<T>.buffered(n)` (async buffer) |
| **CH3: Close on scope exit** | Sender/receiver going out of scope implicitly closes; errors silently ignored |
| **CH4: Explicit close** | `tx.close()` / `rx.close()` return `Result` for error handling |

<!-- test: skip -->
```rask
mut (tx, rx) = Channel<Message>.buffered(100)

let producer = spawn(|| {
    for msg in generate_messages() {
        try tx.send(msg)
    })
}

let consumer = spawn(|| {
    loop {
        let r = rx.receive()
        if r? as msg { process(msg) } else { break }
    }
})

try producer.join()
try consumer.join()
```

### Channel Operations

| Operation | Returns | Description |
|-----------|---------|-------------|
| `tx.send(val)` | `void or SendError` | Send value, pauses/blocks if full |
| `rx.receive()` | `T or ReceiveError` | Receive value, pauses/blocks if empty |
| `tx.close()` | `void or CloseError` | Explicit close with error handling |
| `rx.close()` | `void or CloseError` | Explicit close with error handling |
| `tx.try_send(val)` | `void or TrySendError` | Non-blocking send |
| `rx.try_receive()` | `T or TryReceiveError` | Non-blocking receive |

### Buffered Items on Close

| Scenario | Behavior |
|----------|----------|
| Sender closed, buffer has items | Items remain — receivers can drain |
| All senders closed | Channel closed for writing, readable until empty |
| Receiver closed, buffer has items | Items discarded (lost) |
| All receivers closed | Senders get a `Closed` error on next send |

## Error Messages

```
ERROR [conc.async/H1]: unused Handle
   |
12 |  spawn(|| { work() })
   |  ^^^^^^^^^^^^^^^^ Handle must be joined or detached
```

```
ERROR [conc.async/CC1]: `spawn` needs a `using Multitasking { }` scope
   |
5  |  spawn(|| { fetch(url) })
   |  ^^^^^^^^^^^^^^^^^^^^^^^^ no block installs a runtime for this task

FIX: wrap the caller chain in `using Multitasking { ... }`, typically near main:

    func main() {
        using Multitasking {
            spawn(|| { fetch(url) }).detach()
        }
    }
```

```
ERROR [conc.async/CC2]: calling `fetch_page` requires a Multitasking scope
   |
12 |  fetch_page(url)
   |  ^^^^^^^^^^ this function transitively requires a runtime
   |
NOTE: `fetch_page` reaches `spawn` at stdlib/http.rk:42

FIX: wrap the caller chain in `using Multitasking { ... }`.
```

```
RUNTIME PANIC: spawn() called with no active `using Multitasking` scope

This can happen when:
  - A closure containing spawn is stored and called outside a block
  - A trait object dispatches to an impl that spawns
  - FFI calls back into Rask outside any scope

Install a `using Multitasking { ... }` block that encloses the call.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Direct `spawn` in a root (entry point, `test` block, `@test` function) outside any block | CC1 | Compile error at the `spawn` |
| Direct `spawn` in an ordinary function | CC2 | No error here — reported at each call site outside a block |
| Call to function transitively reaching `spawn`, outside any block | CC2 | Compile error at the call |
| Closure stored / trait object dispatch reaches `spawn` outside a block | CC3 | Runtime panic — target not statically known |
| `.join()` on cancelled task | H2, CN5 | Returns what the body returned when it stopped, or its panic |
| Cancelled while parked on I/O | CN3, CN4 | Task resumes; the pending operation returns `Cancelled`; task exits via its own control flow, ensures run |
| Cancelled while holding a lock | CN4 | No forced release — the lock releases when the task's own exit path leaves the block (`ctrl.panic/LK4`) |
| Panic-unwind of `using` block with tasks still pending | C4 | Cancellation signalled, no drain. A task that never reaches another check point never runs again — its ensures are skipped and locks it held stay held. Teardown of a dying runtime, not a state the program continues from |
| Channel send after all receivers closed | CH3 | Returns `Closed` error |
| Cancelled while an unbuffered send waits for its receiver | CN3 | The offer is withdrawn and `send` returns `Cancelled`, unless a receiver already took the value, in which case it was sent |
| Cancelled while `select` waits | CN3 | Ends with `SelectError.Cancelled` (conc.select/CL4) |
| Nested `using Multitasking` blocks | C1 | Error — second `enter` aborts (compile error if lexically nested, runtime panic otherwise) |
| Library opens `using Multitasking` while app already did | C6 | Falls under C1 — runtime panic |
| Test block spawns | C6 | Tests are application code — the test opens its own `using Multitasking { }`; the runner serializes runtime-holding tests to respect C1 (`std.testing/T17–T19`) |
| Detached task outlives `using` block body | C4 | Block exit still drains detached tasks. Truly outliving the block is impossible |

---

## Appendix (non-normative)

### Rationale

**S4 (must-use handles):** I wanted compile-time tracking of spawned tasks. Go's fire-and-forget `go` is ergonomic but loses track of goroutines — forgotten tasks are silent bugs. Must-use handles make the choice explicit: `.join()` or `.detach()`. (In type theory these are called "affine types" — values that must be used at most once.)

**IO1 (transparent pausing):** No async/await split means no ecosystem split. The same function works whether called from a green task or sync context. IDEs show pause points — transparency through tooling, not syntax.

**CH1 (non-linear channels):** Channels aren't `@resource` types. Fire-and-forget patterns (`.detach()` tasks) would require close ceremony. Go's channels drop without explicit close. Matches `ensure` philosophy — explicit handling available, implicit path simple.

### Comparison with Go

| Aspect | Go | Rask |
|--------|-----|------|
| Spawn syntax | `go func()` | `spawn(|| { }).detach()` |
| Track tasks | Manual (WaitGroup) | Compile-time (must-use handles) |
| Forgotten tasks | Silent | Compile error |
| Async/sync split | No | No |

### Channel Error Types

<!-- test: parse -->
```rask
enum SendError { Closed, Cancelled }
enum ReceiveError { Closed, Cancelled }
enum CloseError { AlreadyClosed, FlushFailed }
enum TrySendError { Full(T), Closed(T) }
enum TryReceiveError { Empty, Closed }
```

### Architecture

```
┌─────────────────────────────────────────────────┐
│                  Multitasking                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐        │
│  │ Thread 1 │ │ Thread 2 │ │ Thread N │        │
│  │   ◇◇◇◇   │ │   ◇◇◇◇   │ │   ◇◇◇◇   │        │
│  └──────────┘ └──────────┘ └──────────┘        │
└─────────────────────────────────────────────────┘
  ◇ = green task (concurrent, interleaved)
```

### Metrics Validation

| Metric | Target | This Design |
|--------|--------|-------------|
| TC (Transparency) | >= 0.90 | `using Multitasking { ... }` block visible at application entry; spawns visible at callsite |
| ED (Ergonomic Delta) | <= 1.2 | Close to Go ergonomics |
| SN (Syntactic Noise) | <= 0.30 | No `.await`, no boilerplate |
| MC (Mechanical Correctness) | >= 0.90 | Must-use handles catch forgotten tasks |

### See Also

- `conc.select` — select and multiplex
- `conc.sync` — synchronization primitives
- `mem.resources` — `@resource` types and `ensure` cleanup

<!-- id: conc.scheduler -->
<!-- status: proposed -->
<!-- summary: What a task is made of, and who decides when it runs. The second becomes a protocol a library conforms to. The first stays the compiler's: a stack sized from what the task calls (mem.stack), and a stackless mode for builds with no heap -->
<!-- depends: memory/stack.md, concurrency/runtime.md, concurrency/runtime-strategy.md, concurrency/async.md, compiler/effects.md, memory/relocatable.md, memory/closures.md, memory/borrowing.md, analysis/library-protocols.md, analysis/bare-metal.md -->

# Tasks and Schedulers

Two questions hide inside "the runtime": what a task is made of, and who decides when
it runs. Today both answers are the runtime's. This page proposes to split them. Who
runs becomes a protocol, so a game loop, a deterministic test harness and a bare chip
can each supply their own. What a task is made of stays the compiler's, but gets a
second shape that costs close to what a stackless task costs, and a third for builds
that have no heap at all.

## What a task is made of

A task that pauses three calls deep has three frames of live locals that have to
survive until the wake. There are only two places to keep them: a stack of the task's
own, or a heap object sized to exactly those locals. Everything else follows from that.

### Three shapes

| | Fixed stack (today) | Sized stack (`mem.stack`) | Stackless frame |
|---|---|---|---|
| Idle cost per task, MMU machine | touched pages (4–8 KB) plus two VMAs | touched pages, one allocation | the live set, exactly |
| Idle cost per task, no MMU | the whole reservation | the root's summary | the live set, exactly |
| Ceiling | `vm.max_map_count` (about 32k tasks at Linux's default) | memory | memory |
| Coloring in source | none | none | none, inferred |
| Preemption | yes (P2) | yes | no, pauses only |
| Indirect call across a pause | fine | fine, a chunk when short | heap frame, or refused without one |
| Recursion across a pause | fine | fine, at its declared depth | heap frame, or refused without one |
| C frames | fine | fine, never moved | a C callback can't pause |
| Stack traces | real | real | reconstructed |
| Needs a heap | no | only for chunks | no |
| Per-call cost | the preempt check (P3) | the preempt check; a compare at indirect calls only | state dispatch on resume |

The fixed stack is `conc.runtime/T3`: 1 MiB reserved per fiber, demand-paged. On a
machine with an MMU the memory cost is already small, since only touched pages are
committed, and what actually limits it is the kernel's map count, two entries per
fiber. On a chip without an MMU the reservation is the cost, so a fixed stack there
has to be small and overflow is a fault. That's the gap bare-metal.md calls the biggest.

### A sized stack

The growable stack this page used to propose (start at a few KB, copy to a bigger
stack when full, repair every pointer into it) is withdrawn in favour of
[mem.stack](../memory/stack.md). The compiler knows how much stack a chain of direct
calls needs, and recursion declares its depth, so a task's stack is reserved at its
root's summary up front (`mem.stack/ST7`). Nothing grows except at an indirect call,
which runs its callee on a chunk when the room left is short (`mem.stack/ST9`). A chunk
never moves, so there's no pointer list to keep and no rule against raw pointers held
across a park. On a chip without an MMU the reservation is real memory, and it's the
exact number `rask check --stack` prints.

### Stackless, as a build mode

A chip with no heap has nowhere to grow a stack into, and a hundred tasks at a fixed
4 KB is 400 KB on a part with 520. For that case, and only that case, the frames go in
static pools sized at compile time, which is what Embassy does with `async fn`. Rask
can do it without the `async`, because the compiler already infers which functions can
pause (`comp.effects/BL2`): the color exists, nobody writes it.

| Rule | Description |
|------|-------------|
| **SL1: A mode, not a type** | `tasks = "stackless"` in `rask.toml`. The source is identical to the stackful build; the same program compiles both ways |
| **SL2: The inferred effect selects** | A function with the Blocks effect reachable from a task gets a resumable body; the compiler, not the author, generates the second body where one function is reached both from a task and from plain code |
| **SL3: Frames are static** | A frame holds what's live across its pauses and nests its callees' frames. A task's frame is one object of known size, placed in a pool declared by the program |
| **SL4: Two refusals** | An indirect call (`any Interface`, a function value) across a pause, and recursion across a pause, have no static size. With a heap they get a heap frame. Without one they're compile errors at the call site, naming the chain, as `@no_alloc` does |
| **SL5: Cooperative** | A stackless task stops only where it pauses. P2 preemption is off in this mode, which is what an interrupt-driven program wants anyway |

This is the territory Zig explored with its old colorless async and retreated from,
because its function pointers made SL4's first refusal fire everywhere. Rask's indirect
calls are rare and explicit: `any` is opt-in and heap-allocated, closures mostly stay on
the stack, and reachability already lists every call it couldn't pin to one receiver.
The refusals are acceptable on a chip, where there is no heap and no `any` to begin
with, and nowhere else, which is why stackful stays the default everywhere else.

`ensure` on a parked frame runs from a heap object rather than a stack, which is a
second unwinder. That's engineering, not design, and it isn't small.

## Who decides when it runs

`using Multitasking { }` installs a process-global runtime slot that stdlib I/O consults
(`conc.runtime/IO1`). Today the slot holds the work-stealing scheduler and nothing else
can go in it. The codebase has already produced the second scheduler, `sim.c`, in C,
because tests needed a seeded, deterministic order. That's the argument for a protocol:
the second instance exists, so the slot should hold a conformance.

<!-- test: skip -->
```rask
interface Scheduler {
    func spawn(mutate self, take t: Task)
    func park(mutate self, take current: Task) -> Waker
    func wake(mutate self, w: Waker)
    func run(mutate self)
}

using Multitasking(FrameScheduler.new(budget: 16.ms)) {
    spawn { render() }                       // on the frame scheduler
    let irq = InterruptScheduler.new()
    spawn(on: irq) { read_sensors() }        // on another one
}
```

| Rule | Description |
|------|-------------|
| **SP1: A task knows its scheduler** | `using Multitasking(s)` installs `s` where `s: Scheduler` as the default; with no argument that's the stdlib's work-stealing scheduler. A plain `spawn` goes on the scheduler of the task doing the spawning, `spawn(on: s)` on another, and the task header carries its scheduler's address from then on. A park site therefore asks the current task, never a global, and the slot keeps only the installed-or-not flag the sync fallback (`conc.runtime/IO2`) already needs. The set of scheduler types a program uses is a fact of the whole program at monomorphization, like a `Shared` strategy (`conc.sync/SH2`): one type and every park is a direct call; several and it's a tag check over that closed set, the way a widened method call is dispatched. One check next to a context switch is noise. `conc.async/C1` still forbids two `using` blocks at once; `spawn(on:)` is how a second scheduler is reached |
| **SP2: `Task` is the compiler's, and linear** | A `Task` is a fiber with its stack, or a frame under SL1; a scheduler never sees inside. It's linear: `resume(take t) -> Parked or Done` hands it back only when the task stops, so running one task on two threads at once is unrepresentable (`rulings` test 3). A wrong scheduler can hang; it can't corrupt a stack |
| **SP3: `Waker` is an opaque token** | Handed out by `park`, consumed by `wake`. It carries the parked task's scheduler, so a wake from a task on another scheduler, or from an interrupt, lands on the right one. On an MCU an interrupt handler's whole job is `wake(w)`, which is Embassy's model with the `async` removed |
| **SP4: The sync primitives speak the protocol** | `Shared<T, Mutex>`'s blocking path, channels, `join` and `sleep` call `park` and `wake` on the slot. Every primitive therefore works under every scheduler, and no scheduler has to know about any of them |
| **SP5: The reactor is the scheduler's** | How readiness becomes a wake is policy: `epoll` on a server, the interrupt controller on a chip, a virtual clock under sim. The stdlib scheduler keeps `conc.runtime/R1`–`R3` as its own detail |
| **SP6: Still no coloring** | I/O functions check the slot's flag exactly as before and then park on the current task's scheduler. No `any Scheduler`, no hidden parameter, nothing in a signature |

What a library gets: a frame-budgeted scheduler for a game, a deterministic one for a
test, a run-to-completion loop for a chip, written in Rask and installed with one line.
`sim` becomes the first conformance and stops being C.

What it doesn't buy: a task's size. SP2 means the scheduler can't change what a `Task`
is; the three shapes above do that, and they're the build's choice.

## How to prove it

The stackless mode rests on one claim: that SL4's two refusals are rare enough to
live with. That one is a count. For every function the Blocks effect reaches from a
task, count the indirect calls and the recursive edges inside it. The effect inference
exists (`rask-effects`) but has no CLI; a `rask effects <file>` that prints it per
function is the measurement, and the number of refusals over the suite is the answer.
If a plain server program trips them everywhere, the mode is for chips only, as
written. If it rarely trips them, the mode is more useful than I've claimed.

## What changes if this is accepted

- `conc.runtime/IO1`: the slot is a flag; the scheduler is the program's type.
- `sim` (`specs/sim.md`) is a `Scheduler` conformance written in Rask.
- `analysis/bare-metal.md` section 5: a bare build may run tasks under SL1, and its
  scheduler is a package.
- `analysis/library-protocols.md`: the scheduler item in "Still open" is this page.

## Still open

- **A library that installs nothing but wants to run under a specific scheduler**
  (a test harness that needs `sim`'s determinism) has to ask the program to install
  it, since the type is the program's. `conc.async/C6` already says libraries don't
  install runtimes, so this is the existing rule with a sharper edge.
- **`Thread.spawn` and `ThreadPool`** are OS threads and untouched; only green tasks
  go through the slot. Whether a scheduler conformance should be able to say "I have
  no threads" for a chip is a question for the bare build.

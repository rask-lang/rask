<!-- id: conc.scheduler -->
<!-- status: proposed -->
<!-- summary: What a task is made of, and who decides when it runs. The second becomes a protocol a library conforms to. The first stays the compiler's, but gets a growable stack, which Rask can have without a GC because nothing stores a reference, and a stackless mode for builds with no heap -->
<!-- depends: concurrency/runtime.md, concurrency/runtime-strategy.md, concurrency/async.md, compiler/effects.md, memory/relocatable.md, memory/closures.md, memory/borrowing.md, analysis/library-protocols.md, analysis/bare-metal.md -->

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

| | Fixed stack (today) | Growable stack | Stackless frame |
|---|---|---|---|
| Idle cost per task, MMU machine | touched pages (4–8 KB) plus two VMAs | touched pages, one allocation | the live set, exactly |
| Idle cost per task, no MMU | the whole reservation | current size, from 1 KB | the live set, exactly |
| Ceiling | `vm.max_map_count` (about 32k tasks at Linux's default) | memory | memory |
| Coloring in source | none | none | none, inferred |
| Preemption | yes (P2) | yes | no, pauses only |
| Indirect call across a pause | fine | fine | heap frame, or refused without one |
| Recursion across a pause | fine | fine | heap frame, or refused without one |
| C frames | fine | fine, never moved | a C callback can't pause |
| Stack traces | real | real | reconstructed |
| Needs a heap | no | yes, for growth | no |
| Per-call cost | the preempt check (P3) | the same branch, one more compare | state dispatch on resume |

The fixed stack is `conc.runtime/T3`: 1 MiB reserved per fiber, demand-paged. On a
machine with an MMU the memory cost is already small, since only touched pages are
committed, and what actually limits it is the kernel's map count, two entries per
fiber. On a chip without an MMU the reservation is the cost, so a fixed stack there
has to be small and overflow is a fault. That's the gap bare-metal.md calls the biggest.

### A growable stack, without a GC

Go's answer is to start every goroutine with a 2 KB stack and copy it to a bigger one
when it fills, fixing up every pointer into the old one. Go can do that because its
garbage collector already knows where every pointer is. Rust can't, because a `&T`
into the stack can live anywhere, which is why Rust went stackless and paid in
`async`. Rask is in neither position: it has no GC, but it also has no stored
references, so at any point the compiler can say exactly what points into a task's
stack. That's the same fact `mem.relocatable` uses to serialize a rack.

| Rule | Description |
|------|-------------|
| **SG1: Start small, grow by copying** | A fiber's stack starts at a few KB. When a function entry finds less room than it needs, the runtime allocates a larger stack, copies the old one over, repairs every pointer into it, and frees the old one |
| **SG2: The compiler lists the pointers** | At every call that can park, codegen records which stack slots and registers hold an address into the current stack: an aggregate passed down by address (`mem.parameters`), a stack closure's environment (`mem.closures/CM1`), a `with` binding (`mem.borrowing/W1`), the frame's unwind record (`ctrl.panic/U6`), and the saved context itself. Nothing else can hold one, by `mem.borrowing/S3` |
| **SG3: A raw pointer to a local may not cross a park** | `unsafe` is the one way to take a stack address the compiler can't track. Holding one across a call that can park is a compile error naming both lines: the compiler knows the address was taken (`addr_taken`) and which calls block (`comp.effects/BL2`) |
| **SG4: Moves happen at parks and at entry, never under C** | A stack is copied when a function entry asks for room or when a parked task is resumed onto a bigger stack. Preemption (P2) never moves one. A C frame never sits on a stack that moves: C code can't park (`conc.runtime/P2.4`), and a Rask callback called from C runs on the caller's stack, which is pinned for the call |
| **SG5: One check, shared** | The room check is a compare of the stack pointer against the fiber's limit, folded into the safe-point check P3 already places at every function entry. The cold path grows; the common path is one more compare on a branch that is already there |

SG2 is the design. The list is short because the language made it short: every other
language that tried growable stacks without a GC (Rust, 2014) gave up at exactly this
point, because the list wasn't enumerable. Here it's five items, each with a rule that
says why it's the whole list.

What it costs: the compare in SG5; a copy whenever a task outgrows its stack, which is
once per power of two and amortizes to nothing for a task that lives long; and a heap
to grow into, which `Vec` already needs. What it doesn't change: coloring, preemption,
FFI, stack traces, or a single line of a program.

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

The growable stack rests on one claim: SG2's list is complete. There are three ways to
check it, and I'd do all three in order.

1. **From the rules.** Enumerate every construct that can hold the address of stack
   memory, and for each name the rule that bounds its life to a scope the compiler
   sees. Parameters: `mem.borrowing/B3`. Views: `S3`. `with`: `W8`. Closures: `CM1`,
   `MC3`. Sequences: `type.sequence/SEQ34`, `SEQ44`. Raw pointers: SG3. Anything not on
   that list is a hole in the argument, so the argument is also an audit of the specs.
2. **Measured on the corpus.** A MIR pass that, at every call site codegen knows can
   park, lists each live local of pointer type and says which of SG2's five kinds it
   is, or that it can't tell. Run over `tests/suite/` and `examples/` under an
   environment variable, the way `RASK_LIST_WIDENED_CALLS` works today. The output is
   either "every pointer at every park is classified" or the list of ones that aren't,
   each a design question with a file and line. No tool prints this yet; it's the
   first thing to build.
3. **Enforced per program.** The pass in 2 becomes a compile-time check, so the
   property is proven for each program as it's built rather than assumed for the
   language. That's how the unwind records already work (`ctrl.panic/U6`): nothing
   trusts that a frame owns nothing, the analysis says so.

The stackless mode has a different claim: that SL4's two refusals are rare enough to
live with. That one is a count. For every function the Blocks effect reaches from a
task, count the indirect calls and the recursive edges inside it. The effect inference
exists (`rask-effects`) but has no CLI; a `rask effects <file>` that prints it per
function is the measurement, and the number of refusals over the suite is the answer.
If a plain server program trips them everywhere, the mode is for chips only, as
written. If it rarely trips them, the mode is more useful than I've claimed.

A runtime prototype of SG1 is a day's work once 2 exists: `fiber.c` already has the
stack and the switch, and Cranelift can emit a stack map at a call site, which is the
list SG2 asks for in the form the copy needs.

## What changes if this is accepted

- `conc.runtime/T3` gains stack growth; the "demand-paged fixed reservation" line in
  the P3 rationale goes, since the room check now exists and P3 shares it.
- `conc.runtime/IO1`: the slot is a flag; the scheduler is the program's type.
- `sim` (`specs/sim.md`) is a `Scheduler` conformance written in Rask.
- `conc.runtime-strategy/B1` lists SG1 among Phase B's parts.
- `analysis/bare-metal.md` section 5: a bare build may run tasks under SL1, and its
  scheduler is a package.
- `analysis/library-protocols.md`: the scheduler item in "Still open" is this page.

## Still open

- **The starting size.** Go uses 2 KB (it was 8). Rask's frames are leaner than Go's
  in one way (no GC headers) and fatter in another (strings are 16 bytes). Measure.
- **A library that installs nothing but wants to run under a specific scheduler**
  (a test harness that needs `sim`'s determinism) has to ask the program to install
  it, since the type is the program's. `conc.async/C6` already says libraries don't
  install runtimes, so this is the existing rule with a sharper edge.
- **`Thread.spawn` and `ThreadPool`** are OS threads and untouched; only green tasks
  go through the slot. Whether a scheduler conformance should be able to say "I have
  no threads" for a chip is a question for the bare build.
- **Growth under a signal.** SG4 says preemption never moves a stack, so a task that
  grows only at entry is safe. A task that never calls anything and never grows is
  fine too. The case to think through is a park inside a deeply nested call that was
  entered before the last growth; the saved context's addresses are on the list in
  SG2, so it should be covered, but it wants the prototype.

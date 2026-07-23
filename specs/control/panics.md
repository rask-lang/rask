<!-- id: ctrl.panic -->
<!-- status: decided -->
<!-- summary: Panic kills the task — unwind runs ensures, locks release without poisoning, recovery only at join -->
<!-- depends: control/ensure.md, concurrency/async.md, concurrency/sync.md, memory/linear.md, memory/borrowing.md -->

# Panic Semantics

A panic is a detected bug, not an error value. It kills the panicking task: the stack unwinds to the task root, running `ensure` blocks and releasing lock access on the way, and the failure surfaces as `JoinError.Panicked` at the join point. There is no in-task recovery.

## What Panics

Every panic source is a programmer bug by definition. Expected failures use `T or E` (`type.errors`).

| Rule | Source |
|------|--------|
| **S1: Explicit** | `panic(msg)`, `todo()`, `unreachable()` (`type.errors/DP1–DP2`) |
| **S2: Force operators** | `x!` / `r!` on empty/error values (`type.errors/ER15`) |
| **S3: Checked arithmetic** | Overflow, divide-by-zero, `i32.MIN / -1` (`type.overflow/OV1–OV3`) |
| **S4: Access checks** | Index out of bounds, stale/wrong-pool handle (`mem.pools`), `with` aliasing (`mem.borrowing/W3–W4`) |
| **S5: Runtime guards** | `spawn` with no runtime (`conc.async/CC3`), `TaskHandle` dropped unconsumed (`conc.async/H1`), non-empty `Pool<Resource>` at scope exit (`mem.resources/R5`), stack overflow via guard page (`conc.runtime`) |
| **S6: Message + location** | Every panic carries a message and the source location of the failing operation |

Panic during `comptime` evaluation is not a runtime event — it's a compile error (`ctrl.comptime`).

## The Task Is the Unit of Failure

| Rule | Description |
|------|-------------|
| **P1: Unwind** | Panic unwinds the stack frame by frame toward the task root. Each block exit runs its scheduled ensures in LIFO order (`ctrl.ensure/EN1–EN2`) |
| **P2: Task-kill** | Unwind stops at the task root. The task transitions to Complete; the scheduler, other tasks, and the process continue |
| **P3: No in-task recovery** | No catch/rescue construct. The only place a panic becomes a value is the task boundary: `join()` returns `JoinError.Panicked(msg)` |
| **P4: main is a task** | A panic escaping `main` unwinds `main`'s stack (ensures run), then the process exits with status 101 (`struct.targets/EX4`) |
| **P5: exit is not a panic** | `os.exit(n)` terminates immediately — no unwind, no ensures (`struct.targets/EX3`) |

<!-- test: parse -->
```rask
func observe() {
    const h = spawn(|| { risky_work() })
    match h.join() {
        T as val                => process(val),
        JoinError.Panicked(msg) => log("worker died: {msg}"),  // P3: only observation point
        JoinError.Cancelled     => {},
    }
}
```

## Unwind Semantics

| Rule | Description |
|------|-------------|
| **U1: Ensures run** | Every ensure scheduled between the panic point and the task root runs during unwind |
| **U2: Access released, writes kept** | Unwind releases *access* (locks, borrows, bindings) but never rolls back *data*. Values keep whatever mutations happened before the panic |
| **U3: `with` release** | Unwinding through a `with` block releases what the block held: Mutex/Shared unlock, Cell borrow flag clears, pool element access ends |
| **U4: Inline access release** | Expression-scoped locks (`mutex.lock().f`, `shared.read().f` — `conc.sync/R5, MX3`) release when the expression is abandoned mid-unwind |
| **U5: Unensured linears leak** | A linear value with no scheduled ensure at panic time is leaked — no destructor runs, ever. The leak window is acquisition-to-ensure; keep it to one statement (`ctrl.ensure/L2` covers the `try` paths; panics are the residual) |

U5 is deliberate. Rask has no hidden destructors — that's the point of linear types + `ensure`. Inventing panic-only drop glue would reintroduce invisible cleanup to cover code that is, by definition, already broken. Mitigation is a lint (candidate) that wants `ensure` on the line right after acquisition, shrinking the window to zero statements.

## Locks: Released, Not Poisoned

| Rule | Description |
|------|-------------|
| **LK1: Clean release** | A Mutex/Shared lock held by the panicking task unlocks during unwind (via U3/U4). Waiting tasks acquire normally |
| **LK2: No poison state** | There is no poisoned flag. The next `with mutex` succeeds and sees the value exactly as the dying task left it |
| **LK3: Torn invariants are yours** | A panic mid-mutation can leave *application-level* invariants broken for survivors. Language-level invariants (memory safety, lock state, generation counts) always hold |

Where LK3 isn't acceptable — a multi-field invariant that other tasks will read — opt into **staged access**: `with mutex.staged() as v { }` works on a copy that commits as one move on non-panic exit and is discarded on unwind. Torn state impossible by construction at staged sites. Rules and example: `conc.sync/ST1–ST4`.

## Ensure × Panic

Closes the panic half of [#280](https://github.com/rask-lang/rask/issues/280). Supersedes the "subsequent ensures don't run" edge case in `ctrl.ensure`.

| Rule | Description |
|------|-------------|
| **E1: Ensure panic panics the task** | A panic inside an ensure body (or its `else` handler) ends that ensure and starts — or continues — unwind. The task dies |
| **E2: Remaining ensures still run** | The other scheduled ensures, in this block and every outer block, run anyway in LIFO order. One failing cleanup never skips other releases |
| **E3: First panic wins** | The first panic becomes the task's `Panicked` message. Any panic raised later in the same unwind — an ensure body, or a runtime guard firing at an unwound scope exit (`mem.resources/R5`, `conc.async/H1`) — is contained at its boundary and reported to stderr as a secondary panic |
| **A1: Abort escape hatch** | If the runtime itself cannot continue unwinding (panic inside the unwind machinery, stack exhaustion during unwind), the process aborts (SIGABRT). This is a runtime failure mode, not a semantic rule programs may rely on |

<!-- test: parse -->
```rask
func work() {
    const a = try open("a.txt")
    ensure a.close()               // E2: still runs
    const b = try open("b.txt")
    ensure b.flush_and_close()     // panics during unwind → contained (E3), reported

    process(a, b)                  // panics → unwind starts
}
// Task's panic: the process() panic (E3)
// b.flush_and_close() panic: printed to stderr as secondary
// a.close(): ran anyway (E2) — no leak
```

There is no Rust-style double-panic abort. The only code that executes during unwind is ensure bodies and runtime guards at scope exits — both bounded, runtime-invoked regions where containment is cheap (`ctrl.ensure/ER5` makes ensure *errors* independent; E2–E3 extend the same shape to unwind-time *panics*).

## What Survivors Observe

| Rule | Description |
|------|-------------|
| **O1: Join** | `h.join()` returns `JoinError.Panicked(msg)` (`conc.async`) |
| **O2: Channels** | The dying task's senders/receivers close as their scopes unwind (`conc.async/CH3`). Receivers drain the buffer, then see Closed; blocked senders get Closed |
| **O3: Locks** | Next acquirer gets the lock and last-written state (LK1–LK3) |
| **O4: Detached tasks** | Panic message and location print to stderr; the process continues. Detach means "I don't need the result," not "hide my bugs" |
| **O5: Multitasking body** | A panic in the `using Multitasking` block *body* cancels pending tasks and unwinds (`conc.async/C4`) |

## Panic Output

| Rule | Description |
|------|-------------|
| **F1: Format** | `panic at <file>:<line>:<col>: <message>` — task id prepended when a runtime is active. (Already the compiled runtime's format) |
| **F2: Backtrace** | `RASK_BACKTRACE=1` adds a stack trace. Not part of the deterministic surface |
| **F3: Deterministic message** | Messages are a deterministic function of the failing operation's operands — values, indices, lengths, generations. Never addresses |

## Determinism

Resolves the panic open question in `determinism`.

| Rule | Description |
|------|-------------|
| **PD1: Deterministic panic point** | Whether and where a panic fires is a function of executed values — every S1–S5 check is value-driven. Same sim seed → same panic |
| **PD2: Deterministic unwind** | Unwind order is the static block structure + LIFO ensure order. Sim replays it exactly, including E2/E3 secondary panics |
| **PD3: Replayable surface** | Message (F3) and unwind effects are replay-stable. Backtraces (F2) are diagnostic only and excluded from the contract |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Panic in ensure body during normal block exit | E1–E2 | Task dies with that panic; remaining ensures run |
| Panic in ensure body during unwind | E3 | Contained, reported as secondary; original panic wins |
| Panic in `else \|e\|` handler | E1 | Same as ensure-body panic |
| Non-empty `Pool<Resource>` scope exits during unwind | E3 | R5 guard fires as secondary — reported, contained; elements leak (U5's consequence) |
| Unconsumed `TaskHandle` scope exits during unwind | E3 | H1 guard fires as secondary — reported, contained; the task keeps running as if detached |
| Panic while holding nested pool bindings (`with pool[h1] as a, pool[h2] as b`) | U3 | Both accesses released |
| Panic between linear acquisition and its `ensure` | U5 | Resource leaks; lint nudges ensure-immediately-after |
| `os.exit()` inside an ensure body | P5 | Immediate exit — remaining ensures skipped (that's what exit means) |
| Detached task panics during `using` block drain | O4, C4 | Reported to stderr; drain continues |
| Panic crossing an FFI boundary (Rask fn called from C) | A1 | Unwind must not enter foreign frames — the runtime aborts at the boundary |
| Panic in comptime | S-note | Compile error, not a runtime event |
| Stack overflow | S5 | Guard page converts to panic; overflow *during unwind* falls into A1 |

---

## Appendix (non-normative)

### Decision A: task-kill, not process-abort

The rest of the design already voted. `JoinError.Panicked` exists (`conc.async`), the task lifecycle has a panic transition (`conc.runtime`), and supervision-as-a-library (`rejected-features`) only works if a supervisor can observe a child's death without dying itself. Process-abort would make all of that dead code and reduce the reliability story to "restart the process."

Worked alternative — **abort everywhere**: smaller runtime (no unwinder), no torn-state question, no E-rules. Right for tiny embedded targets, wrong as the default: one bad index in one request handler kills every connection the server holds. If embedded needs it, a `panic=abort` build profile can come later; the *semantics* stay unwind-based.

**Go's middle ground** (unhandled panic in any goroutine kills the process unless recovered) was rejected because Rask has no `recover` — adding one would create in-task panic handling (P3 exists to prevent exception-style control flow).

### Decision B: no poisoning; staged access instead

Poisoning (Rust `std`) turns every lock acquisition into a fallible operation. Rask's `with mutex as v { }` has no error channel, and adding one taxes every call site with ceremony for a case that is already a bug elsewhere. The Rust ecosystem's own drift to `parking_lot` (no poisoning) is evidence the tax doesn't pay.

Rust is stuck with detection because a `MutexGuard` is a free-floating reference — the language can't see where an update starts and ends. Rask can: the `with` block *is* the update, with compiler-known boundaries. That's what makes staged access (`conc.sync/ST1–ST4`) possible — prevention by construction, priced per site, instead of detection taxed everywhere. Commit is an infallible move under a held lock, so unlike poisoning there is nothing to handle and nothing to cascade.

Other alternatives worked through:

- **Sticky poison-panic** (next acquirer panics): converts possible corruption into loud failure, which fits "fail loudly." Rejected because it cascades — one panicked request handler turns a shared metrics mutex into a landmine that kills every future task touching it, and recovery requires a heal/clear API that admits panics into normal control flow.
- **Recovery arm** (`with mutex as v else abandoned { rebuild }` — sticky "holder panicked" flag, observed only by sites that opt in): the strongest detection variant, zero tax on ordinary sites. Rejected as the primary answer because unhandled it degrades to exactly LK3's silent inconsistency, and the flag needs healing semantics. Could still be added later; it composes with staged rather than competing.
- **Invariant validators** (a check function attached to the box, run at release): hidden user code at unlock time — hidden cost and hidden control flow at once.

LK3 + staged is the honest split: the language guarantees its invariants everywhere, and gives you a visible, by-construction tool for yours where they matter.

### Decision C: run remaining ensures

The current `ctrl.ensure` edge case ("panic propagates, subsequent ensures don't run") means one failing `close()` silently leaks every other resource in scope — exactly the complaint in #280. Linear resources are the correctness spine of the stdlib; skipping their cleanup on the cleanup path is the wrong default. Rust agrees in substance: a panicking drop doesn't skip sibling drops.

Where this draft *diverges* from Rust: no double-panic abort. Rust aborts because arbitrary drop code runs during unwind and nested unwinding is unmanageable. In Rask the only unwind-time user code is ensure bodies, each already a bounded, runtime-invoked, error-isolated region (ER5) — containing a panic there (E3) is cheap and keeps the blast radius at the task. A server should not die because a log-flush ensure panicked while a request task was already unwinding.

### Decision D: detached panics go to stderr

Silence hides bugs (violates "no unreproducible failures" — the failure didn't even *print*). Abort contradicts Decision A. Stderr + continue matches what detach promises: the task's *result* is unobserved, not its existence.

### Implementation status

The interpreter already implements most of this model; compiled code has the big gaps. Deltas to file as issues once the spec is accepted:

**Interpreter** (panic = `Err` propagation, `rask-interp`):
- Matches P1–P3, U1, U3, LK1–LK2, O1: ensures run on unwind, task panic → `JoinError.Panicked`, locks/Cell borrows release cleanly, no poisoning.
- Matches E2/E3 (`interp/call.rs`, `run_ensures`): a panic in an ensure body no longer skips the remaining ensures — they all run in LIFO order; the first panic wins and later ones (including any raised while already unwinding) are reported to stderr as secondary panics.
- Matches U2 (`eval_expr.rs`, WithAs): `with`-block writes are flushed before the panic propagates, so mutations made before the panic are kept.
- Exits with code 101 on uncaught panic (`struct.targets/EX4`, `run.rs`).
- Residual: the `mem.resources/R5` and `conc.async/H1` runtime guards firing at an unwound scope exit still override the primary panic instead of being contained as secondary (`call_function` → `check_scope_exit`) — the E3 guard case, tracked under #298.
- `staged()` (`conc.sync/ST1–ST4`) is unimplemented in both paths.

**Compiled** (`rask-codegen` + C runtime):
- Main-thread panic now runs the ensure-hook stack then `exit(101)` (P4), not `abort()`. The hook stack + `rask_ensure_run_all` (with E2/E3 containment) live in the linked `panic.c` and are shared by every backend; `green.c` uses them through take/set accessors.
- Ensures are still inlined only on the normal-return path (`CleanupReturn`); **codegen does not yet push hooks**, so nothing runs on the panic path — U1 unmet on native until codegen emits `rask_ensure_push/pop` (the hard part: reifying each ensure body as a thunk over its by-reference captures).
- `thread.c` tasks: panic → `JoinError.Panicked` via setjmp/longjmp (matches P2/O1); `rask_panic` now drains the hook stack before the longjmp, so ensures will run once codegen pushes them.
- `green.c` tasks: join of a panicked task *re-panics in the joiner* instead of returning `JoinError.Panicked` — violates O1.
- Panic messages truncate at 512 bytes; backtrace prints unconditionally (no `RASK_BACKTRACE` gate).

### See Also

- [Ensure](ensure.md) — cleanup scheduling this spec extends (`ctrl.ensure`)
- [Async](../concurrency/async.md) — task model, `JoinError` (`conc.async`)
- [Sync](../concurrency/sync.md) — Mutex/Shared access rules (`conc.sync`)
- [Linear types](../memory/linear.md) — consume-exactly-once (`mem.linear`)
- [Determinism](../determinism.md) — replay contract this spec plugs into
- [Targets](../structure/targets.md) — process exit statuses (`struct.targets/EX3–EX4`)

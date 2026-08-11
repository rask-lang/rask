<!-- id: sim -->
<!-- status: proposed -->
<!-- summary: The machine behind the determinism contract — seeded scheduler, virtual clock, seeded faults, and the replay line a failing test prints -->
<!-- depends: determinism.md, concurrency/runtime.md, stdlib/testing.md, control/panics.md, stdlib/random.md -->

# Sim Mode

`determinism` states the promise. This spec builds the thing that keeps it: a single-threaded runtime that draws every scheduling and fault decision from one seed, runs the clock in virtual time, and prints a paste-able replay command when a test fails.

Sim is a **link choice, not a dialect** (`determinism/D2`, `D3`). `rask test --sim` links the sim runtime under the same stdlib surface. User code compiles identically, and nothing in the source says which runtime it got.

## Invocation and replay

| Rule | Description |
|------|-------------|
| **I1: Test mode** | `rask test --sim` runs the selected tests on the sim runtime. v1 is test-only — `rask run --sim` is not part of it |
| **I2: Run seed** | `--seed N` fixes the run seed (u64, decimal). Without it the runner draws from system entropy and prints it in the header |
| **I3: Per-test seed** | Each test's seed derives from (run seed, test's full name). A test replays identically no matter which other tests ran, in what order, or whether they ran at all |
| **I4: Replay line** | Every failure prints the exact command that reproduces it. The printed line is the repro — that's the whole point |
| **I5: Seed search** | `--seeds N` runs each selected test on N seeds derived from the run seed. Stops at the first failure per test; `--keep-going` runs all N. The sweep itself replays from `--seed` |
| **I6: Sequential** | `--sim` implies `--sequential`. One sim runtime per test, installed and torn down around it (`conc.async/C1`). Parallelism belongs in seed search, across processes, not inside one |

```
rask test --sim                          # whole suite, fresh seed
rask test --sim --seed 8419230744151203  # replay that run
rask test --sim --seeds 1000 -f "raft"   # search 1000 schedules
```

## Seed streams

| Rule | Description |
|------|-------------|
| **SD1: Split streams** | The test seed splits into independent streams: scheduler, faults, hash order (`determinism/D7`), and one user-random stream per task |
| **SD2: Stream independence** | Draws in one stream never shift another. How many times a test calls `random.u64()` cannot change its interleaving, and vice versa |
| **SD3: Task streams** | A task's user-random stream derives from (test seed, task id). Task ids are assigned in spawn order, which is itself deterministic |

SD2 is what makes seed search honest. Without split streams, adding one `random.f64()` to a test body reshuffles every schedule the sweep already explored, and a seed from yesterday's failure means nothing today.

## Scheduler

| Rule | Description |
|------|-------------|
| **S1: One task at a time** | Sim runs single-threaded. A task runs until it parks, is preempted, or completes. No stealing, no reactor thread, no timer thread |
| **S2: Uniform among runnable** | At each scheduling point, the next task is drawn uniformly at random from the runnable set, from the scheduler stream |
| **S3: Scheduling points** | Every park point (I/O, channel op, sleep, join, lock acquire) plus preemption after a seed-drawn budget of safe points (`conc.runtime/P2.2`). CPU-bound code gets interleaved too — sim preempts on a step count, never on wall time |
| **S4: Step counter** | Scheduling decisions are numbered from 0. The step number is the coordinate in every failure report and the unit `--seeds` search reasons about |
| **S5: Deadlock is a failure** | All tasks parked, no timer pending, no simulated I/O outstanding → the test fails with a deadlock report naming what each task is waiting on. Sim never hangs |
| **S6: ThreadPool** | `using ThreadPool` jobs are scheduled as tasks under the same rule (`determinism/D13`) |

## Virtual clock

| Rule | Description |
|------|-------------|
| **C1: Fixed start** | `Instant` starts at 0; `SystemTime` starts at 2020-01-01T00:00:00Z. Wall-clock start is not an input (`determinism/D10`) |
| **C2: Jump when idle** | When nothing is runnable and a timer is pending, the clock jumps to the earliest deadline and wakes it. A 30-day `sleep` costs no wall time |
| **C3: Time is charged, not free** | The clock advances at two points: every scheduling step (1 µs), and every clock read — `Instant.now()`, `elapsed()`, `SystemTime.now()` each cost 1 µs. Observing time costs time |
| **C4: I/O latency** | Every simulated I/O completes at `now + latency`, drawn per operation class from the fault stream. Slow-peer orderings come from the seed, not from a mock |
| **C5: No advance API** | Tests cannot advance the clock explicitly at v1. Same code in both modes (`determinism/D3`) — a test that wants time to pass sleeps |

## Faults

| Rule | Description |
|------|-------------|
| **F1: Always on** | Adversarial scheduling (S2), short reads/writes, and I/O latency (C4). These are legal behavior, not faults — code that breaks on them was already broken |
| **F2: Opt-in** | `Fault.IoError`, `Fault.Disconnect`, `Fault.ClockJump`. A test enables them by calling `sim.require(faults: [...])` as its first statement. `Instant` never jumps — `std.time/I1` is monotonic and stays monotonic |
| **F3: Sim-only tests** | Outside sim, `sim.require` skips the rest of the test and says why, reusing `std.testing/T12`. A fault test never passes vacuously under a plain `rask test` |
| **F4: Seeded rates** | The seed picks the injection rate, not just the schedule. Each seed draws its own intensity, so a sweep explores calm runs and storms without a second knob |
| **F5: All-or-nothing** | An injected error means the operation had no effect. Partial effects come only from the short-read/short-write class, where partial *is* the behavior |
| **F6: Rendered, not recorded** | The fault log in a failure report is regenerated from the seed. Nothing is stored between runs |

<!-- test: skip -->
```rask
test "replica catches up after the leader drops" {
    sim.require(faults: [Fault.IoError, Fault.Disconnect])

    using Multitasking {
        // ordinary code — nothing here knows it is being simulated
    }
}
```

No declarative scenario layer at v1. "Partition {a,b} from {c} at step 3980" is something a *report* says, not something a test writes. If real tests turn out to need scheduled scenarios, that's a second spec with evidence behind it.

## Boundaries

| Rule | Description |
|------|-------------|
| **B1: Threads refused** | A test whose capability metadata (`struct.build`) reaches `Thread.spawn` is refused before it runs, with the call path. A runtime panic backstops what the metadata missed (`determinism/D13`) |
| **B2: Escaping C is refused** | C that reaches the real world through something sim cannot replace — `pthread_create`, raw sockets and file descriptors, `fork`, a `syscall` instruction written by hand — is outside the contract, so the test does not run under sim. The refusal names the symbol. `--sim-permissive` runs it anyway, marked `unsimulated: ffi` (`determinism/D14`) |
| **B3: Unsimulated calls panic** | A stdlib call with no simulated implementation panics naming the call. It never falls through to the real thing |
| **B4: Environment** | Sim owns the environment. It starts empty at every test, and a test that needs a variable sets it with `os.set_env` (`std.os/E3`) in its body. The real process env is never visible, and never leaks from one test to the next. `os.args()` is `["<test>"]` |
| **B5: Filesystem** | Reads fall through to the real filesystem (a recorded input under `determinism/D10`); writes land in an in-memory overlay and are discarded at test end. The real tree is never modified |
| **B6: Sealed C is inside the contract** | C that only computes is already deterministic — same bytes in, same bytes out. Sim classifies each linked object by its undefined symbols: if they all fall in the pure set (`memcpy`, `strlen`, libm, …), the code is sealed. No mark, full contract, nothing to simulate |
| **B7: Reaching C is interposed** | Between sealed and escaping sits C that asks the world one question at a time: `clock_gettime`, `gettimeofday`, `getrandom`, `getpid`, `sysconf`, and `malloc`. Sim resolves those at link time to a virtual clock, seeded random, fixed answers, and a fixed-base allocator that poison-fills what it hands back. Interposed is still inside the contract |
| **B8: Addresses are the C-side hole** | `determinism/D11` says addresses can't leak into logic — true of Rask, not of C, which can hash or sort by a pointer freely. The fixed-base allocator (B7) is what closes it, and it is the reason `malloc` is interposed rather than treated as pure |

## Failure output

| Rule | Description |
|------|-------------|
| **R1: Header** | A sim run prints its seed once at the top, whether or not anything fails |
| **R2: Failure block** | Test name, seed, the panic or assertion (`ctrl.panic/F1`, `F3`), the sim step and virtual time it happened at, the faults injected before it, and the replay line |
| **R3: Search summary** | Seed search prints how many seeds ran and one replay line per distinct failure |

```
sim: seed 8419230744151203, 47 tests

FAIL: replica catches up after the leader drops
  panic at raft.rk:214:9: index 3 out of bounds (len 3)
  step 4127, virtual time 00:00:12.400
  faults: latency 210ms on peer c (step 3980), io error on write (step 4102)
  replay: rask test --sim --seed 8419230744151203 -f "replica catches up after the leader drops"
```

## Error messages

```
ERROR [sim/B1]: test reaches Thread.spawn, which sim mode cannot schedule
   |
12 |  test "worker pool drains" {
   |       ^^^^^^^^^^^^^^^^^^^ reaches Thread.spawn via pool.rk:31 -> worker.rk:8

WHY: Sim runs every task on one thread so ordering comes from the seed. A raw OS
     thread runs outside that, so its interleaving would not replay.

FIX: Use `using ThreadPool { }` — sim schedules pool jobs like tasks.
```

```
ERROR [sim/S5]: deadlock — no task can make progress
   |
   |  step 812, virtual time 00:00:00.812

  task 0 (main)      waiting on join(task 2)
  task 2 (fetch)     waiting on channel receive, 0 senders live
  no timers pending

WHY: Every task is parked and nothing will wake them. Under a real runtime this
     would hang; sim can prove nothing is coming and fail instead.

replay: rask test --sim --seed 4471 -f "fetch pipeline"
```

```
ERROR [sim/B3]: no simulated implementation for `os.exec`
   |
30 |      let out = try os.exec("git", ["rev-parse"])
   |                    ^^^^^^^ sim has no model for subprocesses

WHY: Falling through to the real call would make the run unreplayable without
     saying so. Sim fails loudly instead of quietly leaving the contract.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| Test passes under `rask test`, fails under `--sim` | Real ordering bug — the schedule was just never hit | S2 |
| Test calls `random` a different number of times after an edit | Schedules from the old seeds still mean the same thing | SD2 |
| Two tests with the same name in different modules | Seeds differ — derivation uses the full path, not the leaf name | I3 |
| Busy-wait on `Instant.elapsed()` | Terminates; the step tick advances the clock | C3 |
| Test spawns and never joins | `TaskHandle` drop panic (`conc.async/H1`), replayed like any panic | ctrl.panic/PD1 |
| Detached task still running at block exit | Drain runs it to completion in virtual time | conc.async/C4 |
| `sim.require` test under plain `rask test` | Skipped at that line, reported as sim-only | F3 |
| Test reaches sealed C (a hash, a decompress) | Runs, no mark — already deterministic | B6 |
| Test reaches `pthread_create` through C | Refused, symbol named | B2 |
| Test writes a file, later test reads it | Second test does not see it — the overlay is per-test | B5 |
| `--seeds 1000` with a test that fails on all of them | One replay line per distinct failure signature, not 1000 | R3 |

## Non-goals

- **Production record-replay.** Capturing real inputs to replay a production incident is a different, heavier feature. Still parked (`determinism` open questions).
- **Cross-platform bit-exact floats.** Same binary, same platform (`determinism/D12`). Bit-exactness across machines is Raido's job.
- **Performance realism.** Virtual latencies are plausible, not measured. Sim finds interleaving and fault bugs; it will never tell you something is slow.
- **Model checking.** Sim samples the schedule space at random. It does not enumerate it, and a green sweep is evidence, not proof.
- **Crash and restart.** Torn writes surviving a process death need a durability model. Out of v1; it is the obvious next step after it.

---

## Appendix (non-normative)

### Rationale

**I4 (the printed line is the repro):** The failure output could print a seed and trust the reader to assemble a command. Every tool that does this makes you look up the flag spelling at the exact moment you are annoyed. Printing the whole command costs one line and removes the step.

**I6 (sequential):** In-process parallelism buys nothing here. Sim time is virtual, so a suite that sleeps for hours finishes in milliseconds; the wall-clock cost is real CPU work, and that parallelizes across processes during seed search where it actually matters.

**S2 (uniform random):** Weighted or history-guided schedulers find bugs faster in papers. Uniform is the one you can hold in your head when reading a failure report, and it composes with seed search: a schedule that needs 1-in-10,000 luck is 10,000 seeds away, and 10,000 seeds is a coffee break.

**S5 (deadlock is a failure):** This is the feature people will not expect. Under a real runtime a deadlock is a hang, and a hung test is a timeout with no information. Sim knows the full set of parked tasks and pending wakeups, so it can prove nothing is coming and print who was waiting on whom.

**C3 (time is charged):** Pure event-driven virtual clocks freeze when a task spins on elapsed time — the loop never parks, so the clock never advances, so the loop never exits. Tokio's paused clock has exactly this hole (auto-advance stops while the runtime has work), and madsim intercepts `clock_gettime` to return virtual time without charging for it, so it has the hole too.

Two charge points close it between them, and each covers what the other misses. The scheduling-step tick means CPU work eventually lets pending timers fire, so a task burning cycles can't starve a `sleep` forever. The clock-read tick covers the case the step tick can't: a spin loop whose body inlines away has no safe points, so it produces no scheduling steps — but it must still call into the runtime to read the clock, or it has no exit condition to spin on. A loop that observes time advances time; a loop that doesn't observe time can't observe that time hasn't moved.

The cost is that virtual duration measures scheduling steps and clock reads, not work. Sim was never going to tell you something is slow (see non-goals), so this trades nothing anyone had.

**F1 vs F2 (on vs opt-in):** Short reads and adversarial ordering are things a correct program already handles, so turning them on by default costs nothing but finds real bugs the first time someone runs `--sim` over an existing suite. Injected I/O errors are different: turning them on globally would fail every test that opens a file, and a mode that cries wolf on first contact does not get run twice.

**F3 (`sim.require`, not an attribute):** An attribute would work and was drafted first. It doesn't earn the grammar: `std.testing/T12` already has `skip("reason")`, which is exactly the "this test doesn't apply here" mechanism, and a call reads as the precondition it is. The reader learns the same fact either way, so the version that costs no syntax wins (`NORTH_STAR` commitment 5). The one thing lost is static visibility — the runner can't tell a test is sim-only without starting it — and that costs nothing, because the call aborts on the first line.

**F4 (seeded rates):** A fixed rate would mean a thousand-seed sweep explores a thousand schedules at exactly one intensity — never the calm run where one late failure matters, never the storm where everything fails at once. Drawing the rate from the seed explores severity and ordering together and still leaves one knob, which was the point. Rejected: a per-test rate parameter, which is a second dial that changes what a seed means.

The distribution the rate is drawn from is still unset. That number wants real tests behind it.

**B2 (refuse, not report):** A mark next to a passing result is only as good as the reader, and the failure mode is nasty and delayed: someone pastes a replay line that doesn't replay and loses an afternoon before suspecting the C. Sim's entire value is that a seed reproduces a failure, so a quiet path where the seed doesn't reproduce it costs more than the coverage it saves.

Refusing was the expensive option when FFI was one undifferentiated bucket — it would have knocked out every test that touches a hashing library. The tiering below is what made it cheap: refusal now bites only the genuinely uncontainable cases, and `--sim-permissive` is there for the person who has measured that their `pthread_create` doesn't matter.

**B2/B6/B7 (three tiers of C):** The first draft put all of FFI outside the contract, which was lazy. You don't simulate the C — you don't have to. Machine code that only computes is already a pure function of its inputs; zlib decompresses the same buffer to the same bytes on every run of every seed. What breaks determinism is the small set of things C reaches *for*, and every one of them is a named symbol resolved at link time. Sim is already a link-time swap, so the interposition point is the one that already exists.

That splits FFI into three, with a shrinking residue:

| Tier | Undefined symbols | Under the contract? |
|------|-------------------|---------------------|
| Sealed | `memcpy`, `strlen`, libm | Yes, untouched |
| Reaching | `clock_gettime`, `getrandom`, `malloc` | Yes, interposed |
| Escaping | `pthread_create`, `socket`, `fork` | No — marked or refused |

Classification is a set intersection against the object's undefined symbols, which `compile_c` and `link_library` (`struct.build/PM10`) already put on the link line. v1 climbs the first rung: classify, and let sealed C keep the full contract instead of losing it for nothing. Interposition is rung two.

The residue is honest and probably permanent. A `syscall` instruction written in inline assembly has no symbol to intercept, so no symbol scan will find it — statically linked musl and anything Go-shaped does exactly this. Those get marked, which is what B2 is for.

**B8 (addresses):** Worth stating out loud because it is the one place `determinism/D11` stops being true. Rask can't observe an address, so D11 costs nothing to guarantee. C can hash a pointer, sort by it, or key a table on it, and ASLR then makes the run different every time with no clock and no random anywhere in sight. This is why `malloc` sits in the interposed tier rather than the pure one — a fixed-base allocator is cheap and closes the hole, and poison-filling what it returns makes uninitialized reads deterministic too, the same trick `RASK_POISON_STACK` already plays on the Rask side.

**B4 (sim owns the env):** Passing the real environment through — wholesale or by allowlist — buys an input `determinism/D1` never sees. `PORT=5` set in one shell and not another means the replay line stops working on someone else's machine, and nothing says why.

No new syntax is needed to avoid that, because `os.set_env` already exists. A test that depends on a variable writes the variable:

<!-- test: parse -->
```rask
test "config reads the port" {
    os.set_env("PORT", "9000")
    assert Config.load().port == 9000
}
```

That is better than an attribute on two counts. The value is pinned rather than inherited, so the run is a function of source plus seed with nothing left over. And it sits one line above the assertion that depends on it, instead of in a header — a reader who wonders where 9000 came from is already looking at the answer.

The consequence is that sim is a sealed world: there is no way to read the machine's real `HOME`, and no way to ask for one. A test that genuinely needs the developer's environment is testing the machine, and that is a different job from testing the program.

**B5 (read-through filesystem):** An empty simulated filesystem is purer and would break every test with a fixture directory. Reads are a recorded external input; treating the real tree as read-only input keeps existing tests working while guaranteeing sim never writes anything.

### Target

Sim is built on the native runtime. Its three interposition points — scheduler, clock, reactor — are the ones `conc.runtime` already specifies, so sim replaces components that have a designed shape rather than inventing parallel ones, and what it finds is what ships.

The cost is order: sim lands after Phase B fibers. The interpreter would have been quicker to make deterministic, since stepping evaluation makes "pick a random runnable task" nearly free, but it spawns OS threads today and so has no seedable scheduler either — and a green scheduler built there would be one nobody ships, verifying orderings the compiled program may not have.

### Open questions

- **Fault rate distribution (F4).** The seed picks the rate; what it picks from is unset. That number wants real tests behind it, not taste.
- **Sealed-set membership (B6).** Which libc symbols count as pure is a list, and lists are where this kind of design rots. `memcpy` is obvious, `qsort` takes a comparator, `strerror` reads a locale. Needs writing down properly, once, with a rule for adding to it.

### See Also

- `determinism` — the contract this implements (D1–D14)
- `std.testing` — test declaration, T17–T19 runtime rules the sim runner inherits
- `conc.runtime` — the production runtime sim replaces
- `ctrl.panic` — PD1–PD3, the panic surface sim replays
- `std.random` — R3 seeded generation, the user-facing half of SD3

# Rask Roadmap

**A version ships when a number a script prints hits its target.** Not when a
list feels done.

Bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). Unscheduled
work is in [TODO.md](TODO.md).

## Why this shape

The old roadmap answered "what next" and never "when is this finished", so
everything was 60–90% done and nothing was shippable. It also kept two
scoreboards — "no tracked bugs" counted the suite's registered backlog, which
was five, while forty bug issues were open — and the status line quoted the
flattering one.

The thing that has actually worked here is making a claim measurable. The leak
gate reported zero for months because it grepped output `rask test` was throwing
away; once it measured honestly it said 173, and 173 became 31 within a release.
A macOS job that compiles the runtime found three bugs on its first morning. The
book gate stops chapters rotting. Every time a claim became a number, the number
moved.

So each version below is one theme and one number. Nothing else goes in it.

**The rule that makes it finish:** a bug found inside the current version's theme
joins that version. A bug outside it goes to the backlog and does **not** delay
the release. The theme has to be narrow enough to close and wide enough that
closing it means something.

## Where things stand

Re-measure these rather than trusting them — each line names the command.

| Measure | Now | Command |
|---------|-----|---------|
| Suite programs agreeing on both backends | 555 green, 8 registered red | `tests/differential.sh` |
| Programs that leak | 5, holding 8 allocations this milestone and 2 deferred | `tests/leak_gate.sh` |
| Matrix cells clean on both backends | 280 of 282, 6 pairs skipped | `tests/matrix/run.sh` |
| Programs memcheck finds an error in | 0 of 557 | `tests/memcheck_gate.sh` |
| Concurrency files TSan reports a race in | 0 of 72 | `tests/tsan_gate.sh` |
| Soak programs within their thread budget | 5 of 5 | `tests/soak_gate.sh` |
| Examples with a pinned golden | 37 of 37 | `tests/examples_gate.sh` |
| Runtime builds under the other compiler | clean | `tests/clang_gate.sh` |
| Open bugs | 39 of 85 open issues | issue search |
| Open design questions | 22 | issue search |

Nine more gates cover prototypes, packages, projects, tutorials, the book, the
agent benchmark, internal spellings, formatter round-trips and the HTTP server.
All green.

This table was a month stale when it was last checked — it claimed 31 leaking
programs holding 86 allocations while the gate printed 0, and 500 of 505 while
the suite had grown to 525. That is the failure the preamble above says this
file exists to prevent, so: re-measure before quoting it, and if you quoted it,
you have re-measured it.

## v0.3 — Memory is settled — **shipped 2026-09-18**

**Done when `tests/leak_gate.sh` reports 0 allocations this milestone. Today: 0.**

The gate's condition is met — 521 suite files clean. That is the measure, not
the claim that nothing leaks, and what "settled" should mean beyond a green gate
is the open question now.

Every leak on this list was the compiler getting *who frees this* wrong. A leak
is the polite version of that mistake — the same confusion releasing early
instead is a use-after-free, which is what
[#1161](https://github.com/rask-lang/rask/issues/1161) was. Ownership is the
whole thesis of the language, so this goes first.

**The impolite half now has a gate too.** Nothing measured it: no valgrind, no
sanitiser, in any of the thirteen gates or in CI, so every use-after-free,
double free and read of an uninitialised field found so far was found by a
crash. `tests/memcheck_gate.sh` runs the same suite binaries under memcheck.
Within a minute of first being pointed at the suite it found a live bug in
week-old code — a pool built field by field that had grown two fields nobody
initialised — which the leak gate, twelve other gates, CI and a 30× poisoned
stack run had all called green. It is in CI beside the leak gate now, with
`tests/known_memcheck.txt` as its ledger, currently empty.

The gate reports a second number beside that one, and it isn't part of this
milestone. It is 2 allocations now, down from 15, and one file:
`t_shared_freed.rk`, waiting on `clone_elision` knowing which block
`s.clone()` handed back. Freeing it today is a double free rather than a smaller
leak, which is what makes it a wait rather than a task. Its line in
`tests/known_leaks.txt` says `deferred`, and the gate still measures it and
still holds it to its count. It just doesn't judge a memory milestone on what a
memory milestone can't fix.

The other 13 were one bug, and the reason to say so is that the ledger blamed
the wrong thing for a month. It read "a task killed by a panic doesn't unwind
its captures, waits on the unwinder in v0.5" — but the unwinder landed in
August. What was actually wrong was two lines of C: a task's closure allocation
was freed on the line after the body's own call, which a panicking body longjmps
straight past, and `TaskHandle.join` read as returning a view into its receiver
because its return type names a type parameter, so the frame released nothing it
got back. Three files went clean ([#1223](https://github.com/rask-lang/rask/issues/1223)).
A deferred line is worth re-measuring, not re-reading.

The last one was [#1205](https://github.com/rask-lang/rask/issues/1205), and it
took six attempts because the question was never "how does a swallowed closure
get freed" — ten lines answer that — but "which of the two owners frees it,
when one body is built from sites in different positions". `main` drops the
adapter it builds; a `flat_map` callback returns the one it builds; the glue is
named after the body. Splitting those sites is what made one answer possible.
The five measurements are on the issue.

**The gate is not the whole story**, and its blind spot is worth knowing about
before anyone reads 0 as "done": it runs suite files as `test` blocks, so a leak
that only shows in `main` is invisible to it.
[#1213](https://github.com/rask-lang/rask/issues/1213) lived there — a value
that hands its old version into its new one, freed by nobody — and was found by
running the repro rather than by the gate. It is fixed and now has a suite file,
but the next one of its kind will hide in the same place.

[#1224](https://github.com/rask-lang/rask/issues/1224) was the last one the
gate still counted, and it was two bugs wearing one number. A variable an
`ensure` body names becomes a memory slot — the hook holds its address, because
the body may run on a panic long after the frame stopped — and nothing freed
what those slots held: `ensure v.push(2)` leaked the vector. Underneath it, a
cleanup chain ends in `unreachable` because MIR has nothing left to say after
it, and codegen turns that into the real return; read as an abort, it made every
exit through an `ensure` look like a path the process never leaves, so a release
the assert lowering owes on the *passing* branch was dropped as dead code. That
second one is why `assert s == "…"` leaked its string only in functions that
also had an `ensure` somewhere.

The list of leaks that were open without showing in the gate is empty now. Each
one's repro was re-run on both backends and each has a suite file keeping it
that way: [#1035](https://github.com/rask-lang/rask/issues/1035) (a string
between two containers, and a container inside one),
[#1117](https://github.com/rask-lang/rask/issues/1117) (a container returned
inside `T?` from a callee small enough to inline),
[#1131](https://github.com/rask-lang/rask/issues/1131) (`for x in h.items`,
which dies on a block that never mentions it),
[#1153](https://github.com/rask-lang/rask/issues/1153) (a fused `zip` over two
struct fields), [#1157](https://github.com/rask-lang/rask/issues/1157)
(`with s.staged()` on a local box) and
[#1158](https://github.com/rask-lang/rask/issues/1158) (`io.copy` through a
boxed writer).

[#882](https://github.com/rask-lang/rask/issues/882) was the last thing this
milestone was waiting on, and the audit it asked for is done: four passes over
the grid — `@resource`, `Heap<T>`, then the crossed cells and
the panic path — seventy-odd cells, six holes, all fixed. The result worth
keeping is the shape. Not one was a wrong rule. Every one was a point nobody had
put on the list: a `break`, a wrapper, a call form, a thunk. And the crossed
cells were each caught by the fix for an uncrossed one, so the grid is
`creation + exit + consumption` rather than the product of the three — the next
pass doesn't have to be combinatorial.

The issue stays open for one cell the audit couldn't reach: a resource crossing
a *task* boundary when the task panics. That waits on
[#299](https://github.com/rask-lang/rask/issues/299) — captures aren't unwound
at all yet — which is v0.5's theme, not this one.

## v0.4 — A value works in every position — **shipped 2026-09-22**

**Done when `tests/matrix/run.sh` is green. It is: 281 of 283 cells clean, 2
registered red, 0 new, and 5 pairs skipped as pairs the design rules out.**

Read the cell count against the old one with care — it was 284 of 286 while
`Vec<Heap<T>>` still compiled. Fixing [#1245](https://github.com/rask-lang/rask/issues/1245)
turned three of those cells from "should be rejected and isn't" into pairs that
aren't legal Rask, so they moved to `gen.py`'s SKIPS with `std.collections/C4`
next to them. Fewer cells, one more enforced rule.

These read as unrelated bugs and aren't. A closure works as a local and not out
of a `Map`; a function works as an argument and not as a struct field. Nothing
enumerated value-kind × position, so the holes were found one report at a time.
The deliverable is the matrix — every payload kind in every carrier, one small
program per cell, run on both backends — and then the bugs it lights up.

The matrix exists: `tests/matrix/gen.py` writes 283 cells over 18 payloads and
16 carriers, `tests/matrix/run.sh` runs them, and `tests/matrix/known_red.txt`
holds each red cell to what it claims — which backend fails it and how far that
backend gets. A registered cell that starts passing is reported so the line gets
pruned.

It lit up ten bugs across 25 cells on its first full run, and they are fixed:

[#1234](https://github.com/rask-lang/rask/issues/1234) ·
[#1239](https://github.com/rask-lang/rask/issues/1239) ·
[#1151](https://github.com/rask-lang/rask/issues/1151) ·
[#1235](https://github.com/rask-lang/rask/issues/1235) ·
[#1237](https://github.com/rask-lang/rask/issues/1237) ·
[#1238](https://github.com/rask-lang/rask/issues/1238) ·
[#1240](https://github.com/rask-lang/rask/issues/1240) ·
[#1241](https://github.com/rask-lang/rask/issues/1241) ·
[#1236](https://github.com/rask-lang/rask/issues/1236) ·
[#1242](https://github.com/rask-lang/rask/issues/1242) ·
[#1243](https://github.com/rask-lang/rask/issues/1243)

Plus two the matrix doesn't reach, found the same week and the same theme:
[#1232](https://github.com/rask-lang/rask/issues/1232), a string field assigned
through an inline lock chain, and
[#1228](https://github.com/rask-lang/rask/issues/1228), a closure in a struct
field leaking once the struct is a `Vec` element.

**[#1046](https://github.com/rask-lang/rask/issues/1046) closed, and not the way
it was written.** It was filed as "the adapters wait for `Vec.iter()` to return a
`Sequence`". `.iter()` is gone instead (SEQ48) — a collection is its own chain
head, so `v.filter(p)` and `for x in v` are the spellings and there is no second
call whose return type needs changing. The adapters landed, `v.filter(p)` hands
back a `Sequence<T>` rather than a second `Vec`, and fusion is untouched:
`v.filter(p).to_vec()` is the same index loop with no closure it always was.

The last thing left under that number was a value failing in a position, which
is why it belonged here. A method call on the element of a Rask-bodied sequence
died in lowering — `for w in words.as_sequence() { w.len() }` gave "method `len`
on receiver of unresolved type". MIR knew a `Sequence<T>` holds `T`; the
checker's own `container_elem_type` special-cased `Iterator` and `Range` and let
`Sequence` fall through to "element type isn't readable from here", so the loop
variable kept a free type variable and dispatch had nothing to name. One arm,
four shapes in `t34_vec_as_sequence.rk`.

Two things that carried #1046's number are not it and are filed on their own:
[#1324](https://github.com/rask-lang/rask/issues/1324), the interpreter writing a
`mutate` parameter back before a lazy chain has run, and
[#1325](https://github.com/rask-lang/rask/issues/1325), a `Vec` not filling a
`Sequence<T>` parameter because SEQ48's coercion only runs inside Vec's own
adapter bodies.

**Four came out of this list.** Same rule the three below came out under — a
version is one theme, and a question `specs/` doesn't answer isn't a bug in it:

- **#1244** — can a function type be the success side of `T or E`?
  `-> func(i64) -> i64 or Oops` parses as `func(i64) -> (i64 or Oops)`, which is
  a defensible reading; what is missing is a way to write the other one, and
  there is no parenthesised type form. `specs/types/error-types.md` and
  `specs/SYNTAX.md` don't cover it. Its two cells stay registered in
  `known_red.txt` — the gate keeps watching them — and they are not what this
  milestone closes on.
- **#1245** — `Vec<Heap<i64>>` compiles, where `std.collections/C4` says it
  shouldn't. A missing *rejection* is not a value failing in a position, and its
  cells could never go green: the program isn't supposed to compile at all.
- **#1233** — an array literal only takes its shape from an annotated `let`. An
  inference gap that shows up in eight cells and is the reason `gen.py` writes
  `Vec.from([…])`; the carriers still measure their carriers.
- **#1248** — a generic function whose name ends in `_free` leaks the `Vec` it
  returns. A name collision in the ownership metadata, not a position.

Three of those four are fixed anyway, off the backlog rather than off this
milestone — #1245, #1233 and #1248 all came in with #1298. #1244 is the one
still open, and it was briefly closed by that same batch without anything in it
answering the question; the repro still fails and it has been reopened. Its two
cells are the 2 red in the number above.

**What this list used to say.** It named eight. Two were already closed when
the milestone was written (#843, #886), and three were not bugs at all — they
were questions `specs/` didn't answer, which the "Not in any version" section
below says don't belong in a version. They were answered first, before the
matrix work, rather than left to stall it:

- **#1152** — is `h.run(5)` on a function-typed field a call? No
  (`type.structs/M6`). A struct of functions is a shape Rask answers with a
  trait, and it appears in no spec, no validation program and no stdlib module;
  the error says the name is a field and how to call it instead. `M7` — one
  name per member — is separate and did land, and found three collisions on its
  first run (`Range.step`, `Command.args`, `Session.id`).
- **#1079** — what does consuming a `const` mean? It can't be consumed
  (`mem.ownership/O11`). The error lands at the consume rather than at the next
  read, which is what made it come and go per function. `PM6c` came with it:
  `take` on a type that is always Copy is a signature that lies, so it is
  rejected too.
- **#869** — must a returned closure own its captures? No: it may borrow the
  function's lent parameters, not its locals (`mem.closures/SL3`), and the
  limit travels to the caller (`SL4`). Requiring `own` would have cost every
  adapter chain a `take self`. The fix needed `spawn` to stop declaring that it
  borrows a closure it keeps, and the ownership checker to be handed the stdlib
  signatures it had never seen.

**#985 went to the backlog.** It's a `Vec` layout and raw-pointer-width
disagreement, not a value kind failing in a position — a narrow theme is the
only kind that closes.

## v0.5 — Concurrency you can trust

**Done when two numbers hold in CI:**

- **The stress gate finds no deadlock, race or lost panic** across its seeds and
  its soak.
- **OS threads never exceed the worker count** under the soak: 100k tasks,
  nested joins, `workers: 1`, panics mixed in. It holds on Linux since the
  fiber switch. Before it, a worker blocked in join got a replacement thread
  (up to 32), which is how [#1130](https://github.com/rask-lang/rask/issues/1130)
  was first fixed, and a blocked receive kept its worker
  ([#1353](https://github.com/rask-lang/rask/issues/1353)).

Fibers are in this version, not after it. Writing them is cheap; knowing they
work is what costs, so the bench comes first and has to fail on today's runtime
before any fiber code lands. "Fibers work" then means those checks turned green.

The payoff is one scheduler instead of three. `green.c` ran spawned closures
as poll functions that ran to completion (it runs fibers now), `thread.c`
gives `Thread.spawn` a pthread, and `green_threads.c` stands in with threads
off Linux. Sim mode
([#1337](https://github.com/rask-lang/rask/pull/1337)) adds a fourth shape: it
builds without `green.c` and passes a baton between OS threads. With fibers,
sim is the real scheduler with one worker and a seeded pick of the next fiber,
so the deterministic tests run the code that ships.

### The bench

1. **Sim over many seeds.** Every concurrency and panic suite file, with the
   deadlock report and a replay line on failure. `rask test --sim` is most of it.
2. **The thread-count soak.** Real runtime, reads `/proc/self/task`, fails the
   moment the count passes the worker count.
3. **Fiber-aware checkers.** The runtime under TSan, with each switch announced
   (`__tsan_switch_to_fiber`), and fiber stacks registered with valgrind so
   `tests/memcheck_gate.sh` doesn't drown. This is the leg that catches deque
   races.
4. **Hostile cases.** Overflow on a fiber stack hits a guard page and panics
   instead of segfaulting. A C call made from a fiber stack. A fiber that moves
   workers mid-function still sees its own `errno` and runtime thread-locals.
   `RASK_POISON_STACK` covers each new fiber stack.
5. **Both architectures.** `fiber_switch` is assembly per target. Linux CI is
   x86_64 and the macOS job is aarch64, but that job only builds and links one
   program today. The fiber tests have to run there too.

### Order

1. Bench legs 2 and 3, failing on today's runtime. Done: the soak held 1 of
   5 programs in budget on the thread-per-join runtime, and `tests/tsan_gate.sh`
   was clean. Both run in CI as `gates-concurrency`.
2. Cooperative fibers. **Done on Linux:** a task parks in join, channel, lock
   and sleep; the join helper threads are gone; the soak holds 5 of 5 and TSan
   is clean with every switch annotated. A started fiber stays on its worker
   (`conc.runtime/S3a`). The dead poll-function path is deleted
   ([#1336](https://github.com/rask-lang/rask/issues/1336)). Still to go in
   this step: I/O parking (stdlib I/O still blocks the worker), and macOS,
   which needs a kqueue backend before `green_threads.c` can go. The aarch64 switch
   is written and assembles; nothing has run it yet.
3. Sim on fibers, replacing the baton.
4. Preemption last. Codegen puts a flag check in every function prologue, and
   a loop that never calls anything gets a signal instead (`conc.runtime/P2`),
   so it touches the compiler, not only the runtime. Its test: a task spinning
   in a loop doesn't stop another task from finishing.

### Bugs in the theme

What the bench finds joins this list. Fixed in #1344: #1311 (the closure form
of a blocking `Shared` access is rejected, E0897), #1335 (`rask compile` hung
on a reassigned closure), #1342 (select parks), #1353 (a blocked receive held
its worker), and three found on the way — an `ensure` running after its value
was consumed, past the 256th ensure and after a `join` whose result returned
early, and E0353 on recursion through a spawned closure. Open:

- [#1218](https://github.com/rask-lang/rask/issues/1218): rare double free, two
  tasks over one `Shared` plus a channel.
- [#1302](https://github.com/rask-lang/rask/issues/1302): a `Shared` holding a
  `Shared` leaks the inner box.
- [#891](https://github.com/rask-lang/rask/issues/891) and
  [#1288](https://github.com/rask-lang/rask/issues/1288): `join_all` takes a
  `Vec` of task handles, which can't be built. Native `TaskGroup` is the answer,
  since spawning N tasks in a loop is the common case and a variadic call can't
  express it.
- [#830](https://github.com/rask-lang/rask/issues/830): a `Link` captured by
  `spawn` lets two tasks write one node. Reject the capture now; snapshot versus
  read-only links can wait.
- [#298](https://github.com/rask-lang/rask/issues/298) and
  [#299](https://github.com/rask-lang/rask/issues/299): panic leftovers,
  `staged()` the main one. #298's last case goes when Pool does
  ([#1296](https://github.com/rask-lang/rask/issues/1296)).
- [#890](https://github.com/rask-lang/rask/issues/890): can no longer be
  written. Close it.
- [#1354](https://github.com/rask-lang/rask/issues/1354): a real deadlock
  hangs silently; report it once every task is parked.

## v0.6 — The stdlib matches its own spec

**Done when the stdlib coverage gate reads 100% for every module.**

That gate doesn't exist yet, and building it is the first deliverable. It
compares each module's spec'd surface against what exists and runs; a function
reaches 100% by being implemented or by the spec dropping it.

Today [TODO.md](TODO.md) claims coverage per module between 40% and 90%. Those
numbers are typed by hand and checked by nobody, which is the same shape as the
leak gate before it measured. The first deliverable here is the gate, not the
missing functions; the percentages will move on their own once they're real.

[#726](https://github.com/rask-lang/rask/issues/726) ·
[#980](https://github.com/rask-lang/rask/issues/980) · the module gaps in
[TODO.md](TODO.md)

## Cadence

A version ships the day its gate hits its target. Not on a date, and not when
the list feels done.

If the gate number stops moving across a run of merged work, the theme was too
wide. Cut what's green, ship it, carry the rest into the next version. A theme
that can't close is a planning mistake, not a work mistake.

**Release more often than feels necessary.** Nobody depends on this yet, so a
release costs nothing — and it buys the only end-to-end test there is. v0.2.0's
smoke step caught five bugs that twelve green gates had missed, and every one of
them had been sitting in `main` for months.

**Run the release build nightly, and throw the artifacts away.** Not a published
nightly — there's nobody to download it. This is a gate: the `build` job's two
legs on a schedule, each binary compiling a hello-world from an empty directory,
nothing uploaded.

It earns its place on a narrow but real gap. Of the five breaks that held v0.2.0
up, three are now caught on every PR — two by the macOS runtime job, one by the
clang gate. The other two only showed up when a packaged binary *linked a
program* on macOS, and nothing does that outside the release workflow. So they
waited for release day, having sat in `main` for months.

macOS runners bill at 10×, so this is a real cost — roughly an hour of billed
macOS time a night. Yesterday cost five pull requests and a day.

A published rolling `nightly` prerelease is the obvious next step once someone
wants to try `main` without building it. Not yet.

## v1.0

Years away, and it isn't a date — it's a promise that what's in `specs/` won't
change under you.

[CLAUDE.md](CLAUDE.md) currently says the opposite: nothing is stable, backward
compatibility is never a reason for anything. That's the right setting for now,
and v1.0 is exactly when that sentence has to change. Which is why it can't be
scheduled — only earned. What has to be true first:

- **`specs/` has stopped moving.** No normative change across several
  consecutive releases, measured with `git log specs/` rather than by feel. This
  is the real gate and the others are downstream of it: a language is 1.0 when
  it has stopped changing, not when it is popular.
- Every design question closed rather than deferred. Twenty are open, and each
  one is a spec that hasn't stopped moving yet.
- The stdlib at 100% of its own spec, measured.
- No untracked bugs, and nothing registered red without an issue and a decision.

Adoption is not on that list. Zig has Bun, TigerBeetle and Ghostty built on it
and is still 0.x, which settles the question: people shipping real work on a
language says nothing about whether the language is finished. What adoption does
buy is *discovery* — you find out a spec is wrong because someone hit it. Until
there is someone, the validation programs, the agent benchmark and the corpus are
standing in for that, and they are a weaker instrument. Spec stability measured
against a language nobody exercises is stability by neglect.

**The minor number is a counter, not a measure.** Don't try to land v1.0 at a
tidy number, and don't slow down to keep it low. Ship monthly through the years
v1.0 needs and you arrive in the dozens; ship weekly and it's the hundreds.
That's arithmetic, not ambition — 0.50 says nothing bad about a language, and
0.9 would say nothing good.

Don't plan past the next two versions. v0.9's contents are fiction today.

## Not in any version

**Design questions** — twenty open issues. They're upstream of features, they
have no gate, and putting them in a version is how a version stops closing. Work
them between releases, or when one blocks a scheduled feature, and say which.

**Patch releases** — a bug that makes the shipped binary unusable gets an x.y.z
off the release branch and doesn't wait for a theme.

## Positions that aren't changing

**No LLVM backend until something measures slow.** The largest class of bug in
this project is the two backends disagreeing — 39% of open bugs when
[#724](https://github.com/rask-lang/rask/issues/724) was written. A third thing
that produces an answer is a third thing that can disagree. Cranelift already
reaches ARM and WASM, so "more targets" isn't the argument; generated-code speed
is, and that's a decision for a benchmark to make.

**The five validation programs stay green.** An HTTP JSON API server, a grep
clone, a game loop, a text editor with undo, an embedded sensor processor. Each
is in a gate with its output compared across both backends. When one of these
gets worse, that outranks whatever version is in flight.

## Post-1.0

LLVM if the benchmarks ask for it · macros / `format!` · comptime debugger ·
fuzzing · code coverage · `std.reflect` · inline assembly · pointer provenance ·
`compile_cpp()` · cbindgen wrapper generation · platform-specific deps and
multi-target builds

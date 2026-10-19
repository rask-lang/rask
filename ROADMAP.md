# Rask Roadmap

**A version ships when a number a script prints hits its target.** Not when a
list feels done.

Bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). Unscheduled
work is in [TODO.md](TODO.md).

## Why this shape

The old roadmap answered "what next" and never "when is this finished", so
everything was 60–90% done and nothing was shippable. It also kept two
scoreboards, and the status line quoted the flattering one.

What has worked here is making a claim measurable. The leak gate reported zero
for months because it grepped output `rask test` was throwing away; measured
honestly it said 173, and 173 became 0. The memcheck gate found a live bug on
its first run that twelve green gates had missed. Every time a claim became a
number, the number moved.

So each version is one theme and one number.

**The rule that makes it finish:** a bug found inside the current version's theme
joins that version. A bug outside it goes to the backlog and does **not** delay
the release.

## Where things stand

Measured 2026-10-19 on `main` at `46870289`, each line by its command.

| Measure | Now | Command |
|---------|-----|---------|
| Suite programs agreeing on both backends | 753 green, 2 registered red (both unbuilt features: SIMD values, gradual generalization), 0 known divergences | `tests/differential.sh` |
| Programs that leak | 0 allocations, 0 deferred, 753 clean | `tests/leak_gate.sh` |
| Programs memcheck finds an error in | **6 registered**: #1587 (4 files), #1588, #1590; 0 new | `tests/memcheck_gate.sh` |
| Matrix cells clean on both backends | 293 of 293, 13 pairs skipped | `tests/matrix/run.sh` |
| Concurrency files TSan reports a race in | 0 of 92 | `tests/tsan_gate.sh` |
| Soak programs within their thread budget | 6 of 6 | `tests/soak_gate.sh` |
| Concurrency files clean under sim | 92 of 92 at 20 seeds, 44 exempt | `tests/sim_gate.sh` |
| Examples / projects / prototypes | 38 ok / 22 ok / 9 agree | `tests/{examples,projects,prototypes}_gate.sh` |
| Formatter round-trip, HTTP server, agent bench | ok / ok / ok (1 quarantined) | `tests/{fmt_roundtrip,http_api_harness,agentbench}_gate.sh` |
| Runtime under clang | ok, 3 warnings | `tests/clang_gate.sh` |
| Compiler unit tests | 1539 pass, 0 fail | `cargo test --release` |
| Open issues | 82; 28 labelled bug, 11 newest not yet labelled (at least three are bugs) | issue search |
| Open design questions | 18 | issue search |

Not run this pass: the tutorials, book, lint and spellings gates, and the fiber
gate (needs qemu). Their last numbers are not repeated here.

The memcheck line is the one that moved the wrong way. It was an empty ledger
when v0.3 shipped; six files are on it now. The leak gate still says 0, and it
should: leaks and invalid frees are different failures. But "memory is settled"
was claimed on the strength of one gate, and the other one has found six
native bugs since, all in how a value is freed while something else still reads
it.

## Next: v0.6 — Each fact has one owner

**Done when all fifteen issues under [#1570](https://github.com/rask-lang/rask/issues/1570)
are closed. Today: 10 of 15 open.**

#1570 argues that most bugs here are one thing: a fact an earlier pass already
knew, worked out again later from a name or a hand-kept list, and drifting.
I haven't checked that against every open bug; #1564, #1566 and #1567 are
high-priority and are its root causes.
`Vec.sort()` leaving the vector out of order (#1574) and a `Shared` never
freeing its struct's fields (#1571) are what that looks like from outside.

#1570's own order is the order here: the live bugs first (#1561, #1562 left),
then #1568 (one driver instead of a pipeline assembled by hand in five places),
then #1566 + #1567 (checker and layout hand types and sizes down), then
#1564 + #1565 (callee identity, stdlib facts on the `.rk` declarations), then
#1555 and #1556. #1569 any time. Each root-cause PR lands a guard test in the
style of `no_type_text_reparsing`, so the count can only go down.

The memcheck ledger (#1587, #1588, #1590) rides along: it's in the theme when
the cause is a re-derived fact, and goes to the backlog when it isn't.

## Then: v0.7 — The stdlib matches its own spec

**Done when the stdlib coverage gate reads 100% for every module.**

The gate still doesn't exist (no script under `tests/`, no issue). Building it
is the first deliverable. [TODO.md](TODO.md) still carries per-module
percentages typed by hand, between 40% and 90%. It compares each module's
spec'd surface against what exists and runs; a function reaches 100% by being
implemented or by the spec dropping it.

This moved behind v0.6 because #1565 puts stdlib facts on the `.rk`
declarations, and a coverage gate reads exactly those. Built first, it would
read the Rust lists #1565 deletes.

[#726](https://github.com/rask-lang/rask/issues/726) ·
[#980](https://github.com/rask-lang/rask/issues/980) · the module gaps in
[TODO.md](TODO.md)

## Shipped

- **v0.3 — Memory is settled** (2026-09-18). Leak gate at 0 allocations. The
  invalid-free half got its own gate (memcheck) the same month; see above for
  how that is going.
- **v0.4 — A value works in every position** (2026-09-22). The matrix is every
  payload kind in every carrier, one program per cell, both backends: 293 of
  293 now.
- **v0.5.0 — Concurrency you can trust** (released 2026-10-08). Fibers, sim on
  the real scheduler, preemption; the stress, soak and TSan gates hold. One
  known issue ships: [#1218](https://github.com/rask-lang/rask/issues/1218), a
  rare double free with two tasks over one `Shared` plus a channel, never
  reproduced (960 runs, memcheck, helgrind all clean). macOS runs tasks on
  threads until it has a kqueue poller.

## What came off this list

- **#882 (linearity audit)** was held open for one cell, a resource crossing a
  task boundary when the task panics, "waiting on #299". #299 closed on
  2026-10-01 with all 11 sub-issues done, and #827 and #1301, the two other
  named holes, closed too. What is left is the tracking issue's own
  bookkeeping, not work. Open on convention.
- **#1244** (function type on the left of `or`) is closed, and the matrix's
  known-red list is empty because of it. The old text said it was reopened.
- **The two registered-red matrix cells** are gone with it.
- **The memcheck ledger being empty** is no longer true. See the table.
- **#299 / #298** closed with v0.5.0.

## Cadence

A version ships the day its gate hits its target. Not on a date, and not when
the list feels done.

If the gate number stops moving across a run of merged work, the theme was too
wide. Cut what's green, ship it, carry the rest into the next version.

**Release more often than feels necessary.** Nobody depends on this yet, so a
release costs nothing, and it buys the only end-to-end test there is: v0.2.0's
smoke step caught five bugs that twelve green gates had missed.

**Run the release build nightly, and throw the artifacts away.** The `build`
job's two legs on a schedule, each binary compiling a hello-world from an empty
directory. It covers what only a packaged binary linking a program on macOS
shows, and macOS bills at 10×, so it's a real cost. Not a published nightly
until someone wants to try `main` without building it.

## v1.0

Not a date. It's a promise that what's in `specs/` won't change under you.
[CLAUDE.md](CLAUDE.md) currently says the opposite (nothing is stable), which is
right for now and is the sentence that has to change at 1.0. What has to be true
first:

- **`specs/` has stopped moving.** No normative change across several
  consecutive releases, measured with `git log specs/`. This is the real gate.
- Every design question closed rather than deferred. 18 are open.
- The stdlib at 100% of its own spec, measured.
- No untracked bugs, and nothing registered red without an issue and a decision.

Adoption is not on the list. What it buys is *discovery*: you find a spec is
wrong because someone hit it. Until there is someone, the validation programs,
the agent benchmark and the corpus stand in, and they are a weaker instrument.

**The minor number is a counter, not a measure.** Don't aim v1.0 at a tidy
number. Don't plan past the next two versions.

## Not in any version

**Design questions.** They're upstream of features and have no gate; putting
them in a version is how a version stops closing. Work them between releases, or
when one blocks a scheduled feature, and say which.

**Patch releases.** A bug that makes the shipped binary unusable gets an x.y.z
off the release branch and doesn't wait for a theme.

## Positions that aren't changing

**No LLVM backend until something measures slow.** The biggest class of bug here
is the two backends disagreeing. A third thing that produces an answer is a
third thing that can disagree. Generated-code speed is the argument, and a
benchmark decides it.

**The five validation programs stay green.** HTTP JSON API server, grep clone,
game loop, text editor with undo, embedded sensor processor. When one gets
worse, that outranks whatever version is in flight.

## Post-1.0

LLVM if the benchmarks ask for it · macros / `format!` · comptime debugger ·
fuzzing · code coverage · `std.reflect` · inline assembly · pointer provenance ·
`compile_cpp()` · cbindgen wrapper generation · platform-specific deps and
multi-target builds

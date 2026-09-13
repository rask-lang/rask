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
| Suite programs agreeing on both backends | 500 of 505, 5 registered red | `tests/differential.sh` |
| Programs that leak | 31, holding 86 allocations | `tests/leak_gate.sh` |
| Examples with a pinned golden | 35 of 37 | `tests/examples_gate.sh` |
| Runtime builds under the other compiler | clean | `tests/clang_gate.sh` |
| Open bugs | 40 of 85 open issues | issue search |
| Open design questions | 20 | issue search |

Nine more gates cover prototypes, packages, projects, tutorials, the book, the
agent benchmark, internal spellings, formatter round-trips and the HTTP server.
All green.

## v0.3 — Memory is settled

**Done when `tests/leak_gate.sh` reports 0 known-leaking. Today: 31.**

Every one of these is the compiler getting *who frees this* wrong. A leak is the
polite version of that mistake; [#1161](https://github.com/rask-lang/rask/issues/1161)
is the same bug releasing early instead, which is a use-after-free. Ownership is
the whole thesis of the language, so this goes first.

[#1035](https://github.com/rask-lang/rask/issues/1035) ·
[#1117](https://github.com/rask-lang/rask/issues/1117) ·
[#1131](https://github.com/rask-lang/rask/issues/1131) ·
[#1139](https://github.com/rask-lang/rask/issues/1139) ·
[#1153](https://github.com/rask-lang/rask/issues/1153) ·
[#1154](https://github.com/rask-lang/rask/issues/1154) ·
[#1157](https://github.com/rask-lang/rask/issues/1157) ·
[#1158](https://github.com/rask-lang/rask/issues/1158) ·
[#1161](https://github.com/rask-lang/rask/issues/1161) ·
[#1162](https://github.com/rask-lang/rask/issues/1162)

[#882](https://github.com/rask-lang/rask/issues/882) is the umbrella: linearity
is enforced at points, and the holes are wherever a point was missed. Closing the
ten above should be most of its answer.

## v0.4 — A value works in every position

**Done when a new positional-matrix gate is green, and `p08_sequence.rk` leaves
`tests/pending_features.txt`.**

These read as unrelated bugs and aren't. A closure works as a local and not out
of a `Map`; a function works as an argument and not as a struct field. Nothing
enumerates value-kind × position, so the holes are found one report at a time.
The deliverable is the matrix — every value kind (closure, container, box,
string, struct, function) in every position (local, struct field, `Vec` element,
`Map` value, return, capture, argument) — and then the bugs it lights up.

[#843](https://github.com/rask-lang/rask/issues/843) ·
[#869](https://github.com/rask-lang/rask/issues/869) ·
[#886](https://github.com/rask-lang/rask/issues/886) ·
[#985](https://github.com/rask-lang/rask/issues/985) ·
[#1046](https://github.com/rask-lang/rask/issues/1046) ·
[#1079](https://github.com/rask-lang/rask/issues/1079) ·
[#1151](https://github.com/rask-lang/rask/issues/1151) ·
[#1152](https://github.com/rask-lang/rask/issues/1152)

[#1151](https://github.com/rask-lang/rask/issues/1151) is the worst of them —
making it compile currently gives a wrong answer.

## v0.5 — Concurrency you can trust

**Done when [#299](https://github.com/rask-lang/rask/issues/299) closes and a
concurrency stress gate runs in CI without deadlocking.**

[#1130](https://github.com/rask-lang/rask/issues/1130) is the one that matters:
a task that joins another deadlocks when every worker is blocked in join. A
language whose pitch includes "no function coloring" cannot have that.

[#298](https://github.com/rask-lang/rask/issues/298) ·
[#299](https://github.com/rask-lang/rask/issues/299) ·
[#830](https://github.com/rask-lang/rask/issues/830) ·
[#890](https://github.com/rask-lang/rask/issues/890) ·
[#891](https://github.com/rask-lang/rask/issues/891) ·
[#1111](https://github.com/rask-lang/rask/issues/1111) ·
[#1130](https://github.com/rask-lang/rask/issues/1130) ·
[#1180](https://github.com/rask-lang/rask/issues/1180)

## v0.6 — The stdlib matches its own spec

**Done when a gate compares each module's spec'd surface against what exists and
runs, and every module reads 100% — or the spec drops the function.**

Today [TODO.md](TODO.md) claims coverage per module between 40% and 90%. Those
numbers are typed by hand and checked by nobody, which is the same shape as the
leak gate before it measured. The first deliverable here is the gate, not the
missing functions; the percentages will move on their own once they're real.

[#726](https://github.com/rask-lang/rask/issues/726) ·
[#980](https://github.com/rask-lang/rask/issues/980) · the module gaps in
[TODO.md](TODO.md)

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

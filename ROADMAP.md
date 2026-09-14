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

**Done when `tests/leak_gate.sh` reports 0 allocations this milestone. Today: 13.**

Every one of these is the compiler getting *who frees this* wrong. A leak is the
polite version of that mistake; [#1161](https://github.com/rask-lang/rask/issues/1161)
is the same bug releasing early instead, which is a use-after-free. Ownership is
the whole thesis of the language, so this goes first.

The gate reports a second number beside that one, and it isn't part of this
milestone. A task killed by a panic doesn't unwind its captures
([#299](https://github.com/rask-lang/rask/issues/299)), which is 13 allocations
across three files and waits on the unwinder in v0.5; `t_shared_box_freed.rk` is
a box held on purpose and will never be zero. Their lines in
`tests/known_leaks.txt` say `deferred`, and the gate still measures them and
still holds them to their count — it just doesn't judge a memory milestone on
what a memory milestone can't fix.

What the 13 are, and which issue owns each:

| Allocations | Issue | What |
|---|---|---|
| 6 | [#1198](https://github.com/rask-lang/rask/issues/1198) | A new container written over a struct field leaks the one the field held |
| 2 | [#1202](https://github.com/rask-lang/rask/issues/1202) | A `Heap` box stored in an enum payload |
| 2 | [#1205](https://github.com/rask-lang/rask/issues/1205) | A closure swallowed by another closure that gets returned |
| 1 | [#1204](https://github.com/rask-lang/rask/issues/1204) | A closure held through a *generic* struct field |
| 1 | [#1200](https://github.com/rask-lang/rask/issues/1200) | A phi operand reads as live out of every predecessor, not its own edge |
| 1 | [#1206](https://github.com/rask-lang/rask/issues/1206) | A return that is fresh on one path and borrowed on the other |

Two of those are rulings rather than fixes: #1202 asks whether `drop(x.field)`
should stop being a consume, and #1206 whether returning a lent container should
compile at all. The rest are engineering, and #1198 is the biggest single piece.

Still open from the earlier list, and no longer showing in the gate — they are
about shapes the suite doesn't cover yet rather than shapes that are fixed:
[#1035](https://github.com/rask-lang/rask/issues/1035) ·
[#1117](https://github.com/rask-lang/rask/issues/1117) ·
[#1131](https://github.com/rask-lang/rask/issues/1131) ·
[#1153](https://github.com/rask-lang/rask/issues/1153) ·
[#1157](https://github.com/rask-lang/rask/issues/1157) ·
[#1158](https://github.com/rask-lang/rask/issues/1158) ·
[#1161](https://github.com/rask-lang/rask/issues/1161)

[#882](https://github.com/rask-lang/rask/issues/882) is the umbrella: linearity
is enforced at points, and the holes are wherever a point was missed. Closing the
six in the table should be most of its answer.

## v0.4 — A value works in every position

**Done when a new positional-matrix gate is green.**

These read as unrelated bugs and aren't. A closure works as a local and not out
of a `Map`; a function works as an argument and not as a struct field. Nothing
enumerates value-kind × position, so the holes are found one report at a time.
The deliverable is the matrix — every value kind (closure, container, box,
string, struct, function, and a `Sequence` over `Vec.iter()`) in every position
(local, struct field, `Vec` element, `Map` value, return, capture, argument) —
and then the bugs it lights up. Sequence is in there because
[#1046](https://github.com/rask-lang/rask/issues/1046) is the same shape: the
adapters are written and work, and `Vec.iter()` not returning a `Sequence` is the
position they can't occupy.

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

**Done when a concurrency-and-panic stress gate runs in CI without deadlocking.**

One gate, covering both, because they're the same programs: a task that panics
while another is blocked joining it is where
[#299](https://github.com/rask-lang/rask/issues/299)'s panic semantics and
[#1130](https://github.com/rask-lang/rask/issues/1130)'s deadlock meet.

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

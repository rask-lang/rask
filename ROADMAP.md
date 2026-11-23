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

Measured 2026-11-23 on `53a80616`. Re-measure these rather than trusting them;
each line names the command.

| Measure | Now | Command |
|---------|-----|---------|
| Suite programs agreeing on both backends | 989 green, 9 registered red (4 fail on both backends) | `tests/differential.sh` |
| Programs that leak | 1, holding 40 allocations (#1609), 0 deferred; 6 not measured (build fails) | `tests/leak_gate.sh` |
| Matrix cells clean on both backends | 284 of 284, 0 red, 22 pairs skipped | `tests/matrix/run.sh` |
| Programs memcheck finds an error in | 1 known-bad (#1588), 0 new, 6 not measured | `tests/memcheck_gate.sh` |
| Concurrency files TSan reports a race in | 0 of 125 | `tests/tsan_gate.sh` |
| Soak programs within their thread budget | 6 of 6 | `tests/soak_gate.sh` |
| Concurrency files clean under sim | 125 of 125 at 20 seeds locally (CI runs 100), 46 exempt | `tests/sim_gate.sh` |
| Examples with a pinned golden | 38 of 38 | `tests/examples_gate.sh` |
| Runtime builds under the other compiler | clean (3 warnings) | `tests/clang_gate.sh` |
| Stdlib items a spec promises that exist and run on both backends | 873 of 873 | `tests/stdlib_coverage_gate.sh` |
| v0.7 restated-fact sites | 222, 215 to go | `cargo test --release -p rask-cli --test one_owner_sites` |
| Open bugs | 197 labelled `bug`, of 281 open issues | issue search |
| Open design questions | 18 labelled `design` | issue search |

Prototypes (9 agree), projects (22 ok), formatter round-trips, the HTTP server,
the agent benchmark (19 green) and `cargo test --release` (1681 passed, 0
failed) were re-run too. All green. The tour, book and spellings gates were not
re-run this pass.

**Every gate is green and the bug count is still climbing.** 165 open bugs
became 197: 68 filed since 2026-11-16, 51 closed. The leak and memcheck gates
only see what the suite runs, so their green says the suite is clean, not that
memory is. A bug gets counted once it has a suite file.

The six "not measured" programs are registered red files that don't build. The
gates can't see them until they do.

## Shipped

Each version shipped the day its gate hit its target. The full story of each is
in git history and its issues; what's worth carrying forward is below.

- **v0.3, memory is settled** (2026-09-18). `tests/leak_gate.sh` at 0. The gate
  runs suite files as `test` blocks, so a leak that only shows in `main` is
  invisible to it (#1213 lived there). `tests/memcheck_gate.sh` came with it and
  found a live bug in its first minute. #882 stays open for one cell: a resource
  crossing a task boundary when the task panics.
- **v0.4, a value works in every position** (2026-09-22). `tests/matrix/run.sh`
  green. The lesson: the holes were never a wrong rule, always a position nobody
  had listed, and crossed cells were caught by fixing uncrossed ones.
- **v0.5, concurrency you can trust** (2026-09-30). Fibers, sim on fibers,
  preemption, TSan, soak. Shipped open: #1218 (rare double free, two tasks over
  one `Shared`, won't reproduce on demand) and macOS, which runs tasks on
  threads until it has a kqueue poller. Neither has moved; there is still no
  kqueue code in `compiler/runtime/`.
- **v0.6, the stdlib matches its own spec** (2026-10-22). Coverage gate at 100%
  with an empty gap registry. It has grown since, from 855 promised items to 873,
  and stays at 100%.

## Next: v0.7 — Each fact has one owner

**Done when `cargo test -p rask-cli --test one_owner_sites` reads every
ratchet at its target. Today: 222 sites restate a fact, 215 to go. Same as
2026-11-16.**

The rule is that each fact has one owner, the earliest stage that knows it, and
flows down as data. Most bugs in this compiler are one fact worked out twice and
drifting. The audit is [#1570](https://github.com/rask-lang/rask/issues/1570).

| Restated fact | Today | Target | Issue |
|---|---|---|---|
| Hand-written codegen dispatch entries (`internal_entries`) | 209 | 0 | #1555 |
| Public entry points into the passes | 13 | 7 | #1568 |
| Name sniffs in MIR and codegen | 0 | 0 | done |
| Method names the checker answers by hand | 0 | 0 | done |
| `_Any` wildcards in the checker | 0 | 0 | done |
| MIR metadata rows keyed by spelling | 0 | 0 | done |
| Interpreters run without the driver's setup | 0 | 0 | done |
| Size or alignment computed outside the layout model | 0 | 0 | done |

**The number didn't move across eleven merges, and that's the finding.** The
week's work was real one-owner work: lowering reads callee signatures from the
checker's resolved call (#1988), asks the checker what type a name has instead
of guessing it from the code (#1994), and every call carries what it does with
its arguments (#1979). #1565 closed. None of it is counted, because six of the
eight ratchets were already at zero and the two that aren't measure something
else. So 209 of the 215 sites left are one issue, #1555, and the rest is six
entry points.

By the cadence rule below, a number that stops moving means the theme is too
wide. I don't think that's it here. The theme is fine; the ratchets have run
out of things to count except the one big table. What's left is narrow: #1555.

**Order.**

1. **#1555.** MIR states what an assignment and a runtime call do; codegen
   translates. 209 of 215 sites, and it's the last thing #1556 (comptime on the
   MIR interpreter, the likely v0.8) waits on. The prerequisites it needed are
   in: calls carry their callee symbol and a `CallShape`, and stdlib facts live
   on the `.rk` declarations. Method: tally which rows the corpus hits, move
   each into lowering, delete the row, lower the ratchet.
2. **#1568**, the six extra pass entry points. Small and separate; a good lane
   beside #1555.
3. **#1569** any time.

**Off the critical path.** Work left under the other issues that no ratchet
counts. It doesn't hold v0.7, by the same rule that keeps bugs outside the theme
out of it. Each either gets a ratchet or gets done; it doesn't get to stall the
number.

- #1564: the closure analysis still reads callee bodies
  (`build_callee_escape_map` in `rask-mir/src/closures.rs`) to decide whether a
  closure passed to a function can escape through it. Replacing it with the
  parameter mode waits on a design answer: may a function store a closure it
  only borrowed? Today it may, because a closure is Copy, so "borrowed" doesn't
  promise "not kept".
- #1566, #1567: small leftovers (a `catch` arm picks its side by type; mono
  re-resolving field syntax).
- `rask-codegen` and `rask-mir` still depend on `rask-check`. Taking that edge
  away is what makes the layering hold without a grep test; `crate_layers`
  gets the edge when it goes.

**How each PR goes.** Tally which fallback answers on the whole corpus, close
the gap upstream, delete the fallback, lower the ratchet. A PR that adds the new
field and leaves the old sniff beside it "for now" doesn't merge.

## What came off this list since the last pass

- **v0.7 steps 1–5 are done or done enough not to lead.** #1565 closed
  (stdlib facts live on declarations; `func_sigs`, `is_io_source` and
  `elem_strs::CTORS` are gone). Callee identity is by resolved symbol. The
  crate split has its `crate_layers` test. What's left of them is listed above
  as off the critical path.
- **#1561's ratchets are at zero.** Both bugs it was filed for now fail to
  check: `"s".contains(5)` is a type mismatch and `Duration.from_millis` is
  "no method". The issue is open on its notes about `Iterator` return types
  and SIMD, not on checker tables.
- **Two of three leaks:** #1902 and #1927 have suite files that run leak-clean,
  and `known_leaks.txt` holds only #1609. That was already true at the last
  pass; this file still counted three. Both issues are still open.
- **#1834**, `T or E` match arms routed by name, is closed.
- **#1601**, an imported `File` closed twice natively, is fixed by resolving
  calls by symbol. `t_fs_handle_imported_ensure.rk` came off both the
  divergence and memcheck ledgers.

Ordering changed: #1555 leads now, where it was step 6. Steps 2–5 were ahead of
it because each delivered a fact #1555 needed, and they've delivered it.

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
- Every design question closed rather than deferred. Eighteen are open, and each
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

**Design questions**: eighteen open issues. They're upstream of features, they
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

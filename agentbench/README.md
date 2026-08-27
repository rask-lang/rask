# agentbench

Have models write Rask against this compiler, measure how fast they converge,
and keep the failures readable.

NORTH_STAR says decisions get made by measurement: "have models write Rask
against the compiler, measure convergence, and read the failure transcripts."
This is that instrument. It exists because "the native compiler is stable" had
no exit criterion — every gate was green while `match n { 1 => 2.5, _ => 0.0 }`
returned 2. A bug count doesn't say what fraction of ordinary programs compile
and run correctly. This does.

## Running it

```bash
cd compiler && cargo build --release -p rask-cli   # the benchmark needs a compiler

agentbench/bench.py tasks                          # what's in the set
agentbench/bench.py selftest                       # free — do the references still build?
agentbench/bench.py run --model mock:solves@2      # free — exercises the whole loop
agentbench/bench.py run --model cli:claude-sonnet-5 --yes-spend
agentbench/bench.py report agentbench/runs/<run>   # re-score old transcripts, no new spend
```

A real model costs money, so `run` prints an estimate and refuses without
`--yes-spend`. `--select day`, `--limit 3` and `--max-attempts 2` are the knobs
for a cheap partial run.

Everything a run produces lands in `agentbench/runs/<stamp>-<model>/`, which is
gitignored: `run.json` (everything, machine-readable), `report.md` (the scores),
and `transcripts/<task>.md` (one readable log per task — prompt, every attempt,
every diagnostic).

## What a task is

A directory under `tasks/` with three files:

| file | what it is |
|---|---|
| `task.toml` | the prompt, the contract the solution must satisfy, metadata |
| `verify.rk` | `test` blocks that exercise the contract — the model never sees these |
| `reference.rk` | a hand-written solution |

The model gets `LANGUAGE_GUIDE.md` as its system prompt, the task prompt, and
the contract. Its reply is glued in front of `verify.rk` and the whole thing goes
through `rask test --interp` and `rask test`. Both green and identical, or the
attempt failed and the compiler's own words go back as the next turn.

**The reference is what makes the numbers mean anything.** Without it a red task
could be "the model can't write this" or "the compiler can't build this" and
there's no telling from the outside. `selftest` runs every reference on both
backends; a task whose reference is red goes in `quarantine.txt` with an issue
number and drops out of the scored set until the bug is fixed. Same contract as
`tests/known_divergences.txt`, including the UNEXPECTED PASS that tells you to
prune the line.

Granularity matches `tests/suite/`'s day/week/month files rather than the five
validation programs in `examples/` — a task should be one sitting, not a
project.

## What it measures

| metric | reading |
|---|---|
| **solve rate** | fraction correct on both backends inside the attempt budget |
| **pass@1** | fraction correct on the first try — how well the language reads, before the error messages get a chance to help |
| **convergence** | mean attempts among solved tasks. 1.0 is perfect; a number climbing over releases means the language got harder to hit |
| **divergences** | attempts where the two backends disagreed |
| **thrash** | tasks where two attempts in a row failed the same way with the same error codes |
| **teach rate** | per error code: of the times it appeared with a retry after it, how often was it gone next time |

Two of those are not about the model at all.

**Divergences are compiler bugs the benchmark found.** They're reported
separately, never scored against the model, and each one belongs in
`tests/suite/` as a probe. The first task set turned up two on the way in:
#1000 (a `const [string; N]` element reads back as garbage natively) and #1002
(a method on a union-narrowed error dispatches against the whole union).

**Teach rate is a diagnostics metric.** NORTH_STAR commitment 4 says every rule
the compiler enforces is a rule it explains. A code that keeps appearing and
keeps not getting cleared is a message that didn't teach — that's a bug to file
against the diagnostic, not a model to blame.

## Targets

`report.md` grades every run against four numbers, in the style of
`specs/METRICS.md`:

```
ASR  agent solve rate      solved / total, day+week only     ≥ 0.95
AC   agent convergence     mean attempts among solved         ≤ 1.5
BD   backend divergence    tasks that hit one / total         = 0
TR   teach rate            per code seen 3+ times             ≥ 0.7
```

ASR excludes the month tasks on purpose. Those are the frontier; holding them to
the same bar would make the number say "the hard tasks are hard" rather than
"ordinary programs work".

**BD is the exit criterion "the native compiler is stable" never had.** A run
where models write nineteen ordinary programs and not one of them exposes a
backend disagreement is evidence in a way a bug count isn't. Today it fails
before a model is even called: two of the nineteen references tripped a
divergence while being written (#1000, #1002).

## The card is under test too

The system prompt is `LANGUAGE_GUIDE.md`, unedited. Where the card and the
compiler disagree, models believe the card and burn an attempt — which is the
measurement, so the card doesn't get patched to make the numbers look better.
Live example: the card says `Error` is auto-derived for enums and the compiler
rejects every error enum without a hand-written `message` (#1001). Anything
touching `T or E` pays for that on attempt one.

## Model adapters

```
mock:reference     emits the reference immediately        free
mock:solves@N      fails N-1 times, then solves           free
mock:garbage       never produces anything buildable      free
cli:<model>        `claude -p`, using the machine's auth  paid
api:<model>        Anthropic messages API                 paid
```

The mocks exist so the loop — prompting, assembly, compilation, scoring,
transcripts — can be exercised end to end for nothing. `mock:garbage` emits
Rust-shaped code (`fn`, `let mut`, `Ok(...)`), which is what a model actually
gets wrong here, so the harness gets tested against realistic diagnostics.

## Adding a task

1. `mkdir tasks/<horizon>_<name>` and write the three files.
2. `agentbench/bench.py selftest --select <name>` until the reference is green
   on both backends.
3. If it can't go green, that's a compiler bug: file it, add a probe to
   `tests/suite/`, and put the task in `quarantine.txt` against the issue.

Keep a task small enough to be one prompt and specific enough that the contract
pins the API. `verify.rk` should test behaviour the prompt actually states —
a test the prompt didn't call for measures the model's mind-reading, not its
Rask.

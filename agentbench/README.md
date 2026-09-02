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

A real model spends something, so `run` prints an estimate and refuses without
`--yes-spend`. `--select day`, `--limit 3` and `--max-attempts 2` are the knobs
for a cheap partial run.

## Running it on a Claude subscription

`cli:<model>` shells out to `claude -p` and reuses whatever login the machine
has. On a Pro/Max plan that login *is* the subscription, so a run spends plan
quota and no API credit — no key, no billing setup, nothing to buy:

```bash
claude auth login      # once, if the machine isn't logged in
agentbench/bench.py run --model cli:claude-sonnet-5 --yes-spend
```

`run` asks `claude auth status` before spending anything and stops with the
login command if the machine isn't signed in — eighteen tasks each discovering
the same missing login makes for a confusing report. That's also where the
billing label comes from: `authMethod: oauth_token` is a plan, and a key reached
through `apiKeyHelper` is metered even with nothing in the environment.

Three things make this work rather than half-work.

**An API key silently wins over the subscription.** If `ANTHROPIC_API_KEY` is
exported, `claude` bills the key and the plan sees nothing. The adapter drops
it (and `ANTHROPIC_AUTH_TOKEN`) from the child's environment, so a run on a
developer machine with a key lying around still spends plan quota. Set
`RASK_BENCH_CLI_API_KEY=1` to keep the key and bill it instead. The run header
says which one it used.

**The child gets nothing from this machine.** `--safe-mode --tools ""` and
friends: no CLAUDE.md, no skills, hooks, plugins, MCP servers, or settings, and
no tools at all — the model writes Rask from the card or it doesn't. That's
required for the measurement to mean anything, and it's also 24k tokens of
plan quota per call that would otherwise go on Claude Code's own system prompt
and tool definitions. Measured: 24,288 tokens per call before, 243 after.

**A usage limit is not a failed task.** Hitting the rolling window mid-run used
to record `model_error` and score the task as unsolved, so a plan limit read as
"the language is hard". Now a rate limit is waited out with backoff (a wait
never consumes an attempt), and a task the provider never answered for is
marked *not scored*, listed separately in `report.md`, and left out of every
denominator. The first task to conclude the window is gone trips a shared latch
so the rest abort immediately instead of each grinding through its own backoff:
the partial run is scored and written in seconds. `--model-retries`,
`--retry-wait` and `--max-wait` tune it.

For a subscription the report says *list-price equivalent* rather than *cost* —
the CLI prices every call at API rates, which is a useful number, but no money
changed hands. Token counts include cached input, because cached reads come off
the plan window too.

Rough size, so nobody wipes out their own window by accident: the full
eighteen-task set at three attempts is about **335k tokens** (277k in, 57k out
on the first Sonnet run) and takes ~11 minutes at `--jobs 4`. `--select day`
is about a fifth of that.

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
cli:<model>        `claude -p`, using the machine's auth  plan quota
api:<model>        Anthropic messages API                 API credit
```

The mocks exist so the loop — prompting, assembly, compilation, scoring,
transcripts — can be exercised end to end for nothing. Run one by hand when you
change the harness; CI doesn't, on purpose — the gate there checks the reference
solutions and nothing else, so a green build never depends on benchmark
plumbing. `mock:garbage` emits
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

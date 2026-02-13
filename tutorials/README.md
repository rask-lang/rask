# Rask Tutorials

Hands-on challenges to learn the language. Each level introduces new concepts with
the reference material you need right there — no spec-hunting required.

**Setup:**

```bash
# Build (if you haven't)
cd compiler && cargo build --release

# Add to PATH
export PATH="$PWD/target/release:$PATH"
```

Create your files in a `tutorials/solutions/` folder (or wherever you like).

```bash
rask run my_solution.rk      # execute
rask check my_solution.rk    # type-check only
rask lint my_solution.rk     # style/idiom check
rask fmt my_solution.rk      # auto-format
```

## Levels

| Level | Focus | Challenges |
|-------|-------|------------|
| [01 — Core Feel](01-core-feel/) | Variables, loops, functions, match | FizzBuzz, Temps, Word Counter |
| [02 — Ownership & Errors](02-ownership-errors/) | Moves, mutate, Result, try | Stack Machine, File Analyzer, Contacts |
| [03 — Concurrency](03-concurrency/) | Threads, channels, Shared | Parallel Sum, Pipeline |
| [04 — Data Modeling](04-data-modeling/) | Enums, generics, extend, traits | JSON Parser, Scheduler |
| [05 — Integration](05-integration/) | Full programs | Log Analyzer, Chat Room |

Start at Level 1 even if you know Rust — the syntax differs in subtle ways.

## Scoring Yourself

After each challenge, rate 1–5:

| Criterion | Question |
|-----------|----------|
| **Fluency** | Did I write this without checking docs/examples? |
| **Brevity** | Is this shorter than the Go equivalent? |
| **Safety feel** | Did safety features help or annoy me? |
| **Error handling** | Was `try`/`match` on Results natural? |
| **Ownership** | Did I fight moves/borrows or barely notice them? |

### Red Flags

- Needed `clone()` more than twice in one function — borrowing model may be too restrictive
- Wrote 3+ `match` blocks for error handling in a row — need better `try` ergonomics
- Wanted a feature that doesn't exist — write it down, it's a design signal
- Code is 2x longer than Go for the same thing — the design is failing its own litmus test
- Reached for `unsafe` — the safe surface area has a gap

### Green Flags

- "I forgot this has ownership" — safety is invisible, that's the goal
- Code reads like pseudocode — ergonomic simplicity is working
- Error handling felt like normal control flow — `T or E` design is paying off
- Concurrency "just worked" — no function coloring is paying off

## After You're Done

Collect your friction notes and scores. The patterns will tell you:
- Same friction in 3+ challenges — it's a language problem, not a you problem
- Scored <3 on brevity consistently — revisit the Go litmus test
- Ownership was invisible — the core thesis is validated

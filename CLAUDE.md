Keep docs short.

## Talk to me in plain language

I know the compiler pipeline — MIR, lowering, codegen, and the rest aren't the problem. What loses me is dense, abstract phrasing: spec-speak, piled-up noun phrases, and internal rule IDs dropped into a sentence as if they explained something. Explain your work so a colleague could follow it at a whiteboard — in chat, and in commit/PR descriptions.

- Lead with the plain-English point: what broke, what you changed, why. Save the fine detail for after.
- Say it like a person, not the spec. "Both variables were pointing at the same struct, so editing one edited the other" beats "the aggregate aliases the source cell."
- Don't cite rule IDs (VS1, PM2, …) as if they carry meaning — say what the rule actually requires, in words.
- Show a concrete example when you can: this input → got X → now gives Y.
- Don't over-explain things I clearly know. The goal is clarity, not a glossary.
- Short and clear wins. Three sentences I understand beat a precise paragraph I don't.

(This is about explaining to me. Code comments follow `.claude/rules/code-comments.md`.)

## I work from my phone

Chat is my interface. Reading diffs and files on a phone is painful — never point me at a diff as the explanation. The explanation in chat IS the deliverable.

- Bring code to me, don't send me to it: quote the 10–20 relevant lines in chat with your commentary when a decision needs my eyes.
- The durable record lives in **commit messages and issues** — write those to stand alone, they're what future sessions read. PR bodies get one sentence plus the closing keywords (`Closes #N`); nobody reads more than that there.

### One recap, at the end

Long sessions bury the state under rolling summaries and check-in chatter. I read the last message, not the scrollback.

- **No interim summaries.** Mid-task, speak only to ask a decision or report a blocker. Progress lives in commits.
- **Quiet check-ins are silent.** A PR check-in or event that finds nothing new ends the turn with no text. If the harness insists on output, one line: `#N: nothing new.` No "no reviews, rearming", no restated status.
- **Hand back with a recap** whenever you stop after substantial work, and whenever I say "recap". Write it for someone who saw none of the session. It replaces every earlier summary and digest, so repeat what still matters rather than saying "as above". Sections, in order:
  1. **Decisions for me**: the judgment digest. The 2–4 most questionable calls, each with the problem in a sentence, what you picked, the alternative, and the few lines of code that embody it. Framed so I can answer in one word ("keep / wrong / show me more"). Empty is fine; say so.
  2. **Status**: branch, PR link, CI on the head, what it's waiting on.
  3. **Done**: one line per change, with the commit. Say what broke and what now works, not which files moved.
  4. **Decided along the way**: calls you made without asking and why, so I don't reopen them by accident.
  5. **Left open**: filed issues, dead ends and what they taught, anything skipped.
- If state changes after a recap (review lands, CI goes red and you fix it), send a fresh full recap, not a delta.

Prefer long term proper fixes over quick-fixes.

Choose simple over easy.

## Design is mostly settled

The big decisions are made: the Decided table in `.claude/rules/design-status.md` lists them, and it loads when you work in `specs/`. Don't re-derive them and don't open design debates inside implementation tasks. Spend energy on implementation, rough edges, and bugs.

If something genuinely seems wrong, flag it once with a concrete reason — then drop it unless I bite. Keep critique pointed; no broad "have you considered" rounds on settled areas.

For a question the specs genuinely don't answer, [specs/RULINGS.md](specs/RULINGS.md) has the six tests the existing rules fall out of. Rule from those instead of guessing.

### Nothing is stable — settled is not frozen

Settled means "don't reopen it for fun". It does not mean "can't be changed".
Nobody is using this language. There are no downstream users, no released API, no
migration to plan for. **Backward compatibility is never a reason for anything.**

So when a design turns out to be wrong, change it — all of it, everywhere, in one
go. I would rather have a large destructive change that leaves the design better
than a careful patch that keeps a bad shape alive. Renaming a method with different
semantics, deleting an operator, rewriting a decided spec, breaking every call site
in the repo: all fine, all cheap, do it properly.

What this rules out:

- Hedging a change to avoid breaking existing code. There is no existing code
  worth protecting — the repo is ours and the compiler will find every call site.
- Leaving a stub that wears a real name. A method that silently does nothing is
  worse than a missing one: implement it or delete it.
- "Keep these two copies in step" comments. Duplication a human has to maintain is
  rot with a delay fuse — generate it from one source or collapse it.
- Deprecation periods, aliases, compatibility shims. Delete the old spelling.

If you catch yourself weighing "how much would this break", you are weighing the
wrong thing. Weigh whether the result is better.

### Don't re-litigate

- **Clone cost is intentional.** Types >16 bytes require explicit `.clone()` even when all fields are Copy. This is the transparency principle — the cost is visible. Don't suggest raising the Copy threshold, making clones implicit, or treating this as a problem to solve. It's a deliberate tradeoff.
- **The set of compiler-provided types is closed.** `Shared` (with its `Local`/`Readers`/`Mutex` strategies), `Rack`+`Link`, `Heap`, `Atomic*`, `string` — users can't build equivalents, and there's no unsafe hatch for it. Argued in `specs/memory/shared-rack-heap.md` (BX1–BX4). Don't propose one. The compiler may add types to the set (refcounting is fine when the compiler provides it, like `string` and the proposed `bytes`); users still can't.

# Working relationship

- No sycophancy.
- Be direct, matter-of-fact, and concise.
- Be critical; challenge my reasoning.
- Don’t include timeline estimates in plans.
- Don’t add yourself as a co-author to git commits.
- When creating a PR, tag the issues it resolves in the body with closing keywords (`Closes #N` / `Fixes #N`) so GitHub auto-closes them on merge. Issues it only relates to get a plain `#N` reference.

## Keep going

**Don't hand back unless there's a decision only I can make.**

Bug crunching has no decisions in it — work the whole list. A judgment call with a
defensible answer isn't a decision for me: pick it, say which way you went, carry
on. When the two backends disagree, the interpreter is the reference — read it
instead of asking. "I've been at this a while" is not a reason to stop.

Do stop for: a change that would make the flagship example worse, a spec question
with no answer in `specs/`, or anything needing credentials or an outward-facing
action.

Messy is fine. A branch with eight fixes and two written-up dead ends beats three
fixes and a status report. If a fix trades one failure for a worse one, revert it
and file what you learned — that's a finished piece of work, not a blocker.

## Before you push

Hand any change to compiler, runtime or stdlib to the `skeptic` agent before
pushing. Give it the issue or the goal, not your reasoning, and let it try to
break the change. Fix what it confirms, or say in the recap why a finding is wrong.
Skip it for docs-only and config-only changes.

At the end of a long session, `/retro` turns what went wrong into proposed edits
to this file and the skills.

## Debugging discipline

- Understand before changing. If you can't explain why something is broken, you're not ready to fix it.
- Fix causes, not symptoms. State the root cause in one sentence before writing the fix; if the diff doesn't touch the thing that sentence names, it's a patch, not a fix — trace deeper. Workarounds are allowed only when labeled as workarounds (in the code comment and the commit) with the real cause filed as an issue.
- **When a reduction won't reproduce, stop writing reductions.** Guessing at what's
  essential can cost hours and still miss. Copy the real program, instrument it —
  print each field, each hop — and let it tell you which one is wrong. Two of this
  round's bugs were found in one pass that way after several failed guesses.
- **Pre-existing errors that surface during unrelated work get filed, not ignored.** If a test fails, the compiler panics, or a spec breaks for reasons unrelated to your current change, search `rask-lang/rask` issues first; if it's not tracked, open one with a minimal repro before moving on. Don't paper over it, don't only mention it in chat, don't bundle it into the current commit silently.
- **A sentence about Rask is a claim. Check it like you'd check code.** An
  unchecked one ships sounding exactly as sure as a checked one. Three got onto
  the website in an afternoon: "pre-0.1" with v0.2.0 already released, "Rask
  doesn't offer pointer-level control" when `unsafe` gives raw pointers and Rust
  gates the same things behind the same word, and a headline that described Go.
  Each was one command away. Version and status against `git tag` and the
  releases; what the language does or doesn't have against `specs/`; and a claim
  that Rask *differs* has to name what the other language actually does — if the
  sentence is equally true of Go, it isn't about Rask.
- **Verify the path, not only the artifact.** The front page's code came out
  unstyled while the deployed file was byte-perfect: the file was right and the
  URL was stale, so checking the bytes answered a question nobody had asked. Ask
  how the thing reaches a reader, and check that.
- **When a change removes the reason for a workaround, remove the workaround in
  the same edit.** A nudge that centred a dot against a wrapped label became a
  nudge that pushed it off centre the moment the label stopped wrapping.

**Tool usage:**
- Use `Write` tool for creating test files, not `Bash` with cat/heredocs
- Avoid pipes (`|`), redirects (`2>&1`), and command chaining (`&&`) in Bash commands - they break permission matching
- Run commands separately instead of chaining them
- Create test scripts in `/tmp`, not the main project folder

**CLI tools** (binary at `compiler/target/release/rask`):

| Command | Use |
|---------|-----|
| `rask lint <file>` | Check .rk files for naming/style/idiom issues |
| `rask test-specs <path>` | Verify spec code blocks parse + show staleness warnings |
| `rask api <file>` | Show a module's public API (structs, funcs, enums) |
| `rask fmt <file>` | Format .rk source files |
| `rask check <file>` | Type-check a .rk file |
| `rask check --claims <dir>` | Also build and run the package's `claims.rk` ([type.claims](specs/types/claims.md)); `build`, `run` and `test` run them anyway |
| `rask claims <dir>` | List a package's claims and verdicts; `--since <rev>` and `--mutate` for review |
| `rask run <file>` | Execute a .rk program (native) |
| `rask run --interp <file>` | Execute via interpreter (no codegen) |
| `rask test <file>` | Run `test` blocks — **native**, like `run` |
| `rask test --interp <file>` | Run `test` blocks on the interpreter |
| `rask compile --dump-mir <file>` | Print MIR before codegen (debug codegen issues) |
| `rask bindc <header.h> [-o f.rk]` | Print the Rask a C header translates to: what `import c` compiles |

**Native is the default everywhere.** `rask run` and `rask test` both compile and
run natively; `--interp` opts out. `rask test --native` is accepted and does
nothing, so a bug you "reproduced on the interpreter" with it was native all
along. When the two disagree the interpreter is the reference for *what the answer
should be* — it isn't what anyone ships.

Binary: `compiler/target/release/rask` (build: `cd compiler && cargo build --release -p rask-cli`)
Releases: https://github.com/rask-lang/rask/releases

**Debugging a native binary** (segfault, SIGILL, flaky, disagrees with the interpreter): the `debug-native` skill has the env vars and tools.

**Things that will waste your time:**

- In a cloud session `compiler/target/release/rask` comes from the environment snapshot and can be days older than your clone, while `runtime/*.c` is current. A stale binary gives raw `ld` undefined-symbol errors for `rask_*` internals, which read like a codegen bug in your program (#1041). A SessionStart hook rebuilds it in the background (log in `compiler/target/session-start-build.log`); run `cargo build --release -p rask-cli` before trusting the binary, which waits for the hook's build and then returns at once. [docs/cloud-cache.md](docs/cloud-cache.md) covers what the snapshot does and doesn't cover.
- `stdlib/*.rk` is `include_str!`'d into the compiler (`rask-stdlib/src/stubs.rs`), so editing it does nothing until `cargo build --release -p rask-cli`. `runtime/*.c` is different: the linker compiles those sources itself and caches the objects keyed by size+mtime, so an edit takes effect on the next compile with no rebuild and no `make`. (`librask_runtime.a` isn't linked by anything — `make` in `compiler/runtime` builds it for its own sake.) Cached objects live in `$XDG_CACHE_HOME/rask/runtime`, or `RASK_RUNTIME_CACHE` if set.
- A **failed** `rask build` exits 1 and leaves the previous binary in `build/debug/`. Run it without checking and you're testing old code — which reads exactly like "my fix didn't work". Don't pipe the build through `tail`; check the exit code.
- `rask build` caches Rask object files in `build/.cache/*.o` (separate from the runtime object cache above). The key covers source, profile, target **and the compiler binary** (path + size + mtime), so rebuilding `rask` invalidates it on its own — no `rm -rf build/.cache` needed. Two compilers keep separate entries rather than evicting each other, so alternating between builds still hits. `rask build --verbose` prints the compiler fingerprint on a cache hit; `--force` or `--no-cache` bypasses. (`rask run` / `rask compile` on a single file don't cache at all.)
- `println` is fully buffered to a pipe, so output before a crash is lost. Always `stdbuf -o0 -e0 ./binary > log 2>&1`, or you'll place the crash earlier than it is.

Hooks auto-run `rask lint` and `rask check` after editing `.rk` files (check skips files that fail on purpose; the list is in `.claude/hooks/post-edit.sh`) and `rask test-specs` after editing `specs/*.md`.

## Writing

Add `// SPDX-License-Identifier: (MIT OR Apache-2.0)` to the top of source files (.rs, .rk), not docs (.md).

The style guide for docs and code comments lives in `.claude/rules/` and loads when you touch matching files. Short version: docs say "I" for design choices and stay neutral for technical rules; comments say why, not what; no filler.

## Rask Syntax

**Claude: Use Rask syntax, not Rust.** Full reference: [specs/SYNTAX.md](specs/SYNTAX.md)

Key differences from Rust: `let`/`mut` (never `let mut`; `const` is module-level constants only), `func` (not `fn`), `extend` (not `impl`), `public` (not `pub`), `string` (lowercase), `Token.Plus` (not `::`), `try expr` (not `?`), `T or E` (not `Result<T,E>`), explicit `return` in functions, newlines as terminators.


## Compiler

Pipeline: `.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime → Ownership → MIR → Codegen/Interp`

**Each pass hands its results down as data. No pass re-derives them from text.** A type is a `TypeExpr` or `Type` and never a string to `split('<')`. When a later pass needs a fact, give the earlier pass a field to carry it. [compiler/CLAUDE.md](compiler/CLAUDE.md) has the per-crate file map and the rules for error messages, and loads when you work under `compiler/`.

## Goal

Systems language where **safety is invisible**. Eliminate abstraction tax, cover 80%+ of real use cases.

**Non-negotiable:** Feel simple, not safe. Safety is a property, not an experience.

## Core Principles

Unifying thread: **safety through visibility.** Safety mechanisms are visible in source (explicit `ensure`, `mutate`, `take`, scoped `with`) rather than hidden in destructors, lifetime annotations, or effect types. The compiler guarantees invariants; the source shows the mechanism.

1. **Transparency of Cost** — Major costs visible in code (allocations, locks, I/O). Small costs (bounds checks) can be implicit.
2. **Mechanical Safety** — Safety by structure. Data races, null derefs, dangling pointers and, in safe code, memory leaks impossible by construction; deleting a rack node nulls every link to it, and a local link used after the delete is a compile error. The leak claim is the design's; `tests/leak_gate.sh` holds the implementation to it and `tests/known_leaks.txt` lists where it still doesn't.
3. **Practical Coverage** — Handle web services, CLI, data processing, embedded. Not limited to fixed-size programs.
4. **Ergonomic Simplicity** — Low ceremony. If Rask needs 3+ lines where Go needs 1, question the design.
5. **Information Without Enforcement** — Track effects, captures, and modes as metadata surfaced via tooling (IDE ghosts, lints) instead of type-system constraints. No function coloring, no effect polymorphism.

Full nine-principle set: [specs/CORE_DESIGN.md](specs/CORE_DESIGN.md). Scoring methodology: [METRICS.md](specs/METRICS.md).

---

## Design Status

Start with [CORE_DESIGN.md](specs/CORE_DESIGN.md). For specs: [specs/README.md](specs/README.md). For spec ID conventions and citation format: [specs/CONVENTIONS.md](specs/CONVENTIONS.md).

**Citing spec rules:** `spec-id/rule-id` — e.g., `mem.ownership/O1`, `type.structs/M3`. See CONVENTIONS.md for the full ID scheme.

---

## Validation

Test programs that must work naturally:
1. HTTP JSON API server
2. grep clone
3. Text editor with undo
4. Game loop with entities
5. Embedded sensor processor

**Litmus test:** If Rask is longer/noisier than Go for core loops, fix the design.

---

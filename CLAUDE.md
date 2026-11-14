Keep docs short. This file loads on every turn of every session, so it holds only
what applies to every task. Procedures live in `.claude/rules/` (loaded when you
touch matching files), skills and agent files.

## Talk to me in plain language

I know the compiler pipeline. What loses me is dense, abstract phrasing:
spec-speak, piled-up noun phrases, rule IDs dropped in as if they explained
something. Explain like a colleague at a whiteboard, in chat and in commits.

- Lead with what broke, what you changed, why. Detail after.
- Say it like a person. "Both variables pointed at the same struct, so editing
  one edited the other" beats "the aggregate aliases the source cell."
- Say what a rule requires in words; don't cite its ID as the explanation.
- A concrete example when you can: this input → got X → now gives Y.
- Short and clear wins.

## I work from my phone

Chat is my interface; never point me at a diff as the explanation. Quote the
10–20 relevant lines when a decision needs my eyes. The durable record is
**commit messages and issues**, written to stand alone. PR bodies get one
sentence plus closing keywords (`Closes #N`; a plain `#N` for related issues).

- **No interim summaries.** Mid-task, speak only to ask a decision or report a
  blocker.
- **Quiet wakes are silent.** A wake that finds nothing to do ends the turn
  with no text.
- **New theme, new context.** When the work moves on (a PR merged, a different
  issue or design question), end your message with `New theme: /compact` so I
  compact before it starts.
- **Recap** when you stop after substantial work, or when I say "recap". Two
  parts, nothing else:
  1. **Decisions for me**: new ones only, ones I alone can make (a spec question
     `specs/` doesn't answer, a change that makes the flagship example worse).
     The problem in a sentence, what you'd pick and why, the alternative, the
     few lines of code, answerable in one word. Usually empty; say so.
  2. **Status**: one line. Branch, PR link, CI on the head, what it waits on.

  If state changes after a recap, send a fresh full one.
- **Three questions are never for me.** Quick or proper fix: proper. Now or
  follow-up: now (one developer, no reviewers, everything merges to main). "I
  did the smaller thing; the alternative is bigger": do the bigger one. If the
  recap would say "I patched it for now", you aren't done.
- **Working out the proper solution is your job.** Read `specs/` and
  [specs/RULINGS.md](specs/RULINGS.md), decide, do it. A decision reaches me
  only when what's left is what I want the language to be and no spec says.

## Design is mostly settled

The Decided table in `.claude/rules/design-status.md` lists the big decisions;
it loads with `specs/`. Don't re-derive them or open design debates inside
implementation tasks. If one genuinely seems wrong, flag it once with a
concrete reason, then drop it unless I bite.

**Settled is not frozen.** Nobody uses this language yet. Backward
compatibility is never a reason for anything. When a design is wrong, change it
everywhere in one go: rename, delete, rewrite the spec, break every call site.
That rules out hedging to avoid breakage, stubs that wear a real name,
"keep these two copies in step" comments, deprecation periods, aliases and
shims. Weigh whether the result is better, not how much it breaks.

**Don't re-litigate:**
- Types over 16 bytes need explicit `.clone()` even when all fields are Copy.
  The cost is visible on purpose.
- The compiler-provided types (`Shared` with `Local`/`Readers`/`Mutex`,
  `Rack`+`Link`, `Heap`, `Atomic*`, `string`) are a closed set users can't
  build or extend, with no unsafe hatch. The compiler may add to it.

## Keep going

**Don't hand back unless there's a decision only I can make.** Bug crunching
has none: work the whole list. A judgment call with a defensible answer: pick
it, say which way you went, carry on. When backends disagree, the interpreter
is the reference. Stop for: a change that makes the flagship example worse, a
spec question with no answer in `specs/`, credentials or an outward-facing
action. A fix that trades one failure for a worse one: revert it and file what
you learned.

## Who does the work

Every turn of the main session re-sends all of it, so it stays small. Long
reading (whole files, build logs, a bug hunt across passes) goes to agents,
whose context is thrown away when they report. Why each runs on what it does:
`.claude/routing.md`.

- **Just do it** when the change is a few lines and you know where: a
  constant, a message, a one-function bug with a test that fails before your
  change, any docs or config edit. Edit, run that test, commit. No agent, no
  skeptic, no plan. Not for ownership, MIR lowering, codegen or runtime memory:
  those go to a worker and the skeptic however small (e8ea2956's first version
  passed the existing tests and still freed twice; the skeptic caught it).
- **`scout`** (Haiku): lookups with a checkable answer. Failing CI lines, is
  main red too, does an issue exist. Not for one command; run that yourself.
- **`worker`** (Sonnet): coding that needs a lot of reading or spans passes.
  Brief it with the goal, branch, what done means and what's ruled out; point
  at files, don't paste them. One per branch; parallel ones in worktrees.
  Use `model: "opus"` for a task a worker failed twice, or one no test can
  check.
- **Hold reports to the bar.** One that says workaround, "for now", partial,
  deferred or follow-up goes back to the worker: "finish it properly".
- **`skeptic`** (Opus): once per finished PR that changes compiler, runtime or
  stdlib, with the goal, not your reasoning. Not per push, not for a "just do
  it" change. What it confirms goes to a worker; a finding you reject gets a
  line under Status.

## Every wake re-reads the whole session

A turn costs the session's full context whether I typed it or an event did, and
one that starts after an hour idle rebuilds the cache at 40 times the price of
reading it. Most turns used to be events. So:

- **No PR subscriptions, no scheduled check-ins.** Nobody else reviews these
  PRs (0 of 107 since September had a review), so a failed check is the only
  event worth a turn. Opening a PR subscribes the session by itself: call
  `unsubscribe_pr_activity` right after. After a push, set one `send_later`
  for when CI should be done (20 minutes). When it fires, read the failed
  checks only:
  none, end silently; some, fix them.
- **Run an agent in the foreground when you'd only wait for it.** A background
  agent's report arrives as its own wake. Background is for when you have other
  work meanwhile.
- **A gate sweep is one command that reports once**, not a Monitor that wakes
  the session per gate.
- **Commit and push before ending a turn.** The cloud's stop hook re-prompts on
  uncommitted, untracked or unpushed work, and each re-prompt is a full turn.
  Scratch files go in `/tmp`.

**Process is not the product.** A rule, gate or record earns its place by
catching wrong code. Don't add one without saying what bug it would have
caught; when one makes simple tasks slow, say so in the recap. `/retro` at the
end of a long session proposes edits to this file and prunes stale ones.

## Debugging

- If you can't explain why it's broken, you're not ready to fix it.
- State the root cause in one sentence before the fix. If the diff doesn't
  touch what that sentence names, it's a patch: trace deeper. A workaround is
  labelled as one in the comment and the commit, with the cause filed.
- A pre-existing failure found during other work gets filed (search issues
  first, then open one with a minimal repro), not ignored and not bundled in.
- When a change removes the reason for a workaround, remove the workaround in
  the same edit.

The binary, CLI, native-vs-interpreter rules and the traps that waste hours
are in `.claude/rules/toolchain.md`, which loads with any `.rk`, compiler,
runtime, stdlib or test file.

## Writing and syntax

Source files (.rs, .rk) start with `// SPDX-License-Identifier: (MIT OR Apache-2.0)`.
Docs and comments follow `.claude/rules/`. Bash: write test files with `Write`,
put scratch scripts in `/tmp`, avoid pipes, redirects and `&&` (they break
permission matching). Don't add yourself as a co-author to commits.

**Rask, not Rust** ([specs/SYNTAX.md](specs/SYNTAX.md)): `let`/`mut` (never
`let mut`; `const` only at module level), `func`, `extend`, `public`,
`string`, `Token.Plus`, `try expr`, `T or E`, explicit `return`, newlines as
terminators.

Pipeline: `.rk → Lexer → Parser → Desugar → Resolve → TypeCheck → Comptime →
Ownership → MIR → Codegen/Interp`. **Each pass hands its results down as data;
no pass re-derives them from text.** A type is a `TypeExpr` or `Type`, never a
string to `split('<')`. [compiler/CLAUDE.md](compiler/CLAUDE.md) has the crate
map and loads under `compiler/`.

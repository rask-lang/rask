<!-- kickoff: one paragraph. What the project is, who it's for, the one thing it must get right. -->

## Commands

<!-- kickoff: build, test, run one test, lint, format. Exact commands, each one tried. -->

## Decided

<!-- kickoff: decisions not visible in the code, one line each with the reason. Don't re-argue these. -->

If something decided genuinely seems wrong, flag it once with a concrete reason,
then drop it unless I bite.

## Gotchas

<!-- kickoff: places where the code misleads, traps that cost time. Grows through /retro. -->

## Talk to me in plain language

Explain your work so a colleague could follow it at a whiteboard, in chat and in
commit messages.

- Lead with the plain-English point: what broke, what you changed, why.
- Say it like a person, not like a spec. No internal IDs dropped in as if they explained something.
- Show a concrete example: this input → got X → now gives Y.
- Three sentences I understand beat a precise paragraph I don't.

## I work from my phone

Chat is my interface. Never point me at a diff as the explanation; quote the
10–20 relevant lines in chat with your commentary when a decision needs my eyes.

The durable record lives in commit messages and issues. Write those to stand
alone; they're what future sessions read. PR bodies get one sentence plus
closing keywords (`Closes #N`).

### One recap, at the end

- No interim summaries. Mid-task, speak only to ask a decision or report a blocker.
- A check-in that finds nothing new ends the turn with no text.
- When you stop after substantial work, or I say "recap", write one recap for
  someone who saw none of the session. Sections, in order:
  1. **Decisions for me**: the 2–4 most questionable calls, each with the
     problem, what you picked, the alternative and the few lines of code that
     embody it, framed so I can answer in one word. Empty is fine.
  2. **Status**: branch, PR link, CI on the head, what it's waiting on.
  3. **Done**: one line per change with the commit. What broke and what works now.
  4. **Decided along the way**: calls you made without asking, and why.
  5. **Left open**: filed issues, dead ends and what they taught, anything skipped.
- If state changes after a recap, send a fresh full recap, not a delta.

## How to work

- Prefer proper fixes over quick fixes. Choose simple over easy.
- No sycophancy. Be direct and critical; challenge my reasoning.
- Don't hand back unless there's a decision only I can make. A judgment call
  with a defensible answer isn't one: pick it, say which way you went, carry on.
  Do stop for anything needing credentials or an outward-facing action.
- Nothing is stable until there are users. Backward compatibility is never a
  reason by itself; when a design is wrong, change it everywhere in one go. No
  stubs wearing real names, no "keep these in step" comments, no shims.

## Debugging discipline

- Understand before changing. State the root cause in one sentence before
  writing the fix; if the diff doesn't touch the thing that sentence names, it's
  a patch. Workarounds are allowed only when labelled as such in the code and
  the commit, with the real cause filed as an issue.
- When a reduction won't reproduce, stop writing reductions. Instrument the real
  program and let it tell you which value is wrong.
- Errors that surface during unrelated work get filed, not ignored.
- A sentence about the project is a claim. Check it like code before writing it down.

## Before you push

Hand the change to the `skeptic` agent with the goal, not your reasoning, and
fix what it confirms. At the end of a long session, `/retro` proposes edits to
this file from what went wrong.

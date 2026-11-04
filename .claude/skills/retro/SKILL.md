---
name: retro
description: End-of-session retrospective. Turns what went wrong or got corrected in this session into proposed edits to CLAUDE.md, .claude/rules, skills, agents and hooks, and prunes what has gone stale.
disable-model-invocation: true
---

# Retro

Look back over this session and make the instructions better for the next one.
The next session starts from a fresh clone and knows only what's committed.

## 1. Collect

Go through the session and list, with the moment it happened:

- Corrections from the user, especially ones they've made before.
- Time lost: wrong guesses, a stale binary, a reduction that wouldn't reproduce,
  a tool used wrong.
- Instructions that were wrong or out of date: a file, command, env var or issue
  that doesn't exist any more, a claim about the project that didn't hold.
- Workarounds whose reason went away this session.
- Findings from a reviewer agent that a written rule would have prevented.

If the list is empty, say so in one line and stop. A quiet session is fine.

## 2. Place each one

Pick where it belongs. Nowhere is often the right answer.

| If it is | It goes in |
|---|---|
| A fact every session needs (a command, a trap, a decision not in the code) | `CLAUDE.md` |
| Only true for part of the tree | `.claude/rules/<topic>.md` with `paths:`, or a `CLAUDE.md` in that directory |
| A procedure for one kind of task | a skill under `.claude/skills/` |
| Something that must happen every time, no exceptions | a hook in `.claude/settings.json` |
| Something Claude can read from the code, or a one-off | nowhere |

Prefer editing or deleting an existing line over adding one. `CLAUDE.md` aims
for under 200 lines; every line costs attention on every task. If a line keeps
getting missed, it's either buried in a long file or it should be a hook.
Shouting (IMPORTANT, capitals) doesn't help.

## 3. Prune

Check what the instruction files point at: every path, command, env var and
issue number still exists, and no two instructions contradict each other.
A sentence about the project is a claim; check it against the specs and the
code before keeping or adding it.

## 4. Propose, then apply

The user reads this on a phone. For each proposal, show the file, the old text
and the new text as short quoted blocks, and one sentence on what happened in
the session that motivates it. Then ask which to apply (AskUserQuestion,
multi-select). Apply only those, run the hooks' checks, and commit with a
message that says what changed in the instructions and why, so a future session
reading `git log` understands it.

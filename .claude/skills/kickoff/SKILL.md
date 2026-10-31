---
name: kickoff
description: Set up a new project for Claude Code. Interviews the user about the project, then writes a short CLAUDE.md, hooks, a skeptic reviewer agent and a retro skill, and commits them. Use when starting a new repository or one with no CLAUDE.md.
argument-hint: "[one-line description of the project]"
disable-model-invocation: true
---

# Kickoff

Set this repository up so every future session, usually started from a phone in
a fresh cloud container, starts out knowing what the user would otherwise
re-explain. Everything goes into committed files; nothing on a laptop reaches a
cloud session.

Project description from the user, if given: $ARGUMENTS

## 1. Look before asking

Read what's there: README, manifests (`Cargo.toml`, `package.json`,
`pyproject.toml`, `go.mod`...), CI config, existing scripts. Work out the build,
test, lint and format commands yourself and run them once to confirm they work.
Don't ask what you can find out.

## 2. Ask what the code can't tell you

One AskUserQuestion round, at most four questions, each standing on its own.
Pick from:

- What the project is for, and who uses it, if the description doesn't say.
- What's already decided and shouldn't be re-argued (language, architecture,
  key libraries, deliberate tradeoffs).
- What "done" means: which check has to pass before a change counts.
- What has bitten them before, or what they expect Claude to get wrong.

## 3. Write the files

- **`CLAUDE.md`** from `templates/CLAUDE.md`. Fill the project sections from
  steps 1 and 2. Keep the working-agreement sections as they are; they're the
  user's. Delete any placeholder you can't fill rather than leaving it. Aim for
  well under 200 lines. Don't describe the file layout; Claude reads the repo.
- **`.claude/settings.json`** with two hooks, each a script in `.claude/hooks/`:
  - `SessionStart`, matcher `startup`, cloud only (`CLAUDE_CODE_REMOTE=true`):
    install dependencies and build, so the first command in a session works.
    If it takes over ~20 seconds, make it async
    (print `{"async": true, "asyncTimeout": 600000}` first).
  - `PostToolUse`, matcher `Edit|Write|MultiEdit`: run the project's linter or
    formatter check on the edited file (`jq -r .tool_input.file_path` from
    stdin). Exit 2 with the output on stderr when it finds something, so Claude
    sees it; exit 0 otherwise.
  Run each hook script by hand once and show it works.
- **`.claude/agents/skeptic.md`** from `templates/skeptic.md`. Fill in the
  project's check commands and the ways this kind of project usually breaks.
- **`.claude/skills/retro/`**: copy `templates/retro/` as it is.

Add one line to CLAUDE.md for each of the last two: changes go to `skeptic`
before pushing, and `/retro` at the end of a long session.

## 4. Commit

One commit, `chore: set up Claude Code`, whose message lists what each file is
for. Push and open a draft PR if the session's instructions say to. Then give
the user a short recap: what was set up, and the two or three calls you made
that they might want to change, quoting the lines.

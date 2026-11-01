# Claude Code hooks for Rask projects

Two hook scripts for a Rask project that uses Claude Code. They're opt-in: copy
them in if you want them, and nothing changes if you don't.

- `rask-check.sh` runs `rask check --json` after every edit to a `.rk` file and
  hands any type errors straight back to Claude, one line each, with the fix the
  compiler suggests. Claude sees a broken build the moment it causes it, instead
  of three edits later. This repo runs the same script on itself, next to `rask lint`.
- `install-rask.sh` puts `rask` on PATH at session start. You need it for cloud
  sessions, which is where a session started from the mobile app runs: the
  container starts without rask, so it downloads the latest Linux release once.
  On a machine that already has rask it does nothing.

## Setup

Copy both scripts into your project's `.claude/hooks/`, make them executable,
and add this to `.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup",
        "hooks": [
          { "type": "command", "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/install-rask.sh" }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [
          { "type": "command", "command": "$CLAUDE_PROJECT_DIR/.claude/hooks/rask-check.sh" }
        ]
      }
    ]
  }
}
```

Commit `.claude/`. Cloud sessions start from a fresh clone, so anything that only
lives on your laptop never reaches them.

## What Claude sees

After an edit that breaks the build:

```
rask check found 1 error(s) after editing /home/user/app/main.rk:
/home/user/app/main.rk:2:18: error[E0308] mismatched types: expected `string`, found `i32`
    help: change this to type `string`
`rask explain <code>` describes an error in full.
```

Inside a package the whole package is checked, so an edit to one file that
breaks a caller in another shows up too. A clean check prints nothing. Warnings
are left out on purpose; they'd nag on every edit, and `rask lint` is the place
for them.

If `rask` isn't on PATH the hook says so after every edit rather than skipping
quietly, because a check that doesn't run looks exactly like one that passed.
Set `RASK=/path/to/rask` to use a compiler that isn't on PATH.

## Notes

- Needs `jq`, which Claude Code's cloud containers have.
- `install-rask.sh` tracks the latest release. To pin one, change
  `releases/latest/download/` to `releases/download/v0.6.0/`.
- `tests/agent_hook_gate.sh` runs the check hook against the real compiler in
  CI, so a change to `rask check --json` that breaks it fails the build.

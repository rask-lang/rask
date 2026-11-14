# Why each agent runs on what it does

Read this before changing a model or effort in `.claude/agents/`. Prices are
per million tokens, input/output, from the API pricing page (checked
2026-11-14). Plan usage isn't billed this way, but it moves with it.

- **Haiku 5.5 ($0.10/$0.50) is twenty times cheaper than Sonnet only while its
  prompt is under 100K tokens.** Past that the whole request costs five times
  as much ($0.50/$2.50). So the scout is capped at 100K, gets no CLAUDE.md and
  does short jobs. Anything with a long loop or a judgment in it is a worker's.
- **Sonnet 5.5 ($2/$10) has no such step**, so a worker may read as much as the
  task needs. It runs at `high` because at lower effort it is documented to
  call a change done without running the check, which is the failure this
  setup exists to stop. Not `xhigh`: for the hardest work the guidance is to
  move up to Opus instead of pushing Sonnet harder.
- **Opus 5.5 ($4/$20) defaults to `medium`**, right for routine work and wrong
  for the two places it's used here. Both are pinned to `high`: a review runs
  once and a miss ships, and an escalated task already beat Sonnet twice.
  `xhigh` and `max` stay off until a bug that `high` missed shows they'd have
  caught it.
- **Opus for coding, by rule**: a task a Sonnet worker failed twice, and a task
  with no test that can tell a wrong answer from a right one (a design change
  across passes, a spec rewrite). Retrying on a bigger model only works when
  the failure shows.

## What a turn costs

A cached read is 5% of the input price on Opus and Sonnet 5.5 ($0.20 and
$0.10). Writing the cache for one hour costs twice the input price ($8 and $4),
forty times a read. A turn reads the whole session, so its price is the
session's size, and a turn after an hour idle pays the write price on all of
it. That's why `CLAUDE.md` cuts wakes and `.claude/settings.json` sets
`autoCompactWindow` to 800K: cloud sessions compact at 80% of the window, so
about 640K, before the model gets worse past ~600K.

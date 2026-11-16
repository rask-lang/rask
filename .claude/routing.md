# Why each agent runs on what it does

Read this before changing a model or effort in `.claude/agents/`. Prices are
per million tokens, input/output, from the API pricing page (checked
2026-11-14). Plan usage isn't billed this way, but it moves with it.

- **Haiku 5.5 ($0.10/$0.50) is twenty times cheaper than Sonnet only while its
  prompt is under 100K tokens.** Past that the whole request costs five times
  as much ($0.50/$2.50). So the scout is capped at 100K, gets no CLAUDE.md and
  does short jobs. Anything with a long loop or a judgment in it is the main session's.
- **Sonnet 5.5 ($2/$10)** runs the one optional background worker, at `high`:
  at lower effort it is documented to call a change done without running the
  check. It is cheaper per token than Opus, but that only pays while its cache
  stays warm (below).
- **Opus 5.5 ($4/$20) defaults to `medium`**, right for the main session's
  routine work. The skeptics are pinned to `high`: a review runs once and a
  miss ships. `xhigh` and `max` stay off until a bug that `high` missed shows
  they'd have caught it.
- **Opus does the coding**, in the main session. Its cache lives an hour.
  A subagent's cache lasts minutes, so a worker that waits on a ten-minute
  build pays to write its whole context again when it returns, at 25 times
  the read price. Twice the per-token price is the smaller cost.
- **What a subagent costs is its context size times its cache misses.** A
  fresh agent with a short brief that finishes inside one cache window is
  cheap, and the Haiku scout costs cents even when every call misses. The
  overnight builders were the other end: long-lived, resumed, waiting on slow
  builds. Spawn the first kind freely; don't make the second.

## What a turn costs

A cached read is 5% of the input price on Opus and Sonnet 5.5 ($0.20 and
$0.10). Writing the cache for one hour costs twice the input price ($8 and $4),
forty times a read. A turn reads the whole session, so its price is the
session's size, and a turn after an hour idle pays the write price on all of
it. That's why `CLAUDE.md` cuts wakes.

`.claude/settings.json` sets `autoCompactWindow` to 250K, so a session compacts
at about 200K. It was 800K (compact at ~640K), and the night of 2026-11-16 showed
what that costs: seven Sonnet workers each grew to 400–620K, every tool call
re-read all of it, and the account hit its five-hour spend limit twice. A worker
that compacts at 200K loses some of its own history; a fresh one with a short
brief came in at 120K for the same kind of job.

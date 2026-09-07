# Faster first build in cloud sessions

A fresh Claude Code web session clones the repo with nothing built, so the first
`cargo build --release` compiles all ~287 dependency crates (Cranelift is the
slow part) — a few minutes that rarely change between sessions.

The web environment can snapshot that work. A **setup script** runs once when an
environment is first used; its filesystem output is snapshotted and reused by
every later session. Put the build there and the third-party crates come
pre-built.

## Setup

In your cloud environment settings (web UI, or `/remote-env` in the terminal),
set the **Setup script** to:

```bash
#!/bin/bash
/home/user/rask/scripts/warm-cache.sh
```

Use the absolute path: the setup script doesn't start in the repo root, so a
relative `./scripts/...` fails with exit 127. The script itself finds the repo
from its own location, so it works from any working directory.

The whole script takes 3m07 from cold (1m56 release, 1m09 for the dev-profile
test binaries) against a ~5-minute budget, so there's about two minutes of
headroom before an addition starts costing you the snapshot. It rebuilds itself
when you change the script or after ~7 days, and lands around 5G
(`~/.cargo/registry` 0.4G, `target/release` 0.5G, `target/debug` 4.1G — debug
is fat because dev-profile debug info is).

A `SessionStart` hook is *not* a substitute: it isn't snapshotted, so it re-runs
the build on every session instead of caching once.

Not in there: the `wasm32-unknown-unknown` target CI's `rask-wasm` gate needs.
It's another rustup download plus a cold dep build for that target, and the
setup script has a ~5-minute budget to fit in — a run that overshoots leaves
you with no snapshot at all, which is worse than any of this.

## What a session still pays

Measured on a 4-core session:

| | cold | with snapshot |
|---|---|---|
| `cargo build --release -p rask-cli` | 1m56 | 58-71s |
| first `cargo test` | 85s | 8s |
| first native `rask run` (C runtime) | 1.5s | 1.5s |

58s of that release build is unavoidable (below); 71s was the real figure with
three days of commits between the snapshot and HEAD.

**Build the compiler first thing.** The snapshot's `rask` binary is as old as
the snapshot, while the runtime C sources come from your fresh clone. Run a
program with the stale binary and you get raw `ld` undefined-symbol output for
runtime internals (#1041) — the compiler emits or expects a symbol the other
side doesn't have yet. `cargo build --release -p rask-cli` fixes it.

That 58s is a floor, not the snapshot underperforming. Cargo decides freshness
by mtime, and every session clones the repo fresh, so the 22 `rask-*` crates
the CLI needs look modified even when their content is byte-identical to what
the snapshot built. Verified: `touch` every `.rs` file, change nothing, and
cargo recompiles 23 units in 58s.

## Why there's no shared cache to download

The obvious fix for that 58s is a content-keyed compilation cache — sccache, or
a registry a session pulls from. Measured, both fail here:

- **sccache in the snapshot** works exactly as advertised on the mtime problem:
  58s → 8s, 22/22 hits, for a tree whose content is unchanged. But a cache in
  the snapshot is frozen the moment the snapshot is taken, and the repo moves.
  Three days of commits touched 21 of the 22 crates, `rask-ast` and `rask-lexer`
  among them — and a changed crate changes its dependents' inputs, so the hit
  rate on a real session's first build is close to zero. And a miss costs extra
  to write: the same 22 crates took 85s through sccache against 58s without it.
  On the builds that matter it's a net loss.
- **A remote registry** (CI populates an S3/sccache bucket on every `main`
  push, sessions read it) is the only shape that would actually hit, since it
  tracks `main` instead of freezing. It buys back ~50s per session in exchange
  for a bucket, credentials in environment settings, a network-allowlist entry,
  the same ~45% write overhead in CI, and a new way for a session to fail
  slowly. Not worth it for 50s.

The rask-level caches can't be shared at all as things stand: `build/.cache`
keys on the compiler executable's path, size and mtime, and the runtime object
cache keys on source size and mtime. Both are deliberately machine-local, so a
downloaded copy always misses. Content-addressed keys would be the prerequisite
for a Rask package cache anyone could publish — worth knowing, not worth doing
for a 50s build.

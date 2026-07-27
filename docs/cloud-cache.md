# Faster first build in cloud sessions

A fresh Claude Code web session clones the repo with nothing built, so the first
`cargo build --release` compiles all ~287 dependency crates (Cranelift is the
slow part) — a few minutes that rarely change between sessions.

The web environment can snapshot that work. A **setup script** runs once when an
environment is first used; its filesystem output is snapshotted and reused by
every later session. Put the build there and the compiled dependencies come
pre-built, so the first in-session build only recompiles the `rask-*` crates you
changed — seconds instead of minutes.

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

That's it. The build fits under the ~5-minute setup-script budget, `crates.io`
is on the default Trusted network allowlist, and the snapshot rebuilds itself
when you change the script or after ~7 days.

This lives in environment settings, not the repo — it can't be committed, so
each person configures it once for their environment. A `SessionStart` hook is
*not* a substitute: it isn't snapshotted, so it re-runs the full build on every
session instead of caching once.

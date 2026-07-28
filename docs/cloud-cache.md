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
set the **Setup script** to the block below. It's self-contained on purpose —
the setup script may run before the repo is checked out at a fixed path, so it
can't just call a committed script by path. It finds the checkout wherever it
is, builds if present, and logs what it saw either way:

```bash
#!/bin/bash
set -uo pipefail
log() { echo "[warm-cache] $*"; }
log "user=$(whoami) cwd=$(pwd)"

# Find the repo the platform cloned. Neither the path nor the working
# directory is guaranteed, so probe the likely spots then fall back to a scan.
REPO=""
for c in "$PWD" /home/user/rask /workspace/rask /root/rask; do
  if [ -f "$c/compiler/Cargo.lock" ]; then REPO="$c"; break; fi
done
if [ -z "$REPO" ]; then
  hit="$(find /home /root /workspace -maxdepth 5 -name Cargo.lock -path '*/compiler/*' 2>/dev/null | head -1)"
  [ -n "$hit" ] && REPO="${hit%/compiler/Cargo.lock}"
fi

if [ -z "$REPO" ]; then
  log "repo not checked out at setup time — nothing to warm; skipping"
  exit 0
fi
log "repo=$REPO"

cd "$REPO/compiler"
cargo fetch
cargo build --release -p rask-cli
make -C runtime
log "done"
```

The build fits under the ~5-minute setup-script budget, `crates.io` is on the
default Trusted network allowlist, and the snapshot rebuilds itself when you
change the script or after ~7 days.

`scripts/warm-cache.sh` in the repo runs the same build for local or manual use
(it self-locates from its own path). The setup script above is inline rather
than a call to it because the file may not exist yet when setup runs.

If the log prints `repo not checked out at setup time`, the platform clones the
repo *after* the setup snapshot, and pre-building the whole compiler in setup
isn't possible — the fallback is a `SessionStart` hook that builds per session
(no snapshot, so it re-runs each time). Check the setup log before assuming the
snapshot route works.

This lives in environment settings, not the repo — each person configures it
once for their environment.

#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# The runtime configuration macOS gets, run on Linux.
#
# There is no green scheduler off Linux — it needs an I/O engine and the only
# two are epoll and io_uring — so a macOS program links `green_threads.c`
# instead, where a task is an OS thread. Nothing on a Linux machine exercised
# that: `spawn` didn't even link on macOS for two releases, and the way anyone
# found out was compiling on their own Mac (#1180, #1170).
#
# `RASK_NO_GREEN=1` builds that set here — the same switch `RASK_HAS_GREEN` in
# rask_runtime.h reads — so a break in it fails a PR on an ubuntu runner.
#
# Just the files that spawn. The rest of the suite doesn't touch the scheduler,
# and the differential harness already runs it.
#
# A file the project has already registered as expected-red is skipped, from
# the same two lists differential.sh reads. Without that, a registered
# divergence that happens to mention `spawn(` fails this gate for a reason this
# gate isn't about — `t_native_reach_taskgroup.rk` did, and the failure read as
# "macOS is broken" when what it says is "TaskGroup has no native entry point",
# which is #1288 and already tracked.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

export RASK_NO_GREEN=1

# First field of each non-comment line; a trailing `# note` is ignored.
names_in() { [ -f "$1" ] && awk 'NF && $1 !~ /^#/ {print $1}' "$1"; }

EXPECTED_RED="$({ names_in "$ROOT/tests/known_divergences.txt"
                  names_in "$ROOT/tests/pending_features.txt"; } | sort -u)"

ALL=$(grep -rl 'spawn(' "$ROOT"/tests/suite/*.rk | sort)
if [ -z "$ALL" ]; then
  echo "error: no suite file spawns — this gate would pass by checking nothing" >&2
  exit 1
fi

FILES=""
skipped=0
for f in $ALL; do
  if printf '%s\n' "$EXPECTED_RED" | grep -qxF "$(basename "$f")"; then
    skipped=$((skipped + 1))
    continue
  fi
  FILES="$FILES $f"
done
if [ -z "${FILES// /}" ]; then
  echo "error: every spawning suite file is registered red — nothing left to check" >&2
  exit 1
fi

# macOS has no `timeout` — it's GNU coreutils, and this gate runs on Darwin too
# (it's the configuration Darwin gets). Use it where it exists, and where it
# doesn't rely on the job's own timeout: a hang is what this gate is for, so it
# has to fail rather than be skipped.
TIMEOUT=""
if command -v timeout > /dev/null 2>&1; then
  TIMEOUT="timeout 180"
elif command -v gtimeout > /dev/null 2>&1; then
  TIMEOUT="gtimeout 180"
fi

ok=0
failed=0
for f in $FILES; do
  name="$(basename "$f")"
  if out=$($TIMEOUT "$RASK" test "$f" 2>&1); then
    ok=$((ok + 1))
  else
    failed=$((failed + 1))
    echo "FAIL: $name"
    echo "$out" | tail -20 | sed 's/^/    /'
  fi
done

echo "──────────────────────────────────────────────────"
echo "no-green gate: $ok ok, $failed failed, $skipped registered-red (the source set macOS links)"
[ "$failed" -eq 0 ]

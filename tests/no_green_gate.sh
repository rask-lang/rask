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

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

export RASK_NO_GREEN=1

FILES=$(grep -rl 'spawn(' "$ROOT"/tests/suite/*.rk | sort)
if [ -z "$FILES" ]; then
  echo "error: no suite file spawns — this gate would pass by checking nothing" >&2
  exit 1
fi

ok=0
failed=0
for f in $FILES; do
  name="$(basename "$f")"
  if out=$(timeout 180 "$RASK" test "$f" 2>&1); then
    ok=$((ok + 1))
  else
    failed=$((failed + 1))
    echo "FAIL: $name"
    echo "$out" | tail -20 | sed 's/^/    /'
  fi
done

echo "──────────────────────────────────────────────────"
echo "no-green gate: $ok ok, $failed failed (the source set macOS links)"
[ "$failed" -eq 0 ]

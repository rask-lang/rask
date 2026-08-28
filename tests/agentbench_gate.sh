#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Agent-benchmark reference gate. Costs nothing — no model is called, and the
# paid adapters aren't reachable from here at all.
#
# What it checks: every reference solution in agentbench/tasks/ still builds and
# passes on BOTH backends, or is registered in agentbench/quarantine.txt against
# an issue. A task whose reference has gone red is measuring the compiler
# instead of the model, so the scores stop meaning anything until it's
# quarantined or fixed. A quarantined task that starts passing is reported as an
# UNEXPECTED PASS so the line gets pruned — same contract as
# tests/known_divergences.txt.
#
# That makes this a differential test over nineteen more ordinary programs,
# which is why it stays in CI: #1000 and #1002 were both caught here, by a
# reference going red, before any model was involved.
#
# What it deliberately does NOT check: the harness itself. Running the attempt
# loop against the mock model used to live here, and CI shouldn't depend on
# benchmark plumbing — that's `agentbench/bench.py run --model mock:solves@2`,
# run by hand when the harness changes.
#
# Usage:  tests/agentbench_gate.sh
# Exit:   0 = green, 1 = a reference broke

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BENCH="$ROOT/agentbench/bench.py"
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

if [ ! -f "$BENCH" ]; then
    echo "error: $BENCH not found" >&2
    exit 2
fi

status=0

echo "=== reference solutions ==="
python3 "$BENCH" selftest || status=1

echo
echo "──────────────────────────────────────────────────"
if [ "$status" -eq 0 ]; then
    echo "agentbench gate: ok"
else
    echo "agentbench gate: FAILED"
fi
exit "$status"

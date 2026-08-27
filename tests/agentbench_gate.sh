#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Agent-benchmark gate. Costs nothing — no model is called.
#
# Two things it keeps honest:
#
#   1. Every reference solution in agentbench/tasks/ still builds and passes on
#      BOTH backends, or is registered in agentbench/quarantine.txt against an
#      issue. A task whose reference has gone red is measuring the compiler
#      instead of the model, so the scores stop meaning anything until it's
#      quarantined or fixed. A quarantined task that starts passing is reported
#      as an UNEXPECTED PASS so the line gets pruned — same contract as
#      tests/known_divergences.txt.
#
#   2. The harness itself still runs end to end, using the deterministic mock
#      model: prompt, assemble, compile both backends, score, write transcripts.
#
# Usage:  tests/agentbench_gate.sh
# Exit:   0 = green, 1 = a reference broke or the harness did.

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
echo "=== the harness itself (mock model, no spend) ==="
# One task per horizon is enough to walk every path: a failed attempt, the
# retry that carries the compiler's output, and a green one.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
python3 "$BENCH" run \
    --model mock:solves@2 \
    --max-attempts 2 \
    --select day_fizzbuzz,week_stack,month_comptime \
    --out "$work/run" || status=1

for artifact in run.json report.md transcripts/day_fizzbuzz.md; do
    if [ ! -s "$work/run/$artifact" ]; then
        echo "FAILURE: the run produced no $artifact"
        status=1
    fi
done

# The mock fails once and then emits the reference, so a green harness scores
# three solved tasks at exactly two attempts each. Anything else means the loop,
# the classifier, or the scoring drifted.
solved="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['score']['solved'])" "$work/run/run.json" 2>/dev/null || echo "?")"
if [ "$solved" != "3" ]; then
    echo "FAILURE: mock:solves@2 solved $solved of 3 tasks"
    status=1
fi

echo
echo "──────────────────────────────────────────────────"
if [ "$status" -eq 0 ]; then
    echo "agentbench gate: ok"
else
    echo "agentbench gate: FAILED"
fi
exit "$status"

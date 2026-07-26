#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Example run-gate.
#
# Nothing ever ran examples/, so native codegen rot went unnoticed there too.
# This gate takes every example that has a checked-in golden output in
# tests/golden/<name>.out and, for BOTH backends, runs it and diffs stdout
# against the golden. A native run that crashes, aborts, or prints something
# different from interp is a FAILURE — an exit-0-but-wrong miscompile fails here.
#
# Adding tests/golden/<name>.out (for examples/<name>.rk) auto-enrolls it.
# Examples that currently fail natively (10_enums prints nothing, sensor_processor
# generic-layout miscompile #272, etc.) have no golden yet and are listed, with
# their tracking issue, in tests/known_fail_examples.txt — regenerate their
# golden and drop the line when the bug is fixed.
#
# Usage:  tests/examples_gate.sh
# Exit:   0 = all gated examples match golden on both backends, 1 = a mismatch.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GOLDEN_DIR="$ROOT/tests/golden"
EXAMPLES_DIR="$ROOT/examples"

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

fails=0
ok=0

for golden in "$GOLDEN_DIR"/*.out; do
    [ -e "$golden" ] || continue
    name="$(basename "$golden" .out)"
    src="$EXAMPLES_DIR/$name.rk"
    if [ ! -f "$src" ]; then
        echo "MISSING SRC: $name.rk (has golden but no example)"
        fails=$((fails+1))
        continue
    fi
    want="$(cat "$golden")"

    iout="$(timeout 60 "$RASK" run --interp "$src" 2>/dev/null)"; ic=$?
    nout="$(timeout 60 "$RASK" run --native "$src" 2>/dev/null)"; nc=$?

    bad=""
    [ "$ic" -ne 0 ] && bad="$bad interp-exit=$ic"
    [ "$nc" -ne 0 ] && bad="$bad native-exit=$nc"
    [ "$iout" != "$want" ] && bad="$bad interp-mismatch"
    [ "$nout" != "$want" ] && bad="$bad native-mismatch"

    if [ -z "$bad" ]; then
        ok=$((ok+1))
    else
        echo "FAIL: $name —$bad"
        if [ "$nout" != "$want" ]; then
            echo "  native diff (want → got):"
            diff <(printf '%s' "$want") <(printf '%s' "$nout") | head -8 | sed 's/^/    /'
        fi
        fails=$((fails+1))
    fi
done

echo "──────────────────────────────────────────────────"
echo "examples gate: $ok ok, $fails failed"
[ "$fails" -eq 0 ] || exit 1
exit 0

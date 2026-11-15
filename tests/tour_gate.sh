#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Tour gate. Every step in docs/book/src/tour/ is a file the reader runs as
# they find it, so each one has to fail in the way the step says and pass
# once it's done:
#
#   - a step with an answer in solutions/ must fail `rask test` as shipped
#     (otherwise there's nothing to do) and its solution must pass;
#   - a step without one must run.
#
# The old tutorial's answer key went unchecked and seven of nineteen answers
# stopped compiling (#1030); this is the same check for the tour.
#
# Usage:  tests/tour_gate.sh
# Exit:   0 = every step behaves as the tour says.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
TOUR="$ROOT/docs/book/src/tour"

if [ ! -x "$RASK" ]; then
    echo "no rask binary at $RASK — cargo build --release -p rask-cli" >&2
    exit 1
fi

failed=0
for step in "$TOUR"/[0-9]*.rk; do
    name="$(basename "$step")"
    answer="$TOUR/solutions/$name"
    if [ -f "$answer" ]; then
        if "$RASK" test "$step" >/dev/null 2>&1; then
            echo "FAIL $name: passes before the reader has done anything"
            failed=1
        fi
        if ! out="$("$RASK" test "$answer" 2>&1)"; then
            echo "FAIL solutions/$name:"
            echo "$out"
            failed=1
        fi
    elif ! out="$("$RASK" run "$step" 2>&1)"; then
        echo "FAIL $name doesn't run:"
        echo "$out"
        failed=1
    fi
done

for answer in "$TOUR"/solutions/*.rk; do
    if [ ! -f "$TOUR/$(basename "$answer")" ]; then
        echo "FAIL solutions/$(basename "$answer") has no step"
        failed=1
    fi
done

if [ "$failed" -eq 0 ]; then
    echo "tour gate: ok"
fi
exit "$failed"

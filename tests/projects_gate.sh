#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Project check-gate.
#
# Nothing checked projects/*/, which is how raido reached 48 type errors and
# tiwaz 11 (#774) — every rule change since they were written landed without
# anything noticing. Each one is a pile now instead of a one-line fix at the
# time, and the pile is what made them look like a compiler problem.
#
# This runs `rask check` over every .rk file in projects/ and fails on any error.
# It is deliberately *only* `check`: neither project runs natively yet (raido
# needs #793 and #794), and a run-gate would be red for reasons that have nothing
# to do with the sources drifting.
#
# A project that isn't expected to check goes in tests/known_fail_projects.txt
# with its tracking issue, same shape as tests/known_fail_examples.txt.
#
# Usage:  tests/projects_gate.sh
# Exit:   0 = every project checks (or is listed as known-fail), 1 = otherwise.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECTS_DIR="$ROOT/projects"
KNOWN_FAIL="$ROOT/tests/known_fail_projects.txt"

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

# Files listed as known-fail, one relative path per line, `#` comments ignored.
is_known_fail() {
    [ -f "$KNOWN_FAIL" ] || return 1
    grep -v '^[[:space:]]*#' "$KNOWN_FAIL" 2>/dev/null \
        | grep -v '^[[:space:]]*$' \
        | grep -qxF "$1"
}

fails=0
ok=0
known=0

for f in $(find "$PROJECTS_DIR" -name '*.rk' | sort); do
    rel="${f#"$ROOT"/}"
    out=$("$RASK" check "$f" 2>&1)
    status=$?
    if [ "$status" -eq 0 ]; then
        if is_known_fail "$rel"; then
            echo "UNEXPECTED-PASS: $rel — checks now, drop it from known_fail_projects.txt"
            fails=$((fails + 1))
        else
            ok=$((ok + 1))
        fi
    elif is_known_fail "$rel"; then
        echo "KNOWN-FAIL: $rel"
        known=$((known + 1))
    else
        echo "FAIL: $rel"
        echo "$out" | grep -E '^error' | sed 's/^/    /'
        fails=$((fails + 1))
    fi
done

echo "──────────────────────────────────────────────────"
echo "projects gate: $ok ok, $fails failed, $known known-fail"
[ "$fails" -eq 0 ]

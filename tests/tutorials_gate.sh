#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Tutorial answer-key gate.
#
# `tutorials/learn-rask/solutions/` wasn't referenced by the workflow or by any
# script in tests/, so nothing had been checking it. Seven of the nineteen
# solutions didn't compile (#1030), and they'd been that way for a while — the
# same seven fail on the commit before the string-API change that surfaced
# them. These are the answer key for people learning the language, so a
# solution that doesn't compile is worse than a missing one.
#
# Every .rk under solutions/ AND the lesson files beside them must pass
# `rask check`. That's the bar the issue set, and it's the right one: most have
# no `main`, and the ones with `test` blocks are covered further down.
#
# The lessons matter as much as the answers, and had drifted further. Six of
# them didn't compile, and lesson 12 was worse than that: it taught structural
# conformance ("You don't need to write implements HasArea — if the methods
# match, it just works"), which #283 replaced with nominal. Text that reads
# fine and is wrong costs more than text that fails to compile. Its puzzle also
# told you to return 3500.0 from `range_nm` and then asserted 2935.0.
#
# Files listed in tests/known_fail_tutorials.txt are expected to fail their
# `test` run and are reported without failing the gate — each line carries the
# issue. A listed file that starts passing fails the gate too, so the list can
# only shrink.
#
# Usage:  tests/tutorials_gate.sh
# Exit:   0 = every solution checks and every unlisted one tests clean.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
LESSONS="$ROOT/tutorials/learn-rask"
DIR="$LESSONS/solutions"
KNOWN="$ROOT/tests/known_fail_tutorials.txt"

if [ ! -x "$RASK" ]; then
    echo "no rask binary at $RASK — cargo build --release -p rask-cli" >&2
    exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# basename → the rest of its line, for reporting.
declare -A known
if [ -f "$KNOWN" ]; then
    while IFS= read -r line; do
        case "$line" in ''|'#'*) continue ;; esac
        name="${line%%[[:space:]]*}"
        known["$name"]="${line#"$name"}"
    done < "$KNOWN"
fi

checked=0
check_failed=0
tested=0
test_failed=0
expected_red=0
unexpected_green=()

for f in "$LESSONS"/*.rk "$DIR"/*.rk; do
    name="$(basename "$f")"
    checked=$((checked + 1))
    if ! "$RASK" check "$f" > "$WORK/out" 2>&1; then
        echo "CHECK-FAIL: $name"
        sed 's/^/  /' "$WORK/out" | head -12
        check_failed=$((check_failed + 1))
        continue
    fi

    # A lesson's puzzle bodies are `todo()` — that's the exercise. Only the
    # answer key is expected to pass its own tests.
    case "$f" in "$DIR"/*) ;; *) continue ;; esac

    # No `test` blocks — checking was the whole job.
    if ! grep -q '^test "' "$f"; then
        continue
    fi

    tested=$((tested + 1))
    # `rask test` exits non-zero on a failing assertion or a compile error, so
    # the exit code is the whole answer — the summary line always contains the
    # word "failed", count and all.
    if "$RASK" test "$f" > "$WORK/out" 2>&1; then
        if [ -n "${known[$name]+set}" ]; then
            unexpected_green+=("$name")
        fi
        continue
    fi
    if [ -n "${known[$name]+set}" ]; then
        echo "KNOWN-FAIL: $name —${known[$name]}"
        expected_red=$((expected_red + 1))
        continue
    fi
    echo "TEST-FAIL: $name"
    sed 's/^/  /' "$WORK/out" | tail -12
    test_failed=$((test_failed + 1))
done

echo "──────────────────────────────────────────────────"
echo "tutorials gate: $checked checked, $check_failed check-failures, \
$tested with tests, $test_failed test-failures, $expected_red known-fail"

status=0
if [ "$check_failed" -gt 0 ] || [ "$test_failed" -gt 0 ]; then
    status=1
fi
if [ "${#unexpected_green[@]}" -gt 0 ]; then
    echo "NOW PASSING (drop the line from known_fail_tutorials.txt): ${unexpected_green[*]}"
    status=1
fi
exit "$status"

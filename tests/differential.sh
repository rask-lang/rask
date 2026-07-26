#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Differential backend harness.
#
# Runs every tests/suite/*.rk file on BOTH backends (interp + native) and fails
# on divergence. Interp is the de-facto reference; native rot otherwise
# accumulates silently because `rask test` only runs one backend per invocation.
#
# A file is DIVERGENT when interp and native disagree on pass/fail OR on output
# (timings stripped). A test that passes on interp but crashes, aborts,
# mis-prints, or produces empty output on native is a FAILURE — the whole point
# of the harness.
#
# KNOWN-FAIL files (tracked native codegen bugs) are listed in
# tests/known_divergences.txt, one basename per line. They're still run and
# reported, but don't fail the harness. When a bug is fixed, drop its line and
# the file rejoins the always-green gate. Any KNOWN-FAIL that has silently
# started passing is reported as UNEXPECTED PASS so the list gets pruned.
#
# Usage:  tests/differential.sh [suite-dir]
# Exit:   0 = no unexpected divergence, 1 = at least one.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUITE_DIR="${1:-$ROOT/tests/suite}"
KNOWN_DIV_FILE="$ROOT/tests/known_divergences.txt"

# Locate the rask binary (release preferred, debug fallback).
if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

# Load known-fail basenames (first field of each non-comment line; a trailing
# `# cluster — #issue` annotation is ignored).
known_fail() {
    [ -f "$KNOWN_DIV_FILE" ] || return 1
    awk 'NF && $1 !~ /^#/ {print $1}' "$KNOWN_DIV_FILE" | grep -qxF "$1"
}

# Strip per-test and total timing tokens so only semantics are compared.
normalize() {
    sed -E 's/\([0-9]+ms\)//g'
}

divergent=0
known=0
unexpected_pass=0
ok=0
both_fail=0
fail_files=()
upass_files=()

for f in "$SUITE_DIR"/*.rk; do
    [ -e "$f" ] || continue
    base="$(basename "$f")"

    # Capture rask's real exit code (a pipe would report the filter's instead).
    iraw="$("$RASK" test --interp "$f" 2>&1)"; icode=$?
    nraw="$("$RASK" test "$f" 2>&1)"; ncode=$?
    iout="$(printf '%s' "$iraw" | normalize)"
    nout="$(printf '%s' "$nraw" | normalize)"

    # A file is GREEN only when both backends pass with identical (timing-
    # stripped) output. Anything else — one backend fails (divergence) or both
    # fail (shared/check-stage bug) — is a red file that MUST be tracked in
    # known_divergences.txt, else it fails the harness.
    green=0
    if [ "$icode" -eq 0 ] && [ "$ncode" -eq 0 ] && [ "$iout" = "$nout" ]; then green=1; fi

    if [ "$green" -eq 1 ]; then
        if known_fail "$base"; then
            echo "UNEXPECTED PASS: $base (listed in known_divergences.txt but now green — prune it)"
            unexpected_pass=$((unexpected_pass+1))
            upass_files+=("$base")
        else
            ok=$((ok+1))
        fi
        continue
    fi

    # Classify the red file for the report.
    kind="DIVERGENT"
    if [ "$icode" -ne 0 ] && [ "$ncode" -ne 0 ]; then kind="BOTH-FAIL"; fi

    if known_fail "$base"; then
        echo "KNOWN-FAIL: $base [$kind] (interp exit $icode, native exit $ncode)"
        known=$((known+1))
        [ "$kind" = "BOTH-FAIL" ] && both_fail=$((both_fail+1))
    else
        echo "FAILURE:    $base [$kind] (interp exit $icode, native exit $ncode) — untracked"
        echo "  --- interp (normalized tail) ---"
        echo "$iout" | tail -4 | sed 's/^/  /'
        echo "  --- native (normalized tail) ---"
        echo "$nout" | tail -4 | sed 's/^/  /'
        divergent=$((divergent+1))
        fail_files+=("$base")
    fi
done

echo "──────────────────────────────────────────────────"
echo "differential: $ok green, $known known-fail (of which $both_fail both-fail), $divergent untracked-failure, $unexpected_pass unexpected-pass"
if [ "$divergent" -gt 0 ]; then
    echo "UNTRACKED FAILURES (add to known_divergences.txt with an issue, or fix): ${fail_files[*]}"
fi
if [ "$unexpected_pass" -gt 0 ]; then
    echo "PRUNE FROM known_divergences.txt: ${upass_files[*]}"
fi

# Fail on new divergence or on a stale known-fail entry that now passes.
if [ "$divergent" -gt 0 ] || [ "$unexpected_pass" -gt 0 ]; then
    exit 1
fi
exit 0

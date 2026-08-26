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
# Expected-red files are registered in two places, both run+reported but
# non-fatal:
#   tests/known_divergences.txt — bugs/regressions (a feature that SHOULD work
#     but is broken on a backend). Red here is bad news.
#   tests/pending_features.txt  — the TDD backlog: tests that assert spec
#     behavior for UNIMPLEMENTED features (SIMD, bits, select, numeric limits…).
#     Red here is expected — the test drives the implementation and flips green
#     when the feature lands.
# When a bug is fixed or a feature is built, drop its line and the file rejoins
# the green gate. Any expected-red file that silently starts passing is reported
# as UNEXPECTED PASS so the list gets pruned.
#
# Usage:  tests/differential.sh [suite-dir]
# Exit:   0 = no unexpected divergence, 1 = at least one.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUITE_DIR="${1:-$ROOT/tests/suite}"
KNOWN_DIV_FILE="$ROOT/tests/known_divergences.txt"
PENDING_FILE="$ROOT/tests/pending_features.txt"

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

# First field of each non-comment line; a trailing `# note` is ignored.
names_in() { [ -f "$1" ] && awk 'NF && $1 !~ /^#/ {print $1}' "$1"; }

# A file is expected-red if it's a tracked bug/regression OR a pending feature.
known_fail() {
    { names_in "$KNOWN_DIV_FILE"; names_in "$PENDING_FILE"; } | grep -qxF "$1"
}

# Which registry a file came from, for reporting: PENDING vs KNOWN-FAIL.
reg_label() {
    if names_in "$PENDING_FILE" | grep -qxF "$1"; then echo "PENDING"; else echo "KNOWN-FAIL"; fi
}

# Which backends a registry line says the file fails on: `(native)`, `(interp)`
# or `(both)` in its note. First exact match wins — a line naming two issues
# leads with the one that describes the file's overall state. Prose in the
# parentheses (`(interp keeps the value, native wraps)`) deliberately doesn't
# match; there's no claim to check against.
reg_scope() {
    awk -v want="$1" '
        NF && $1 !~ /^#/ && $1 == want {
            if (match($0, /\((native|interp|both)\)/)) {
                print substr($0, RSTART + 1, RLENGTH - 2)
            }
            exit
        }' "$KNOWN_DIV_FILE" "$PENDING_FILE"
}

# Strip per-test and total timing tokens so only semantics are compared.
normalize() {
    sed -E 's/\([0-9]+ms\)//g'
}

divergent=0
known=0
misfiled=0
unexpected_pass=0
ok=0
both_fail=0
fail_files=()
upass_files=()
misfiled_files=()

# Each file is independent — two subprocesses and a comparison — so the runs
# fan out across cores and only the classification below stays sequential. The
# native path compiles to a temp binary named with the compiler's own PID, so
# concurrent invocations can't collide on it.
#
# Workers write one record per file; the loop then reads them back in glob
# order, so output and exit codes are identical to running this serially.
JOBS="${DIFF_JOBS:-$(nproc 2>/dev/null || echo 4)}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

run_one() {
    f="$1"
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

    printf '%s\t%s\t%s\n' "$green" "$icode" "$ncode" > "$WORK/$base.status"
    if [ "$green" -eq 0 ]; then
        # `%s\n` not `%s`: without the trailing newline the separator merges
        # into the last output line and the split below cuts in the wrong place.
        {
            printf '%s\n' "$iout" | tail -4 | sed 's/^/  /'
            printf '\037\n'
            printf '%s\n' "$nout" | tail -4 | sed 's/^/  /'
        } > "$WORK/$base.detail"
    fi
}
export -f run_one normalize
export RASK WORK

find "$SUITE_DIR" -maxdepth 1 -name '*.rk' -print0 \
    | xargs -0 -r -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {}

for f in "$SUITE_DIR"/*.rk; do
    [ -e "$f" ] || continue
    base="$(basename "$f")"
    [ -f "$WORK/$base.status" ] || { echo "FAILURE:    $base — worker produced no result"; divergent=$((divergent+1)); fail_files+=("$base"); continue; }
    IFS=$'\t' read -r green icode ncode < "$WORK/$base.status"

    if [ "$green" -eq 1 ]; then
        if known_fail "$base"; then
            note="bug fixed — prune from known_divergences.txt"
            [ "$(reg_label "$base")" = "PENDING" ] && note="FEATURE IMPLEMENTED — promote out of pending_features.txt"
            echo "UNEXPECTED PASS: $base ($note)"
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
        echo "$(reg_label "$base"): $base [$kind] (interp exit $icode, native exit $ncode)"
        known=$((known+1))
        [ "$kind" = "BOTH-FAIL" ] && both_fail=$((both_fail+1))

        # A red file is only honest while it fails the way its registry line
        # says. `t_day_const_string_array.rk` is registered `(native)` for a
        # const-array bug; when it started failing the *check* for a missing
        # import it failed on both backends, the bug stopped being exercised,
        # and nothing noticed — red counted as red either way (#1005).
        scope="$(reg_scope "$base")"
        claim=""
        case "$scope" in
            native) [ "$icode" -eq 0 ] || claim="registered (native) but the interpreter fails too" ;;
            interp) [ "$ncode" -eq 0 ] || claim="registered (interp) but native fails too" ;;
            both)   if [ "$icode" -eq 0 ] || [ "$ncode" -eq 0 ]; then
                        claim="registered (both) but one backend passes"
                    fi ;;
        esac
        if [ -n "$claim" ]; then
            echo "  MISFILED: $claim — the bug it is registered for may not be exercised any more"
            misfiled=$((misfiled+1))
            misfiled_files+=("$base")
        fi
    else
        echo "FAILURE:    $base [$kind] (interp exit $icode, native exit $ncode) — untracked"
        echo "  --- interp (normalized tail) ---"
        sed -n "1,/\o037/p" "$WORK/$base.detail" | sed '$d'
        echo "  --- native (normalized tail) ---"
        sed -n "/\o037/,\$p" "$WORK/$base.detail" | sed '1d'
        divergent=$((divergent+1))
        fail_files+=("$base")
    fi
done

echo "──────────────────────────────────────────────────"
echo "differential: $ok green, $known expected-red (bugs + pending; $both_fail both-fail), $divergent untracked-failure, $unexpected_pass unexpected-pass, $misfiled misfiled"
if [ "$divergent" -gt 0 ]; then
    echo "UNTRACKED FAILURES (add to known_divergences.txt with an issue, or fix): ${fail_files[*]}"
fi
if [ "$unexpected_pass" -gt 0 ]; then
    echo "NOW PASSING — prune from the registries: ${upass_files[*]}"
fi
if [ "$misfiled" -gt 0 ]; then
    echo "MISFILED (failing differently than registered; fix the file or correct the note): ${misfiled_files[*]}"
fi

# Fail on new divergence, on a stale known-fail entry that now passes, or on a
# red file that stopped failing the way its registry line claims.
if [ "$divergent" -gt 0 ] || [ "$unexpected_pass" -gt 0 ] || [ "$misfiled" -gt 0 ]; then
    exit 1
fi
exit 0

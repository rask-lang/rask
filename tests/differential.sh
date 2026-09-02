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
# Every registered line must end its note with what it claims — which backends
# fail and how far each gets:
#
#   t_month_try_in_test.rk   # … — #932 (native compile)
#   t_month_resource_loop.rk # … — #928 (both check)
#
# Backend is native, interp or both; phase is check, compile or run. The gate
# holds the file to that claim, because "still red" is not the same as "still
# testing the bug" — a file registered for a codegen bug that stops compiling
# for an unrelated reason is red either way, and the bug quietly stops being
# exercised (#1005). A mismatch, or a line with no claim, is MISFILED.
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

# What a registry line claims about its file: which backends fail and how far
# each got. Written `(native run)`, `(both check)`, `(interp compile)` — backend
# then phase, in the note. First match wins; a line naming two issues leads with
# the one describing the file's overall state.
#
# Prose in parentheses (`(interp keeps the value, native wraps)`) deliberately
# doesn't match. It makes no checkable claim, and reading one out of it would be
# inventing the claim rather than checking it — so such a line counts as
# unmarked and is reported as such.
reg_claim() {
    awk -v want="$1" '
        NF && $1 !~ /^#/ && $1 == want {
            if (match($0, /\((native|interp|both) +(check|compile|run)\)/)) {
                print substr($0, RSTART + 1, RLENGTH - 2)
            }
            exit
        }' "$KNOWN_DIV_FILE" "$PENDING_FILE"
}

# Strip per-test and total timing tokens so only semantics are compared.
normalize() {
    sed -E 's/\([0-9]+ms\)//g'
}

# How far a backend got before it failed: pass, check, compile or run. This is
# the difference between "the bug is still being exercised" and "the file stopped
# building" — both of which are just `red` to an exit code (#1005).
#
# `error[E…]` is a checker diagnostic and `error: compile:` is codegen giving up;
# anything else that failed got far enough to run a test body, including a
# runtime panic, which prints no marker of its own.
phase_of() {
    if [ "$2" -eq 0 ]; then echo pass; return; fi
    case "$1" in
        *"=== Check FAILED"*|*"error[E"*) echo check ;;
        *"error: compile:"*)              echo compile ;;
        *)                                echo run ;;
    esac
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

    iphase="$(phase_of "$iraw" "$icode")"
    nphase="$(phase_of "$nraw" "$ncode")"
    printf '%s\t%s\t%s\t%s\t%s\n' "$green" "$icode" "$ncode" "$iphase" "$nphase" > "$WORK/$base.status"
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
export -f run_one normalize phase_of
export RASK WORK

find "$SUITE_DIR" -maxdepth 1 -name '*.rk' -print0 \
    | xargs -0 -r -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {}

for f in "$SUITE_DIR"/*.rk; do
    [ -e "$f" ] || continue
    base="$(basename "$f")"
    [ -f "$WORK/$base.status" ] || { echo "FAILURE:    $base — worker produced no result"; divergent=$((divergent+1)); fail_files+=("$base"); continue; }
    IFS=$'\t' read -r green icode ncode iphase nphase < "$WORK/$base.status"

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
        # says. `t_day_const_string_array.rk` is registered `(native run)` for a
        # const-array bug; when it lost an import it failed the *check* on both
        # backends, the bug stopped being exercised, and nothing noticed — red
        # counted as red either way (#1005).
        #
        # Two claims to hold it to: which backends fail, and how far each got.
        # A file that stays red on the same backends but slides from `run` to
        # `check` has stopped testing what it was written to test, and only the
        # phase half sees that.
        claim="$(reg_claim "$base")"
        want_side="${claim%% *}"
        want_phase="${claim##* }"

        # What the file actually does right now, so an unmarked line can be told
        # exactly what to say instead of being told a marker is missing.
        if [ "$icode" -ne 0 ] && [ "$ncode" -ne 0 ]; then
            observed="both $nphase"
            [ "$iphase" = "$nphase" ] || observed="interp=$iphase native=$nphase"
        elif [ "$ncode" -ne 0 ]; then
            observed="native $nphase"
        else
            observed="interp $iphase"
        fi

        why=""
        if [ -z "$claim" ]; then
            why="no (backend phase) marker on its registry line — it currently fails ($observed)"
        else
            case "$want_side" in
                native)
                    if [ "$icode" -ne 0 ]; then
                        why="registered ($claim) but the interpreter fails too, at $iphase"
                    elif [ "$nphase" != "$want_phase" ]; then
                        why="registered ($claim) but native now fails at $nphase"
                    fi ;;
                interp)
                    if [ "$ncode" -ne 0 ]; then
                        why="registered ($claim) but native fails too, at $nphase"
                    elif [ "$iphase" != "$want_phase" ]; then
                        why="registered ($claim) but the interpreter now fails at $iphase"
                    fi ;;
                both)
                    if [ "$icode" -eq 0 ] || [ "$ncode" -eq 0 ]; then
                        why="registered ($claim) but one backend passes"
                    elif [ "$iphase" != "$want_phase" ] || [ "$nphase" != "$want_phase" ]; then
                        why="registered ($claim) but now fails at interp=$iphase native=$nphase"
                    fi ;;
            esac
        fi
        if [ -n "$why" ]; then
            echo "  MISFILED: $why"
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

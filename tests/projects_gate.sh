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
#
# A project that isn't expected to check goes in tests/known_fail_projects.txt
# with its tracking issue, same shape as tests/known_fail_examples.txt.
#
# Then the second half: any project listed in tests/project_runs.txt is *run* on
# both backends and the two outputs are diffed. Checking alone can't see a
# miscompile, and raido spent three bugs (#793, #794, #831) being the thing that
# only native got wrong. A project joins the list the day it runs identically on
# both — one entry per line, the path to its entry `.rk` relative to the repo
# root, `#` comments ignored.
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

# ─── Run the projects that run, and diff the backends ───

RUNS_FILE="$ROOT/tests/project_runs.txt"
ran=0

if [ -f "$RUNS_FILE" ]; then
    while IFS= read -r line; do
        entry="${line%%#*}"
        entry="$(echo "$entry" | tr -d '[:space:]')"
        [ -n "$entry" ] || continue

        src="$ROOT/$entry"
        if [ ! -f "$src" ]; then
            echo "FAIL: $entry — listed in project_runs.txt but not on disk"
            fails=$((fails + 1))
            continue
        fi

        dir="$(dirname "$src")"
        native_out="$(mktemp)"
        interp_out="$(mktemp)"

        # Native goes through `rask build`, so a project's own build.rk (and
        # whatever it links) is what runs — the same binary a user would get.
        build_log=$(cd "$dir" && "$RASK" build 2>&1)
        if [ $? -ne 0 ]; then
            echo "FAIL: $entry — native build failed"
            echo "$build_log" | grep -E '^error' | sed 's/^/    /'
            fails=$((fails + 1))
            rm -f "$native_out" "$interp_out"
            continue
        fi
        bin="$dir/build/debug/$(basename "$dir")"
        if [ ! -x "$bin" ]; then
            bin=$(find "$dir/build/debug" -maxdepth 1 -type f -perm -u+x 2>/dev/null | head -1)
        fi

        (cd "$dir" && timeout 120 "$bin" > "$native_out" 2>&1)
        native_status=$?
        (cd "$dir" && timeout 300 "$RASK" run --interp "$src" > "$interp_out" 2>&1)
        interp_status=$?

        if [ "$native_status" -ne "$interp_status" ]; then
            echo "FAIL: $entry — native exit $native_status, interp exit $interp_status"
            fails=$((fails + 1))
        elif ! diff -q "$native_out" "$interp_out" > /dev/null; then
            echo "FAIL: $entry — the backends disagree"
            diff "$native_out" "$interp_out" | head -20 | sed 's/^/    /'
            fails=$((fails + 1))
        else
            ran=$((ran + 1))
        fi
        rm -f "$native_out" "$interp_out"
    done < "$RUNS_FILE"
fi

echo "──────────────────────────────────────────────────"
echo "projects gate: $ok ok, $fails failed, $known known-fail, $ran run on both backends"
[ "$fails" -eq 0 ]

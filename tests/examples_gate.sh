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
#
# An example that needs command-line arguments gets tests/golden/<name>.args:
# one argv per line, blank and #-comment lines ignored, each line run in order
# with stdout concatenated into the single golden. That's how a CLI example
# covers its flags without needing a golden per invocation. Paths in .args are
# relative to the repo root; put input files under tests/fixtures/.
#
# An example that reads stdin gets tests/golden/<name>.stdin — the session to
# feed it. Examples without one get /dev/null, so an interactive example sees
# EOF and exits rather than hanging the gate.
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
pending=0

# Examples written in canon syntax the compiler hasn't implemented yet. They
# keep their golden (the expected output is still right — only the spelling
# moved ahead), but a failure is expected rather than fatal. If one starts
# passing, that's a hard failure: the feature landed, so drop it from here.
PENDING_FILE="$ROOT/tests/pending_examples.txt"
is_pending() {
    [ -f "$PENDING_FILE" ] || return 1
    grep -qE "^$1\.rk([[:space:]]|#|$)" "$PENDING_FILE"
}

# Each example is independent, so the runs fan out across cores; only the
# reporting below stays sequential, reading results back in glob order so the
# output and exit code match a serial run exactly.
JOBS="${GATE_JOBS:-$(nproc 2>/dev/null || echo 4)}"

# Per-invocation ceiling. Deliberately far above what any example needs — the
# slowest gated one takes ~5s including compilation, run on its own. The gate
# fans out across every core, so under load (another build, another suite) a
# run can take many times its solo time, and a ceiling sized for the solo
# number turns contention into a failed example. 60s did exactly that once.
# It's still bounded, so an example that actually hangs fails instead of
# blocking the gate.
RUN_TIMEOUT="${GATE_TIMEOUT:-300}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Run every invocation for an example on one backend, concatenating stdout.
# With no .args file that's a single bare run, which is what most examples are.
# Otherwise each line of the .args file is one argv, so a CLI example can cover
# its flags in a single golden. Paths inside .args are relative to the repo
# root, and the runs happen there so a fixture path means the same thing on
# both backends.
run_backend() {
    src="$1"
    backend="$2"
    argsfile="$3"
    stdinfile="$4"
    # A stdin-driven example reads its session from a fixture. Without one,
    # stdin is /dev/null so an interactive example hits EOF and exits instead
    # of blocking the gate forever.
    if [ -f "$stdinfile" ]; then
        infile="$stdinfile"
    else
        infile=/dev/null
    fi
    if [ ! -f "$argsfile" ]; then
        (cd "$ROOT" && timeout "$RUN_TIMEOUT" "$RASK" run "$backend" "$src" 2>/dev/null < "$infile")
        return $?
    fi
    rc=0
    while IFS= read -r argv || [ -n "$argv" ]; do
        case "$argv" in ''|\#*) continue ;; esac
        # Word-split argv on purpose: the file holds a command line.
        # shellcheck disable=SC2086
        (cd "$ROOT" && timeout "$RUN_TIMEOUT" "$RASK" run "$backend" "$src" -- $argv 2>/dev/null < "$infile") || rc=$?
    done < "$argsfile"
    return $rc
}

run_one() {
    golden="$1"
    name="$(basename "$golden" .out)"
    src="$EXAMPLES_DIR/$name.rk"
    [ -f "$src" ] || { printf 'MISSINGSRC\n' > "$WORK/$name.res"; return; }
    want="$(cat "$golden")"
    argsfile="$GOLDEN_DIR/$name.args"
    stdinfile="$GOLDEN_DIR/$name.stdin"

    iout="$(run_backend "$src" --interp "$argsfile" "$stdinfile")"; ic=$?
    nout="$(run_backend "$src" --native "$argsfile" "$stdinfile")"; nc=$?

    bad=""
    [ "$ic" -ne 0 ] && bad="$bad interp-exit=$ic"
    [ "$nc" -ne 0 ] && bad="$bad native-exit=$nc"
    [ "$iout" != "$want" ] && bad="$bad interp-mismatch"
    [ "$nout" != "$want" ] && bad="$bad native-mismatch"

    printf '%s\n' "$bad" > "$WORK/$name.res"
    # Only the native diff is reported, so that's all the worker needs to keep.
    if [ "$nout" != "$want" ]; then
        diff <(printf '%s' "$want") <(printf '%s' "$nout") | head -8 | sed 's/^/    /' > "$WORK/$name.diff"
    fi
}
export -f run_one run_backend
export RASK WORK EXAMPLES_DIR GOLDEN_DIR ROOT RUN_TIMEOUT

find "$GOLDEN_DIR" -maxdepth 1 -name '*.out' -print0 \
    | xargs -0 -r -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {}

for golden in "$GOLDEN_DIR"/*.out; do
    [ -e "$golden" ] || continue
    name="$(basename "$golden" .out)"
    if [ ! -f "$WORK/$name.res" ] || [ "$(cat "$WORK/$name.res")" = "MISSINGSRC" ]; then
        echo "MISSING SRC: $name.rk (has golden but no example)"
        fails=$((fails+1))
        continue
    fi
    bad="$(cat "$WORK/$name.res")"

    if is_pending "$name"; then
        if [ -z "$bad" ]; then
            echo "FAIL: $name — listed in pending_examples.txt but passes now; remove the line"
            fails=$((fails+1))
        else
            echo "pending: $name —$bad (canon syntax, compiler behind)"
            pending=$((pending+1))
        fi
        continue
    fi

    if [ -z "$bad" ]; then
        ok=$((ok+1))
    else
        echo "FAIL: $name —$bad"
        if [ -f "$WORK/$name.diff" ]; then
            echo "  native diff (want → got):"
            cat "$WORK/$name.diff"
        fi
        fails=$((fails+1))
    fi
done

echo "──────────────────────────────────────────────────"
echo "examples gate: $ok ok, $fails failed, $pending pending"
[ "$fails" -eq 0 ] || exit 1
exit 0

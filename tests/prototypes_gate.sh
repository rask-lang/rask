#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Prototype backend-parity gate.
#
# specs/analysis/prototype/ holds the programs written to show a design working
# end to end — the doubly-linked list, the scene tree, the targeting loop. They
# are the closest thing to real code that exercises Rack/Link, and nothing ever
# compared their two backends.
#
# That blind spot cost real bugs. #984 left `l1_list_links.rk` printing an
# entirely empty list on native while the interpreter printed the right one, and
# it sat there unnoticed. #866 hid the same way in `l3_scene_links.rk`, which
# segfaulted natively for long enough that when the crash was finally fixed, a
# second divergence surfaced that nobody knew was there.
#
# Two things kept them invisible:
#
#   - tests/differential.sh only scans tests/suite/, so it never saw these files.
#   - They are `main`-only. `rask test` on a file with no `test` blocks runs
#     nothing and exits 0, so even a gate that reached them would pass them
#     without executing a line. They have to be *run*.
#
# So this gate runs each one on both backends and compares. The interpreter is
# the reference (CLAUDE.md): where they differ, native is what's wrong.
#
# A prototype that legitimately diverges goes in tests/known_prototype_divergences.txt
# with its issue. A tracked file that starts matching fails the gate, the same
# way differential.sh handles a stale entry — a registry that can quietly outlive
# its bug is worse than no registry.
#
# Usage:  tests/prototypes_gate.sh
# Exit:   0 = every prototype agrees (or is tracked), 1 = an untracked divergence.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROTO_DIR="$ROOT/specs/analysis/prototype"
KNOWN_FILE="$ROOT/tests/known_prototype_divergences.txt"

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

# Strip what legitimately differs between the two drivers rather than between
# the two backends:
#
#   - `(12ms)` timings.
#   - The trailing banner. A rejected program stops at check on the interpreter
#     and at compile on native, so the same diagnostic is announced under two
#     names. `cascade_hole_links.rk` exists to be rejected, and that one word is
#     its only difference.
normalize() {
    sed -E -e 's/\([0-9]+ms\)//g' \
           -e 's/^=== (Check|Compile|Runtime) FAILED:/=== FAILED:/'
}

known_fail() {
    [ -f "$KNOWN_FILE" ] || return 1
    grep -qE "^$1([[:space:]]|#|$)" "$KNOWN_FILE"
}

JOBS="${GATE_JOBS:-$(nproc 2>/dev/null || echo 4)}"
RUN_TIMEOUT="${GATE_TIMEOUT:-300}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# stderr is folded in on purpose: a native crash says what it was on stderr, and
# comparing only stdout would call a segfault "no output" and match it against a
# program that legitimately prints nothing.
run_one() {
    src="$1"
    base="$(basename "$src")"
    iout="$(cd "$ROOT" && timeout "$RUN_TIMEOUT" "$RASK" run --interp "$src" 2>&1 < /dev/null | normalize)"
    ic=${PIPESTATUS[0]}
    nout="$(cd "$ROOT" && timeout "$RUN_TIMEOUT" "$RASK" run "$src" 2>&1 < /dev/null | normalize)"
    nc=${PIPESTATUS[0]}

    agree=0
    if [ "$iout" = "$nout" ] && [ "$ic" -eq "$nc" ]; then agree=1; fi

    # The delete-cost counters are part of the model, not a per-backend
    # diagnostic (mem.racks, "Measuring it"): the two ends are supposed to keep
    # the same edge records, so they are supposed to report the same numbers.
    # Nothing checked that, and two shapes had drifted — a deleted node's own
    # edges on one side, a replaced container's on the other, both of them
    # records naming storage nobody held any more (#983).
    #
    # A second run rather than folding it into the first: the two drivers print
    # the line at different points relative to stdout, so comparing the streams
    # whole would call the ordering a divergence.
    if [ "$agree" -eq 1 ]; then
        istats="$(cd "$ROOT" && RASK_RACK_STATS=1 timeout "$RUN_TIMEOUT" "$RASK" run --interp "$src" 2>&1 < /dev/null | grep '^rack stats:')"
        nstats="$(cd "$ROOT" && RASK_RACK_STATS=1 timeout "$RUN_TIMEOUT" "$RASK" run "$src" 2>&1 < /dev/null | grep '^rack stats:')"
        if [ "$istats" != "$nstats" ]; then
            agree=0
            iout="$istats"
            nout="$nstats"
        fi
    fi

    printf '%s\t%s\t%s\n' "$agree" "$ic" "$nc" > "$WORK/$base.status"
    if [ "$agree" -eq 0 ]; then
        diff <(printf '%s' "$iout") <(printf '%s' "$nout") | head -12 | sed 's/^/    /' \
            > "$WORK/$base.diff"
    fi
}
export -f run_one normalize
export RASK WORK ROOT RUN_TIMEOUT

find "$PROTO_DIR" -maxdepth 1 -name '*.rk' -print0 \
    | xargs -0 -r -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {}

ok=0
known=0
divergent=0
unexpected_pass=0
fail_files=()
upass_files=()

for src in "$PROTO_DIR"/*.rk; do
    [ -e "$src" ] || continue
    base="$(basename "$src")"
    if [ ! -f "$WORK/$base.status" ]; then
        echo "FAILURE:    $base — worker produced no result"
        divergent=$((divergent+1)); fail_files+=("$base"); continue
    fi
    IFS=$'\t' read -r agree ic nc < "$WORK/$base.status"

    if [ "$agree" -eq 1 ]; then
        if known_fail "$base"; then
            echo "UNEXPECTED PASS: $base (backends agree now — prune from known_prototype_divergences.txt)"
            unexpected_pass=$((unexpected_pass+1)); upass_files+=("$base")
        else
            ok=$((ok+1))
        fi
        continue
    fi

    if known_fail "$base"; then
        echo "known:      $base (interp exit $ic, native exit $nc)"
        known=$((known+1))
    else
        echo "FAILURE:    $base — backends disagree (interp exit $ic, native exit $nc)"
        echo "  --- interp → native ---"
        cat "$WORK/$base.diff"
        divergent=$((divergent+1)); fail_files+=("$base")
    fi
done

echo "──────────────────────────────────────────────────"
echo "prototypes gate: $ok agree, $known known-divergent, $divergent untracked, $unexpected_pass unexpected-pass"
if [ "$divergent" -gt 0 ]; then
    echo "UNTRACKED DIVERGENCE (fix, or add to known_prototype_divergences.txt with an issue): ${fail_files[*]}"
fi
if [ "$unexpected_pass" -gt 0 ]; then
    echo "NOW AGREEING — prune from the registry: ${upass_files[*]}"
fi
if [ "$divergent" -gt 0 ] || [ "$unexpected_pass" -gt 0 ]; then
    exit 1
fi
exit 0

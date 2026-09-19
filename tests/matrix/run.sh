#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Type x carrier matrix gate.
#
# Generates one small program per (payload type, carrier) pair, runs every one
# on BOTH backends, and fails on any cell that is not already registered as
# red. Interp is the reference (CLAUDE.md), so read a row as "this payload type
# breaks in these carriers".
#
# Legend per cell:
#   .  both backends print the expected value
#   N  native wrong/failed, interp right  — a native codegen bug
#   I  interp wrong/failed, native right
#   X  both wrong (usually unimplemented, not a miscompile)
#   -  the pair isn't legal Rask; gen.py's SKIPS names the rule
#
# Expected-red cells live in tests/matrix/known_red.txt, one per line, each
# ending its note with what it claims — which backend fails and how far it
# gets:
#
#   seq_yield__vec  # sequence loses non-Copy items — #1234 (native run)
#
# Backend is native, interp or both; phase is check, compile, run or output.
# The gate holds the cell to that claim, the way tests/differential.sh holds a
# suite file to its line (#1005): a cell registered for a codegen bug that
# stops type-checking for an unrelated reason is red either way, and the bug
# quietly stops being exercised. A mismatch, or a line with no claim, is
# MISFILED. A registered cell that starts passing is an UNEXPECTED PASS —
# prune the line.
#
# A cell's native binary runs MATRIX_RUNS times (default 5) under
# RASK_POISON_STACK, and the first wrong answer decides it — see matrix_cell.
#
# Usage:  tests/matrix/run.sh [--types a,b] [--carriers x,y] [--keep]
# Exit:   0 = every cell is clean or registered, 1 = at least one isn't,
#         2 = the harness itself couldn't run.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${MATRIX_WORK:-${TMPDIR:-/tmp}/rask-matrix-$$}"
KNOWN_RED="$ROOT/tests/matrix/known_red.txt"
KEEP=0
GEN_ARGS=()
source "$ROOT/tests/lib/fanout.sh"

while [ $# -gt 0 ]; do
    case "$1" in
        --types)    GEN_ARGS+=(--types "$2"); shift 2 ;;
        --carriers) GEN_ARGS+=(--carriers "$2"); shift 2 ;;
        --keep)     KEEP=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi
export RASK RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

mkdir -p "$WORK"
export WORK
python3 "$ROOT/tests/matrix/gen.py" "$WORK" "${GEN_ARGS[@]}" >/dev/null || exit 2

# Cell names, in generation order, so rows/columns stay stable run to run.
mapfile -t CELLS < <(python3 "$ROOT/tests/matrix/gen.py" --list "${GEN_ARGS[@]}")
[ "${#CELLS[@]}" -gt 0 ] || { echo "error: no cells to run" >&2; exit 2; }

# One cell, on both backends. Writes `<verdict> <phase>` to $WORK/<name>.verdict.
#
# The native path is compile-then-execute rather than `rask run` so a codegen
# failure and a runtime failure are told apart — that split is what the
# known_red.txt claims are made of. `rask check` is shared, so a cell that
# doesn't type-check fails both backends at `check` without running either.
matrix_cell() {
    local name="$1"
    local src="$WORK/$name.rk"
    local want i_out n_out i_ok n_ok i_phase n_phase attempt
    want="$(cat "$WORK/$name.expected")"

    if ! timeout 60 "$RASK" check "$src" >/dev/null 2>&1; then
        echo "X check" > "$WORK/$name.verdict"
        return
    fi

    i_ok=0
    if i_out="$(timeout 60 "$RASK" run --interp "$src" 2>/dev/null)"; then
        [ "$i_out" = "$want" ] && i_ok=1
        i_phase="output"
    else
        i_phase="run"
    fi

    # Native, several times over. A miscompile that reads a slot codegen never
    # wrote depends on what was already on the stack, so it isn't reliably
    # wrong — the enum in #1235 prints the right answer about one run in forty.
    # One sample would make this gate go red on somebody else's lucky run.
    # RASK_POISON_STACK fills the stack first, which pushes most of these to
    # always-wrong; the repeats cover the rest.
    n_ok=0
    if timeout 120 "$RASK" compile "$src" -o "$WORK/$name.bin" >/dev/null 2>&1; then
        n_phase="output"
        n_ok=1
        for attempt in $(seq 1 "${MATRIX_RUNS:-5}"); do
            if n_out="$(RASK_POISON_STACK=1 timeout 60 "$WORK/$name.bin" 2>/dev/null)"; then
                [ "$n_out" = "$want" ] || { n_ok=0; break; }
            else
                n_ok=0
                n_phase="run"
                break
            fi
        done
    else
        n_phase="compile"
    fi

    # A cell that fails on both is filed under native's phase: the check that
    # would have been shared already returned above, so what's left is native
    # getting further or not, and that is the more useful half to read.
    if   [ $i_ok = 1 ] && [ $n_ok = 1 ]; then echo ". -"        > "$WORK/$name.verdict"
    elif [ $i_ok = 1 ];                  then echo "N $n_phase" > "$WORK/$name.verdict"
    elif [ $n_ok = 1 ];                  then echo "I $i_phase" > "$WORK/$name.verdict"
    else                                      echo "X $n_phase" > "$WORK/$name.verdict"
    fi
}

# The linker compiles the C runtime itself and caches the objects in a
# directory every worker shares. The first compile is what fills it, so it runs
# alone — four of them racing to write the same object is not a thing to
# discover from a flaky gate.
matrix_cell "${CELLS[0]}"
fan_out matrix_cell "${CELLS[@]:1}"

# ── What the registry claims ─────────────────────────────────────
# First field of each non-comment line; the rest is the note.
reg_names() { [ -f "$KNOWN_RED" ] && awk 'NF && $1 !~ /^#/ {print $1}' "$KNOWN_RED"; }
reg_note()  { [ -f "$KNOWN_RED" ] && awk -v n="$1" 'NF && $1 == n {$1=""; print; exit}' "$KNOWN_RED"; }

registered() { reg_names | grep -qxF "$1"; }

# `(native run)` / `(both check)` / `(interp output)` out of a note. Prints
# `<backend> <phase>`, or nothing when the line makes no claim.
reg_claim() {
    reg_note "$1" | sed -n 's/.*(\(native\|interp\|both\) \(check\|compile\|run\|output\)).*/\1 \2/p'
}

# The backend a verdict letter blames, in the note's vocabulary.
claimed_backend() {
    case "$1" in
        N) echo native ;;
        I) echo interp ;;
        X) echo both ;;
    esac
}

declare -A V
for name in "${CELLS[@]}"; do V["$name"]="$(cat "$WORK/$name.verdict")"; done

# ── The grid ─────────────────────────────────────────────────────
declare -a CARRIERS=() PAYLOADS=()
mapfile -t SKIP_LINES < <(python3 "$ROOT/tests/matrix/gen.py" --list-skips "${GEN_ARGS[@]}")
declare -A SKIP
for line in "${SKIP_LINES[@]}"; do SKIP["${line%% *}"]="${line#* }"; done

for name in "${CELLS[@]}" ${SKIP_LINES[@]+"${!SKIP[@]}"}; do
    c="${name%%__*}"; t="${name##*__}"
    [[ " ${CARRIERS[*]-} " == *" $c "* ]] || CARRIERS+=("$c")
    [[ " ${PAYLOADS[*]-} " == *" $t "* ]] || PAYLOADS+=("$t")
done

printf '%-16s' ""
for t in "${PAYLOADS[@]}"; do printf '%-9s' "$t"; done
echo
for c in "${CARRIERS[@]}"; do
    printf '%-16s' "$c"
    for t in "${PAYLOADS[@]}"; do
        if [ -n "${SKIP[${c}__${t}]:-}" ]; then printf '%-9s' "-"
        else printf '%-9s' "${V[${c}__${t}]:0:1}"; fi
    done
    echo
done
echo "legend: . both ok   N native bug (interp ok)   I interp bug   X both fail   - not legal Rask"
echo

# ── The verdict ──────────────────────────────────────────────────
clean=0
new_red=()
misfiled=()
unexpected_pass=()

for name in "${CELLS[@]}"; do
    read -r letter phase <<< "${V[$name]}"
    if [ "$letter" = "." ]; then
        clean=$((clean + 1))
        registered "$name" && unexpected_pass+=("$name")
        continue
    fi
    if ! registered "$name"; then
        new_red+=("$name $letter $phase")
        continue
    fi
    claim="$(reg_claim "$name")"
    want="$(claimed_backend "$letter") $phase"
    [ "$claim" = "$want" ] || misfiled+=("$name: line claims '${claim:-nothing}', run says '$want'")
done

# Only the full matrix can tell a stale registry line from one this run
# didn't reach — under --types/--carriers every other line is out of scope.
if [ "${#GEN_ARGS[@]}" -eq 0 ]; then
    for entry in $(reg_names); do
        printf '%s\n' "${CELLS[@]}" | grep -qxF "$entry" || \
            misfiled+=("$entry: registered but not a cell — renamed or skipped?")
    done
fi

if [ "${#new_red[@]}" -gt 0 ]; then
    echo "NEW RED — not in tests/matrix/known_red.txt:"
    for r in "${new_red[@]}"; do
        read -r name letter phase <<< "$r"
        echo "  $name  ($(claimed_backend "$letter") $phase)"
    done
    echo
fi
if [ "${#unexpected_pass[@]}" -gt 0 ]; then
    echo "UNEXPECTED PASS — prune these from tests/matrix/known_red.txt:"
    printf '  %s\n' "${unexpected_pass[@]}"
    echo
fi
if [ "${#misfiled[@]}" -gt 0 ]; then
    echo "MISFILED — the line no longer describes what the cell does:"
    printf '  %s\n' "${misfiled[@]}"
    echo
fi

known=$(( ${#CELLS[@]} - clean ))
echo "matrix: $clean/${#CELLS[@]} cells clean, $known red (${#new_red[@]} new), ${#SKIP_LINES[@]} pairs skipped"

if [ $KEEP = 1 ]; then
    echo "sources kept in $WORK"
else
    rm -rf "$WORK"
fi

[ "${#new_red[@]}" -eq 0 ] && [ "${#unexpected_pass[@]}" -eq 0 ] && [ "${#misfiled[@]}" -eq 0 ]

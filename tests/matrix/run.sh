#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Type x carrier matrix runner.
#
# Generates one small program per (payload type, carrier) pair, runs every one
# on BOTH backends, and prints a grid of what survived. Interp is the reference,
# so read a row as "this payload type breaks in these carriers".
#
# Legend per cell:
#   .  both backends print the expected value
#   N  native wrong/failed, interp right  — a native codegen bug
#   I  interp wrong/failed, native right
#   X  both wrong (usually unimplemented, not a miscompile)
#
# Usage:  tests/matrix/run.sh [--types a,b] [--carriers x,y] [--keep]
# Exit:   0 always — this is a survey tool, not a gate. Use tests/differential.sh
#         for the gate; promote a cell into tests/suite/ once it's fixed.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${MATRIX_WORK:-${TMPDIR:-/tmp}/rask-matrix-$$}"
KEEP=0
GEN_ARGS=()

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
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

mkdir -p "$WORK"
python3 "$ROOT/tests/matrix/gen.py" "$WORK" "${GEN_ARGS[@]}" >/dev/null || exit 2

# Cell names, in generation order, so rows/columns stay stable run to run.
mapfile -t CELLS < <(python3 "$ROOT/tests/matrix/gen.py" --list "${GEN_ARGS[@]}")

# Verdict for one cell, printed as `carrier type verdict`.
verdict_of() {
    local name="$1"
    local src="$WORK/$name.rk"
    local want
    want="$(cat "$WORK/$name.expected")"

    local i_out n_out
    i_out="$(timeout 60 "$RASK" run --interp "$src" 2>/dev/null)"
    n_out="$(timeout 120 "$RASK" run "$src" 2>/dev/null)"

    local i_ok=0 n_ok=0
    [ "$i_out" = "$want" ] && i_ok=1
    [ "$n_out" = "$want" ] && n_ok=1

    if   [ $i_ok = 1 ] && [ $n_ok = 1 ]; then echo "."
    elif [ $i_ok = 1 ];                  then echo "N"
    elif [ $n_ok = 1 ];                  then echo "I"
    else                                      echo "X"
    fi
}

declare -A V
bad=0
for name in "${CELLS[@]}"; do
    v="$(verdict_of "$name")"
    V["$name"]="$v"
    [ "$v" = "." ] || bad=$((bad + 1))
    printf '%s' "$v"
done
echo

# Rebuild the axes from the cell names to honour --types/--carriers.
declare -a CARRIERS=() PAYLOADS=()
for name in "${CELLS[@]}"; do
    c="${name%%__*}"; t="${name##*__}"
    [[ " ${CARRIERS[*]-} " == *" $c "* ]] || CARRIERS+=("$c")
    [[ " ${PAYLOADS[*]-} " == *" $t "* ]] || PAYLOADS+=("$t")
done

echo
printf '%-16s' ""
for t in "${PAYLOADS[@]}"; do printf '%-8s' "$t"; done
echo
for c in "${CARRIERS[@]}"; do
    printf '%-16s' "$c"
    for t in "${PAYLOADS[@]}"; do printf '%-8s' "${V[${c}__${t}]:-?}"; done
    echo
done
echo
echo "legend: . both ok   N native bug (interp ok)   I interp bug   X both fail"
echo "matrix: $((${#CELLS[@]} - bad))/${#CELLS[@]} cells clean on both backends"

if [ $KEEP = 1 ]; then
    echo "sources kept in $WORK"
else
    rm -rf "$WORK"
fi

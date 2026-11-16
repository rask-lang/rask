#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Unshaped-call gate.
#
# MIR mints names for operations the stdlib declares under another spelling,
# or not at all — `v[i]` is `Vec_index`, a `with` block on a `Shared` is one
# of several `acquire`s, a free is `Vec_free`. What such a call does with its
# arguments and its result (who owns what crosses it) is stated where lowering
# emits the call: `FunctionRef::like`, `borrowing`, `releasing`, `reading`.
# A stdlib method's is read from its declaration in stdlib/*.rk.
#
# A call to something in a stdlib family that states nothing is treated as
# reading everything and keeping nothing. This gate is what stops it staying
# that way: every reported name has to be given a shape where it is emitted.
#
# Why a gate and not a unit test: the set of names isn't statically
# enumerable. Lowering builds most of them from the receiver's type at the call
# site, so the only way to know which ones a real program reaches is to compile
# real programs. That also means `rask compile` alone isn't enough — `rask
# test` lowers test blocks and reaches names it doesn't, which is exactly how
# a first pass at this missed twenty of them.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
source "$ROOT/tests/lib/fanout.sh"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

# Every file is an independent compile, so they fan out across cores. Each
# worker writes its own findings file rather than appending to a shared one —
# concurrent appends of a long line can tear. The names are sorted afterwards,
# so the order the workers finish in doesn't reach the report.
JOBS="${SPELL_JOBS:-${JOBS:-$(nproc 2>/dev/null || echo 4)}}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export RASK WORK
export -f slot

# `test` for anything with test blocks, `compile` for the rest: between them
# they cover both lowering paths.
spell_test() {
  timeout 60 "$RASK" test "$1" 2>&1 >/dev/null |
    grep '^\[unshaped-call\]' > "$WORK/$(slot "$1").found"
  return 0
}
spell_compile() {
  timeout 60 "$RASK" compile "$1" 2>&1 >/dev/null |
    grep '^\[unshaped-call\]' > "$WORK/$(slot "$1").found"
  return 0
}

fan_out spell_test "$ROOT"/tests/suite/*.rk
fan_out spell_compile "$ROOT"/examples/*.rk "$ROOT"/specs/analysis/prototype/*.rk

names="$(cat "$WORK"/*.found 2>/dev/null |
  sed -E 's/^\[unshaped-call\] ([A-Za-z_0-9]+).*/\1/' | sort -u)"

echo "──────────────────────────────────────────────────"
if [ -z "$names" ]; then
  echo "unshaped-call gate: every call in a stdlib family says what it does"
  exit 0
fi

echo "These calls reach MIR without saying what they do with their arguments and"
echo "result, so the ownership passes treat them as reading everything and keeping"
echo "nothing:"
echo
echo "$names" | sed 's/^/  /'
echo
echo "State each one where lowering emits it (rask-mir/src/operand.rs):"
echo "  FunctionRef::like(name, \"Type_method\")  the same operation as that declared method"
echo "  FunctionRef::borrowing(name)               borrows its receiver, keeps nothing, returns something new"
echo "  FunctionRef::releasing(name)               takes the receiver and it is gone"
echo "  FunctionRef::reading(name)                 no receiver, only reads its arguments"
exit 1

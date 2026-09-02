#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Internal-spelling gate.
#
# MIR mints names for operations the stdlib declares under another spelling —
# `v[i]` is `Vec_index`, a `with` block on a `Shared` is one of several
# `acquire`s. `INTERNAL_SPELLINGS` in rask-stdlib/src/mir_metadata.rs says what
# each one stands for, and that answers who owns what crosses the call.
#
# A name with no line there is treated as owning everything it touches, which
# leaks rather than double-frees — the safe direction, but still wrong. This
# gate is what stops it staying that way: every reported name has to get a
# line.
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

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

found="$(mktemp)"
trap 'rm -f "$found"' EXIT

# `test` for anything with test blocks, `compile` for the rest: between them
# they cover both lowering paths.
for file in "$ROOT"/tests/suite/*.rk; do
  timeout 60 "$RASK" test "$file" 2>&1 >/dev/null | grep '^\[unmapped-spelling\]' >>"$found"
done
for file in "$ROOT"/examples/*.rk "$ROOT"/specs/analysis/prototype/*.rk; do
  timeout 60 "$RASK" compile "$file" 2>&1 >/dev/null | grep '^\[unmapped-spelling\]' >>"$found"
done

names="$(sed -E 's/^\[unmapped-spelling\] ([A-Za-z_0-9]+).*/\1/' "$found" | sort -u)"

echo "──────────────────────────────────────────────────"
if [ -z "$names" ]; then
  echo "spellings gate: every internal name is accounted for"
  exit 0
fi

echo "These names reach MIR with nothing saying what they stand for, so they are"
echo "being treated as owning everything they touch — which leaks:"
echo
echo "$names" | sed 's/^/  /'
echo
echo "Give each one a line in INTERNAL_SPELLINGS (rask-stdlib/src/mir_metadata.rs):"
echo "  SameAs(\"<Type>_<method>\")  the same operation as that declared method"
echo "  FreshFromReceiver          borrows its receiver, keeps nothing, returns something new"
echo "  ConsumesReceiver           takes the receiver and it is gone"
echo "  NoReceiver                 a static: nothing to borrow"
exit 1

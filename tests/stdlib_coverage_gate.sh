#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Stdlib coverage gate: does the stdlib match its own spec?
#
# Each specs/stdlib/*.md states what it promises in `<!-- surface -->` blocks.
# `rask stdlib-coverage` holds every promised function to four things: declared
# in stdlib/*.rk, public, with the spec's signature, and called by a suite
# program that is green on both backends. Anything short of that is a gap.
#
# v0.6 ships when there are none. Until then the open gaps are registered in
# tests/known_stdlib_gaps.txt, one `<spec-id> <item> <GAP>` per line, and
# this gate fails on a gap that isn't there — a spec gaining a promise, or the
# stdlib losing a function. A registered gap that has closed is reported so
# its line gets deleted; it fails too, so the registry can't go stale.
#
# A gap closes by implementing the function or by the spec dropping it. Both
# count.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
KNOWN="$ROOT/tests/known_stdlib_gaps.txt"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 2
fi

cd "$ROOT"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if ! "$RASK" stdlib-coverage --gaps > "$WORK/now" 2> "$WORK/err"; then
  cat "$WORK/err" >&2
  exit 2
fi
summary="$(sed -n 's/^# //p' "$WORK/now")"
grep -v '^#' "$WORK/now" | sort -u > "$WORK/gaps"
sed 's/[[:space:]]*#.*$//' "$KNOWN" | grep -v '^$' | sort -u > "$WORK/known"

new="$(comm -23 "$WORK/gaps" "$WORK/known")"
closed="$(comm -13 "$WORK/gaps" "$WORK/known")"

echo "$summary"
echo "──────────────────────────────────────────────────"
status=0
if [ -n "$new" ]; then
  echo "New gaps, not in tests/known_stdlib_gaps.txt:"
  echo "$new" | sed 's/^/  /'
  echo "Close each one, or if it's known work, register it. \`rask stdlib-coverage\` shows why."
  status=1
fi
if [ -n "$closed" ]; then
  echo "Registered gaps that have closed (delete these lines):"
  echo "$closed" | sed 's/^/  /'
  status=1
fi
if [ "$status" -eq 0 ]; then
  echo "stdlib coverage gate: $(wc -l < "$WORK/known") registered gaps, none new"
fi
exit "$status"

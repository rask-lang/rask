#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Memory leak gate.
#
# "Rask is leak free" is a claim about the language, and until this existed
# nothing checked it — every heap string in every program leaked for as long as
# the compiler has had strings (#1024), and nothing went red. A leak has no
# symptom: the answers are all correct, there's just no memory coming back.
#
# `RASK_LEAK_CHECK=1` makes the runtime count live heap string buffers and, at
# the end of `main`, report any it still holds and exit 97. This gate runs every
# suite file under it and fails on the first one that leaks.
#
# Files that are still expected to leak go in tests/known_leaks.txt with the
# issue that tracks them. The gate holds them to it: one that stops leaking is
# reported so the line can be deleted, the same way the differential harness
# treats a known divergence.
#
# What it does not catch yet: a leaked `Vec` handle or data array, a leaked
# closure, a leaked trait object. The counter is strings only, because strings
# are what refcounting is for — the rest are single-owner and freed by their own
# drop passes. Widening it is worth doing.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SUITE="$ROOT/tests/suite"
KNOWN="$ROOT/tests/known_leaks.txt"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

known_leak() {
  [ -f "$KNOWN" ] || return 1
  grep -qE "^$1([[:space:]]|#|$)" "$KNOWN"
}

green=0
leaked=0
expected=0
fixed=()
failures=()

for file in "$SUITE"/*.rk; do
  name="$(basename "$file")"
  out="$(RASK_LEAK_CHECK=1 timeout 120 "$RASK" test "$file" 2>&1)"
  if echo "$out" | grep -q "never released"; then
    detail="$(echo "$out" | grep 'never released' | head -1)"
    if known_leak "$name"; then
      expected=$((expected + 1))
    else
      leaked=$((leaked + 1))
      failures+=("$name — $detail")
    fi
  else
    green=$((green + 1))
    if known_leak "$name"; then
      fixed+=("$name")
    fi
  fi
done

echo "──────────────────────────────────────────────────"
for f in "${failures[@]:-}"; do
  [ -n "$f" ] && echo "LEAK: $f"
done
for f in "${fixed[@]:-}"; do
  [ -n "$f" ] && echo "NO LONGER LEAKS (delete its line from known_leaks.txt): $f"
done
echo "──────────────────────────────────────────────────"
echo "leak gate: $green clean, $expected known-leaking, $leaked new"

if [ "$leaked" -gt 0 ] || [ "${#fixed[@]}" -gt 0 ]; then
  exit 1
fi
exit 0

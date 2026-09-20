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
# `RASK_LEAK_CHECK=1` makes the runtime report anything it allocated and never
# gave back, at the end of `main`, and exit 97. This gate runs every suite file
# under it and fails on any that leak.
#
# Files that are still expected to leak go in tests/known_leaks.txt with the
# issue that tracks them. The gate holds them to it: one that stops leaking is
# reported so the line can be deleted, the same way the differential harness
# treats a known divergence.
#
# A line whose count reads "N allocations, deferred" is one this milestone is
# not going to close — a task killed by a panic doesn't unwind its captures, and
# that waits on the unwinder; `t_shared_freed.rk` is a box held on purpose
# and will never be zero. They are still measured and still held to their count;
# they just don't count toward the number a memory milestone is judged on, which
# is the one the summary calls "this milestone".
#
# The count is every `rask_alloc` the runtime made — a `Vec` handle, a data
# array, a closure box, a trait object, a string buffer. A clean program ends at
# exactly zero, which is what makes this a gate rather than a threshold: the
# runtime itself holds nothing at exit.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SUITE="$ROOT/tests/suite"
KNOWN="$ROOT/tests/known_leaks.txt"
source "$ROOT/tests/lib/fanout.sh"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

known_leak() {
  [ -f "$KNOWN" ] || return 1
  grep -qE "^$1([[:space:]]|#|$)" "$KNOWN"
}

# The count a ledger line claims, or empty if it doesn't say.
#
# Checked against the measured one below, because a line's number is the only
# part of this file nothing verified. A fix that took a file from 22 allocations
# to 3 left "# 22 allocations" sitting there reading like a measurement, and the
# running total in the prose drifted 22 out from the sum of the lines — which is
# how a ledger stops being evidence and becomes a story about the past.
declared_count() {
  [ -f "$KNOWN" ] || return 0
  grep -E "^$1([[:space:]]|#|$)" "$KNOWN" | head -1 |
    sed -nE 's/.*#[[:space:]]*([0-9]+)[[:space:]]+allocation.*/\1/p'
}

# Is this line's leak one a memory milestone is not judged on?
deferred_leak() {
  [ -f "$KNOWN" ] || return 1
  grep -E "^$1([[:space:]]|#|$)" "$KNOWN" | head -1 |
    grep -qE "allocations?, deferred"
}

green=0
leaked=0
expected=0
broken=0
total=0
deferred=0
stale=()
fixed=()
failures=()
unran=()

# `rask test` exits 97 when the leak checker found something, 1 when a test
# failed, 0 when neither. The verdict is that code, not the wording of the
# report: this gate used to grep the output for "never released" and read every
# file as clean, because `rask test` was capturing the binary's stderr and
# dropping it, so the line never arrived (#1048). A gate that depends on prose
# reaching it is a gate that can go quiet without anyone noticing.
LEAK_EXIT=97

# Each file is one `rask test` and nothing else, so the runs fan out across
# cores and only the ledger check below stays sequential. JOBS defaults to one
# per core; LEAK_JOBS overrides it.
JOBS="${LEAK_JOBS:-${JOBS:-$(nproc 2>/dev/null || echo 4)}}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Two lines per file: the exit code, then the report line if there was one.
measure_one() {
  local file="$1" name out rc detail
  name="$(basename "$file")"
  out="$(RASK_LEAK_CHECK=1 timeout 120 "$RASK" test "$file" 2>&1)"; rc=$?
  detail="$(printf '%s\n' "$out" | grep 'never released' | head -1)"
  printf '%s\n%s\n' "$rc" "$detail" > "$WORK/$name.result"
}
export RASK WORK

fan_out measure_one "$SUITE"/*.rk

for file in "$SUITE"/*.rk; do
  name="$(basename "$file")"
  if [ ! -f "$WORK/$name.result" ]; then
    broken=$((broken + 1))
    unran+=("$name (worker produced no result)")
    continue
  fi
  rc="$(sed -n 1p "$WORK/$name.result")"
  detail="$(sed -n 2p "$WORK/$name.result")"
  if ! [[ "$rc" =~ ^[0-9]+$ ]]; then
    broken=$((broken + 1))
    unran+=("$name (worker left no exit code)")
    continue
  fi
  if [ "$rc" -eq "$LEAK_EXIT" ]; then
    [ -n "$detail" ] || detail="exit $rc"
    if known_leak "$name"; then
      expected=$((expected + 1))
      measured="$(echo "$detail" | sed -nE 's/^rask: ([0-9]+) allocation.*/\1/p')"
      declared="$(declared_count "$name")"
      if deferred_leak "$name"; then
        deferred=$((deferred + ${measured:-0}))
      else
        total=$((total + ${measured:-0}))
      fi
      if [ -n "$measured" ] && [ -n "$declared" ] && [ "$measured" != "$declared" ]; then
        stale+=("$name — the line says $declared, it leaks $measured")
      fi
    else
      leaked=$((leaked + 1))
      failures+=("$name — $detail")
    fi
  elif [ "$rc" -ne 0 ]; then
    # Didn't leak, didn't pass. The differential harness owns test failures, so
    # this doesn't fail the gate — but a file that never ran is a file whose
    # leaks nobody measured, and counting it as clean is how a gate quietly
    # shrinks.
    broken=$((broken + 1))
    unran+=("$name (exit $rc)")
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
for f in "${stale[@]:-}"; do
  [ -n "$f" ] && echo "STALE COUNT (update its line in known_leaks.txt): $f"
done
for f in "${unran[@]:-}"; do
  [ -n "$f" ] && echo "NOT MEASURED (failed before the leak check): $f"
done
echo "──────────────────────────────────────────────────"
echo "leak gate: $green clean, $expected known-leaking ($total allocations this milestone, $deferred deferred), $leaked new, $broken not measured"

if [ "$leaked" -gt 0 ] || [ "${#fixed[@]}" -gt 0 ] || [ "${#stale[@]}" -gt 0 ]; then
  exit 1
fi
exit 0

#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Stress gate: every concurrency suite file under sim, over many seeds.
#
# A concurrent test that passes once says one interleaving worked. Sim picks
# which task runs next from a seed, so N seeds are N different interleavings,
# each replayable from the line the runner prints on a failure. A deadlock is
# reported as one, not as a hang, and a lost panic fails the test that lost it.
# This gate is v0.5's first number: across these files and seeds it finds
# nothing.
#
# The file list is found, not kept, the same as the TSan gate's: anything that
# touches a task, channel, lock or atomic.
#
# Some tests are outside what sim runs by design: `Thread.spawn` is refused
# (sim/B1), a subprocess has no simulated implementation (sim/B3), and a test
# that relies on state carried over from an earlier test can't hold when every
# test starts fresh (sim/I7). Those go in tests/known_sim.txt, one per line with
# the reason. An entry that starts passing is flagged so the line goes, the same
# way known_tsan.txt works.
#
#   SIM_SEEDS=200 tests/sim_gate.sh     # a deeper sweep than CI's

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SUITE="$ROOT/tests/suite"
KNOWN="$ROOT/tests/known_sim.txt"
SEEDS="${SIM_SEEDS:-20}"
source "$ROOT/tests/lib/fanout.sh"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

run_one() {
  f="$1"
  base="$(basename "$f")"
  # A hang under sim is a bug in sim: a stuck program is reported as a
  # deadlock, and a busy one runs out of steps.
  timeout 600 "$RASK" test --sim --seeds "$SEEDS" "$f" > "$WORK/$base.out" 2>&1
  echo "$?" > "$WORK/$base.code"
}
export -f run_one
export RASK WORK SEEDS

# `file.rk :: test name` for every exemption, reasons stripped.
known_keys() {
  [ -f "$KNOWN" ] || return 0
  grep -v '^\s*#' "$KNOWN" | grep ' :: ' | awk -F ' :: ' '{ print $1 " :: " $2 }'
}

mapfile -t files < <(grep -l -E 'spawn|Multitasking|Channel|Shared|Atomic|ThreadPool|Thread\.' "$SUITE"/*.rk | sort)
fan_out run_one "${files[@]}"

mapfile -t known < <(known_keys)
is_known() {
  local key="$1" k
  for k in "${known[@]}"; do [ "$k" = "$key" ] && return 0; done
  return 1
}

clean=0
failures=()
broken=()
seen_known=()

for f in "${files[@]}"; do
  base="$(basename "$f")"
  out="$WORK/$base.out"
  code="$(cat "$WORK/$base.code")"
  mapfile -t failed < <(grep '^FAIL: ' "$out" | sed 's/^FAIL: //' | sort -u)

  if [ "$code" -ne 0 ] && [ "${#failed[@]}" -eq 0 ]; then
    # Didn't compile, crashed outside a test, or timed out.
    broken+=("$base")
    echo "BROKEN: $base (exit $code)"
    tail -5 "$out" | sed 's/^/  /'
    continue
  fi

  new=0
  for t in "${failed[@]}"; do
    if is_known "$base :: $t"; then
      seen_known+=("$base :: $t")
    else
      new=1
      failures+=("$base :: $t")
      echo "FAIL: $base :: $t"
      grep -A4 -F "FAIL: $t" "$out" | sed -n '2,5p' | sed 's/^/  /'
    fi
  done
  [ "$new" -eq 0 ] && clean=$((clean + 1))
done

# An exemption that no longer fails is a stale line.
stale=()
for k in "${known[@]}"; do
  found=0
  for s in "${seen_known[@]}"; do [ "$s" = "$k" ] && found=1 && break; done
  [ "$found" -eq 0 ] && stale+=("$k")
done

echo "──────────────────────────────────────────────────"
echo "sim: ${#files[@]} files × $SEEDS seeds, $clean clean, ${#failures[@]} failing tests, ${#broken[@]} broken files, ${#known[@]} exempt"

status=0
if [ "${#broken[@]}" -gt 0 ]; then
  echo "BROKEN under sim: ${broken[*]}"
  status=1
fi
if [ "${#stale[@]}" -gt 0 ]; then
  echo "NO LONGER FAILING (delete from tests/known_sim.txt):"
  printf '  %s\n' "${stale[@]}"
  status=1
fi
if [ "${#failures[@]}" -gt 0 ]; then
  echo "FAILING under sim (fix, or add to tests/known_sim.txt with the reason):"
  printf '  %s\n' "${failures[@]}"
  status=1
fi
exit $status

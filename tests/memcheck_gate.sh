#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Memory-error gate.
#
# The leak gate measures one direction of "who frees this": memory nobody gave
# back. That is the polite half of the mistake. The other half is giving it back
# too early, or reading a field nobody wrote, and it has no symptom at all until
# the day it has a very bad one. Nothing measured it.
#
# Everything found in that direction so far was found by accident: a segfault in
# an example, an abort in a stress run, a gdb session on a flake. #1161 was a
# release placed one statement too early. #1202 dereferenced 0x1. The unwind
# thunk in #882 handed the allocator a stack address. Each of those is the same
# confusion the leak gate catches, pointed the other way, and each took a crash
# to notice.
#
# valgrind's memcheck sees the whole class before it crashes anything: a read of
# an uninitialised field, a free of something already freed, a write past the
# end of a block. It found two files within a minute of first being pointed at
# the suite — a pool built field by field that had grown two fields nobody
# initialised, which the leak gate, twelve other gates, CI and a 30x poisoned
# stack run all called green (#1223's sibling).
#
# Leaks are deliberately *not* checked here. The leak gate owns that question
# and holds a ledger of what is still expected; running both would give two
# answers to one question and let them drift.
#
# Files that are expected to trip memcheck go in tests/known_memcheck.txt with
# the issue that tracks them, the same shape as tests/known_leaks.txt. A file
# that stops tripping is reported so the line can be deleted.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SUITE="$ROOT/tests/suite"
KNOWN="$ROOT/tests/known_memcheck.txt"
source "$ROOT/tests/lib/fanout.sh"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

if ! command -v valgrind > /dev/null 2>&1; then
  echo "error: valgrind not found — this gate needs it (apt install valgrind)" >&2
  exit 1
fi

known_bad() {
  [ -f "$KNOWN" ] || return 1
  grep -qE "^$1([[:space:]]|#|$)" "$KNOWN"
}

# The verdict is memcheck's own output. There is no --error-exitcode here, and
# that is deliberate rather than an omission.
#
# It only applies when memcheck gets to choose the exit code, and on the
# findings that matter most it doesn't: a program that reads freed memory and
# then dies of the resulting SIGSEGV exits 139, because the signal killed it
# before valgrind could substitute anything. The first cut checked for 42 and
# counted every one of those as clean — quiet on exactly the loudest half of
# what this measures. Leaving the flag in afterwards would be worse than not
# having it: a reader would take it for the mechanism.
#
# With --quiet, memcheck prints nothing unless it found something, so any
# `==pid==` line on its channel is the finding, whole. The exit code then says
# only how the program itself ended, which is a separate question with its own
# column below.

green=0
bad=0
expected=0
broken=0
died=0
fixed=()
failures=()
unran=()
crashed=()

JOBS="${MEMCHECK_JOBS:-${JOBS:-$(nproc 2>/dev/null || echo 4)}}"
WORK="$(mktemp -d)"
BIN="$(mktemp -d)"
trap 'rm -rf "$WORK" "$BIN"' EXIT

# Build the test binary, then run *that* under memcheck.
#
# Not `rask compile`: most suite files are `test` blocks with no `main`, so
# compiling them fails with "no `main` function to compile from" and 237 of 524
# files would sit in the not-measured column, which is a gate that measures the
# easy half. `RASK_KEEP_TEST_BIN` leaves the binary `rask test` built and prints
# where, which is the same binary the leak gate measures.
#
# Not valgrind on `rask test` itself either: that would instrument the compiler
# and only reach the program through --trace-children, at minutes per file.
#
# `--errors-for-leak-kinds=none --leak-check=no` is the point. This gate is
# about errors, not about memory still held at exit — the leak gate owns that
# question and has a ledger for it.
check_one() {
  local file="$1" name rc out bin
  name="$(basename "$file" .rk)"
  RASK_KEEP_TEST_BIN=1 timeout 180 "$RASK" test "$file" \
      > "$WORK/$name.build" 2>&1
  bin="$(sed -nE 's/.*test binary kept at (.*)$/\1/p' "$WORK/$name.build" | tail -1)"
  if [ -z "$bin" ] || [ ! -x "$bin" ]; then
    printf 'nobuild\n' > "$WORK/$name.result"
    return
  fi
  # A program killed by a signal makes the shell that ran it announce
  # "Segmentation fault", and that announcement comes from *this* shell's own
  # stderr, not the command's — redirecting the command, or wrapping it in a
  # subshell, doesn't move it. So the worker's stderr is parked for the length
  # of the run. A crash here is a finding to report in the summary, not a line
  # of noise in the middle of the gate's output.
  #
  # --track-origins turns "depends on uninitialised value(s)" into a report that
  # also names where the value came from, which is the difference between a
  # finding someone can act on and one they have to re-derive.
  exec 3>&2 2> /dev/null
  valgrind --leak-check=no --errors-for-leak-kinds=none \
           --track-origins=yes --quiet timeout 120 "$bin" \
           > "$WORK/$name.out" 2> "$WORK/$name.vg"
  rc=$?
  exec 2>&3 3>&-
  rm -f "$bin"
  out="$(grep -m1 -E '^==[0-9]+== ' "$WORK/$name.vg" | sed -E 's/^==[0-9]+== //')"
  printf '%s\n%s\n' "$rc" "$out" > "$WORK/$name.result"
}
export RASK WORK BIN

fan_out check_one "$SUITE"/*.rk

for file in "$SUITE"/*.rk; do
  name="$(basename "$file" .rk)"
  if [ ! -f "$WORK/$name.result" ]; then
    broken=$((broken + 1))
    unran+=("$name.rk (worker produced no result)")
    continue
  fi
  rc="$(sed -n 1p "$WORK/$name.result")"
  detail="$(sed -n 2p "$WORK/$name.result")"
  if [ "$rc" = "nobuild" ]; then
    broken=$((broken + 1))
    unran+=("$name.rk (no test binary was built)")
    continue
  fi
  if [ -n "$detail" ]; then
    if known_bad "$name.rk"; then
      expected=$((expected + 1))
    else
      bad=$((bad + 1))
      failures+=("$name.rk — $detail")
    fi
  elif [ "$rc" -ge 128 ]; then
    # Killed by a signal with memcheck silent, which is a different thing from
    # a test that failed: memcheck watched the whole run and had nothing to say,
    # yet the process died. A stack overflow, an abort, a raised signal — none
    # of them memory errors this tool sees. Named rather than folded into "not
    # measured", because "the instrument found nothing AND the program died" is
    # the one combination worth a second look.
    #
    # Still not a gate failure: the differential harness owns whether a program
    # runs. What this gate owes is not to call it clean.
    died=$((died + 1))
    crashed+=("$name.rk (killed by signal $((rc - 128)), memcheck silent)")
  elif [ "$rc" -ne 0 ]; then
    # Exited nonzero under its own steam: a failed assertion, a `todo()`. The
    # run stopped early, so the code after it went unmeasured.
    broken=$((broken + 1))
    unran+=("$name.rk (exit $rc, nothing from memcheck)")
  else
    green=$((green + 1))
    if known_bad "$name.rk"; then
      fixed+=("$name.rk")
    fi
  fi
done

echo "──────────────────────────────────────────────────"
for f in "${failures[@]}"; do echo "MEMCHECK: $f"; done
for f in "${fixed[@]}"; do echo "NO LONGER TRIPS MEMCHECK (delete its line from known_memcheck.txt): $f"; done
for f in "${crashed[@]}"; do echo "DIED WITH NOTHING FROM MEMCHECK: $f"; done
for f in "${unran[@]}"; do echo "NOT MEASURED: $f"; done
echo "──────────────────────────────────────────────────"
echo "memcheck gate: $green clean, $expected known-bad, $bad new," \
     "$died died with memcheck silent, $broken not measured"

[ "$bad" -eq 0 ] && [ "${#fixed[@]}" -eq 0 ] || exit 1
exit 0

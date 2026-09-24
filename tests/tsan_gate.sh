#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Data-race gate.
#
# The differential harness says a concurrent program printed the right answer.
# It can't say the answer was right for the right reason: a race that loses one
# write in a thousand prints the same thing on nearly every run, and the run
# that doesn't is the flake nobody can reproduce. #1218's double free showed up
# that way, once, in an example.
#
# ThreadSanitizer sees the race itself, not its symptom. It records which
# thread touched which word under which lock, and reports two unordered
# accesses the first time they happen, whether or not they happened to collide
# this run. The runtime is C and compiled by the linker from source, so the
# instrumentation reaches the scheduler, channels and locks as well as the
# program: `RASK_EXTRA_CFLAGS` goes into both the runtime objects and the link.
#
# It runs every suite file that touches a task, channel, lock or atomic. The
# list is found, not kept: a new concurrency test is covered the day it lands.
#
# Only TSan's own reports decide the verdict. Whether a file's tests pass is the
# differential harness's question, and a file registered red there still has
# to be race-free here.
#
# When the scheduler switches stacks (the v0.5 fiber work), TSan has to be told
# about every switch — `__tsan_create_fiber` / `__tsan_switch_to_fiber` — or it
# reads one thread running two stacks as a race on everything they share. That
# annotation lands with the switch, and this gate is what says it's right.
#
# Files expected to report go in tests/known_tsan.txt with the issue that
# tracks them. A file that stops reporting is flagged so the line goes.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SUITE="$ROOT/tests/suite"
KNOWN="$ROOT/tests/known_tsan.txt"
source "$ROOT/tests/lib/fanout.sh"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi

export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"
export RASK_EXTRA_CFLAGS="-fsanitize=thread -g"
# Keep going after the first report so one file lists all of its races.
export TSAN_OPTIONS="halt_on_error=0 second_deadlock_stack=1 ${TSAN_OPTIONS:-}"

known_bad() {
  [ -f "$KNOWN" ] || return 1
  grep -qE "^$1([[:space:]]|#|$)" "$KNOWN"
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

run_one() {
  f="$1"
  base="$(basename "$f")"
  # A hang is a finding too, and a different one: say so rather than wait.
  out="$(timeout 180 "$RASK" test "$f" 2>&1)"; code=$?
  printf '%s\n' "$out" | grep -E 'ThreadSanitizer' > "$WORK/$base.tsan"
  echo "$code" > "$WORK/$base.code"
}
export -f run_one
export RASK WORK

mapfile -t files < <(grep -l -E 'spawn|Multitasking|Channel|Shared|Atomic|ThreadPool|Thread\.' "$SUITE"/*.rk | sort)
fan_out run_one "${files[@]}"

clean=0
expected=0
failures=()
fixed=()
hung=()

for f in "${files[@]}"; do
  base="$(basename "$f")"
  code="$(cat "$WORK/$base.code")"
  if [ "$code" -eq 124 ]; then
    hung+=("$base")
  fi
  if [ -s "$WORK/$base.tsan" ]; then
    if known_bad "$base"; then
      expected=$((expected + 1))
    else
      failures+=("$base")
      echo "RACE: $base"
      sed 's/^/  /' "$WORK/$base.tsan" | head -20
    fi
  else
    if known_bad "$base"; then
      fixed+=("$base")
    fi
    clean=$((clean + 1))
  fi
done

echo "──────────────────────────────────────────────────"
echo "tsan: ${#files[@]} files, $clean clean, $expected expected, ${#failures[@]} reporting, ${#hung[@]} timed out"

status=0
if [ "${#hung[@]}" -gt 0 ]; then
  echo "TIMED OUT under TSan (180s): ${hung[*]}"
  status=1
fi
if [ "${#fixed[@]}" -gt 0 ]; then
  echo "NO LONGER REPORTING (delete from tests/known_tsan.txt): ${fixed[*]}"
  status=1
fi
if [ "${#failures[@]}" -gt 0 ]; then
  echo "UNTRACKED RACES (fix, or add to tests/known_tsan.txt with an issue): ${failures[*]}"
  status=1
fi
exit $status

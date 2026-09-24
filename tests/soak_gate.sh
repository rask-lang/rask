#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Thread-count soak.
#
# `using Multitasking(workers: n)` promises n workers. What it costs in OS
# threads is a separate question, and nothing measured it. Today a task blocked
# in `join` gets a replacement thread for as long as it waits, a task blocked
# in `receive` keeps its worker and starves the rest (#1353), and a program
# shaped like divide-and-conquer runs out of replacements and aborts. The
# fiber switch (ROADMAP.md, v0.5) is what makes a blocked task cost a stack
# instead of a thread, and this is how anyone can tell it does.
#
# Each program in tests/soak/ runs natively under a sampler that counts the
# process's threads (/proc/<pid>/task) every millisecond. A program passes when
#
#   - it exits 0 within 20s,
#   - its last line of output is its `// soak-expect:` line, and
#   - the most threads ever seen is at most `workers + 1` — the workers plus
#     the thread that opened the scope.
#
# Every program has exactly one `using Multitasking(workers: n)`; the gate reads
# n from the source. io_uring's kernel workers (`iou-*`) aren't the runtime's
# and aren't counted.
#
# The sampler can miss a peak that lasts under a millisecond, never invent one:
# a failure is an observation, a pass is "never caught over budget". The
# programs hold their blocked state for many milliseconds so the peak is seen.
#
# Files expected to fail go in tests/known_soak.txt with the issue that tracks
# them. A file that starts passing is flagged so its line goes.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SOAK="$ROOT/tests/soak"
KNOWN="$ROOT/tests/known_soak.txt"
source "$ROOT/tests/lib/fanout.sh"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 1
fi
if [ ! -d /proc/self/task ]; then
  echo "error: this gate counts threads through /proc and needs Linux" >&2
  exit 1
fi

export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

known_bad() {
  [ -f "$KNOWN" ] || return 1
  grep -qE "^$1([[:space:]]|#|$)" "$KNOWN"
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Runs argv, prints "<peak threads> <exit code>" and leaves stdout in $2.
SAMPLER='
import os, subprocess, sys, time
out = open(sys.argv[1], "w")
p = subprocess.Popen(sys.argv[2:], stdout=out, stderr=subprocess.STDOUT)
peak, start = 0, time.monotonic()
while p.poll() is None:
    if time.monotonic() - start > 20:
        p.kill(); p.wait(); print(peak, 124); sys.exit(0)
    try:
        n = 0
        for t in os.listdir(f"/proc/{p.pid}/task"):
            try:
                if open(f"/proc/{p.pid}/task/{t}/comm").read().startswith("iou-"):
                    continue
            except OSError:
                continue
            n += 1
        peak = max(peak, n)
    except OSError:
        pass
    time.sleep(0.001)
print(peak, p.returncode)
'
export SAMPLER

run_one() {
  f="$1"
  base="$(basename "$f" .rk)"
  bin="$WORK/$base.bin"
  if ! "$RASK" compile "$f" -o "$bin" > "$WORK/$base.compile" 2>&1; then
    echo "compile" > "$WORK/$base.result"
    return
  fi
  python3 -c "$SAMPLER" "$WORK/$base.out" "$bin" > "$WORK/$base.result"
}
export -f run_one
export RASK WORK

mapfile -t files < <(ls "$SOAK"/*.rk | sort)
fan_out run_one "${files[@]}"

pass=0
expected=0
failures=()
fixed=()

for f in "${files[@]}"; do
  base="$(basename "$f" .rk)"
  name="$base.rk"
  workers="$(grep -oE 'workers: *[0-9]+' "$f" | grep -oE '[0-9]+')"
  want="$(grep -m1 -oE '// soak-expect: .*' "$f" | sed 's|// soak-expect: ||')"
  read -r peak code < "$WORK/$base.result"
  why=""
  if [ "$(echo "$workers" | wc -l)" -ne 1 ] || [ -z "$workers" ] || [ -z "$want" ]; then
    why="needs one \`workers: n\` and a \`// soak-expect:\` line"
  elif [ "$peak" = "compile" ]; then
    why="does not compile: $(head -1 "$WORK/$base.compile")"
  else
    got="$(tail -n 1 "$WORK/$base.out" 2>/dev/null)"
    budget=$((workers + 1))
    if [ "$code" -eq 124 ]; then
      why="hung (killed after 20s), $peak threads"
    elif [ "$code" -ne 0 ]; then
      why="exit $code, $peak threads: $(tail -n 2 "$WORK/$base.out" | tr "\n" " " | head -c 200)"
    elif [ "$got" != "$want" ]; then
      why="printed '$got', expected '$want'"
    elif [ "$peak" -gt "$budget" ]; then
      why="$peak threads for workers: $workers (budget $budget)"
    fi
  fi

  if [ -z "$why" ]; then
    pass=$((pass + 1))
    if known_bad "$name"; then
      fixed+=("$name")
    fi
    echo "ok       $name  ($peak threads, workers: $workers)"
  elif known_bad "$name"; then
    expected=$((expected + 1))
    echo "expected $name  — $why"
  else
    failures+=("$name")
    echo "FAIL     $name  — $why"
  fi
done

echo "──────────────────────────────────────────────────"
echo "soak: ${#files[@]} programs, $pass within budget, $expected expected over, ${#failures[@]} failing"

status=0
if [ "${#fixed[@]}" -gt 0 ]; then
  echo "NOW PASSING (delete from tests/known_soak.txt): ${fixed[*]}"
  status=1
fi
if [ "${#failures[@]}" -gt 0 ]; then
  echo "UNTRACKED FAILURES (fix, or add to tests/known_soak.txt with an issue): ${failures[*]}"
  status=1
fi
exit $status

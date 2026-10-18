#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# The sim heap (compiler/runtime/sim_libc.c), on its own.
#
# Every sim run allocates through it, Rask's own allocations included, so a
# bug in it breaks every sim test at once. tests/sim_heap/heap_check.c drives
# it with a random workload checked against a model, plus the edges: requests
# too big to fit, a double free, a wild free. Each check was shown to fail by
# putting back the bug it covers.
#
# Built with gcc and clang at -O0 and -O2. -fno-builtin, because clang at -O2
# otherwise deletes an unused malloc and the call never reaches the allocator.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/tests/sim_heap/heap_check.c"
HEAP="$ROOT/compiler/runtime/sim_libc.c"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

failed=0
ran=0
for cc in gcc clang; do
  command -v "$cc" > /dev/null 2>&1 || continue
  for opt in -O0 -O2; do
    bin="$WORK/heap_check_${cc}_${opt#-}"
    if ! "$cc" "$opt" -fno-builtin -std=c11 -Wall -Wextra -Wno-use-after-free \
          -Wno-free-nonheap-object -Wno-unknown-warning-option -D_GNU_SOURCE -DRASK_SIM \
          -I"$ROOT/compiler/runtime" "$SRC" "$HEAP" -o "$bin" > "$WORK/build.log" 2>&1; then
      echo "FAIL  $cc $opt: build"
      sed 's/^/      /' "$WORK/build.log"
      failed=$((failed + 1))
      continue
    fi
    out="$(timeout 120 "$bin" 2>&1)"; rc=$?
    ran=$((ran + 1))
    if [ "$rc" -eq 0 ] && [ "$out" = "heap_check: ok" ]; then
      echo "ok    $cc $opt"
    else
      echo "FAIL  $cc $opt (exit $rc)"
      printf '%s\n' "$out" | sed 's/^/      /'
      failed=$((failed + 1))
    fi
  done
done

echo "──────────────────────────────────────────────────"
echo "sim heap gate: $ran run, $failed failed"
[ "$failed" -eq 0 ] && [ "$ran" -gt 0 ]

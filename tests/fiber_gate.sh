#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# The fiber context switch, on every architecture it's written for.
#
# tests/fiber/switch_check.c drives compiler/runtime/fiber.c directly: two
# fibers each pin their own values in every callee-saved register across a
# switch, a rounding mode set on one must not leak into the other, plus deep
# stacks, 100k switches and 5000 short fibers. It is built for the host at -O0
# and -O2 with gcc and clang, and for aarch64 with a cross compiler under
# qemu-user, because no CI machine runs aarch64 Linux and the macOS job is
# aarch64 but runs no fibers. Each check was shown to fail by deleting the
# save of the register it covers.
#
# aarch64 needs `aarch64-linux-gnu-gcc` and `qemu-aarch64` (apt: gcc-aarch64-
# linux-gnu libc6-dev-arm64-cross qemu-user). Without them the gate says it
# skipped aarch64; set FIBER_GATE_REQUIRE_AARCH64=1 to make that a failure,
# which CI does.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/tests/fiber/switch_check.c"
FIBER="$ROOT/compiler/runtime/fiber.c"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

failed=0
ran=0

run_one() {
  local label="$1" cc="$2" opt="$3" runner="$4" extra="$5"
  local bin="$WORK/switch_check_${label}_${opt#-}"
  # shellcheck disable=SC2086
  if ! "$cc" $opt -Wall -Wextra -std=c11 -D_GNU_SOURCE $extra "$SRC" "$FIBER" \
        -o "$bin" -lpthread -lm > "$WORK/build.log" 2>&1; then
    echo "FAIL  $label $opt: build"
    sed 's/^/      /' "$WORK/build.log"
    failed=$((failed + 1))
    return
  fi
  local out rc
  # shellcheck disable=SC2086
  out="$(timeout 60 $runner "$bin" 2>&1)"; rc=$?
  ran=$((ran + 1))
  if [ "$rc" -eq 0 ] && [ "$out" = "switch_check: ok" ]; then
    echo "ok    $label $opt"
  else
    echo "FAIL  $label $opt (exit $rc)"
    printf '%s\n' "$out" | sed 's/^/      /'
    failed=$((failed + 1))
  fi
}

host="$(uname -m)"
for cc in gcc clang; do
  command -v "$cc" > /dev/null 2>&1 || continue
  for opt in -O0 -O2; do
    run_one "$host-$cc" "$cc" "$opt" "" ""
  done
done

if command -v aarch64-linux-gnu-gcc > /dev/null 2>&1 && command -v qemu-aarch64 > /dev/null 2>&1; then
  for opt in -O0 -O2; do
    run_one "aarch64-gcc" aarch64-linux-gnu-gcc "$opt" qemu-aarch64 "-static"
  done
elif [ "${FIBER_GATE_REQUIRE_AARCH64:-0}" = "1" ]; then
  echo "FAIL  aarch64: aarch64-linux-gnu-gcc or qemu-aarch64 missing"
  failed=$((failed + 1))
else
  echo "skip  aarch64: no aarch64-linux-gnu-gcc / qemu-aarch64"
fi

echo "──────────────────────────────────────────────────"
echo "fiber gate: $ran run, $failed failed"
[ "$failed" -eq 0 ]

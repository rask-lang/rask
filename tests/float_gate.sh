#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Float results are the same bits everywhere (determinism/D12).
#
# tests/fixtures/float_workload.rk runs an n-body step and a few thousand
# transcendental calls and prints one hash of the results' bit patterns. The
# hash is pinned below. It has to come out the same natively, on the
# interpreter, and on aarch64 under qemu: the transcendentals are CORE-MATH's
# correctly rounded ones (runtime/core-math), the runtime is built without
# multiply-add fusion, and the basic operations are IEEE's. A change that moves
# one bit anywhere in the workload fails here instead of in a lockstep game.
#
# If the hash changes on purpose (the workload itself changed), every leg
# changes together; update PINNED. If only one leg changes, that leg is wrong.
#
# aarch64 needs `aarch64-linux-gnu-gcc` and `qemu-aarch64` (apt:
# gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user). Without them that leg
# skips; FLOAT_GATE_REQUIRE_AARCH64=1 makes that a failure, as CI sets it.
#
# Usage:  tests/float_gate.sh
# Exit:   0 = every leg matches, 1 = a leg differs or failed, 2 = setup error.

set -uo pipefail

PINNED=4263211506641661835

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
FIX="$ROOT/tests/fixtures/float_workload.rk"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 2
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

status=0
check() {
  local leg="$1" got="$2"
  if [ "$got" = "$PINNED" ]; then
    echo "ok    $leg: $got"
  else
    echo "FAIL  $leg: got '$got', pinned $PINNED"
    status=1
  fi
}

check native "$("$RASK" run "$FIX" 2>&1)"
check interpreter "$("$RASK" run --interp "$FIX" 2>&1)"

if ! command -v aarch64-linux-gnu-gcc > /dev/null 2>&1 || ! command -v qemu-aarch64 > /dev/null 2>&1; then
  if [ "${FLOAT_GATE_REQUIRE_AARCH64:-0}" = 1 ]; then
    echo "FAIL  aarch64: aarch64-linux-gnu-gcc or qemu-aarch64 missing"
    exit 1
  fi
  echo "skip  aarch64: no aarch64-linux-gnu-gcc / qemu-aarch64"
  exit "$status"
fi

if ! RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}" \
    "$RASK" compile --release --target aarch64-linux "$FIX" -o "$WORK/prog" > "$WORK/build.log" 2>&1; then
  echo "FAIL  aarch64: float_workload.rk didn't build"
  sed 's/^/      /' "$WORK/build.log" | head -20
  exit 1
fi
check aarch64 "$(qemu-aarch64 -L /usr/aarch64-linux-gnu "$WORK/prog" 2>&1)"
exit "$status"

#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# C structs passed by value and returned by value, on aarch64 under qemu.
#
# Each calling convention cuts a struct argument up its own way, and the
# x86_64 test (`a_c_struct_goes_by_value` in rask-cli) only ever saw System V.
# Applied on aarch64 it passed `{float, float}` packed into one register where
# C reads two, so the sum came back NaN; `{double, long}` went half in a float
# register and the sum came back 1; and a 24-byte struct crashed the compiler,
# because AArch64 passes it by the address of a copy and Cranelift can't copy
# one onto the stack there. aarch64 is tier 1, and no CI machine runs it.
#
# Same fixture as the x86_64 test, cross-compiled and run under qemu-user.
# Needs `aarch64-linux-gnu-gcc` and `qemu-aarch64` (apt: gcc-aarch64-linux-gnu
# libc6-dev-arm64-cross qemu-user). Without them it skips;
# C_ABI_GATE_REQUIRE_AARCH64=1 makes that a failure, as CI sets it.
#
# Usage:  tests/c_abi_gate.sh
# Exit:   0 = matches, 1 = a wrong number or a failed build, 2 = setup error.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
FIX="$ROOT/compiler/crates/rask-cli/tests/fixtures"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 2
fi
if ! command -v aarch64-linux-gnu-gcc > /dev/null 2>&1 || ! command -v qemu-aarch64 > /dev/null 2>&1; then
  if [ "${C_ABI_GATE_REQUIRE_AARCH64:-0}" = 1 ]; then
    echo "FAIL  aarch64: aarch64-linux-gnu-gcc or qemu-aarch64 missing"
    exit 1
  fi
  echo "skip  aarch64: no aarch64-linux-gnu-gcc / qemu-aarch64"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if ! aarch64-linux-gnu-gcc -c "$FIX/c_struct.c" -o "$WORK/c_side.o" 2> "$WORK/cc.err"; then
  cat "$WORK/cc.err"
  exit 2
fi
if ! RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}" RASK_EXTRA_CFLAGS="$WORK/c_side.o" \
    "$RASK" compile --target aarch64-linux "$FIX/c_struct.rk" -o "$WORK/prog" > "$WORK/build.log" 2>&1; then
  echo "FAIL  aarch64: c_struct.rk didn't build"
  sed 's/^/      /' "$WORK/build.log" | head -20
  exit 1
fi
qemu-aarch64 -L /usr/aarch64-linux-gnu "$WORK/prog" > "$WORK/out" 2>&1

status=0
for expected in "fields 6 7 4" "area 42" "vec2 3.75" "vec2d 3.75" "stamp 41.5" \
                "triple 321" "rgba 4321" "mixed 50" "seg 4321" "tri 321" \
                "back rect 3 4" "back vec2 1.5 2.5" "back vec2d 1.25 2.5" \
                "back stamp 0.5 7" "back triple 1 2 3" "back rgba 10 11 12 13" \
                "back seg 1 2 3 4" "back tri 5 6 7" "round trip 42" \
                "called area 42" "called vec2 3.75" "called stamp 41.5" \
                "called triple 321" "called seg 4321" "called make 34" \
                "called make vec2 26.5" "called make stamp 70.5" \
                "called make triple 321" "called make tri 765" "called closure 45"; do
  if ! grep -qx "$expected" "$WORK/out"; then
    echo "FAIL  aarch64: expected \`$expected\`"
    status=1
  fi
done
if [ "$status" -ne 0 ]; then
  sed 's/^/      /' "$WORK/out"
  exit 1
fi
echo "ok    aarch64: every struct shape arrives as C reads it, and comes back as C wrote it"

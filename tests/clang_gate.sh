#!/bin/bash
# The C runtime, through the other compiler.
#
# CI builds the runtime with gcc on Linux and clang on macOS, so a difference
# between the two compilers only ever showed up on the macOS job — and that job
# can't build green.c or the two I/O engines at all, since they need Linux
# headers. Those three had no clang coverage anywhere.
#
# That gap cost three CI rounds in one morning. The last of them was real:
# `rask_string_live_buffers` is declared `_Atomic` but was read and written with
# gcc's `__atomic_*` builtins, which want a plain object. gcc waves it through,
# clang rejects it outright, and the error reproduces exactly here on Linux.
#
# Compiles in a copy of the directory so the Makefile supplies the flags and the
# file list — a second copy of CFLAGS is how the platform split went wrong in the
# first place — and so the gcc objects next to the sources are left alone.

set -u
cd "$(dirname "$0")/.." || exit 1

if ! command -v clang > /dev/null 2>&1; then
  echo "clang gate: clang not installed, skipping"
  exit 0
fi

WORK=$(mktemp -d) || exit 1
trap 'rm -rf "$WORK"' EXIT

cp compiler/runtime/Makefile compiler/runtime/*.c compiler/runtime/*.h "$WORK/" || exit 1

echo "clang gate: $(ls "$WORK"/*.c | wc -l) runtime sources, $(clang --version | head -1)"

if make -C "$WORK" CC=clang > "$WORK/log" 2>&1; then
  warnings=$(grep -c 'warning:' "$WORK/log")
  echo "──────────────────────────────────────────────────"
  echo "clang gate: ok ($warnings warnings)"
  exit 0
fi

echo "──────────────────────────────────────────────────"
echo "clang gate: FAILED — the runtime does not build with clang"
echo
grep -A3 'error:' "$WORK/log" | head -40
exit 1

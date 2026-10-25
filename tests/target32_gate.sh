#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# 32-bit gate: does every program still lower to MIR when a pointer is 4 bytes?
#
# No backend can emit 32-bit code yet. Cranelift has no 32-bit ARM or RISC-V,
# and LLVM comes later. But type checking, monomorphization and MIR lowering
# all run per target, and every one of them can grow an assumption that
# `usize` is `u64` or that a pointer is 8 bytes. Nothing ran them at 32 bits,
# so those assumptions piled up unseen: the stdlib's own `fs.rk` failed to type
# check, which meant hello world did too (#1642). This keeps new ones out while
# the backend is still years away.
#
# The target is `arm-none`. Its `cfg.os` is "none", so a program that needs an
# OS can fail here for that reason rather than for its width. Both are things
# an embedded build has to answer, and the known list says which is which.
#
# Each file in tests/suite/ and examples/ is lowered from `main` if it has one
# and from its tests if it has those, both when it has both. A file that
# fails is checked against tests/known_target32.txt: a new failure fails the
# gate, and so does a listed one that now passes, so the list can't go stale.
# A file that doesn't lower on the host either is listed with that reason; the
# other gates own it.
#
# Each file costs a full frontend run, stdlib included, so the whole corpus is
# about four minutes on four cores. CI splits it over two machines:
# `TARGET32_SHARDS=2 TARGET32_SHARD=1|2` lowers every other file, and only
# checks the registered lines that fall in its half.
#
# Usage:  tests/target32_gate.sh
# Exit:   0 = nothing new, 1 = a new failure or a stale line, 2 = setup error.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
KNOWN="$ROOT/tests/known_target32.txt"
TARGET="arm-none"

if [ ! -x "$RASK" ]; then
  echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
  exit 2
fi

cd "$ROOT"
. tests/lib/fanout.sh
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
export RASK WORK TARGET

lower32() {
  local f="$1" out="$WORK/$(slot "$1")" ok=1 tried=0
  if grep -qE '^(public )?func main\(' "$f"; then
    tried=1
    timeout 120 "$RASK" compile --dump-mir --target "$TARGET" "$f" >/dev/null 2>"$out.main" || ok=0
  fi
  if grep -qE '^(comptime )?test |^@test' "$f"; then
    tried=1
    timeout 120 "$RASK" test --dump-mir --target "$TARGET" "$f" >/dev/null 2>"$out.test" || ok=0
  fi
  [ "$tried" -eq 1 ] && [ "$ok" -eq 0 ] && echo "$f" > "$out.fail"
  return 0
}
export -f lower32 slot

SHARDS="${TARGET32_SHARDS:-1}"
SHARD="${TARGET32_SHARD:-1}"
if [ "$SHARDS" -lt 1 ] || [ "$SHARD" -lt 1 ] || [ "$SHARD" -gt "$SHARDS" ]; then
  echo "error: shard $SHARD of $SHARDS is not a shard that exists" >&2
  exit 2
fi
ALL=(tests/suite/*.rk examples/*.rk)
files=()
for i in "${!ALL[@]}"; do
  [ "$(( i % SHARDS ))" -eq "$(( SHARD - 1 ))" ] && files+=("${ALL[i]}")
done
if [ "${#files[@]}" -eq 0 ]; then
  echo "error: shard $SHARD of $SHARDS matched no files — this gate would pass by checking nothing" >&2
  exit 2
fi
printf '%s\n' "${files[@]}" | sort -u > "$WORK/mine"

fan_out lower32 "${files[@]}"

cat "$WORK"/*.fail 2>/dev/null | sort -u > "$WORK/now"
sed 's/[[:space:]]*#.*$//' "$KNOWN" | grep -v '^$' | sort -u | comm -12 - "$WORK/mine" > "$WORK/known"

new="$(comm -23 "$WORK/now" "$WORK/known")"
closed="$(comm -13 "$WORK/now" "$WORK/known")"
# A line for a file that's gone belongs to no shard, so shard 1 reports it.
if [ "$SHARD" -eq 1 ]; then
  printf '%s\n' "${ALL[@]}" | sort -u > "$WORK/all"
  gone="$(sed 's/[[:space:]]*#.*$//' "$KNOWN" | grep -v '^$' | sort -u | comm -23 - "$WORK/all")"
  closed="$(printf '%s\n%s\n' "$closed" "$gone" | grep -v '^$')"
fi

status=0
if [ -n "$new" ]; then
  echo "New 32-bit failures, not in tests/known_target32.txt:"
  for f in $new; do
    echo "  $f"
    cat "$WORK/$(slot "$f")".main "$WORK/$(slot "$f")".test 2>/dev/null \
      | grep -E '^error|-->' | head -6 | sed 's/^/      /'
  done
  echo "Reproduce with \`rask compile --dump-mir --target $TARGET <file>\` (or \`rask test\` for a"
  echo "file without main). Fix it, or register it with the reason if it's known work."
  status=1
fi
if [ -n "$closed" ]; then
  echo "Registered failures that now lower (delete these lines):"
  echo "$closed" | sed 's/^/  /'
  status=1
fi
echo "──────────────────────────────────────────────────"
if [ "$status" -eq 0 ]; then
  what="32-bit gate"
  [ "$SHARDS" -gt 1 ] && what="32-bit gate (shard $SHARD of $SHARDS)"
  echo "$what: ${#files[@]} files, $(wc -l < "$WORK/known") registered failures, none new"
fi
exit "$status"

#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# PostToolUse on Edit/Write: lint and type-check edited .rk files, parse-check
# edited specs. Exit 2 hands stderr back to Claude; exit 0 stays silent.
set -uo pipefail

input=$(cat)
file=$(jq -r '.tool_input.file_path // empty' <<<"$input")
[ -n "$file" ] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null)}"
rask="$root/compiler/target/release/rask"
[ -x "$rask" ] || exit 0
rel="${file#"$root"/}"

# Files that fail `rask check` on purpose, or can't be checked on their own.
check_skipped() {
  case "$1" in
    tests/compile_errors/* | docs/book/errors/*) ;;   # must be rejected
    tests/packages/* | compiler/crates/*/tests/fixtures/*) ;;  # gate fixtures, some meant to fail
    agentbench/tasks/*) ;;                            # verify.rk needs the model's solution
    stdlib/*) ;;                                      # built into the compiler, not a standalone file
    specs/analysis/prototype/cascade_hole_links.rk) ;;  # exists to be rejected
    *) return 1 ;;
  esac
}

report=""
case "$file" in
  *.rk)
    out=$("$rask" lint "$file" 2>&1)
    if ! grep -q 'No lint issues' <<<"$out"; then
      report+="rask lint $file:"$'\n'"$out"$'\n'
    fi
    if ! check_skipped "$rel"; then
      out=$(RASK="$rask" "$root/editors/claude-code/rask-check.sh" <<<"$input" 2>&1)
      [ $? -eq 2 ] && report+="$out"$'\n'
    fi
    ;;
  "$root"/specs/*.md | specs/*.md)
    out=$("$rask" test-specs "$file" 2>&1)
    status=$?
    if [ $status -ne 0 ] || ! grep -q ' 0 failed' <<<"$out"; then
      report+="rask test-specs $file:"$'\n'"$(grep -vE '^\s+✓' <<<"$out")"$'\n'
    fi
    ;;
esac

[ -z "$report" ] && exit 0
printf '%s' "$report" >&2
exit 2

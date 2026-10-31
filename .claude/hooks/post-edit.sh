#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# PostToolUse on Edit/Write: lint edited .rk files, parse-check edited specs.
# Exit 2 hands stderr back to Claude; exit 0 stays silent.
set -uo pipefail

file=$(jq -r '.tool_input.file_path // empty')
[ -n "$file" ] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(git rev-parse --show-toplevel 2>/dev/null)}"
rask="$root/compiler/target/release/rask"
[ -x "$rask" ] || exit 0

case "$file" in
  *.rk)
    out=$("$rask" lint "$file" 2>&1)
    if ! grep -q 'No lint issues' <<<"$out"; then
      echo "rask lint $file:" >&2
      echo "$out" >&2
      exit 2
    fi
    ;;
  "$root"/specs/*.md | specs/*.md)
    out=$("$rask" test-specs "$file" 2>&1)
    status=$?
    if [ $status -ne 0 ] || ! grep -q ' 0 failed' <<<"$out"; then
      echo "rask test-specs $file:" >&2
      grep -vE '^\s+✓' <<<"$out" >&2
      exit 2
    fi
    ;;
esac
exit 0

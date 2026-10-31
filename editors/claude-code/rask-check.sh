#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# Claude Code PostToolUse hook: type-check after every edit to a .rk file.
#
# Reads the hook's JSON on stdin, runs `rask check --json` on the edited file
# (inside a package that checks the whole package), and hands type errors back
# to Claude as one line each. Exit 2 makes Claude read them; exit 0 is silent.
# Warnings are left to `rask lint`.
#
# Needs jq. Set RASK to use a compiler that isn't on PATH.
set -uo pipefail

rask="${RASK:-rask}"
max=20

file=$(jq -r '.tool_input.file_path // empty')
case "$file" in *.rk) ;; *) exit 0 ;; esac
[ -f "$file" ] || exit 0
if ! command -v "$rask" >/dev/null 2>&1; then
  # Loud on purpose: a check that silently doesn't run reads as a passing one.
  echo "rask-check hook: \`$rask\` not found, so $file was not type-checked." >&2
  echo "Install rask or set RASK (editors/claude-code/README.md)." >&2
  exit 2
fi

json=$("$rask" check --json "$file" 2>/dev/null)
[ -n "$json" ] || exit 0

errors=$(jq -r '
  .file as $f
  | .diagnostics[]
  | select(.severity == "error")
  | ([.labels[]? | select(.role == "primary") | .message][0]) as $label
  | "\($f):\(.location.line):\(.location.column): error[\(.code)] \(.message)"
    + (if $label then ": \($label)" else "" end)
    + (if .help then "\n    help: \(.help)" else "" end)
' <<<"$json" 2>/dev/null)
[ -n "$errors" ] || exit 0

count=$(grep -c ': error\[' <<<"$errors")
{
  echo "rask check found $count error(s) after editing $file:"
  awk -v max="$max" '/: error\[/ { n++ } n <= max' <<<"$errors"
  [ "$count" -le "$max" ] || echo "...and $((count - max)) more."
  echo "\`rask explain <code>\` describes an error in full."
} >&2
exit 2

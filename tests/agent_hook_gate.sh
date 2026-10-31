#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# The Claude Code hook in editors/claude-code/ against the real compiler.
#
# The hook parses `rask check --json`, so a change to that output can break it
# without anything else noticing. This feeds it the JSON Claude Code sends and
# checks what comes back: errors and exit 2 for a broken package (including an
# error in a file other than the one edited), silence and exit 0 for clean
# code and for files that aren't Rask.

set -u
cd "$(dirname "$0")/.." || exit 1

export RASK="$PWD/compiler/target/release/rask"
hook="$PWD/editors/claude-code/rask-check.sh"
WORK=$(mktemp -d) || exit 1
trap 'rm -rf "$WORK"' EXIT

fail=0
run() { # name, file, want_exit, want_stderr_regex (empty = must be silent)
  local out status
  out=$(printf '{"tool_input":{"file_path":"%s"}}' "$2" | "$hook" 2>&1 >/dev/null)
  status=$?
  if [ "$status" -ne "$3" ]; then
    echo "FAIL $1: exit $status, want $3"; echo "$out"; fail=1; return
  fi
  if [ -z "$4" ] && [ -n "$out" ]; then
    echo "FAIL $1: want no output, got:"; echo "$out"; fail=1; return
  fi
  if [ -n "$4" ] && ! grep -qE "$4" <<<"$out"; then
    echo "FAIL $1: output doesn't match /$4/:"; echo "$out"; fail=1; return
  fi
  echo "ok   $1"
}

mkdir -p "$WORK/pkg"
printf 'schema = "rask-package@1"\n\n[package]\nname = "pkg"\nversion = "0.1.0"\n' > "$WORK/pkg/rask.toml"
printf 'public func greet(name: string) -> string {\n    return "hi {name}"\n}\n' > "$WORK/pkg/util.rk"
printf 'func main() {\n    let n: i32 = greet("x")\n    println("{n}")\n}\n' > "$WORK/pkg/main.rk"
printf 'func main() {\n    println("fine")\n}\n' > "$WORK/clean.rk"
printf 'func main() {\n' > "$WORK/unclosed.rk"
printf '# notes\n' > "$WORK/notes.md"

run "type error in the edited file"  "$WORK/pkg/main.rk" 2 'main\.rk:2:[0-9]+: error\[E0308\]'
run "type error in a sibling file"   "$WORK/pkg/util.rk" 2 'main\.rk:2:[0-9]+: error\[E0308\]'
run "parse error"                    "$WORK/unclosed.rk" 2 'error\[E0100\]'
run "clean file"                     "$WORK/clean.rk"    0 ''
run "not a Rask file"                "$WORK/notes.md"    0 ''
run "file that doesn't exist"        "$WORK/gone.rk"     0 ''
RASK="$WORK/no-such-rask" \
run "compiler missing says so"       "$WORK/clean.rk"    2 'not found, so .* was not type-checked'

exit $fail

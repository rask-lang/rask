#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Tutorial gate: docs/book/src/learn/ against the compiler. What it checks is
# in tests/learn_gate.py.
#
# Then the gate checks itself, on a small tutorial in tests/learn_gate/: that
# one has to pass, and copies of it broken one way each have to fail. A gate
# that can't fail looks exactly like a tutorial with nothing wrong in it.
#
# Usage:  tests/learn_gate.sh [--update]
# Exit:   0 = in step, 1 = a failure, 2 = no binary.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UPDATE=""
[ "${1:-}" = "--update" ] && UPDATE="--update"

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi

python3 "$ROOT/tests/learn_gate.py" $UPDATE "$ROOT/docs/book/src/learn" "$RASK"
status=$?
[ -n "$UPDATE" ] && exit $status

# ── The gate fails when it should ─────────────────────────────────────────────

FIXTURE="$ROOT/tests/learn_gate"
self_fails=0
cases=0
expect() {
    local want=$1 what=$2 dir=$3
    cases=$((cases + 1))
    python3 "$ROOT/tests/learn_gate.py" "$dir" "$RASK" > "$dir.log" 2>&1
    local got=$?
    python3 "$ROOT/tests/book_from.py" "$dir"/*.md >> "$dir.log" 2>&1
    local from=$?
    if [ "$want" = pass ] && { [ $got -ne 0 ] || [ $from -ne 0 ]; }; then
        echo "FAIL: self-test: $what should pass:"; sed 's/^/    /' "$dir.log"
        self_fails=$((self_fails + 1))
    elif [ "$want" = fail ] && [ $got -eq 0 ] && [ $from -eq 0 ]; then
        echo "FAIL: self-test: $what should fail, and the gate let it through"
        self_fails=$((self_fails + 1))
    fi
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
broken() {
    rm -rf "$WORK/$1"
    cp -r "$FIXTURE" "$WORK/$1"
    echo "$WORK/$1"
}

expect pass "the fixture" "$(broken ok)"

d="$(broken stale)"
sed -i 's/one function, one exercise/one function and its exercise/' "$d/mud/greet.rk"
expect fail "an exercise generated from an older MUD" "$d"

d="$(broken output)"
sed -i 's/1 passed/2 passed/' "$d/output/01-test-solved.out"
expect fail "a pinned output the command doesn't print" "$d"

d="$(broken from)"
sed -i 's/return "Hello/return  "Hello/' "$d/01-hello.md"
expect fail "a block that isn't a verbatim run of its file" "$d"

d="$(broken include)"
printf '\n{{#include mud/greet.rk}}\n' >> "$d/01-hello.md"
expect fail "an {{#include}} in a chapter" "$d"

d="$(broken orphan)"
cp "$d/01-hello.rk" "$d/02-extra.rk"
expect fail "an exercise file nothing generates" "$d"

d="$(broken exits)"
printf '\n```bash\nrask test 01-hello.rk\n```\n' >> "$d/01-hello.md"
expect fail "a failing command with no output block" "$d"

d="$(broken prose)"
python3 - "$d/01-hello.md" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read().replace("rask test mud\n```\n\n", "rask test mud\n```\n\nIt prints:\n\n", 1)
open(p, "w").write(s)
PY
sed -i 's/1 passed/2 passed/' "$d/output/01-test-solved.out" "$d/01-hello.md"
expect fail "an output block with prose before it, and a different output" "$d"

d="$(broken early)"
printf '\n```bash\nrask check no-such-file.rk\nls mud\n```\n' >> "$d/01-hello.md"
expect fail "a block whose first line fails" "$d"

d="$(broken marker)"
sed -i 's|<!-- from: mud/greet.rk -->|<!-- from: mud/greet.rk (the function) -->|' "$d/01-hello.md"
expect fail "a from: marker with words after the path" "$d"

d="$(broken unmarked)"
printf '02-more.rk: greet.rk\n' >> "$d/exercises"
cp "$d/01-hello.rk" "$d/02-more.rk"
expect fail "an exercise with no function marked for it" "$d"

d="$(broken brace)"
sed -i 's/^    return "Hello, {name}!"$/return "Hello, {name}!"/' "$d/mud/greet.rk"
expect fail "a marked function whose body isn't indented, so where it ends is a guess" "$d"

d="$(broken sh)"
printf '\n```sh\nfalse\n```\n' >> "$d/01-hello.md"
expect fail "a failing sh block" "$d"

d="$(broken stray)"
cp "$d/output/01-test.out" "$d/output/stray.out"
expect fail "a pinned output no command prints" "$d"

d="$(broken twice)"
printf '01-hello.rk: greet.rk\n' >> "$d/exercises"
expect fail "an exercise listed twice" "$d"

if [ $self_fails -ne 0 ]; then
    echo "learn gate self-test: $self_fails failed"
    exit 1
fi
echo "learn gate self-test: $cases ok"
exit $status

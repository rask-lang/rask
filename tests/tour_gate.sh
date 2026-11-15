#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Tour gate. Every step in docs/book/src/learn/ is something the reader runs
# as they find it, so each one has to fail in the way the step says and pass
# once it's done:
#
#   - a step with an answer in solutions/ must fail `rask test` as shipped
#     (otherwise there's nothing to do) and its solution must pass, on native
#     and on the interpreter;
#   - a step without one must run.
#
# A step is a .rk file, or a directory holding a package.
#
# The old tutorial's answer key went unchecked and seven of nineteen answers
# stopped compiling (#1030); this is the same check for the tour.
#
# Then two checks on what the steps say rather than what they do:
#
#   - no step uses an idea a later step teaches (tests/tour_ideas.py). It runs
#     once more on a copy with a planted violation, which has to fail, so a
#     check that quietly stopped matching anything can't pass for a clean tour;
#   - the book's pages can be made from the steps, and the book's lists of
#     steps agree with the files (docs/book/learn_pages.py).
#
# Usage:  tests/tour_gate.sh
# Exit:   0 = every step behaves as the tour says.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
TOUR="$ROOT/docs/book/src/learn"

if [ ! -x "$RASK" ]; then
    echo "no rask binary at $RASK — cargo build --release -p rask-cli" >&2
    exit 1
fi

failed=0
for step in "$TOUR"/[0-9]*; do
    name="$(basename "$step")"
    case "$name" in *.md) continue ;; esac
    answer="$TOUR/solutions/$name"
    if [ -e "$answer" ]; then
        if "$RASK" test "$step" >/dev/null 2>&1; then
            echo "FAIL $name: passes before the reader has done anything"
            failed=1
        fi
        for backend in "" "--interp"; do
            if ! out="$("$RASK" test $backend "$answer" 2>&1)"; then
                echo "FAIL solutions/$name ${backend:-(native)}:"
                echo "$out"
                failed=1
            fi
        done
    else
        for backend in "" "--interp"; do
            if ! out="$("$RASK" run $backend "$step" 2>&1)"; then
                echo "FAIL $name doesn't run ${backend:-(native)}:"
                echo "$out"
                failed=1
            fi
        done
    fi
    # `rask test` on a package leaves a build directory beside it.
    rm -rf "$step/build" "$answer/build"
done

for answer in "$TOUR"/solutions/*; do
    if [ ! -e "$TOUR/$(basename "$answer")" ]; then
        echo "FAIL solutions/$(basename "$answer") has no step"
        failed=1
    fi
done

if ! python3 "$ROOT/tests/tour_ideas.py" "$TOUR"; then
    failed=1
fi

# The planted violation: step 5's answer reaching for `match`, which step 12
# teaches.
PLANT="$(mktemp -d)"
cp -r "$TOUR" "$PLANT/learn"
printf '\nfunc planted(n: i32) -> i32 {\n    return match n { _ => 0 }\n}\n' \
    >> "$PLANT/learn/solutions/05-if.rk"
if out="$(python3 "$ROOT/tests/tour_ideas.py" "$PLANT/learn" 2>&1)"; then
    echo "FAIL tour_ideas.py passed a step 5 that uses \`match\`"
    failed=1
elif ! grep -q "05-if.rk.*12-enums" <<<"$out"; then
    echo "FAIL tour_ideas.py failed the planted copy, but not on the planted line:"
    echo "$out"
    failed=1
fi
rm -rf "$PLANT"

PAGES="$(mktemp -d)"
if ! python3 "$ROOT/docs/book/learn_pages.py" --out "$PAGES"; then
    failed=1
fi
rm -rf "$PAGES"

if [ "$failed" -eq 0 ]; then
    echo "tour gate: ok"
fi
exit "$failed"

#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Lint gate for the code that teaches.
#
# `rask lint` was run by a hook after an edit and by nothing else, so its
# findings accumulated: 3 errors and 24 warnings in `stdlib/` by the time
# anyone counted. Most were the linter's fault rather than the stdlib's —
# thirteen told you to rename `struct net { }` to `Net` — which is how a
# tool teaches people to stop reading it, and why the two real ones sat
# there unnoticed.
#
# Everything is clean now, and this keeps it that way.
#
# What's covered: the code a reader is meant to copy. `tests/suite/` is not
# here on purpose — those files exist to exercise edge cases, including bad
# idioms, and a test input isn't an exemplar.
#
# A finding that is right about the rule and wrong about this code gets an
# `@allow(rule/id)` at the declaration with the reason in a comment, which is
# what the annotation is for. In an example the book includes by anchor, put
# the `@allow` *above* the `// ANCHOR:` line so it doesn't show up in the text
# a learner reads.
#
# Usage:  tests/lint_gate.sh
# Exit:   0 = every directory clean, 1 = otherwise.

set -u
cd "$(dirname "$0")/.."

if [ -x compiler/target/release/rask ]; then
    RASK=compiler/target/release/rask
elif [ -x compiler/target/debug/rask ]; then
    RASK=compiler/target/debug/rask
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi

fail=0
for dir in stdlib examples docs projects; do
    [ -d "$dir" ] || continue
    out="$("$RASK" lint "$dir/" 2>&1)"
    if echo "$out" | grep -q '^✓'; then
        printf '  %-10s clean\n' "$dir"
    else
        printf '  %-10s %s\n' "$dir" "$(echo "$out" | tail -1)"
        echo "$out" | grep -E '^(error|warning)\[' | sed 's/^/      /'
        fail=1
    fi
done

echo "──────────────────────────────────────────────────"
if [ "$fail" -eq 0 ]; then
    echo "lint gate: stdlib, examples, docs and projects are clean"
else
    echo "lint gate: FAILED — fix it, or @allow it at the declaration with the reason"
fi
exit "$fail"

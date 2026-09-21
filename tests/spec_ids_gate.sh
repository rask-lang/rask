#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Spec rule-id gate.
#
# `CONVENTIONS.md` makes `spec-id/rule-id` the way anything cites a rule —
# `mem.ownership/O11`, `type.structs/M3`. Diagnostics emit them too:
# `warning[comp.effects/CW2]` is built from a string in rask-effects. So an id
# is a name, and two rules sharing one makes every citation of it ambiguous.
#
# That is easy to do by accident. Adding a rule means picking the next free
# number by reading the file, and a file with two rule tables — a syntax table
# near the top and a capability table near the bottom — numbers each from its
# own end. `mem.racks` had `RK10: Retargeting is constant-time` at line 133
# when a second `RK10: A node has a slot number` was added at line 62; both
# were right about their own content and the citation was ruined.
#
# Nothing caught it. This does.
#
# An id repeated under the *same* name is not a collision: `ctrl.comptime`
# keeps an "Allowed Features" status table whose rows are keyed by the id of
# the rule they report on, so `CT48: Comptime for` appears as a definition and
# again as a status row. That is a cross-reference. A collision is one id over
# two different names, which is what makes a citation ambiguous, and that is
# what fails here.
#
# Exploration and deprecated pages are skipped: they record arguments rather
# than rules, and an argument may quote an id that later moved.

set -u
cd "$(dirname "$0")/.."

fail=0
checked=0

while IFS= read -r f; do
    status=$(grep -m1 -oE 'status: [a-z]+' "$f" || true)
    case "$status" in
        *exploration*|*deprecated*) continue ;;
    esac
    checked=$((checked + 1))

    # id + name, deduplicated, so an id left twice is one carrying two names.
    #
    # Only a *named* row counts as a definition. The bare `| **ER14** |` form
    # is used for both jobs and can't be told apart by shape: `mem.resources`
    # defines `| **R5** | — | …` without a name, while `type.errors` opens its
    # "Three Words, Three Jobs" table with three rows all keyed `**ER14**`,
    # which are references to one rule rather than three definitions of it.
    # Judging the named form covers nearly every rule and is never wrong;
    # judging the bare form would fail an honest cross-reference table.
    dupes=$(grep -oE '^\| \*\*[A-Z]+[0-9]+[a-z]?: [^*]*' "$f" \
            | sed 's/^| \*\*//' \
            | sort -u \
            | cut -d: -f1 \
            | sort | uniq -d)

    [ -z "$dupes" ] && continue

    for id in $dupes; do
        echo "duplicate rule id: ${f#specs/} defines $id more than once"
        grep -nE "^\| \*\*$id:" "$f" | while IFS= read -r line; do
            echo "    ${line%%|*}$(echo "$line" | cut -c1-96 | sed 's/^[0-9]*:/  /')"
        done
        fail=1
    done
done < <(find specs -name '*.md')

echo "──────────────────────────────────────────────────"
if [ "$fail" -eq 0 ]; then
    echo "spec ids gate: $checked files, no duplicate rule ids"
else
    echo "spec ids gate: FAILED — a citation like \`spec/ID\` can't name two rules"
    echo "Give the newer rule the next free number in that file and update its citations."
fi
exit "$fail"

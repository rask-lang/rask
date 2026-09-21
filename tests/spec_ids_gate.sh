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

if [ "$fail" -eq 0 ]; then
    echo "spec ids gate: $checked files, no duplicate rule ids"
else
    echo "spec ids gate: FAILED — a citation like `spec/ID` can't name two rules"
    echo "Give the newer rule the next free number in that file and update its citations."
fi

# ── Part 2: every `spec-id/RULE` citation names a rule that exists ────────────
#
# An id is only a name if it resolves. `mem.closures/SL1` was cited from four
# files after SL1 and SL2 were replaced by SL3/SL4 — the citations still read
# like authority and pointed at nothing. Same for `type.primitives/CV5–CV10`
# in a compiler diagnostic, where the real rules are CV11–CV16.
#
# A rule is "defined" by any bolded table row or section heading in its spec
# that carries the id, which is looser than Part 1's named-definition test on
# purpose: this asks whether the file mentions the rule at all, so a row that
# records a deletion (`| **ER21, ER24, ER25 deleted** |`) still counts — RULINGS
# cites ER21 precisely *because* it was removed.
#
# Exploration and deprecated pages are skipped as citation sources: they quote
# doc comments and older drafts verbatim, and rewriting a quote to keep a gate
# happy falsifies the record.

python3 - <<'PY' || fail=1
import glob, re, sys

ID   = re.compile(r'\b([A-Z]{1,4}[0-9]+(?:\.[0-9]+)?[a-z]?)\b')
ROW  = re.compile(r'^\| \*\*([^*]*)\*\*', re.M)
HEAD = re.compile(r'^#+ .*[(\[]([^)\]]*)[)\]]\s*$', re.M)
CITE = re.compile(r'\b([a-z][a-z0-9]*\.[a-z0-9-]+)/([A-Z]+[0-9]+(?:\.[0-9]+)?[a-z]?)\b')
STATUS = re.compile(r'<!--\s*status:\s*([a-z]+)')

specs, rules, skip = {}, {}, set()
for f in glob.glob('specs/**/*.md', recursive=True):
    txt = open(f, errors='ignore').read()
    m = re.search(r'<!--\s*id:\s*([a-z0-9.-]+)\s*-->', txt)
    if not m:
        continue
    sid = m.group(1)
    specs[sid] = f
    s = STATUS.search(txt)
    if s and s.group(1) in ('exploration', 'deprecated'):
        skip.add(f)
    found = set()
    for grp in ROW.findall(txt) + HEAD.findall(txt):
        for rid in ID.findall(grp):
            found.add(rid)
            found.add(rid.split('.')[0])
    rules[sid] = found

bad = []
sources = sorted(set(glob.glob('specs/**/*.md', recursive=True)
                     + glob.glob('compiler/crates/*/src/**/*.rs', recursive=True)
                     + glob.glob('compiler/crates/*/src/*.rs', recursive=True)))
for f in sources:
    if f in skip:
        continue
    for i, line in enumerate(open(f, errors='ignore'), 1):
        for sid, rule in sorted(set(CITE.findall(line))):
            if sid in specs and rule not in rules[sid]:
                bad.append((f, i, sid, rule))

for f, i, sid, rule in bad:
    print(f"dangling citation: {f}:{i} cites {sid}/{rule}, "
          f"not defined in {specs[sid]}")
print(f"spec ids gate: {len(sources)} files scanned, "
      f"{'no dangling citations' if not bad else 'FAILED'}")
if bad:
    print("Point each citation at the rule it means, "
          "or say in the spec that the old id was retired.")
sys.exit(1 if bad else 0)
PY

echo "──────────────────────────────────────────────────"
exit "$fail"

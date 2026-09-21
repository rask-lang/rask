#!/usr/bin/env python3
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Spec rule-id gate.
#
# `CONVENTIONS.md` makes `spec-id/rule-id` the way anything cites a rule —
# `mem.ownership/O11`, `type.structs/M3`. Diagnostics emit them too:
# `warning[comp.effects/CW2]` is built from a string in rask-effects. So an id
# is a name, and a name has two ways to stop working.
#
# 1. Two rules share one id, and every citation of it becomes ambiguous.
#
#    Easy to do by accident: adding a rule means picking the next free number
#    by reading the file, and a file with two rule tables — a syntax table near
#    the top, a capability table near the bottom — numbers each from its own
#    end. `mem.racks` had `RK10: Retargeting is constant-time` at line 133 when
#    a second `RK10: A node has a slot number` was added at line 62. Both were
#    right about their own content and the citation was ruined. `conc.runtime`
#    had the same thing in headings: `P3` was both safe-point instrumentation
#    and scalability limits, and citations of both were already in the wild.
#
# 2. The rule isn't there at all. `mem.closures/SL1` was cited from four files
#    after SL1 and SL2 were replaced by SL3/SL4. A dangling citation reads with
#    exactly the authority of one that resolves.
#
# A rule is defined by a table row `| **ID: name** |` or a section heading
# `### Title (ID)` / `### Title [ID]`.
#
# Exploration and deprecated pages are skipped: they quote old doc comments and
# earlier drafts verbatim, and editing a quote to please a gate falsifies it.

import glob
import re
import sys

ID = re.compile(r'\b([A-Z]{1,4}[0-9]+(?:\.[0-9]+)?[a-z]?)\b')
ROW = re.compile(r'^\| \*\*([^*]*)\*\*', re.M)
HEAD = re.compile(r'^#+ (.*?)[(\[]([^)\]]*)[)\]]\s*$', re.M)
CITE = re.compile(r'\b([a-z][a-z0-9]*\.[a-z0-9-]+)/([A-Z]+[0-9]+(?:\.[0-9]+)?[a-z]?)\b')
SPEC_ID = re.compile(r'<!--\s*id:\s*([a-z0-9.-]+)\s*-->')
STATUS = re.compile(r'<!--\s*status:\s*([a-z]+)')

# A heading's parens carry cross-references as well as its own id:
# `### Spawn Flow (S4 - realizes conc.async/S1, S4)` defines S4 and mentions
# two rules of another spec. The cross-reference starts at the dash, at
# "realizes", or at the first `spec.id/` — and runs to the end.
CROSSREF = re.compile(r'(\s+[-–—]\s+|\s*\brealizes\b|\s*[a-z][a-z0-9]*\.[a-z0-9-]+/).*$')


def defined_ids(text):
    """Every rule id this file defines, from rows and headings alike."""
    found = set()
    for group in ROW.findall(text):
        for rid in ID.findall(group):
            found.add(rid)
            found.add(rid.split('.')[0])
    for _, group in HEAD.findall(text):
        for rid in ID.findall(CROSSREF.sub('', group)):
            found.add(rid)
            found.add(rid.split('.')[0])
    return found


def named_definitions(text):
    """id -> the names it is defined under, for the collision check.

    Only a *named* definition counts. The bare `| **ER14** |` form is used for
    both jobs and can't be told apart by shape: `mem.resources` defines
    `| **R5** | — | …` without a name, while `type.errors` opens its "Three
    Words, Three Jobs" table with three rows all keyed `**ER14**`, which are
    references to one rule rather than three definitions of it. Judging the
    named form covers nearly every rule and is never wrong; judging the bare
    form would fail an honest cross-reference table.
    """
    names = {}
    for group in ROW.findall(text):
        if ':' not in group:
            continue
        rid, name = group.split(':', 1)
        if ID.fullmatch(rid.strip()):
            names.setdefault(rid.strip(), set()).add(name.strip())
    # A heading carrying an id the file also defines in a row is pointing at
    # that rule, not redefining it — `### Why Hybrid SSA (IR3)` is the
    # rationale for the row above. A heading carrying an id no row defines is
    # the definition: `conc.runtime` has no rule tables at all and writes
    # every rule as `### Task Structure (T1)`.
    from_rows = set(names)
    for title, group in HEAD.findall(text):
        for rid in ID.findall(CROSSREF.sub('', group)):
            if rid in from_rows or rid.split('.')[0] in from_rows:
                continue
            names.setdefault(rid, set()).add(title.strip())
    return names


def main():
    specs, rules, skip = {}, {}, set()
    for f in sorted(glob.glob('specs/**/*.md', recursive=True)):
        text = open(f, errors='ignore').read()
        m = SPEC_ID.search(text)
        if not m:
            continue
        specs[m.group(1)] = f
        s = STATUS.search(text)
        if s and s.group(1) in ('exploration', 'deprecated'):
            skip.add(f)
        rules[m.group(1)] = defined_ids(text)

    failed = False

    checked = 0
    for sid, f in specs.items():
        if f in skip:
            continue
        checked += 1
        for rid, names in sorted(named_definitions(open(f, errors='ignore').read()).items()):
            if len(names) > 1:
                failed = True
                print(f"duplicate rule id: {f} defines {rid} more than once")
                for name in sorted(names):
                    print(f"    {rid}: {name[:88]}")
    if not failed:
        print(f"spec ids gate: {checked} specs, no duplicate rule ids")
    else:
        print("spec ids gate: FAILED — a citation like `spec/ID` can't name two rules")
        print("Give the newer rule the next free number in that file and update its citations.")

    sources = sorted(set(glob.glob('specs/**/*.md', recursive=True)
                         + glob.glob('compiler/crates/*/src/**/*.rs', recursive=True)
                         + glob.glob('compiler/crates/*/src/*.rs', recursive=True)))
    dangling = []
    for f in sources:
        if f in skip:
            continue
        for i, line in enumerate(open(f, errors='ignore'), 1):
            for sid, rid in sorted(set(CITE.findall(line))):
                if sid in specs and rid not in rules[sid]:
                    dangling.append((f, i, sid, rid))
    for f, i, sid, rid in dangling:
        print(f"dangling citation: {f}:{i} cites {sid}/{rid}, not defined in {specs[sid]}")
    if dangling:
        failed = True
        print(f"spec ids gate: {len(sources)} files scanned, FAILED")
        print("Point each citation at the rule it means, or say in the spec "
              "that the old id was retired.")
    else:
        print(f"spec ids gate: {len(sources)} files scanned, no dangling citations")

    print("──────────────────────────────────────────────────")
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())

#!/usr/bin/env python3
# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""No tour step uses an idea before the step that teaches it.

Each step declares the idea it introduces on a line of its own,

    // new: optionals

and IDEAS below says what that idea looks like in source. A step's exercise
and its solution may use its own idea and every earlier one; a marker of a
later step's idea is a failure, reported with the line. Comments and the text
of string literals don't count (prose may say "comes later"), but code inside
a string's `{...}` does.

The old tutorial had chapter 5's exercise needing chapter 6. This is the check
that would have said so.

Usage:  tests/tour_ideas.py [DIR]    (default: docs/book/src/learn)
Exit:   0 = every step keeps to what's been taught.
"""

import os
import re
import sys

# idea -> regexes over code with comments and string text removed.
IDEAS = {
    "hello": [r"\bprintln\b"],
    "functions": [r"\breturn\b", r"^\s*test\s", r"\bassert\b"],
    "names": [r"\blet\b"],
    "mut": [r"\bmut\b", r"[-+*/]="],
    "if": [r"\bif\b", r"\belse\b"],
    "while": [r"\bwhile\b"],
    "for": [r"\bfor\b", r"\.\."],
    "vec": [r"\bVec\b"],
    # `for x in name {`, over a collection rather than a range
    "for-vec": [r"\bfor\s+\w+\s+in\s+[a-z_]\w*(\.\w+(\(\))?)*\s*\{"],
    "structs": [r"\bstruct\b"],
    "methods": [r"\bextend\b", r"\bself\b"],
    "enums": [r"\benum\b", r"\bmatch\b"],
    # a variant built or matched with fields: `Payment.Cash(`
    "enum-data": [r"\b[A-Z]\w*\.[A-Z]\w*\("],
    "optionals": [r"\?\?", r"\?\s*as\b", r"\bnone\b", r"[\w>)\]]\?"],
    "maps": [r"\bMap\b"],
    # `i32 or BankError`, `r is BankError`, `r!`
    "errors": [r"\bor\s+[A-Z]", r"\bis\s+[A-Z]", r"[\w)\]]!(?!=)"],
    "try": [r"\btry\b", r"\bcatch\b"],
    "mutate": [r"\bmutate\b"],
    "take": [r"\btake\b"],
    "clone": [r"\.clone\(\)"],
    "with": [r"\bwith\b"],
    "closures": [r"\bfunc\(", r"\|\s*[a-z_]\w*(\s*,\s*[a-z_]\w*)*\s*\|"],
    "chains": [r"\.(filter|map|to_vec|fold|reduce|sum|count|any|all|find|for_each"
               r"|enumerate|take_while|skip_while|flat_map|flatten|min|max)\("],
    "strings": [r"\.(split|split_whitespace|lines|chars|trim|trim_start|trim_end"
                r"|to_uppercase|to_lowercase|starts_with|ends_with|replace"
                r"|index_of|repeat|to_string|to_strings|parse)\b"],
    "generics": [r"\bfunc\s+\w+\s*<"],
    "interfaces": [r"\binterface\b", r"\bimplements\b"],
    "ensure": [r"\bensure\b"],
    "racks": [r"\bRack\b", r"\bLink\b", r"\bimport\b"],
    "tasks": [r"\bspawn\b", r"\bChannel\b", r"\bMultitasking\b"],
    "shared": [r"\bShared\b"],
    "comptime": [r"\bcomptime\b", r"\bconst\b"],
    "packages": [r"\bpublic\b"],
}

NEW = re.compile(r"^// new:\s*(\S+)\s*$", re.M)


def code_only(src):
    """`src` with comments and string text blanked, line breaks kept."""
    out = []
    i, n = 0, len(src)
    # Each entry is a nesting level: "code", or "str" inside a string literal.
    # A `{` in a string opens code (an interpolation) until its `}`.
    stack = ["code"]
    depth = [0]
    while i < n:
        c = src[i]
        top = stack[-1]
        if top == "code":
            if src.startswith("//", i):
                j = src.find("\n", i)
                i = n if j < 0 else j
                continue
            if c == '"':
                stack.append("str")
                out.append(" ")
            elif c == "'" and re.match(r"'(\\.|[^'\\])'", src[i:]):
                m = re.match(r"'(\\.|[^'\\])'", src[i:])
                out.append(" " * len(m.group(0)))
                i += len(m.group(0))
                continue
            elif c == "{":
                depth[-1] += 1
                out.append(c)
            elif c == "}":
                if depth[-1] == 0 and len(stack) > 1:
                    stack.pop()
                    depth.pop()
                    out.append(" ")
                else:
                    depth[-1] -= 1
                    out.append(c)
            else:
                out.append(c)
        else:
            if c == "\\":
                out.append("  ")
                i += 2
                continue
            if c == '"':
                stack.pop()
                out.append(" ")
            elif c == "{":
                stack.append("code")
                depth.append(0)
                out.append(" ")
            else:
                out.append("\n" if c == "\n" else " ")
        i += 1
    return "".join(out)


def steps(root):
    """(number, name, [step files], [solution files]) in order."""
    found = []
    for name in sorted(os.listdir(root)):
        m = re.match(r"(\d+)-", name)
        if not m:
            continue
        path = os.path.join(root, name)
        answer = os.path.join(root, "solutions", name)
        if os.path.isdir(path):
            files = sorted(os.path.join(path, f) for f in os.listdir(path) if f.endswith(".rk"))
            answers = sorted(os.path.join(answer, f) for f in os.listdir(answer) if f.endswith(".rk")) \
                if os.path.isdir(answer) else []
        elif name.endswith(".rk"):
            files = [path]
            answers = [answer] if os.path.isfile(answer) else []
        else:
            continue
        found.append((int(m.group(1)), name, files, answers))
    return found


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "docs", "book", "src", "learn")
    failed = False
    order = []
    for num, name, files, _ in steps(root):
        declared = [m.group(1) for f in files for m in NEW.finditer(open(f).read())]
        if len(declared) != 1:
            print(f"FAIL {name}: declares {len(declared)} ideas; it needs one `// new: <idea>` line")
            failed = True
            continue
        idea = declared[0]
        if idea not in IDEAS:
            print(f"FAIL {name}: `// new: {idea}` isn't in tests/tour_ideas.py")
            failed = True
            continue
        order.append((num, name, idea))

    seen = {}
    for _, name, idea in order:
        if idea in seen:
            print(f"FAIL {name}: `{idea}` is already introduced by {seen[idea]}")
            failed = True
        seen[idea] = name
    for idea in IDEAS:
        if idea not in seen:
            print(f"FAIL no step introduces `{idea}`")
            failed = True

    by_name = {name: (files, answers) for _, name, files, answers in steps(root)}
    for i, (_, name, idea) in enumerate(order):
        files, answers = by_name[name]
        # A step that never shows its own idea's markers means the markers
        # are wrong, and then they'd catch nothing anywhere else either.
        own = code_only("\n".join(open(p).read() for p in files + answers))
        if not any(re.search(p, own, re.M) for p in IDEAS[idea]):
            print(f"FAIL {name}: none of the markers for `{idea}` appear in it")
            failed = True
        later = order[i + 1:]
        for path in files + answers:
            code = code_only(open(path).read()).split("\n")
            for lineno, line in enumerate(code, 1):
                for _, other, later_idea in later:
                    for pattern in IDEAS[later_idea]:
                        if re.search(pattern, line):
                            rel = os.path.relpath(path, root)
                            print(f"FAIL {rel}:{lineno} uses `{later_idea}` ({pattern}), "
                                  f"which isn't taught until {other}")
                            failed = True
    if not failed:
        print(f"tour ideas: ok, {len(order)} steps")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

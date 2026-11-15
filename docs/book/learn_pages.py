#!/usr/bin/env python3
# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""Book pages for the tour, made from the step files.

Each step in src/learn/ is a .rk file the reader runs in a clone, and its
comments are the lesson. This turns one into a page: the comment blocks
become prose, the code between them becomes code blocks, and the page's code
gets a button that runs it (theme/run.js). The text lives in the .rk file
only; the pages are build output and aren't committed.

A step that is a directory (a package) becomes one page showing each file.

SUMMARY.md and src/learn/README.md list the steps by hand, since mdBook and a
reader in a clone each need the list. This fails when either disagrees with
the step files, so neither can drift.

Usage:  docs/book/learn_pages.py [--out DIR]   (default: docs/book/src/learn)
Exit:   0 = pages written, 1 = a list disagrees with the steps.
"""

import os
import re
import sys

BOOK = os.path.dirname(os.path.abspath(__file__))
LEARN = os.path.join(BOOK, "src", "learn")
TITLE = re.compile(r"^// (\d+)\. (.+)$")
MACHINE = re.compile(r"^// (new|test-specs):")


def is_comment(line):
    return line == "//" or line.startswith("// ")


def segments(source):
    """[(kind, lines)]: "prose" for column-0 comment runs, "code" otherwise."""
    lines = source.rstrip("\n").split("\n")
    if lines and lines[0].startswith("// SPDX-License-Identifier"):
        lines = lines[1:]
    lines = [l for l in lines if not MACHINE.match(l)]
    out = []
    for line in lines:
        kind = "prose" if is_comment(line) else "code"
        text = line[3:] if line.startswith("// ") else ("" if line == "//" else line)
        if out and out[-1][0] == kind:
            out[-1][1].append(text)
        elif kind == "code" and not line.strip() and out and out[-1][0] == "prose":
            out[-1][1].append("")
        else:
            out.append((kind, [text]))
    trimmed = []
    for kind, body in out:
        while body and not body[0].strip():
            body.pop(0)
        while body and not body[-1].strip():
            body.pop()
        if body:
            trimmed.append((kind, body))
    return trimmed


def title_of(source):
    for line in source.split("\n"):
        m = TITLE.match(line)
        if m:
            return int(m.group(1)), m.group(2)
    return None


def render(source, marker, heading=True):
    """Markdown for one .rk file. `marker` goes above each code block."""
    md = []
    for kind, body in segments(source):
        if kind == "prose":
            if heading and TITLE.match("// " + body[0]):
                md.append("# " + TITLE.match("// " + body[0]).group(2))
                body = body[1:]
                while body and not body[0].strip():
                    body.pop(0)
                if not body:
                    continue
                md.append("")
            md.append("\n".join(body))
        else:
            if marker:
                md.append(f"<!-- tour: {marker} -->")
            md.append("```rask\n" + "\n".join(body) + "\n```")
        md.append("")
    return "\n".join(md)


def steps():
    """(number, name, title, page markdown) for each step, in order."""
    found = []
    for name in sorted(os.listdir(LEARN)):
        if not re.match(r"\d+-", name):
            continue
        path = os.path.join(LEARN, name)
        if os.path.isdir(path):
            main = open(os.path.join(path, "main.rk")).read()
            number, title = title_of(main)
            parts = [render(main, "show")]
            for f in sorted(os.listdir(path)):
                if f == "main.rk" or not f.endswith(".rk"):
                    continue
                parts.append(f"## {f}\n\n" + render(open(os.path.join(path, f)).read(), "show", heading=False))
            toml = os.path.join(path, "rask.toml")
            if os.path.isfile(toml):
                parts.append("## rask.toml\n\n```toml\n" + open(toml).read().rstrip("\n") + "\n```\n")
            found.append((number, name, title, "\n".join(parts)))
        elif name.endswith(".rk"):
            source = open(path).read()
            number, title = title_of(source)
            kind = "test" if re.search(r'^test "', source, re.M) else "run"
            found.append((number, name[:-3], title, render(source, kind)))
    return found


def check_lists(found):
    """Problems with SUMMARY.md's and README.md's lists of the steps."""
    problems = []
    want = [(name, title) for _, name, title, _ in found]

    summary = open(os.path.join(BOOK, "src", "SUMMARY.md")).read()
    listed = re.findall(r"^\s+- \[(.+?)\]\(learn/(\d+-[\w-]+)\.md\)", summary, re.M)
    listed = [(name, title) for title, name in listed]
    if listed != want:
        problems.append("SUMMARY.md lists the tour as\n    "
                        + "\n    ".join(f"[{t}](learn/{n}.md)" for n, t in listed)
                        + "\n  but the steps are\n    "
                        + "\n    ".join(f"[{t}](learn/{n}.md)" for n, t in want))

    readme = open(os.path.join(LEARN, "README.md")).read()
    items = re.findall(r"^(\d+)\. (.+)$", readme, re.M)
    want_items = [(str(number), title) for number, _, title, _ in found]
    if items != want_items:
        problems.append("src/learn/README.md lists the steps as\n    "
                        + "\n    ".join(f"{n}. {t}" for n, t in items)
                        + "\n  but they are\n    "
                        + "\n    ".join(f"{n}. {t}" for n, t in want_items))
    return problems


def main():
    out = LEARN
    if len(sys.argv) > 2 and sys.argv[1] == "--out":
        out = sys.argv[2]
    found = steps()
    numbers = [number for number, _, _, _ in found]
    if numbers != list(range(1, len(found) + 1)):
        print(f"learn_pages: steps aren't numbered 1..{len(found)}: {numbers}")
        return 1
    problems = check_lists(found)
    for p in problems:
        print("learn_pages: " + p)
    if problems:
        return 1
    os.makedirs(out, exist_ok=True)
    for _, name, _, page in found:
        with open(os.path.join(out, name + ".md"), "w") as f:
            f.write(page.rstrip("\n") + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

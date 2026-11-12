# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""The `from:` rule for the book.

A fenced block with `<!-- from: path -->` on the line above it shows lines out
of a file the reader also has, so its text has to be a run of that file's
lines, verbatim. The path is relative to the page.

This is what the tutorial uses instead of `{{#include}}`: a chapter has to read
correctly as raw Markdown in a clone, where an include is a directive and not
the code. The copy is kept honest by checking it, not by generating it.

Usage: python3 tests/book_from.py <file.md>...
Prints one line per failing block; exits 1 if there was one.
"""

import os
import re
import sys

MARKER = re.compile(r"^<!--\s*from:\s*(\S+)\s*-->\s*$")
FENCE = re.compile(r"^(```+)")


def blocks(lines):
    """(line number of the marker, path, block lines) for each `from:` block.

    A line that starts like a marker and isn't one comes back with path
    `None`: `<!-- from: world.rk (the Room type) -->` would otherwise be
    neither checked here nor refused by the book gate's count.
    """
    i = 0
    while i < len(lines):
        m = MARKER.match(lines[i])
        if not m:
            if lines[i].lstrip().startswith("<!-- from:"):
                yield i + 1, None, None
            i += 1
            continue
        fence = FENCE.match(lines[i + 1]) if i + 1 < len(lines) else None
        if not fence:
            yield i + 1, m.group(1), None
            i += 1
            continue
        close = fence.group(1)
        body = []
        j = i + 2
        while j < len(lines) and lines[j].rstrip() != close:
            body.append(lines[j])
            j += 1
        yield i + 1, m.group(1), body
        i = j + 1


def is_run_of(body, source):
    if not body:
        return False
    n = len(body)
    return any(source[k:k + n] == body for k in range(len(source) - n + 1))


def check(md):
    fails = []
    with open(md) as f:
        lines = f.read().split("\n")
    for line, path, body in blocks(lines):
        where = f"{md}:{line}"
        if path is None:
            fails.append(f"{where}: a `from:` marker is `<!-- from: path -->` and nothing else")
            continue
        if body is None:
            fails.append(f"{where}: `from: {path}` isn't on the line above a fenced block")
            continue
        target = os.path.join(os.path.dirname(md), path)
        if not os.path.isfile(target):
            fails.append(f"{where}: `from: {path}` names a file that isn't there")
            continue
        try:
            with open(target) as f:
                source = f.read().split("\n")
        except UnicodeDecodeError:
            fails.append(f"{where}: {path} isn't UTF-8 text")
            continue
        if not is_run_of(body, source):
            fails.append(f"{where}: the block isn't a run of lines in {path}, verbatim")
    return fails


def main(paths):
    fails = [f for p in paths for f in check(p)]
    for f in fails:
        print(f"FAIL: {f}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

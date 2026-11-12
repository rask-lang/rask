# SPDX-License-Identifier: (MIT OR Apache-2.0)
"""The tutorial gate: tests/learn_gate.sh runs this.

The tutorial in docs/book/src/learn/ is one program, the MUD in mud/, and
chapters that show it. Three things there can drift from the compiler, and
this checks each:

1. Exercise files. A chapter's exercise file is the MUD with some function
   bodies replaced by `todo()`. It's generated, never written, so the
   exercise and the program it comes from can't disagree. A function is marked
   in the MUD with the task statement on the lines above it:

       // exercise 06: Find the room with this name, or none.
       public func room(self, name: string) -> Link<Room>? {

   `exercises` lists which MUD files make up each exercise file:

       06-holding-many-things.rk: world.rk start.rk

   A committed exercise file that differs from what the MUD generates is
   stale, and so is one with no line in `exercises`. Every exercise file has
   to pass `rask check`.

2. Chapters read as raw Markdown, since offline that's all a reader has. So
   no `{{#include}}`: a block that shows a file says `<!-- from: path -->`
   instead, and the book gate checks it's a verbatim run of that file.

3. The shell commands in the chapters run, in chapter order, in a copy of
   the tutorial directory, with the built `rask` first on PATH. A `bash`
   block followed by `<!-- from: output/x.out -->` and a `text` block has to
   print what that file holds; one with no output block has to exit 0. A
   block marked `<!-- learn: skip -->` isn't run (the install line).
   Durations print as `(0ms)`, so a pinned output doesn't change with the
   machine.

`--update` regenerates exercise files and pinned outputs, and rewrites an
output block in a chapter that showed the whole of the old output.

Usage: learn_gate.py [--update] <learn-dir> <rask-binary>
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile

MARK = re.compile(r"^(\s*)// exercise (\d+):")
FUNC = re.compile(r"^(\s*)(public\s+)?func\s+\w+")
FENCE = re.compile(r"^```(\w*)\s*$")
FROM = re.compile(r"^<!--\s*from:\s*(\S+)\s*-->\s*$")
SKIP = "<!-- learn: skip -->"
GENERATED = (
    "// Generated from mud/ by `tests/learn_gate.sh --update`. Edit the MUD, not this.\n"
    "// test-specs: skip: an exercise starts with its tests failing\n"
)


class Gate:
    def __init__(self, learn, rask, update):
        self.learn = learn
        self.rask = rask
        self.update = update
        self.fails = []
        self.ok = 0

    def fail(self, msg):
        self.fails.append(msg)

    # ── 1. Exercise files ─────────────────────────────────────────────────

    def manifest(self):
        path = os.path.join(self.learn, "exercises")
        if not os.path.isfile(path):
            return {}
        out = {}
        for n, line in enumerate(open(path).read().split("\n"), 1):
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            name, _, mods = line.partition(":")
            m = re.match(r"(\d+)-[\w-]+\.rk$", name.strip())
            if not m:
                self.fail(f"exercises:{n}: `{name.strip()}` isn't `NN-name.rk`")
                continue
            out[name.strip()] = (m.group(1), mods.split())
        return out

    def module(self, name):
        path = os.path.join(self.learn, "mud", name)
        if not os.path.isfile(path):
            return None
        return open(path).read().split("\n")

    def generate(self, number, modules):
        imports, bodies = [], []
        for name in modules:
            lines = self.module(name)
            if lines is None:
                self.fail(f"exercises: names mud/{name}, which isn't there")
                return None
            body = self.todo(lines, number)
            # The SPDX line and the imports go once, at the top.
            kept = []
            for line in body:
                if line.startswith("// SPDX-License-Identifier"):
                    continue
                if line.startswith("import "):
                    if line not in imports:
                        imports.append(line)
                    continue
                kept.append(line)
            while kept and not kept[0].strip():
                kept.pop(0)
            bodies.append(f"// ── mud/{name} " + "─" * max(0, 60 - len(name)) + "\n\n" + "\n".join(kept).rstrip() + "\n")
        head = "// SPDX-License-Identifier: (MIT OR Apache-2.0)\n" + GENERATED + "\n"
        if imports:
            head += "\n".join(imports) + "\n\n"
        return head + "\n".join(bodies)

    def todo(self, lines, number):
        """`lines` with the bodies of the functions marked for `number` replaced."""
        out, i = [], 0
        while i < len(lines):
            m = MARK.match(lines[i])
            if not m or m.group(2) != number:
                out.append(lines[i])
                i += 1
                continue
            # The task statement is the marker and the comments under it.
            while i < len(lines) and lines[i].strip().startswith("//"):
                out.append(lines[i])
                i += 1
            if i >= len(lines) or not FUNC.match(lines[i]) or not lines[i].rstrip().endswith("{"):
                self.fail(f"exercise {number}: a marker isn't followed by a `func … {{` line")
                return lines
            indent = FUNC.match(lines[i]).group(1)
            out.append(lines[i])
            i += 1
            while i < len(lines) and lines[i] != indent + "}":
                i += 1
            out.append(indent + "    todo()")
            out.append(indent + "}")
            i += 1
        return out

    def markers(self):
        """(number, module) for every marker in the MUD."""
        found = []
        mud = os.path.join(self.learn, "mud")
        if not os.path.isdir(mud):
            return found
        for name in sorted(os.listdir(mud)):
            if name.endswith(".rk"):
                for line in open(os.path.join(mud, name)).read().split("\n"):
                    m = MARK.match(line)
                    if m:
                        found.append((m.group(2), name))
        return found

    def exercises(self):
        manifest = self.manifest()
        for number, module in self.markers():
            if not any(n == number and module in mods for n, mods in manifest.values()):
                self.fail(f"mud/{module} marks a function for exercise {number}, and no exercise {number} in `exercises` uses {module}")
        committed = {f for f in os.listdir(self.learn) if re.match(r"\d+-[\w-]+\.rk$", f)}
        for name in sorted(committed - set(manifest)):
            self.fail(f"{name} isn't in `exercises`: delete it, or list what it's made of")
        for name, (number, modules) in sorted(manifest.items()):
            text = self.generate(number, modules)
            if text is None:
                continue
            path = os.path.join(self.learn, name)
            if self.update:
                open(path, "w").write(text)
                print(f"updated: {name}")
            elif not os.path.isfile(path) or open(path).read() != text:
                self.fail(f"{name} is stale: the MUD has changed since it was generated (run with --update)")
                continue
            r = subprocess.run([self.rask, "check", path], capture_output=True, text=True)
            if r.returncode != 0:
                errors = [l for l in (r.stdout + r.stderr).split("\n") if l.startswith("error")]
                self.fail(f"{name} doesn't pass `rask check`: {'; '.join(errors[:3])}")
                continue
            self.ok += 1

    # ── 2 and 3. Chapters ─────────────────────────────────────────────────

    def chapters(self):
        return sorted(f for f in os.listdir(self.learn) if f.endswith(".md"))

    def commands(self, md):
        """(line, script, output path or None, output block span) per bash block."""
        lines = open(os.path.join(self.learn, md)).read().split("\n")
        out, i = [], 0
        while i < len(lines):
            m = FENCE.match(lines[i])
            if not m or m.group(1) != "bash":
                i += 1
                continue
            skipped = i > 0 and lines[i - 1].strip() == SKIP
            start = i + 1
            j = start
            while j < len(lines) and lines[j].strip() != "```":
                j += 1
            script = "\n".join(lines[start:j])
            # An output block is the next thing on the page, blank lines aside.
            k = j + 1
            while k < len(lines) and not lines[k].strip():
                k += 1
            output, span = None, None
            if k + 1 < len(lines) and FROM.match(lines[k]) and FENCE.match(lines[k + 1]):
                output = FROM.match(lines[k]).group(1)
                end = k + 2
                while end < len(lines) and lines[end].strip() != "```":
                    end += 1
                span = (k + 2, end)
            if not skipped:
                out.append((i + 1, script, output, span))
            i = j + 1
        return out

    def run_chapters(self):
        for md in self.chapters():
            text = open(os.path.join(self.learn, md)).read()
            if re.search(r"(?<!\\){{#include", text):
                self.fail(f"{md} uses {{{{#include}}}}, which a reader of the raw file sees as a directive: show the code with `<!-- from: path -->` instead")
        work = tempfile.mkdtemp(prefix="learn_gate_")
        try:
            copy = os.path.join(work, "learn")
            shutil.copytree(self.learn, copy, ignore=shutil.ignore_patterns("build"))
            bindir = os.path.join(work, "bin")
            os.mkdir(bindir)
            os.symlink(os.path.abspath(self.rask), os.path.join(bindir, "rask"))
            env = dict(os.environ, PATH=bindir + os.pathsep + os.environ.get("PATH", ""), NO_COLOR="1")
            for md in self.chapters():
                for line, script, output, span in self.commands(md):
                    self.run_one(md, line, script, output, span, copy, env)
        finally:
            shutil.rmtree(work, ignore_errors=True)

    def run_one(self, md, line, script, output, span, copy, env):
        r = subprocess.run(["bash", "-o", "pipefail", "-c", script], cwd=copy, env=env,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=600)
        got = r.stdout.replace(copy + "/", "").replace(copy, ".")
        got = re.sub(r"\(\d+(\.\d+)?m?s\)", "(0ms)", got)
        got = "\n".join(l.rstrip() for l in got.rstrip().split("\n")) + "\n"
        where = f"{md}:{line}"
        if output is None:
            if r.returncode != 0:
                self.fail(f"{where}: the command exits {r.returncode}, and no output block says it should fail:\n" + indent(got))
            else:
                self.ok += 1
            return
        pinned = os.path.join(self.learn, output)
        if self.update:
            self.rewrite(md, span, pinned, got)
            os.makedirs(os.path.dirname(pinned), exist_ok=True)
            open(pinned, "w").write(got)
            print(f"updated: {output}")
            self.ok += 1
            return
        if not os.path.isfile(pinned):
            self.fail(f"{where}: {output} isn't there (run with --update)")
        elif open(pinned).read() != got:
            self.fail(f"{where}: the command printed something else than {output}:\n" + indent(got))
        else:
            self.ok += 1

    def rewrite(self, md, span, pinned, new):
        """Put the new output in the chapter, when the block showed all of the old."""
        if not os.path.isfile(pinned):
            return
        path = os.path.join(self.learn, md)
        lines = open(path).read().split("\n")
        start, end = span
        old = open(pinned).read().rstrip("\n").split("\n")
        if lines[start:end] != old:
            return
        lines[start:end] = new.rstrip("\n").split("\n")
        open(path, "w").write("\n".join(lines))


def indent(text):
    return "\n".join("    " + l for l in text.rstrip().split("\n")[:20])


def main(argv):
    update = "--update" in argv
    args = [a for a in argv if a != "--update"]
    if len(args) != 2:
        print(__doc__.strip().split("\n")[-1], file=sys.stderr)
        return 2
    gate = Gate(args[0], args[1], update)
    gate.exercises()
    gate.run_chapters()
    for f in gate.fails:
        print(f"FAIL: {f}")
    print("──────────────────────────────────────────────────")
    print(f"learn gate: {gate.ok} ok, {len(gate.fails)} failed")
    return 1 if gate.fails else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Book gate.
#
# A book is a second copy of the language, and a second copy a human keeps in
# step is rot with a delay fuse.
#
# The "Docs snippets parse" CI step already ran the book's inline blocks, but
# parsing is a weak signal and the step's own comment says so. The front page
# parsed fine for months while calling `fs.open`, which the real grep program
# doesn't use, with both of its imports missing: `rask check` gives two errors
# on the language's own first page. Parsing proves the syntax is current, and
# syntax is not what rots.
#
# This gate adds what parsing can't see:
#
#  1. Code. Every ```rask block is either an {{#include}} out of a program
#     another gate already runs, or an inline block carrying a test-specs
#     marker that at least type-checks (`compile`, `run | expected`). For
#     includes this checks the file exists and the named ANCHOR/ANCHOR_END
#     pair is really in it, because mdBook renders a missing anchor as nothing
#     and says so quietly.
#
#     `parse` does not count here. The docs step runs whatever marker a block
#     carries; this gate is what stops a block from carrying none, or from
#     carrying one too weak to catch an API that moved.
#
#     A block on a tour page carries `<!-- tour: ... -->`. Those pages are
#     made from the tour's step files at build time (docs/book/learn_pages.py),
#     and tests/tour_gate.sh runs every step and its solution.
#
# Usage:  tests/book_gate.sh
# Exit:   0 = everything resolves, 1 = a block nothing verifies, 2 = no binary.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BOOK_SRC="$ROOT/docs/book/src"
# The long-form posts left the book but still carry rask blocks.
WRITING_SRC="$ROOT/writing"

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi

fails=0
ok=0

# ── 1. Every {{#include}} resolves, anchors included ──────────────────────────

while IFS= read -r md; do
    base="$(dirname "$md")"
    # One include per line is the convention; more than one on a line still works.
    # A backslash-escaped `\{{#include ...}}` is mdBook's way of showing the
    # syntax without running it, so strip those before looking for real ones:
    # the page documenting this file's rules is full of them.
    sed 's/\\{{#include [^}]*}}//g' "$md" \
        | grep -o '{{#include [^}]*}}' 2>/dev/null | while read -r _ spec; do
        spec="${spec%\}\}}"
        path="${spec%%:*}"
        anchor=""
        case "$spec" in *:*) anchor="${spec#*:}" ;; esac
        target="$base/$path"

        if [ ! -f "$target" ]; then
            echo "FAIL: ${md#$ROOT/} includes a file that isn't there: $path"
            continue
        fi
        if [ -n "$anchor" ]; then
            if ! grep -q "ANCHOR: *$anchor\b" "$target" || \
               ! grep -q "ANCHOR_END: *$anchor\b" "$target"; then
                echo "FAIL: ${md#$ROOT/} includes anchor '$anchor', not in ${path}"
            fi
        fi
    done
done < <(find "$BOOK_SRC" "$WRITING_SRC" -name '*.md') > "$ROOT/.book_gate_includes" 2>/dev/null

if [ -s "$ROOT/.book_gate_includes" ]; then
    cat "$ROOT/.book_gate_includes"
    fails=$((fails + $(wc -l < "$ROOT/.book_gate_includes")))
else
    ok=$((ok + 1))
fi
rm -f "$ROOT/.book_gate_includes"

# ── 2. Every rask block is verified by something ──────────────────────────────
#
# An include is verified by whatever gate runs the program it points at. An
# inline block is verified by its test-specs marker, but only at `compile` or
# `run`; `parse` and `skip` leave it unchecked, which is how a snippet with
# missing imports sat on the front page.

while IFS= read -r md; do
    rel="${md#$ROOT/}"

    # A tour page is made from a step (docs/book/learn_pages.py), and the step
    # is what tests/tour_gate.sh runs. Only such a page may use `tour:`, so the
    # marker can't wave a hand-written block through.
    tour=0
    case "$md" in
        "$BOOK_SRC"/learn/[0-9]*.md)
            step="${md%.md}"
            if [ -f "$step.rk" ] || [ -d "$step" ]; then tour=1; fi ;;
    esac

    # For each ```rask fence: an include on the first body line is fine; so is
    # a compile/run marker on the line above the fence. Anything else is loose.
    loose=$(awk -v tour="$tour" '
        /^<!-- test:/ { marker = $0; next }
        /^<!-- tour: *[a-z]+ *-->[ \t]*$/ { marker = $0; next }
        /^```rask/ {
            verified = 0
            if (marker ~ /test: *(compile|run|run-interp)/) verified = 1
            if (tour == 1 && marker ~ /^<!-- tour:/) verified = 1
            infence = 1
            marker = ""
            next
        }
        # A ```text block is compiler or program output. Typing it by hand is
        # how a page ends up quoting a diagnostic nobody emits any more, so the
        # only accepted form is an include of a golden.
        /^```text/ {
            verified = 0
            infence = 1
            marker = ""
            next
        }
        infence == 1 {
            if ($0 ~ /\{\{#include/) verified = 1
            if (verified == 0) { count++; printf "    line %d\n", NR > "/dev/stderr" }
            infence = 0
            next
        }
        { marker = "" }
        END { print count + 0 }
    ' "$md" 2>/dev/null)

    if [ "$loose" -gt 0 ]; then
        echo "FAIL: $rel has $loose unverified block(s). Code: use {{#include}}, or mark it \`<!-- test: compile -->\` / \`<!-- test: run | output -->\`. Output: {{#include}} a golden"
        fails=$((fails + 1))
    else
        ok=$((ok + 1))
    fi
done < <(find "$BOOK_SRC" "$WRITING_SRC" -name '*.md')

# ── 3. The front page's code runs ─────────────────────────────────────────────
#
# The landing page shows code in two places the book's checks never reach: the
# static examples in its HTML and the program the Try box opens with. When value
# literals moved from braces to parens, both stopped compiling and nothing said
# so. Each one has to run on the interpreter, which is what the Try box uses.

LANDING_DIR="$(mktemp -d)"
python3 - "$ROOT/docs/landing" "$LANDING_DIR" <<'PY'
import html, re, sys
src, out = sys.argv[1], sys.argv[2]
page = open(f"{src}/index.html").read()
for n, block in enumerate(re.findall(r"<pre><code>(.*?)</code></pre>", page, re.S)):
    open(f"{out}/index_{n}.rk", "w").write(html.unescape(re.sub(r"<[^>]+>", "", block)))
script = open(f"{src}/try.js").read()
m = re.search(r"const SNIPPET = `(.*?)`;", script, re.S)
if m:
    open(f"{out}/try_box.rk", "w").write(m.group(1))
PY
for rk in "$LANDING_DIR"/*.rk; do
    [ -e "$rk" ] || continue
    if ! "$RASK" run --interp "$rk" > "$LANDING_DIR/out" 2>&1; then
        echo "FAIL: docs/landing: $(basename "$rk" .rk) doesn't run"
        sed 's/^/    /' "$LANDING_DIR/out" | head -12
        fails=$((fails + 1))
    else
        ok=$((ok + 1))
    fi
done
rm -rf "$LANDING_DIR"

echo "──────────────────────────────────────────────────"
if [ $fails -eq 0 ]; then
    echo "book gate: $ok ok, 0 failed"
    exit 0
fi
echo "book gate: $ok ok, $fails failed"
exit 1

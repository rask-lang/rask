#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Book gate.
#
# A book is a second copy of the language, and a second copy a human keeps in
# step is rot with a delay fuse. Nothing checked docs/book at all: test-specs
# covers specs/, and the docs workflow only builds mdBook and deploys. So a
# chapter could quote an API that no longer exists, or a diagnostic whose
# wording changed three releases ago, and stay green forever.
#
# This gate removes the two ways a chapter can drift from the compiler:
#
#  1. Code. Every ```rask block is either an {{#include}} out of a program
#     another gate already runs, or an inline block carrying a test-specs
#     marker that at least type-checks (`compile`, `run | expected`). For
#     includes this checks the file exists and the named ANCHOR/ANCHOR_END
#     pair is really in it — mdBook renders a missing anchor as nothing and
#     says so quietly.
#
#     `parse` does not count. Parsing proves the syntax is current, and syntax
#     is not what rots: the front page called `fs.open` with no imports and
#     passed `test: parse` the whole time. `rask test-specs docs/book/src`
#     runs the markers; this gate is what stops a block having none.
#
#  2. Diagnostics. Chapters teach with real compiler errors, which means the
#     error text is content and has to be pinned like any other output. Each
#     docs/book/errors/<chapter>/<case>.rk is a program that MUST NOT compile;
#     its committed .out is what `rask check` prints. A wording change shows up
#     as a book diff in review, which is the point — improving a message should
#     make you look at the page teaching it.
#
#     A case that starts compiling is a hard failure, not a stale golden: the
#     chapter is claiming the compiler rejects something it now accepts.
#
# Regenerate the renderings after a deliberate diagnostics change:
#
#     tests/book_gate.sh --update
#
# Usage:  tests/book_gate.sh [--update]
# Exit:   0 = everything resolves and matches, 1 = a mismatch, 2 = no binary.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BOOK_SRC="$ROOT/docs/book/src"
ERRORS_DIR="$ROOT/docs/book/errors"

UPDATE=0
if [ "${1:-}" = "--update" ]; then
    UPDATE=1
fi

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
    grep -o '{{#include [^}]*}}' "$md" 2>/dev/null | while read -r _ spec; do
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
done < <(find "$BOOK_SRC" -name '*.md') > "$ROOT/.book_gate_includes" 2>/dev/null

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
# `run` — `parse` and `skip` leave it unchecked, which is how a snippet with
# missing imports sat on the front page.

while IFS= read -r md; do
    rel="${md#$ROOT/}"

    # For each ```rask fence: an include on the first body line is fine; so is
    # a compile/run marker on the line above the fence. Anything else is loose.
    loose=$(awk '
        /^<!-- test:/ { marker = $0; next }
        /^```rask/ {
            verified = 0
            if (marker ~ /test: *(compile|run|run-interp)/) verified = 1
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
        echo "FAIL: $rel has $loose unverified rask block(s) — use {{#include}}, or mark the block \`<!-- test: compile -->\` / \`<!-- test: run | output -->\`"
        fails=$((fails + 1))
    else
        ok=$((ok + 1))
    fi
done < <(find "$BOOK_SRC" -name '*.md')

# ── 3. Error renderings match what the compiler prints ────────────────────────

if [ -d "$ERRORS_DIR" ]; then
    while IFS= read -r rk; do
        rel="${rk#$ROOT/}"
        out="${rk%.rk}.out"

        # Run from ROOT with a relative path so the rendering has no absolute
        # paths in it and is identical on every machine.
        actual="$(cd "$ROOT" && "$RASK" check "$rel" 2>&1)"
        status=$?

        if [ $status -eq 0 ]; then
            echo "FAIL: $rel compiles — the chapter says it shouldn't"
            fails=$((fails + 1))
            continue
        fi

        if [ $UPDATE -eq 1 ]; then
            printf '%s\n' "$actual" > "$out"
            echo "updated: ${out#$ROOT/}"
            ok=$((ok + 1))
            continue
        fi

        if [ ! -f "$out" ]; then
            echo "FAIL: $rel has no committed rendering (run with --update)"
            fails=$((fails + 1))
            continue
        fi

        if ! printf '%s\n' "$actual" | diff -q - "$out" > /dev/null; then
            echo "FAIL: $rel rendering changed:"
            printf '%s\n' "$actual" | diff "$out" - | sed 's/^/    /'
            fails=$((fails + 1))
        else
            ok=$((ok + 1))
        fi
    done < <(find "$ERRORS_DIR" -name '*.rk' | sort)
fi

echo "──────────────────────────────────────────────────"
if [ $fails -eq 0 ]; then
    echo "book gate: $ok ok, 0 failed"
    exit 0
fi
echo "book gate: $ok ok, $fails failed"
exit 1

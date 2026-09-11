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
#  1. Code. Chapters don't contain Rask — they `{{#include}}` it out of a
#     program that other gates already run. This checks every include
#     resolves: the file exists, and the named ANCHOR/ANCHOR_END pair is
#     actually in it. An include that silently renders nothing is the failure
#     mode this catches.
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
LEGACY_FILE="$ROOT/tests/book_legacy_snippets.txt"

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

# ── 2. Chapters carry no literal Rask ─────────────────────────────────────────
#
# A ```rask fence whose body isn't an include is a hand-copied snippet — the
# thing this gate exists to prevent. Chapters written before the gate are
# listed in book_legacy_snippets.txt with a count that may only go down.

# Returns the recorded count for a legacy chapter, or empty if it isn't listed.
legacy_budget() {
    [ -f "$LEGACY_FILE" ] || return 0
    awk -v want="$1" '$1 == want { print $2; exit }' "$LEGACY_FILE"
}

while IFS= read -r md; do
    rel="${md#$ROOT/}"
    # Count ```rask fences whose next line is not an include.
    literal=$(awk '
        /^```rask/ { infence = 1; next }
        infence == 1 {
            if ($0 !~ /\{\{#include/) count++
            infence = 0
        }
        END { print count + 0 }
    ' "$md")

    budget="$(legacy_budget "$rel")"
    if [ -z "$budget" ]; then
        budget=0
    fi

    if [ "$literal" -gt "$budget" ]; then
        echo "FAIL: $rel has $literal hand-copied rask block(s), budget $budget — use {{#include}}"
        fails=$((fails + 1))
    elif [ "$literal" -lt "$budget" ]; then
        echo "FAIL: $rel is down to $literal hand-copied block(s) from $budget — lower it in ${LEGACY_FILE#$ROOT/}"
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

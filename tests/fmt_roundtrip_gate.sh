#!/bin/bash
# The formatter's output has to still be the same program (#805).
#
# `rask fmt` used to drop `as` bindings, `using` clauses, enum and trait
# attributes, the `duck` modifier, and every parenthesis that mattered —
# `(a - b).as_nanos()` came out as `a - b.as_nanos()` and `!(x < y)` as `!x < y`.
# 21 of 30 examples stopped compiling after being formatted, silently, because
# nothing compared the two.
#
# For each file that checks today: format it, check the result. A file that
# doesn't check to begin with is skipped — this gate is about what formatting
# changes, not about what was already broken.
#
# Self-contained files are formatted into a temp file. Package members are
# formatted inside a copy of their package, because a stray extra module in the
# real directory would fail for its own reasons.

set -u
cd "$(dirname "$0")/.." || exit 1
RASK=./compiler/target/release/rask
if [ ! -x "$RASK" ]; then
    echo "no rask binary at $RASK — build with: cd compiler && cargo build --release -p rask-cli"
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

checked=0
broken=0

check_one() {
    local src="$1" out="$2"
    if ! "$RASK" fmt "$src" > "$out" 2>/dev/null; then
        # A file that doesn't parse is reported by fmt itself and has nothing to
        # round-trip.
        return 0
    fi
    if ! "$RASK" check "$out" > "$TMP/err.log" 2>&1; then
        broken=$((broken + 1))
        echo "BROKEN $src"
        echo "       $(grep -m1 '^error' "$TMP/err.log")"
    fi
}

# --- Self-contained files ---
for f in tests/suite/*.rk tests/compile_errors/*.rk examples/*.rk stdlib/*.rk; do
    [ -f "$f" ] || continue
    "$RASK" check "$f" > /dev/null 2>&1 || continue
    checked=$((checked + 1))
    check_one "$f" "$TMP/one.rk"
done

# --- Packages: format a copy in place, then check the package ---
for pkg in projects/raido projects/tiwaz examples/lsm_database examples/validation; do
    [ -d "$pkg" ] || continue
    "$RASK" check "$pkg" > /dev/null 2>&1 || continue
    name=$(basename "$pkg")
    rm -rf "$TMP/$name"
    cp -r "$pkg" "$TMP/$name" || continue
    n=0
    while IFS= read -r f; do
        "$RASK" fmt -w "$f" > /dev/null 2>&1
        n=$((n + 1))
    done < <(find "$TMP/$name" -name '*.rk')
    checked=$((checked + n))
    if ! "$RASK" check "$TMP/$name" > "$TMP/err.log" 2>&1; then
        broken=$((broken + 1))
        echo "BROKEN $pkg (as a package, after formatting all $n files)"
        echo "       $(grep -m1 '^error' "$TMP/err.log")"
    fi
done

# --- Every .rk file: the output has to parse, and formatting it again has to be
# --- a no-op. This catches files the check pass skips because they don't compile
# --- standalone — `Receiver<void>` came back out as `Receiver<void>`'s internal
# --- spelling `Receiver<()>`, which doesn't parse, and only stdlib/time.rk
# --- showed it.
unstable=0
parsed=0
# The intermediate keeps the file's relative path: a stdlib stub is parsed with
# the keyword-name allowance the stub loader uses, and that is decided by the
# path. Written to a flat `once.rk` the second pass lost the allowance and
# `stdlib/builtins.rk` — which declares `assert` and `print` — failed to parse.
for f in $(find stdlib examples tests projects -name '*.rk' 2>/dev/null); do
    once="$TMP/once/$f"
    twice="$TMP/twice/$f"
    mkdir -p "$(dirname "$once")" "$(dirname "$twice")"
    "$RASK" fmt "$f" > "$once" 2>/dev/null || continue
    parsed=$((parsed + 1))
    if ! "$RASK" fmt "$once" > "$twice" 2>"$TMP/err.log"; then
        unstable=$((unstable + 1))
        echo "UNPARSEABLE OUTPUT $f"
        echo "       $(grep -m1 '^error' "$TMP/err.log")"
        continue
    fi
    if ! cmp -s "$once" "$twice"; then
        unstable=$((unstable + 1))
        echo "NOT IDEMPOTENT $f"
        diff "$once" "$twice" | head -6 | sed 's/^/       /'
    fi
done

# --- `fmt --check` over the tree the formatter owns.
# --- stdlib/ and examples/ are kept formatted, so `--check` is a gate rather
# --- than a wish. tests/ and projects/ are not: their files carry deliberate
# --- layout that a reformat would churn for no gain.
dirty=0
if ! "$RASK" fmt --check stdlib/ > "$TMP/check.log" 2>&1; then
    dirty=$((dirty + $(grep -c '✗' "$TMP/check.log")))
    grep '✗' "$TMP/check.log" | sed 's/^/       /'
fi
if ! "$RASK" fmt --check examples/ > "$TMP/check.log" 2>&1; then
    dirty=$((dirty + $(grep -c '✗' "$TMP/check.log")))
    grep '✗' "$TMP/check.log" | sed 's/^/       /'
fi

echo "──────────────────────────────────────────────────"
echo "fmt round-trip: $checked files formatted, $broken still-compiles failures"
echo "fmt stability:  $parsed files reformatted, $unstable parse/idempotence failures"
echo "fmt --check:    stdlib/ and examples/, $dirty files not formatted"
[ "$broken" -eq 0 ] && [ "$unstable" -eq 0 ] && [ "$dirty" -eq 0 ]

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
#
# Every phase fans out across cores: each file is its own `rask` run and none of
# them look at each other. Workers write their verdicts to files and the loops
# below read them back in list order, so the report reads the same as it did
# when this ran one file at a time. FMT_JOBS sets the width.

set -u
cd "$(dirname "$0")/.." || exit 1
RASK=./compiler/target/release/rask
source tests/lib/fanout.sh
if [ ! -x "$RASK" ]; then
    echo "no rask binary at $RASK — build with: cd compiler && cargo build --release -p rask-cli"
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/r" "$TMP/f" "$TMP/p" "$TMP/s"

JOBS="${FMT_JOBS:-${JOBS:-$(nproc 2>/dev/null || echo 4)}}"
export RASK TMP
export -f slot

checked=0
broken=0

# A worker that dies before writing its verdict — killed, out of memory, a rask
# that hung — leaves no result file. The differential harness and the leak gate
# both treat that as a failure, and this gate used not to: an empty verdict fell
# through to "not broken" and the file was counted as having passed. A
# regression in a file whose worker died would have vanished from the report
# instead of failing the gate.
died=0
dead=()

# --- Self-contained files ---
# Verdict file: `skip` (didn't check before formatting either), `checked`, or
# `broken` followed by the first error line.
roundtrip_one() {
    local f="$1" s out
    s="$(slot "$f")"
    if ! "$RASK" check "$f" > /dev/null 2>&1; then
        echo skip > "$TMP/r/$s"
        return 0
    fi
    out="$TMP/f/$s"
    # A file that doesn't parse is reported by fmt itself and has nothing to
    # round-trip.
    if ! "$RASK" fmt "$f" > "$out" 2>/dev/null; then
        echo checked > "$TMP/r/$s"
        return 0
    fi
    if "$RASK" check "$out" > "$TMP/r/$s.err" 2>&1; then
        echo checked > "$TMP/r/$s"
    else
        { echo broken; grep -m1 '^error' "$TMP/r/$s.err"; } > "$TMP/r/$s"
    fi
    return 0
}

selfcontained=()
for f in tests/suite/*.rk tests/compile_errors/*.rk examples/*.rk stdlib/*.rk; do
    [ -f "$f" ] && selfcontained+=("$f")
done
fan_out roundtrip_one "${selfcontained[@]}"

for f in "${selfcontained[@]}"; do
    res="$TMP/r/$(slot "$f")"
    case "$(sed -n 1p "$res" 2>/dev/null)" in
        skip) ;;
        checked) checked=$((checked + 1)) ;;
        broken)
            checked=$((checked + 1))
            broken=$((broken + 1))
            echo "BROKEN $f"
            echo "       $(sed -n 2p "$res")" ;;
        *)
            died=$((died + 1))
            dead+=("$f (round-trip)") ;;
    esac
done

# --- Packages: format a copy in place, then check the package ---
package_one() {
    local pkg="$1" name n=0 f
    name="$(basename "$pkg")"
    if ! "$RASK" check "$pkg" > /dev/null 2>&1; then
        echo "skip 0" > "$TMP/p/$name"
        return 0
    fi
    rm -rf "$TMP/p/$name.d"
    if ! cp -r "$pkg" "$TMP/p/$name.d"; then
        echo "skip 0" > "$TMP/p/$name"
        return 0
    fi
    while IFS= read -r f; do
        "$RASK" fmt -w "$f" > /dev/null 2>&1
        n=$((n + 1))
    done < <(find "$TMP/p/$name.d" -name '*.rk')
    if "$RASK" check "$TMP/p/$name.d" > "$TMP/p/$name.err" 2>&1; then
        echo "ok $n" > "$TMP/p/$name"
    else
        { echo "broken $n"; grep -m1 '^error' "$TMP/p/$name.err"; } > "$TMP/p/$name"
    fi
    return 0
}

packages=()
for pkg in projects/raido projects/tiwaz examples/lsm_database examples/validation; do
    [ -d "$pkg" ] && packages+=("$pkg")
done
[ "${#packages[@]}" -gt 0 ] && fan_out package_one "${packages[@]}"

for pkg in "${packages[@]}"; do
    res="$TMP/p/$(basename "$pkg")"
    if [ ! -f "$res" ]; then
        died=$((died + 1))
        dead+=("$pkg (package)")
        continue
    fi
    read -r verdict n < "$res"
    case "$verdict" in
        skip) ;;
        ok) checked=$((checked + n)) ;;
        broken)
            checked=$((checked + n))
            broken=$((broken + 1))
            echo "BROKEN $pkg (as a package, after formatting all $n files)"
            echo "       $(sed -n 2p "$res")" ;;
        *)
            died=$((died + 1))
            dead+=("$pkg (package)") ;;
    esac
done

# --- Every .rk file: the output has to parse, and formatting it again has to be
# --- a no-op. This catches files the check pass skips because they don't compile
# --- standalone — `Receiver<void>` came back out as `Receiver<void>`'s internal
# --- spelling `Receiver<()>`, which doesn't parse, and only stdlib/time.rk
# --- showed it.
unstable=0
parsed=0
# Files the formatter can't read at all. Most are tests/compile_errors/*.rk,
# which exist to be rejected — but the skip was `|| continue` with no counter
# and no name, so a file that newly stopped formatting left no trace and the
# gate's "N files reformatted" quietly went down by one.
unformattable=0
skipped=()

# The intermediate keeps the file's relative path: a stdlib stub is parsed with
# the keyword-name allowance the stub loader uses, and that is decided by the
# path. Written to a flat `once.rk` the second pass lost the allowance and
# `stdlib/builtins.rk` — which declares `assert` and `print` — failed to parse.
stability_one() {
    local f="$1" s once twice
    s="$(slot "$f")"
    once="$TMP/once/$f"
    twice="$TMP/twice/$f"
    mkdir -p "$(dirname "$once")" "$(dirname "$twice")"
    if ! "$RASK" fmt "$f" > "$once" 2>/dev/null; then
        echo unformattable > "$TMP/s/$s"
        return 0
    fi
    if ! "$RASK" fmt "$once" > "$twice" 2>"$TMP/s/$s.err"; then
        { echo unparseable; grep -m1 '^error' "$TMP/s/$s.err"; } > "$TMP/s/$s"
        return 0
    fi
    if cmp -s "$once" "$twice"; then
        echo stable > "$TMP/s/$s"
    else
        { echo unstable; diff "$once" "$twice" | head -6; } > "$TMP/s/$s"
    fi
    return 0
}

mapfile -t allfiles < <(find stdlib examples tests projects -name '*.rk' 2>/dev/null)
[ "${#allfiles[@]}" -gt 0 ] && fan_out stability_one "${allfiles[@]}"

for f in "${allfiles[@]}"; do
    res="$TMP/s/$(slot "$f")"
    case "$(sed -n 1p "$res" 2>/dev/null)" in
        unformattable)
            unformattable=$((unformattable + 1))
            skipped+=("$f") ;;
        unparseable)
            parsed=$((parsed + 1))
            unstable=$((unstable + 1))
            echo "UNPARSEABLE OUTPUT $f"
            echo "       $(sed -n 2p "$res")" ;;
        unstable)
            parsed=$((parsed + 1))
            unstable=$((unstable + 1))
            echo "NOT IDEMPOTENT $f"
            sed -n '2,$p' "$res" | sed 's/^/       /' ;;
        stable)
            parsed=$((parsed + 1)) ;;
        *)
            died=$((died + 1))
            dead+=("$f (stability)") ;;
    esac
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
if [ "$unformattable" -gt 0 ]; then
    echo "fmt skipped:    $unformattable files the formatter can't read —"
    for f in "${skipped[@]:-}"; do
        [ -n "$f" ] && echo "                  $f"
    done
fi
echo "fmt --check:    stdlib/ and examples/, $dirty files not formatted"
if [ "$died" -gt 0 ]; then
    echo "NO VERDICT:     $died worker(s) died before writing a result —"
    for f in "${dead[@]:-}"; do
        [ -n "$f" ] && echo "                  $f"
    done
fi
[ "$broken" -eq 0 ] && [ "$unstable" -eq 0 ] && [ "$dirty" -eq 0 ] && [ "$died" -eq 0 ]

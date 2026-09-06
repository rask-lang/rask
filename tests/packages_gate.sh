#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Multi-package build gate.
#
# Nothing in the repo declared a dependency. `grep -rn 'dep "'` across projects/,
# examples/ and tests/ found no hits, so `rask build` across a package boundary
# had no coverage at all — which is how #1112 got in: a library that exports a
# function returning its own struct can't be consumed, and *every* consumer
# fails, whether or not it calls that function.
#
# Each fixture in tests/packages/ is a `libpkg/` and an `app/` that depends on it
# by path. This builds `app/`, runs it, and diffs against `expected.txt`.
#
# A fixture that isn't expected to build goes in tests/known_fail_packages.txt
# with its tracking issue — same shape as tests/known_fail_examples.txt. One that
# starts working is reported so the line can be deleted.
#
# Usage:  tests/packages_gate.sh
# Exit:   0 = every fixture builds and matches (or is listed), 1 = otherwise.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG_DIR="$ROOT/tests/packages"
KNOWN_FAIL="$ROOT/tests/known_fail_packages.txt"

if [ -x "$ROOT/compiler/target/release/rask" ]; then
    RASK="$ROOT/compiler/target/release/rask"
elif [ -x "$ROOT/compiler/target/debug/rask" ]; then
    RASK="$ROOT/compiler/target/debug/rask"
else
    echo "error: rask binary not found; build with 'cargo build --release -p rask-cli'" >&2
    exit 2
fi
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

known_fail() {
    [ -f "$KNOWN_FAIL" ] || return 1
    grep -qE "^$1([[:space:]]|#|$)" "$KNOWN_FAIL"
}

ok=0
failed=0
expected=0
fixed=()
failures=()

for app in "$PKG_DIR"/*/app; do
    [ -d "$app" ] || continue
    name="$(basename "$(dirname "$app")")"
    expected_out="$(dirname "$app")/expected.txt"

    rm -rf "$app/build"
    build_out="$(cd "$app" && "$RASK" build 2>&1)"
    build_rc=$?

    got=""
    run_rc=1
    if [ $build_rc -eq 0 ]; then
        # The binary takes the package's name, which is the app directory's.
        bin="$app/build/debug/$(basename "$app")"
        if [ -x "$bin" ]; then
            got="$("$bin" 2>&1)"
            run_rc=$?
        fi
    fi

    if [ $build_rc -ne 0 ] || [ $run_rc -ne 0 ]; then
        if known_fail "$name"; then
            expected=$((expected + 1))
        else
            failed=$((failed + 1))
            detail="$(echo "$build_out" | grep -m1 'error' || echo "exit $build_rc")"
            failures+=("$name — $detail")
        fi
        continue
    fi

    if [ ! -f "$expected_out" ]; then
        failed=$((failed + 1))
        failures+=("$name — no expected.txt")
        continue
    fi

    if [ "$got" = "$(cat "$expected_out")" ]; then
        ok=$((ok + 1))
        if known_fail "$name"; then
            fixed+=("$name")
        fi
    else
        failed=$((failed + 1))
        failures+=("$name — output differs from expected.txt")
    fi
done

echo "──────────────────────────────────────────────────"
for f in "${failures[@]-}"; do
    [ -n "$f" ] && echo "FAIL: $f"
done
for f in "${fixed[@]-}"; do
    [ -n "$f" ] && echo "FIXED (drop its line from known_fail_packages.txt): $f"
done
echo "packages gate: $ok ok, $failed failed, $expected known-fail"

if [ ${#fixed[@]} -gt 0 ] && [ -n "${fixed[0]-}" ]; then
    exit 1
fi
[ "$failed" -eq 0 ]

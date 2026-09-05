#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Flagship validation program: examples/http_api_server.rk.
#
# The examples gate can't hold this one — it never exits, so there's no stdout
# to diff. This harness does the equivalent: start the server, drive a fixed
# request sequence, compare the responses against tests/golden/http_api.expected,
# shut it down.
#
# Both backends are checked, because that's the divergence this catches.
# Responses are normalised before the diff: uptime_ms is wall-clock, so the
# value is replaced with a placeholder while the field itself stays asserted.
#
# Usage:  tests/http_api_harness.sh [--backend native|interp|both] [--bless]
# Exit:   0 = every response matched, 1 = a mismatch, a missing golden, or the
#         server didn't come up.
#
# A missing golden is a failure, not a licence to write one. It used to copy the
# backend's own output into place and pass, which means a deleted golden would
# be silently re-created from whatever the compiler did that day. `--bless`
# writes it deliberately.

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RASK="$ROOT/compiler/target/release/rask"
SRC="$ROOT/examples/http_api_server.rk"
EXPECTED="$ROOT/tests/golden/http_api.expected"
PORT=8080
BASE="http://127.0.0.1:$PORT"

[ -x "$RASK" ] || { echo "error: build with 'cargo build --release -p rask-cli'" >&2; exit 2; }
export RASK_RUNTIME_DIR="${RASK_RUNTIME_DIR:-$ROOT/compiler/runtime}"

WANT_BACKEND=both
BLESS=0
while [ $# -gt 0 ]; do
    case "$1" in
        --backend) WANT_BACKEND="${2:-both}"; shift 2 ;;
        --bless)   BLESS=1; shift ;;
        *) echo "usage: $0 [--backend native|interp|both] [--bless]" >&2; exit 2 ;;
    esac
done

WORK="$(mktemp -d)"
SERVER_PID=""
cleanup() {
    [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

# The request sequence. One per line: METHOD PATH [BODY].
# Ordered so later requests see the effects of earlier ones.
requests() {
    cat <<'REQ'
GET /health
GET /users
POST /users {"name":"Ada","email":"ada@example.com"}
GET /users
GET /users/1
GET /users/999
DELETE /users/1
GET /users
GET /nope
REQ
}

# Wall-clock values can't be goldened; keep the field, drop the number.
normalise() {
    sed -E 's/"uptime_ms":[0-9]+/"uptime_ms":<N>/g'
}

drive() {
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        method="${line%% *}"
        rest="${line#* }"
        path="${rest%% *}"
        body=""
        case "$rest" in *" "*) body="${rest#* }" ;; esac

        printf '=== %s %s\n' "$method" "$path"
        if [ -n "$body" ]; then
            curl -s -m 5 -X "$method" -H 'Content-Type: application/json' \
                 -d "$body" "$BASE$path"
        else
            curl -s -m 5 -X "$method" "$BASE$path"
        fi
        printf '\n'
    done
}

run_backend() {
    backend="$1"
    out="$WORK/$backend.out"

    if [ "$backend" = native ]; then
        "$RASK" compile "$SRC" -o "$WORK/server" >/dev/null 2>&1 \
            || { echo "FAIL: $backend — compile failed"; return 1; }
        "$WORK/server" >"$WORK/$backend.log" 2>&1 &
    else
        "$RASK" run --interp "$SRC" >"$WORK/$backend.log" 2>&1 &
    fi
    SERVER_PID=$!

    # Wait for the port rather than sleeping a fixed amount.
    ready=0
    for _ in $(seq 1 50); do
        if curl -s -m 1 "$BASE/health" >/dev/null 2>&1; then ready=1; break; fi
        sleep 0.2
    done
    if [ "$ready" -ne 1 ]; then
        echo "FAIL: $backend — server never answered on :$PORT"
        sed 's/^/    /' "$WORK/$backend.log" | head -5
        kill -9 "$SERVER_PID" 2>/dev/null; SERVER_PID=""
        return 1
    fi

    requests | drive | normalise > "$out"

    kill -9 "$SERVER_PID" 2>/dev/null; SERVER_PID=""
    wait "$SERVER_PID" 2>/dev/null
    sleep 0.3

    if [ ! -f "$EXPECTED" ]; then
        if [ "$BLESS" -eq 1 ]; then
            echo "note: --bless — writing $EXPECTED from $backend"
            cp "$out" "$EXPECTED"
            return 0
        fi
        echo "FAIL: $backend — no golden at $EXPECTED"
        echo "    a gate that writes its own answer verifies nothing: run with"
        echo "    --bless once, read the file, and commit it."
        return 1
    fi
    if diff -u "$EXPECTED" "$out" > "$WORK/$backend.diff"; then
        echo "ok: $backend"
        return 0
    fi
    echo "FAIL: $backend — responses differ from golden"
    head -20 "$WORK/$backend.diff" | sed 's/^/    /'
    return 1
}

rc=0
case "$WANT_BACKEND" in
    native) run_backend native || rc=1 ;;
    interp) run_backend interp || rc=1 ;;
    both)   run_backend interp || rc=1; run_backend native || rc=1 ;;
    *) echo "usage: $0 [--backend native|interp|both]" >&2; exit 2 ;;
esac

echo "──────────────────────────────────────────────────"
[ "$rc" -eq 0 ] && echo "http api harness: ok" || echo "http api harness: FAILED"
exit "$rc"

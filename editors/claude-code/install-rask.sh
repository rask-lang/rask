#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# Claude Code SessionStart hook: make sure `rask` is on PATH.
#
# Cloud sessions (including ones started from the mobile app) run in a fresh
# container with no rask in it. This downloads the latest Linux release once
# into ~/.local/bin and puts that on the session's PATH. Where rask is already
# installed it does nothing.
set -euo pipefail

bin="$HOME/.local/bin"
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  echo "export PATH=\"$bin:\$PATH\"" >> "$CLAUDE_ENV_FILE"
fi
export PATH="$bin:$PATH"
command -v rask >/dev/null 2>&1 && exit 0

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) asset=rask-linux-x86_64 ;;
  Darwin-arm64) asset=rask-macos-aarch64 ;;
  *) echo "install-rask: no release binary for $(uname -s)-$(uname -m)" >&2; exit 0 ;;
esac

mkdir -p "$bin"
curl -fsSL -o "$bin/rask.part" \
  "https://github.com/rask-lang/rask/releases/latest/download/$asset"
chmod +x "$bin/rask.part"
mv "$bin/rask.part" "$bin/rask"

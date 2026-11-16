#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# Claude Code SessionStart hook: make sure `rask` is on PATH.
#
# Cloud sessions (including ones started from the mobile app) run in a fresh
# container with no rask in it. This runs the site's installer once, which puts
# the latest release in ~/.local/bin, and puts that on the session's PATH.
# Where rask is already installed it does nothing.
#
# Set RASK_VERSION=0.6.0 in the hook's environment to pin a release.
set -euo pipefail

bin="$HOME/.local/bin"
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  echo "export PATH=\"$bin:\$PATH\"" >> "$CLAUDE_ENV_FILE"
fi
export PATH="$bin:$PATH"
command -v rask >/dev/null 2>&1 && exit 0

# The installer's progress and advice go to stderr: a SessionStart hook's
# stdout lands in Claude's context, and none of it is news to Claude.
curl -fsSL https://rask-lang.dev/install.sh | RASK_INSTALL_DIR="$bin" sh >&2

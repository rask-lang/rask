#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Runs once when the container is created: build the compiler and the editor
# extension. No `set -e` — a failed step leaves a usable container to fix it
# from, rather than none.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Named volumes come up owned by root.
sudo chown vscode:vscode /workspaces/projects /home/vscode/.claude /usr/local/cargo/registry

cd "$REPO_ROOT/compiler" \
  && cargo build --release -p rask-cli -p rask-lsp \
  || echo "post-create: compiler build failed"

# The extension isn't on the marketplace; package it here, install on attach.
cd "$REPO_ROOT/editors/vscode" \
  && npm ci \
  && npx --yes @vscode/vsce package -o /tmp/rask.vsix \
  || echo "post-create: extension build failed"

exit 0

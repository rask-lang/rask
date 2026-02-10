#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# Manual setup: build rask and symlink to PATH

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPILER_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$COMPILER_DIR/.." && pwd)"

echo "→ Building rask compiler..."
cd "$COMPILER_DIR"
cargo build --release

BINARY="$COMPILER_DIR/target/release/rask"
SYMLINK="$HOME/.local/bin/rask"

echo "→ Setting up PATH symlink..."
mkdir -p "$(dirname "$SYMLINK")"
ln -sf "$BINARY" "$SYMLINK"

echo "✓ rask is now available in PATH"
echo ""
echo "Test with: rask --version"
echo "Binary location: $BINARY"
echo "Symlink location: $SYMLINK"

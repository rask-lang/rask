#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Warm the build cache for a fresh cloud session.
#
# A fresh Claude Code web session clones the repo but builds nothing. The first
# `cargo build --release` then compiles ~287 dependency crates (Cranelift is the
# slow one) — a few minutes of work that almost never changes between sessions.
#
# Run this from a cloud environment's SETUP SCRIPT. That script's filesystem
# output is snapshotted and reused by every later session, so the compiled
# dependencies in ~/.cargo/registry and compiler/target/ come pre-built. The
# first in-session build then only recompiles the rask-* crates you actually
# touched — seconds instead of minutes.
#
# See docs/cloud-cache.md for where to paste this.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT/compiler"

# Download every dependency up front (keyed to Cargo.lock; ~10s).
cargo fetch

# Compile deps + the CLI once so they land in the snapshot.
cargo build --release -p rask-cli

# The C runtime is a prerequisite for running compiled programs and is cheap.
make -C runtime

#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Warm the build caches for a fresh cloud session.
#
# A fresh Claude Code web session clones the repo but builds nothing. The first
# `cargo build --release` then compiles ~287 dependency crates (Cranelift is the
# slow one) — a few minutes of work that almost never changes between sessions.
#
# Run this from a cloud environment's SETUP SCRIPT. That script's filesystem
# output is snapshotted and reused by every later session, so the compiled
# dependencies in ~/.cargo/registry and compiler/target/ come pre-built.
#
# Only the third-party crates carry over. Each session clones the repo fresh, so
# every rask-* source file has a new mtime and cargo recompiles the 22 workspace
# crates whether or not their content changed — 58s in release. Nothing here can
# dodge that; docs/cloud-cache.md has the measurements.
#
# See docs/cloud-cache.md for where to paste this.

# No `set -e`. A setup script that exits non-zero stops every session in the
# environment from starting, so a broken main would lock you out of the session
# you need to fix it. Everything below is a cache warm-up, not a gate: on
# failure it says so and carries on, and you get a slow session instead of no
# session.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT/compiler" || exit 0

# Download every dependency up front (keyed to Cargo.lock; ~10s).
cargo fetch || echo "warm-cache: cargo fetch failed, continuing"

# Compile deps + the CLI once so they land in the snapshot.
cargo build --release -p rask-cli || echo "warm-cache: release build failed, continuing"

# `cargo test` builds with the dev profile, which shares nothing with the
# release artifacts above — cold it compiles 238 crates in debug (85s, 4.1G)
# before a single test runs. Building the test binaries here leaves the ~215
# third-party ones in the snapshot; a session then pays 8s for the rask-* ones.
cargo test --no-run --workspace || echo "warm-cache: dev test build failed, continuing"

# No `make -C runtime` here. Nothing links librask_runtime.a — the linker
# compiles the runtime .c files itself and caches those objects keyed by size
# and mtime, which a fresh clone invalidates anyway. It cost setup budget and
# bought a session nothing.

exit 0

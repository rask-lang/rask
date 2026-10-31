#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
# Cloud sessions only: rebuild the compiler, since the snapshot's binary is
# older than the clone and links against a newer runtime (#1041).
# Async so the first message isn't held for the ~60s build. A `cargo build`
# started by Claude meanwhile waits on cargo's lock and then finds it done.
set -euo pipefail

[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0

echo '{"async": true, "asyncTimeout": 600000}'

cd "${CLAUDE_PROJECT_DIR:-.}/compiler"
cargo build --release -p rask-cli >target/session-start-build.log 2>&1

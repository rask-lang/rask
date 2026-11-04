#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Runs on every attach: `code` only exists once VS Code is connected, so the
# extension built in post-create installs here.
[ -f /tmp/rask.vsix ] || exit 0
command -v code >/dev/null || exit 0
code --install-extension /tmp/rask.vsix --force >/dev/null && rm /tmp/rask.vsix
exit 0

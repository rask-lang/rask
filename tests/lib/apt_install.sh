#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Install apt packages on a GitHub runner, surviving a slow mirror.
#
# The runners read packages through Azure's Ubuntu mirror. Most days it's
# fast. Some days it serves at tens of kilobytes a second, and then a 27 MB
# valgrind install or the aarch64 toolchain runs past any timeout a job can
# afford, before the job has built or tested anything. Three jobs died that
# way in one afternoon on changes to two spec files.
#
# So: try the runner's mirror for a bounded time. If that fails or runs out,
# put Ubuntu's main archive first in the mirror list and try again. A slow
# mirror then costs a couple of minutes, not the job.
#
#   tests/lib/apt_install.sh <package>...
#
# APT_FIRST_TRY_SECONDS bounds the first attempt (default 150).

set -u
if [ "$#" -eq 0 ]; then
  echo "usage: $0 <package>..." >&2
  exit 2
fi

APT="-o Acquire::Retries=3 -o Acquire::http::Timeout=30"
MIRRORS=/etc/apt/apt-mirrors.txt
first="${APT_FIRST_TRY_SECONDS:-150}"

try_install() {
  sudo apt-get $APT update -qq && \
    sudo apt-get $APT install -y -qq --no-install-recommends "$@" > /dev/null
}
export -f try_install
export APT

if timeout "$first" bash -c 'try_install "$@"' _ "$@"; then
  exit 0
fi

echo "apt: the runner's mirror didn't deliver in ${first}s; retrying through archive.ubuntu.com"
if [ -f "$MIRRORS" ]; then
  # apt's mirror method takes the lowest priority first and falls back down
  # the list, so the main archive goes to the top and Azure's stays as a
  # fallback.
  sudo tee "$MIRRORS" > /dev/null <<'EOF'
http://archive.ubuntu.com/ubuntu/	priority:1
http://security.ubuntu.com/ubuntu/	priority:2
http://azure.archive.ubuntu.com/ubuntu/	priority:3
EOF
else
  sudo sed -i 's|azure.archive.ubuntu.com|archive.ubuntu.com|g' /etc/apt/sources.list /etc/apt/sources.list.d/*.sources 2>/dev/null
fi
try_install "$@"

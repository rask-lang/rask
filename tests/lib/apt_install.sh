#!/usr/bin/env bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Install apt packages on a GitHub runner without depending on a mirror.
#
# The runners read packages through Azure's Ubuntu mirror, and some days it
# serves at tens of kilobytes a second. Falling back to archive.ubuntu.com
# wasn't enough: on 2026-11-11 both stalled together and the valgrind and
# aarch64 installs ran out their 8 minutes on five runs.
#
# So the .deb files live in GitHub's cache (.github/actions/apt-install keys
# it on the runner image and the package list). With APT_DEB_DIR holding
# them, this installs straight from disk with dpkg and never opens a socket.
# The runner image is the same one the debs were fetched on, so they are
# exactly the packages it lacks, at versions that fit what it has.
#
# Otherwise it goes to the network: the runner's mirror for a bounded time,
# then Ubuntu's main archive. Whatever apt downloaded lands in APT_DEB_DIR
# for the cache to save.
#
#   APT_DEB_DIR=<dir> tests/lib/apt_install.sh <package>...
#
# APT_FIRST_TRY_SECONDS bounds the first network attempt (default 150).

set -u
if [ "$#" -eq 0 ]; then
  echo "usage: $0 <package>..." >&2
  exit 2
fi

deb_dir="${APT_DEB_DIR:-}"

installed() {
  local p
  for p in "$@"; do
    [ "$(dpkg-query -W -f='${Status}' "$p" 2>/dev/null)" = "install ok installed" ] || return 1
  done
}

if installed "$@"; then
  exit 0
fi

if [ -n "$deb_dir" ] && compgen -G "$deb_dir/*.deb" > /dev/null; then
  # One dpkg call for the whole set, so it orders their dependencies itself.
  if sudo dpkg -i "$deb_dir"/*.deb > /dev/null && installed "$@"; then
    echo "apt: installed $* from the cache"
    exit 0
  fi
  echo "apt: the cached debs didn't install; going to the network"
  rm -f "$deb_dir"/*.deb
fi

archives=$(mktemp -d)
chmod 755 "$archives"
mkdir -p "$archives/partial"
APT="-o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Dir::Cache::Archives=$archives -o APT::Keep-Downloaded-Packages=true"
MIRRORS=/etc/apt/apt-mirrors.txt
first="${APT_FIRST_TRY_SECONDS:-150}"

try_install() {
  sudo apt-get $APT update -qq && \
    sudo apt-get $APT install -y -qq --no-install-recommends "$@" > /dev/null
}
export -f try_install
export APT

keep_debs() {
  if [ -n "$deb_dir" ]; then
    mkdir -p "$deb_dir"
    cp "$archives"/*.deb "$deb_dir"/ 2>/dev/null || true
  fi
}

if timeout "$first" bash -c 'try_install "$@"' _ "$@"; then
  keep_debs
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
try_install "$@" || exit 1
keep_debs
